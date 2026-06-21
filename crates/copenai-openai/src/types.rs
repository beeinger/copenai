use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub metadata: Option<Value>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub tools: Option<Value>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub functions: Option<Value>,
    #[serde(default)]
    pub function_call: Option<Value>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default)]
    pub content: Option<MessageContent>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
    File { file: FileReference },
    InputFile { file_id: String },
    InputAudio { input_audio: InputAudio },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileReference {
    pub file_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InputAudio {
    pub data: String,
    pub format: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCallPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallPayload {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: AssistantMessage,
    pub finish_reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssistantMessage {
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: ChunkDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallChunk>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallChunk {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCallChunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallChunk {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelList {
    pub object: &'static str,
    pub data: Vec<ModelObject>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelObject {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub owned_by: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileObject {
    pub id: String,
    pub object: &'static str,
    pub bytes: u64,
    pub created_at: i64,
    pub filename: String,
    pub purpose: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileList {
    pub object: &'static str,
    pub data: Vec<FileObject>,
    pub first_id: String,
    pub last_id: String,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDeleted {
    pub id: String,
    pub object: &'static str,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAiErrorBody {
    pub error: OpenAiErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenAiErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: Option<String>,
}

impl ChatCompletionRequest {
    pub fn has_tool_fields(&self) -> bool {
        self.tools.is_some()
            || self.tool_choice.is_some()
            || self.functions.is_some()
            || self.function_call.is_some()
            || self.extra.keys().any(|k| {
                matches!(
                    k.as_str(),
                    "tools" | "tool_choice" | "functions" | "function_call"
                )
            })
    }

    pub fn parallel_tool_calls(&self) -> bool {
        self.parallel_tool_calls.unwrap_or(true)
    }

    pub fn tool_execution_mode(&self) -> Option<String> {
        self.metadata
            .as_ref()
            .and_then(|m| m.get("tool_execution"))
            .and_then(|v| v.as_str())
            .map(String::from)
    }
}

impl ChatMessage {
    pub fn content_or_default(&self) -> MessageContent {
        self.content
            .clone()
            .unwrap_or(MessageContent::Text(String::new()))
    }
}

impl MessageContent {
    pub fn as_text(&self) -> Option<String> {
        match self {
            Self::Text(t) => Some(t.clone()),
            Self::Parts(parts) => {
                let texts: Vec<String> = parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .collect();
                if texts.is_empty() {
                    None
                } else {
                    Some(texts.join("\n"))
                }
            }
        }
    }
}
