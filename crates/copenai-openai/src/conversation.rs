use uuid::Uuid;

use crate::types::ChatCompletionRequest;

/// Resolve conversation id from header or metadata only — OpenAI `user` is not overloaded.
pub fn resolve_conversation_id(request: &ChatCompletionRequest, header: Option<&str>) -> String {
    if let Some(id) = header.map(str::trim).filter(|s| !s.is_empty()) {
        return id.to_string();
    }
    if let Some(meta) = &request.metadata {
        if let Some(id) = meta.get("conversation_id").and_then(|v| v.as_str()) {
            if !id.is_empty() {
                return id.to_string();
            }
        }
    }
    Uuid::new_v4().to_string()
}

pub fn openai_user_field(request: &ChatCompletionRequest) -> Option<String> {
    request.user.as_ref().filter(|u| !u.is_empty()).cloned()
}
