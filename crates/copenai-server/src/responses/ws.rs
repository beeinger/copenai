use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use copenai_openai::{ResponseCreateRequest, ResponsesStreamEvent};
use futures::{SinkExt, StreamExt};
use tokio::sync::Mutex;

use crate::responses::{CreateOutcome, ResponsesOrchestrator};
use crate::state::SharedState;

struct WsSession {
    last_response_id: Option<String>,
    in_flight: bool,
    created_at: std::time::Instant,
}

const SESSION_MAX: Duration = Duration::from_secs(60 * 60);

pub async fn responses_websocket(
    ws: WebSocketUpgrade,
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let conv_header = headers
        .get("x-conversation-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let tool_header = headers
        .get("x-tool-execution")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    ws.on_upgrade(move |socket| handle_socket(socket, state, conv_header, tool_header))
}

async fn handle_socket(
    socket: WebSocket,
    state: SharedState,
    conv_header: Option<String>,
    tool_header: Option<String>,
) {
    let session = Mutex::new(WsSession {
        last_response_id: None,
        in_flight: false,
        created_at: std::time::Instant::now(),
    });

    let (mut sender, mut receiver) = socket.split();

    while let Some(msg) = receiver.next().await {
        let msg = match msg {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => break,
            _ => continue,
        };

        {
            let s = session.lock().await;
            if s.created_at.elapsed() > SESSION_MAX {
                let _ = sender
                    .send(Message::Text(
                        serde_json::json!({"type":"error","error":{"message":"session expired"}})
                            .to_string()
                            .into(),
                    ))
                    .await;
                break;
            }
            if s.in_flight {
                let _ = sender
                    .send(Message::Text(
                        serde_json::json!({"type":"error","error":{"message":"response already in progress"}})
                            .to_string()
                            .into(),
                    ))
                    .await;
                continue;
            }
        }

        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&msg) else {
            continue;
        };
        let event_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if event_type != "response.create" {
            continue;
        }
        if let Some(obj) = value.as_object_mut() {
            obj.remove("type");
        }
        let mut request: ResponseCreateRequest = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(e) => {
                let _ = sender
                    .send(Message::Text(
                        serde_json::json!({"type":"error","error":{"message": e.to_string()}})
                            .to_string()
                            .into(),
                    ))
                    .await;
                continue;
            }
        };
        request.stream = false;

        {
            let mut s = session.lock().await;
            if request.previous_response_id.is_none() {
                request.previous_response_id = s.last_response_id.clone();
            }
            s.in_flight = true;
        }

        let conv = conv_header.as_deref();
        let tool = tool_header.as_deref();
        let outcome = ResponsesOrchestrator::create(&state, request, conv, tool).await;

        match outcome {
            Ok(CreateOutcome::Json(resp)) => {
                let id = resp.id.clone();
                let events = vec![
                    ws_event("response.created", serde_json::json!({ "response": &resp })),
                    ws_event("response.completed", serde_json::json!({ "response": &resp })),
                ];
                for e in events {
                    let _ = sender.send(Message::Text(e.into())).await;
                }
                let mut s = session.lock().await;
                s.last_response_id = Some(id);
                s.in_flight = false;
            }
            Ok(CreateOutcome::Stream(mut stream)) => {
                let mut last_id = None;
                while let Some(event) = stream.next().await {
                    if let Some(text) = encode_ws_event(&event) {
                        if let ResponsesStreamEvent::Completed(resp) = &event {
                            last_id = Some(resp.id.clone());
                        }
                        let _ = sender.send(Message::Text(text.into())).await;
                    }
                }
                let mut s = session.lock().await;
                s.last_response_id = last_id;
                s.in_flight = false;
            }
            Err(e) => {
                let _ = sender
                    .send(Message::Text(
                        serde_json::json!({"type":"error","error":{"message": e}})
                            .to_string()
                            .into(),
                    ))
                    .await;
                let mut s = session.lock().await;
                s.in_flight = false;
            }
        }
    }
}

fn ws_event(event_type: &str, payload: serde_json::Value) -> String {
    serde_json::json!({
        "type": event_type,
        "sequence_number": 0,
        "response": payload.get("response"),
    })
    .to_string()
}

fn encode_ws_event(event: &ResponsesStreamEvent) -> Option<String> {
    copenai_openai::encode_responses_event(event.clone(), 0)
}
