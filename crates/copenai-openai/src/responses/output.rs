use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputItem {
    Message {
        id: String,
        role: String,
        status: String,
        content: Vec<OutputContentPart>,
    },
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
        status: String,
    },
    FunctionCallOutput {
        id: String,
        call_id: String,
        output: String,
        status: String,
    },
    Reasoning {
        id: String,
        summary: Vec<ReasoningSummaryPart>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        status: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputContentPart {
    OutputText {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        annotations: Vec<Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningSummaryPart {
    #[serde(rename = "type")]
    pub part_type: String,
    pub text: String,
}

impl OutputItem {
    pub fn message_text(text: &str, item_id: &str) -> Self {
        Self::Message {
            id: item_id.to_string(),
            role: "assistant".into(),
            status: "completed".into(),
            content: vec![OutputContentPart::OutputText {
                text: text.to_string(),
                annotations: vec![],
            }],
        }
    }

    pub fn function_call(item_id: &str, call_id: &str, name: &str, arguments: &str) -> Self {
        Self::FunctionCall {
            id: item_id.to_string(),
            call_id: call_id.to_string(),
            name: name.to_string(),
            arguments: arguments.to_string(),
            status: "completed".into(),
        }
    }

    pub fn function_call_output(item_id: &str, call_id: &str, output: &str) -> Self {
        Self::FunctionCallOutput {
            id: item_id.to_string(),
            call_id: call_id.to_string(),
            output: output.to_string(),
            status: "completed".into(),
        }
    }

    pub fn reasoning(item_id: &str, text: &str) -> Self {
        Self::Reasoning {
            id: item_id.to_string(),
            summary: vec![ReasoningSummaryPart {
                part_type: "summary_text".into(),
                text: text.to_string(),
            }],
            encrypted_content: None,
            status: "completed".into(),
        }
    }

    pub fn agent_function_call(
        item_id: &str,
        call_id: &str,
        title: &str,
        arguments: Option<&Value>,
    ) -> Self {
        let args = arguments
            .map(|v| v.to_string())
            .unwrap_or_else(|| "{}".into());
        Self::FunctionCall {
            id: item_id.to_string(),
            call_id: call_id.to_string(),
            name: format!("agent_{}", sanitize_agent_name(title)),
            arguments: args,
            status: "in_progress".into(),
        }
    }

    pub fn text_content(&self) -> Option<&str> {
        match self {
            Self::Message { content, .. } => content
                .iter()
                .map(|c| match c {
                    OutputContentPart::OutputText { text, .. } => text.as_str(),
                })
                .next(),
            _ => None,
        }
    }
}

fn sanitize_agent_name(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(64)
        .collect()
}

pub fn new_item_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

pub fn new_call_id() -> String {
    format!("call_{}", uuid::Uuid::new_v4().simple())
}

pub fn new_response_id() -> String {
    format!("resp_{}", uuid::Uuid::new_v4().simple())
}
