use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::output::OutputItem;
use crate::types::Usage;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Queued,
    InProgress,
    Completed,
    Incomplete,
    Failed,
    Cancelled,
}

impl ResponseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Incomplete => "incomplete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseObject {
    pub id: String,
    pub object: String,
    pub created_at: i64,
    pub model: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<OutputItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incomplete_details: Option<IncompleteDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_response_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncompleteDetails {
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseList {
    pub object: String,
    pub data: Vec<ResponseObject>,
    pub first_id: String,
    pub last_id: String,
    pub has_more: bool,
}

impl ResponseObject {
    pub fn new(id: String, model: String, created_at: i64) -> Self {
        Self {
            id,
            object: "response".into(),
            created_at,
            model,
            status: ResponseStatus::InProgress.as_str().into(),
            output: vec![],
            usage: None,
            error: None,
            incomplete_details: None,
            metadata: None,
            previous_response_id: None,
        }
    }

    pub fn assistant_text(&self) -> String {
        self.output
            .iter()
            .filter_map(|o| o.text_content())
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn has_function_calls(&self) -> bool {
        self.output
            .iter()
            .any(|o| matches!(o, OutputItem::FunctionCall { .. }))
    }
}
