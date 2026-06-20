use std::pin::Pin;
use std::time::{SystemTime, UNIX_EPOCH};

use async_stream::stream;
use futures::Stream;
use futures::StreamExt;
use serde_json::json;

use crate::types::{ChatCompletionChunk, ChatCompletionResponse, ChunkChoice, ChunkDelta, Usage};

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Delta(String),
    Usage(Usage),
    Done { finish_reason: String, usage: Usage },
    Error(String),
}

pub fn completion_response(
    id: &str,
    model: &str,
    content: &str,
    finish_reason: &str,
    usage: Usage,
) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: id.to_string(),
        object: "chat.completion",
        created: unix_now(),
        model: model.to_string(),
        choices: vec![crate::types::Choice {
            index: 0,
            message: crate::types::AssistantMessage {
                role: "assistant",
                content: content.to_string(),
            },
            finish_reason: finish_reason.to_string(),
        }],
        usage,
    }
}

pub fn chunk_delta(id: &str, model: &str, content: &str, first: bool) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk",
        created: unix_now(),
        model: model.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: ChunkDelta {
                role: if first { Some("assistant") } else { None },
                content: Some(content.to_string()),
            },
            finish_reason: None,
        }],
    }
}

pub fn chunk_done(id: &str, model: &str, finish_reason: &str) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk",
        created: unix_now(),
        model: model.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: ChunkDelta::default(),
            finish_reason: Some(finish_reason.to_string()),
        }],
    }
}

/// Legacy helper for mock/tests — splits completed text on whitespace.
pub fn to_sse_stream(
    id: String,
    model: String,
    content: String,
) -> Pin<Box<dyn Stream<Item = Result<String, std::convert::Infallible>> + Send>> {
    let chunks: Vec<String> = content
        .split_whitespace()
        .map(|w| format!("{w} "))
        .collect();
    Box::pin(stream! {
        let mut first = true;
        for chunk in chunks {
            let data = serde_json::to_string(&chunk_delta(&id, &model, &chunk, first)).unwrap();
            first = false;
            yield Ok(format!("data: {data}\n\n"));
        }
        let done = serde_json::to_string(&chunk_done(&id, &model, "stop")).unwrap();
        yield Ok(format!("data: {done}\n\n"));
        yield Ok("data: [DONE]\n\n".to_string());
    })
}

pub fn chunk_role_start(id: &str, model: &str) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk",
        created: unix_now(),
        model: model.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: ChunkDelta {
                role: Some("assistant"),
                content: None,
            },
            finish_reason: None,
        }],
    }
}

pub fn live_sse_stream<E: Stream<Item = StreamEvent> + Send + 'static>(
    id: String,
    model: String,
    events: E,
) -> Pin<Box<dyn Stream<Item = Result<String, std::convert::Infallible>> + Send>> {
    Box::pin(stream! {
        let role = serde_json::to_string(&chunk_role_start(&id, &model)).unwrap();
        yield Ok(format!("data: {role}\n\n"));

        let mut first = true;
        let mut finish_reason = "stop".to_string();
        futures::pin_mut!(events);
        while let Some(event) = events.next().await {
            match event {
                StreamEvent::Delta(delta) => {
                    if !delta.is_empty() {
                        let data = serde_json::to_string(&chunk_delta(&id, &model, &delta, first)).unwrap();
                        first = false;
                        yield Ok(format!("data: {data}\n\n"));
                    }
                }
                StreamEvent::Usage(_) => {}
                StreamEvent::Done { finish_reason: fr, .. } => {
                    finish_reason = fr;
                    break;
                }
                StreamEvent::Error(e) => {
                    let body = json!({ "error": { "message": e, "type": "server_error" } });
                    yield Ok(format!("data: {}\n\n", body));
                    yield Ok("data: [DONE]\n\n".to_string());
                    return;
                }
            }
        }
        let done = serde_json::to_string(&chunk_done(&id, &model, &finish_reason)).unwrap();
        yield Ok(format!("data: {done}\n\n"));
        yield Ok("data: [DONE]\n\n".to_string());
    })
}

pub fn sse_error(message: &str) -> String {
    let body = json!({
        "error": {
            "message": message,
            "type": "server_error"
        }
    });
    format!("data: {}\n\n", body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_includes_usage() {
        let resp = completion_response(
            "id",
            "m",
            "hi",
            "stop",
            Usage {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
            },
        );
        assert_eq!(resp.usage.total_tokens, 3);
    }
}
