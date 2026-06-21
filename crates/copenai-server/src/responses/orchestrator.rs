use async_stream::stream;
use copenai_openai::{
    completed_response, new_response_id, parse_response_request, response_created, InputItem,
    ResponseCreateRequest, ResponseList, ResponseObject, ResponsesStreamEvent,
};
use copenai_store::{ResponseStore, StoredResponse};
use serde_json::Value;

use crate::state::SharedState;
use crate::tools::{ToolExecutionMode, ToolLoopEngine, ToolOrchestrator};

pub struct ResponsesOrchestrator;

impl ResponsesOrchestrator {
    pub async fn create(
        state: &SharedState,
        request: ResponseCreateRequest,
        conv_header: Option<&str>,
        tool_header: Option<&str>,
    ) -> Result<CreateOutcome, String> {
        let tool_engine = ToolLoopEngine::new(state.config.responses.clone());
        let mode = tool_engine.resolve_mode_responses(request.tool_execution_mode(), tool_header);
        tool_engine.validate_server_mode(mode)?;

        let models = state.models.read().await.clone();
        let model = copenai_openai::validate_model(&request.model, &models)?;

        let conversation_id = resolve_conv_id(&request, conv_header);
        let response_id = new_response_id();
        let request_json =
            serde_json::to_string(&request).map_err(|e| format!("serialize request: {e}"))?;

        let should_store = request.store.unwrap_or(false) || request.previous_response_id.is_some();
        if should_store {
            ResponseStore::create(
                state.store.pool(),
                &response_id,
                Some(&conversation_id),
                &model,
                mode.as_str(),
                &request_json,
                request.previous_response_id.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())?;
        }

        if request.stream {
            let events = Self::run_stream(
                state.clone(),
                request,
                conversation_id,
                response_id,
                model,
                mode,
                should_store,
            );
            Ok(CreateOutcome::Stream(events))
        } else {
            let response = Self::run_sync(
                state,
                request,
                &conversation_id,
                &response_id,
                &model,
                mode,
                should_store,
            )
            .await?;
            Ok(CreateOutcome::Json(Box::new(response)))
        }
    }

    pub async fn get(state: &SharedState, id: &str) -> Result<Option<ResponseObject>, String> {
        let stored = ResponseStore::get(state.store.pool(), id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(stored.map(stored_to_response))
    }

    pub async fn delete(state: &SharedState, id: &str) -> Result<bool, String> {
        ResponseStore::cancel(state.store.pool(), id)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn list(
        state: &SharedState,
        limit: u32,
        after: Option<&str>,
        order: Option<&str>,
    ) -> Result<ResponseList, String> {
        let desc = order != Some("asc");
        let rows = ResponseStore::list(state.store.pool(), limit, after, desc)
            .await
            .map_err(|e| e.to_string())?;
        let data: Vec<ResponseObject> = rows.into_iter().map(stored_to_response).collect();
        let first_id = data.first().map(|r| r.id.clone()).unwrap_or_default();
        let last_id = data.last().map(|r| r.id.clone()).unwrap_or_default();
        let has_more = data.len() as u32 >= limit;
        Ok(ResponseList {
            object: "list".into(),
            data,
            first_id,
            last_id,
            has_more,
        })
    }

    async fn run_sync(
        state: &SharedState,
        request: ResponseCreateRequest,
        conversation_id: &str,
        response_id: &str,
        model: &str,
        mode: ToolExecutionMode,
        store: bool,
    ) -> Result<ResponseObject, String> {
        let mut parsed = parse_response_request(&request)?;
        enrich_from_previous(state, &request, &mut parsed).await?;

        let outcome = ToolOrchestrator::execute_responses_turn(
            state,
            &request,
            &parsed,
            conversation_id,
            response_id,
            model,
            mode,
            false,
            None,
            None,
        )
        .await?;

        let mut response = completed_response(
            response_created(response_id, model),
            &outcome.text,
            outcome.usage,
            outcome.incomplete,
        );
        response.output = outcome.outputs;
        response.metadata = request.metadata.clone();
        response.previous_response_id = request.previous_response_id.clone();
        if let Some(reason) = outcome.incomplete_reason {
            response.incomplete_details = Some(copenai_openai::IncompleteDetails {
                reason: reason.as_str().into(),
            });
        }

        if store {
            persist_response(state, response_id, &response, &parsed, &request).await?;
        }
        Ok(response)
    }

    fn run_stream(
        state: SharedState,
        request: ResponseCreateRequest,
        conversation_id: String,
        response_id: String,
        model: String,
        mode: ToolExecutionMode,
        store: bool,
    ) -> futures::stream::BoxStream<'static, ResponsesStreamEvent> {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            let mut response = response_created(&response_id, &model);
            response.metadata = request.metadata.clone();
            response.previous_response_id = request.previous_response_id.clone();
            let _ = tx
                .send(ResponsesStreamEvent::Created(response.clone()))
                .await;
            let _ = tx
                .send(ResponsesStreamEvent::InProgress(response.clone()))
                .await;

            let event_tx = tx.clone();
            let mut parsed = match parse_response_request(&request) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx
                        .send(ResponsesStreamEvent::Failed { response, error: e })
                        .await;
                    return;
                }
            };
            if let Err(e) = enrich_from_previous(&state, &request, &mut parsed).await {
                let _ = tx
                    .send(ResponsesStreamEvent::Failed { response, error: e })
                    .await;
                return;
            }

            let result = ToolOrchestrator::execute_responses_turn(
                &state,
                &request,
                &parsed,
                &conversation_id,
                &response_id,
                &model,
                mode,
                true,
                Some(event_tx),
                Some(response.clone()),
            )
            .await;

            match result {
                Ok(outcome) => {
                    response.output = outcome.outputs;
                    response = completed_response(
                        response,
                        &outcome.text,
                        outcome.usage,
                        outcome.incomplete,
                    );
                    if let Some(reason) = outcome.incomplete_reason {
                        response.incomplete_details = Some(copenai_openai::IncompleteDetails {
                            reason: reason.as_str().into(),
                        });
                    }
                    if store {
                        let _ =
                            persist_response(&state, &response_id, &response, &parsed, &request)
                                .await;
                    }
                    let _ = tx.send(ResponsesStreamEvent::Completed(response)).await;
                }
                Err(e) => {
                    response.status = "failed".into();
                    let _ = tx
                        .send(ResponsesStreamEvent::Failed { response, error: e })
                        .await;
                }
            }
        });

        Box::pin(stream! {
            let mut rx = rx;
            while let Some(event) = rx.recv().await {
                yield event;
            }
        })
    }
}

pub enum CreateOutcome {
    Json(Box<ResponseObject>),
    Stream(futures::stream::BoxStream<'static, ResponsesStreamEvent>),
}

async fn enrich_from_previous(
    state: &SharedState,
    request: &ResponseCreateRequest,
    parsed: &mut copenai_openai::ParsedResponse,
) -> Result<(), String> {
    let Some(prev_id) = request.previous_response_id.as_deref() else {
        return Ok(());
    };
    let chain = ResponseStore::chain_input_items(state.store.pool(), prev_id)
        .await
        .map_err(|e| e.to_string())?;
    for item in chain {
        if let Ok(input_item) = serde_json::from_value::<InputItem>(item) {
            if let Some((call_id, output)) = input_item.function_call_output() {
                parsed
                    .function_call_outputs
                    .push((call_id.to_string(), output.to_string()));
            }
        }
    }
    Ok(())
}

async fn persist_response(
    state: &SharedState,
    id: &str,
    response: &ResponseObject,
    parsed: &copenai_openai::ParsedResponse,
    request: &ResponseCreateRequest,
) -> Result<(), String> {
    let response_json = serde_json::to_string(response).map_err(|e| e.to_string())?;
    let output_json = serde_json::to_string(&response.output).map_err(|e| e.to_string())?;
    let usage_json = response
        .usage
        .as_ref()
        .map(|u| serde_json::to_string(u).unwrap_or_default());

    let chain_items = build_input_chain_items(parsed, response);
    let chain_json = serde_json::to_string(&chain_items).map_err(|e| e.to_string())?;

    ResponseStore::update_completed(
        state.store.pool(),
        id,
        &response.status,
        &response_json,
        &output_json,
        usage_json.as_deref(),
        Some(&chain_json),
    )
    .await
    .map_err(|e| e.to_string())?;

    let _ = request;
    Ok(())
}

fn build_input_chain_items(
    parsed: &copenai_openai::ParsedResponse,
    response: &ResponseObject,
) -> Vec<Value> {
    let mut items = Vec::new();
    for (call_id, output) in &parsed.function_call_outputs {
        items.push(serde_json::json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": output,
        }));
    }
    for item in &response.output {
        if let Some(json) = output_item_to_chain(item) {
            items.push(json);
        }
    }
    items
}

fn output_item_to_chain(item: &copenai_openai::OutputItem) -> Option<Value> {
    match item {
        copenai_openai::OutputItem::FunctionCall {
            call_id,
            name,
            arguments,
            ..
        } => Some(serde_json::json!({
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "arguments": arguments,
        })),
        copenai_openai::OutputItem::Message { role, content, .. } => {
            let text: String = content
                .iter()
                .map(|p| match p {
                    copenai_openai::OutputContentPart::OutputText { text, .. } => text.as_str(),
                })
                .collect();
            if text.is_empty() {
                None
            } else {
                Some(serde_json::json!({
                    "type": "message",
                    "role": role,
                    "content": text,
                }))
            }
        }
        _ => None,
    }
}

fn stored_to_response(stored: StoredResponse) -> ResponseObject {
    if let Some(json) = stored.response_json {
        if let Ok(resp) = serde_json::from_str::<ResponseObject>(&json) {
            return resp;
        }
    }
    let mut resp = ResponseObject::new(stored.id, stored.model, 0);
    resp.status = stored.status;
    resp.previous_response_id = stored.previous_response_id;
    if let Some(out) = stored.output_json {
        if let Ok(items) = serde_json::from_str(&out) {
            resp.output = items;
        }
    }
    if let Some(u) = stored.usage_json {
        if let Ok(usage) = serde_json::from_str(&u) {
            resp.usage = Some(usage);
        }
    }
    resp
}

fn resolve_conv_id(request: &ResponseCreateRequest, header: Option<&str>) -> String {
    if let Some(id) = header {
        return id.to_string();
    }
    if let Some(id) = request.conversation_id() {
        return id.to_string();
    }
    uuid::Uuid::new_v4().to_string()
}
