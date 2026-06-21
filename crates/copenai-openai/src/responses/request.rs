use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::input::InputItem;
use crate::tools::{FunctionTool, ResponsesToolChoice};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseCreateRequest {
    pub model: String,
    #[serde(default)]
    pub input: ResponseInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub truncation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<FunctionTool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ResponsesToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationParam>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseInput {
    Text(String),
    Items(Vec<InputItem>),
}

impl Default for ResponseInput {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationParam {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<TextFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextFormat {
    #[serde(rename = "type")]
    pub format_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

impl ResponseCreateRequest {
    pub fn tool_execution_mode(&self) -> Option<&str> {
        self.metadata
            .as_ref()
            .and_then(|m| m.get("tool_execution"))
            .map(String::as_str)
    }

    pub fn conversation_id(&self) -> Option<&str> {
        self.metadata
            .as_ref()
            .and_then(|m| m.get("conversation_id"))
            .map(String::as_str)
            .or_else(|| self.conversation.as_ref().map(|c| c.id.as_str()))
    }

    pub fn wants_json_schema(&self) -> bool {
        self.text
            .as_ref()
            .and_then(|t| t.format.as_ref())
            .map(|f| f.format_type == "json_schema")
            .unwrap_or(false)
    }

    pub fn json_schema(&self) -> Option<&Value> {
        self.text
            .as_ref()
            .and_then(|t| t.format.as_ref())
            .and_then(|f| f.schema.as_ref())
    }

    pub fn include_reasoning(&self) -> bool {
        self.include
            .as_ref()
            .map(|i| i.iter().any(|s| s == "reasoning"))
            .unwrap_or(false)
            || self.reasoning.is_some()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseListQuery {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub order: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
}
