use serde::{Deserialize, Serialize};

use crate::types::{ContentPart, MessageContent};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputItem {
    #[serde(rename = "message")]
    Message {
        role: String,
        #[serde(default)]
        content: InputMessageContent,
    },
    FunctionCall {
        id: Option<String>,
        call_id: String,
        name: String,
        arguments: String,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputMessageContent {
    Text(String),
    Parts(Vec<InputContentPart>),
}

impl Default for InputMessageContent {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputContentPart {
    InputText {
        text: String,
    },
    InputImage {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
    },
    InputFile {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
    InputAudio {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_id: Option<String>,
    },
    /// Assistant history items from @ai-sdk/openai multi-turn `/v1/responses` requests.
    OutputText {
        text: String,
    },
}

impl InputContentPart {
    pub fn to_content_part(&self) -> ContentPart {
        match self {
            Self::InputText { text } => ContentPart::Text { text: text.clone() },
            Self::InputImage { image_url, .. } => ContentPart::ImageUrl {
                image_url: crate::types::ImageUrl {
                    url: image_url.clone().unwrap_or_default(),
                    detail: None,
                },
            },
            Self::InputFile { file_id, .. } => ContentPart::InputFile {
                file_id: file_id.clone().unwrap_or_default(),
            },
            Self::InputAudio { data, format, .. } => ContentPart::InputAudio {
                input_audio: crate::types::InputAudio {
                    data: data.clone().unwrap_or_default(),
                    format: format.clone().unwrap_or_else(|| "wav".into()),
                },
            },
            Self::OutputText { text } => ContentPart::Text { text: text.clone() },
        }
    }
}

impl InputItem {
    pub fn to_message_content(&self) -> Option<MessageContent> {
        match self {
            Self::Message { content, .. } => Some(match content {
                InputMessageContent::Text(t) => MessageContent::Text(t.clone()),
                InputMessageContent::Parts(parts) => MessageContent::Parts(
                    parts
                        .iter()
                        .map(InputContentPart::to_content_part)
                        .collect(),
                ),
            }),
            _ => None,
        }
    }

    pub fn role(&self) -> Option<&str> {
        match self {
            Self::Message { role, .. } => Some(role.as_str()),
            _ => None,
        }
    }

    pub fn function_call_output(&self) -> Option<(&str, &str)> {
        match self {
            Self::FunctionCallOutput { call_id, output } => {
                Some((call_id.as_str(), output.as_str()))
            }
            _ => None,
        }
    }

    pub fn function_call(&self) -> Option<(&str, &str, &str)> {
        match self {
            Self::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => Some((call_id.as_str(), name.as_str(), arguments.as_str())),
            _ => None,
        }
    }
}

/// Re-export for backward compatibility with responses input module.
pub use crate::tools::ParsedFunctionCall;
