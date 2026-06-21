use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_stream::stream;
use futures::Stream;
use serde::Serialize;
use serde_json::{json, Value};

use super::output::{OutputContentPart, OutputItem};
use super::response::ResponseObject;
use crate::sse::unix_now;
use crate::types::Usage;

#[derive(Debug, Clone)]
pub enum ResponsesStreamEvent {
    Created(ResponseObject),
    InProgress(ResponseObject),
    OutputItemAdded {
        response: ResponseObject,
        output_index: usize,
        item: OutputItem,
    },
    ContentPartAdded {
        response: ResponseObject,
        output_index: usize,
        content_index: usize,
        part: OutputContentPart,
    },
    OutputTextDelta {
        response: ResponseObject,
        output_index: usize,
        content_index: usize,
        delta: String,
    },
    ReasoningDelta {
        response: ResponseObject,
        output_index: usize,
        delta: String,
    },
    FunctionCallArgumentsDelta {
        response: ResponseObject,
        output_index: usize,
        delta: String,
    },
    FunctionCallArgumentsDone {
        response: ResponseObject,
        output_index: usize,
        arguments: String,
    },
    OutputItemDone {
        response: ResponseObject,
        output_index: usize,
        item: OutputItem,
    },
    Completed(ResponseObject),
    Failed {
        response: ResponseObject,
        error: String,
    },
    Error(String),
}

#[derive(Serialize)]
struct StreamEnvelope {
    #[serde(rename = "type")]
    event_type: String,
    sequence_number: u64,
    #[serde(flatten)]
    payload: Value,
}

pub fn responses_sse_stream<S>(
    events: S,
) -> Pin<Box<dyn Stream<Item = Result<String, std::convert::Infallible>> + Send>>
where
    S: Stream<Item = ResponsesStreamEvent> + Send + 'static,
{
    let seq = Arc::new(AtomicU64::new(0));
    Box::pin(stream! {
        futures::pin_mut!(events);
        while let Some(event) = events.next().await {
            let n = seq.fetch_add(1, Ordering::Relaxed);
            match encode_event(event, n) {
                Some(data) => yield Ok(format!("data: {data}\n\n")),
                None => {}
            }
        }
        yield Ok("data: [DONE]\n\n".to_string());
    })
}

pub fn encode_responses_event(event: ResponsesStreamEvent, sequence_number: u64) -> Option<String> {
    encode_event(event, sequence_number)
}

fn encode_event(event: ResponsesStreamEvent, sequence_number: u64) -> Option<String> {
    match event {
        ResponsesStreamEvent::Created(resp) => Some(serialize(
            "response.created",
            sequence_number,
            json!({ "response": resp }),
        )),
        ResponsesStreamEvent::InProgress(resp) => Some(serialize(
            "response.in_progress",
            sequence_number,
            json!({ "response": resp }),
        )),
        ResponsesStreamEvent::OutputItemAdded {
            response,
            output_index,
            item,
        } => Some(serialize(
            "response.output_item.added",
            sequence_number,
            json!({
                "response": response,
                "output_index": output_index,
                "item": item,
            }),
        )),
        ResponsesStreamEvent::ContentPartAdded {
            response,
            output_index,
            content_index,
            part,
        } => Some(serialize(
            "response.content_part.added",
            sequence_number,
            json!({
                "response": response,
                "output_index": output_index,
                "content_index": content_index,
                "part": part,
            }),
        )),
        ResponsesStreamEvent::OutputTextDelta {
            response,
            output_index,
            content_index,
            delta,
        } => Some(serialize(
            "response.output_text.delta",
            sequence_number,
            json!({
                "response": response,
                "output_index": output_index,
                "content_index": content_index,
                "delta": delta,
            }),
        )),
        ResponsesStreamEvent::ReasoningDelta {
            response,
            output_index,
            delta,
        } => Some(serialize(
            "response.reasoning_text.delta",
            sequence_number,
            json!({
                "response": response,
                "output_index": output_index,
                "delta": delta,
            }),
        )),
        ResponsesStreamEvent::FunctionCallArgumentsDelta {
            response,
            output_index,
            delta,
        } => Some(serialize(
            "response.function_call_arguments.delta",
            sequence_number,
            json!({
                "response": response,
                "output_index": output_index,
                "delta": delta,
            }),
        )),
        ResponsesStreamEvent::FunctionCallArgumentsDone {
            response,
            output_index,
            arguments,
        } => Some(serialize(
            "response.function_call_arguments.done",
            sequence_number,
            json!({
                "response": response,
                "output_index": output_index,
                "arguments": arguments,
            }),
        )),
        ResponsesStreamEvent::OutputItemDone {
            response,
            output_index,
            item,
        } => Some(serialize(
            "response.output_item.done",
            sequence_number,
            json!({
                "response": response,
                "output_index": output_index,
                "item": item,
            }),
        )),
        ResponsesStreamEvent::Completed(resp) => Some(serialize(
            "response.completed",
            sequence_number,
            json!({ "response": resp }),
        )),
        ResponsesStreamEvent::Failed { response, error } => Some(serialize(
            "response.failed",
            sequence_number,
            json!({
                "response": response,
                "error": { "message": error },
            }),
        )),
        ResponsesStreamEvent::Error(msg) => Some(serialize(
            "error",
            sequence_number,
            json!({ "error": { "message": msg, "type": "server_error" } }),
        )),
    }
}

fn serialize(event_type: &str, sequence_number: u64, payload: Value) -> String {
    let envelope = StreamEnvelope {
        event_type: event_type.to_string(),
        sequence_number,
        payload,
    };
    serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".into())
}

pub fn completed_response(
    mut response: ResponseObject,
    text: &str,
    usage: Usage,
    incomplete: bool,
) -> ResponseObject {
    if response.output.is_empty() && !text.is_empty() {
        response.output.push(OutputItem::message_text(
            text,
            &super::output::new_item_id("msg"),
        ));
    }
    response.status = if incomplete {
        "incomplete".into()
    } else {
        "completed".into()
    };
    response.usage = Some(usage);
    response
}

pub fn response_created(id: &str, model: &str) -> ResponseObject {
    ResponseObject::new(id.to_string(), model.to_string(), unix_now())
}

use futures::StreamExt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_has_sequence() {
        let resp = response_created("resp_test", "m");
        let data = encode_event(ResponsesStreamEvent::Created(resp), 0).unwrap();
        assert!(data.contains("\"sequence_number\":0"));
        assert!(data.contains("response.created"));
    }

    #[test]
    fn golden_function_call_arguments_delta() {
        let resp = response_created("resp_test", "m");
        let data = encode_event(
            ResponsesStreamEvent::FunctionCallArgumentsDelta {
                response: resp,
                output_index: 1,
                delta: "{\"loc".into(),
            },
            3,
        )
        .unwrap();
        assert!(data.contains("response.function_call_arguments.delta"));
        assert!(data.contains("\"output_index\":1"));
    }
}
