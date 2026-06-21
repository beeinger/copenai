use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::function_tool::FunctionTool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedToolChoice {
    Auto,
    None,
    Required,
    Named(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponsesToolChoice {
    Mode(String),
    Named { r#type: String, name: String },
}

/// Parse Responses API `tool_choice`.
pub fn from_responses_choice(choice: Option<&ResponsesToolChoice>) -> ResolvedToolChoice {
    match choice {
        None => ResolvedToolChoice::Auto,
        Some(ResponsesToolChoice::Mode(m)) => parse_mode_string(m),
        Some(ResponsesToolChoice::Named { name, .. }) => ResolvedToolChoice::Named(name.clone()),
    }
}

/// Parse Chat Completions `tool_choice` / legacy `function_call` Value.
pub fn from_chat_choice(
    tool_choice: Option<&Value>,
    function_call: Option<&Value>,
) -> ResolvedToolChoice {
    if let Some(tc) = tool_choice {
        return parse_chat_choice_value(tc);
    }
    if let Some(fc) = function_call {
        return parse_legacy_function_call(fc);
    }
    ResolvedToolChoice::Auto
}

fn parse_chat_choice_value(value: &Value) -> ResolvedToolChoice {
    if let Some(s) = value.as_str() {
        return parse_mode_string(s);
    }
    if let Some(obj) = value.as_object() {
        if let Some(name) = obj
            .get("function")
            .and_then(|f| f.get("name"))
            .and_then(|n| n.as_str())
            .or_else(|| obj.get("name").and_then(|n| n.as_str()))
        {
            return ResolvedToolChoice::Named(name.to_string());
        }
    }
    ResolvedToolChoice::Auto
}

fn parse_legacy_function_call(value: &Value) -> ResolvedToolChoice {
    if let Some(s) = value.as_str() {
        if s == "none" {
            return ResolvedToolChoice::None;
        }
        if s == "auto" {
            return ResolvedToolChoice::Auto;
        }
    }
    if let Some(obj) = value.as_object() {
        if let Some(name) = obj.get("name").and_then(|n| n.as_str()) {
            return ResolvedToolChoice::Named(name.to_string());
        }
    }
    ResolvedToolChoice::Auto
}

fn parse_mode_string(s: &str) -> ResolvedToolChoice {
    match s.to_lowercase().as_str() {
        "none" => ResolvedToolChoice::None,
        "required" => ResolvedToolChoice::Required,
        "auto" => ResolvedToolChoice::Auto,
        other if !other.is_empty() => ResolvedToolChoice::Named(other.to_string()),
        _ => ResolvedToolChoice::Auto,
    }
}

/// Filter tools per `tool_choice`; returns empty when `none`.
pub fn filter_tools(tools: &[FunctionTool], choice: &ResolvedToolChoice) -> Vec<FunctionTool> {
    match choice {
        ResolvedToolChoice::None => vec![],
        ResolvedToolChoice::Named(name) => tools
            .iter()
            .filter(|t| t.name == *name)
            .cloned()
            .collect(),
        _ => tools.to_vec(),
    }
}

pub fn require_tool_call(choice: &ResolvedToolChoice) -> bool {
    matches!(
        choice,
        ResolvedToolChoice::Required | ResolvedToolChoice::Named(_)
    )
}

pub fn tools_allowed(choice: &ResolvedToolChoice) -> bool {
    !matches!(choice, ResolvedToolChoice::None)
}

/// Validate detected calls against `tool_choice`.
pub fn validate_detected_calls(
    calls: &[super::detect::ParsedFunctionCall],
    choice: &ResolvedToolChoice,
    tools: &[FunctionTool],
) -> Result<(), String> {
    if matches!(choice, ResolvedToolChoice::None) && !calls.is_empty() {
        return Err("tool_choice is none but model emitted tool calls".into());
    }
    if require_tool_call(choice) && calls.is_empty() {
        return Err("tool_choice requires a tool call but none were detected".into());
    }
    if let ResolvedToolChoice::Named(name) = choice {
        for call in calls {
            if call.name != *name {
                return Err(format!(
                    "tool_choice requires tool {name} but got {}",
                    call.name
                ));
            }
        }
        if tools.iter().all(|t| t.name != *name) {
            return Err(format!("tool_choice names unknown tool: {name}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auto_none_required() {
        assert_eq!(
            from_responses_choice(Some(&ResponsesToolChoice::Mode("none".into()))),
            ResolvedToolChoice::None
        );
        assert_eq!(
            from_responses_choice(Some(&ResponsesToolChoice::Mode("required".into()))),
            ResolvedToolChoice::Required
        );
    }

    #[test]
    fn named_from_chat() {
        let tc = json!({"type": "function", "function": {"name": "foo"}});
        assert_eq!(
            from_chat_choice(Some(&tc), None),
            ResolvedToolChoice::Named("foo".into())
        );
    }

    #[test]
    fn filter_named() {
        let tools = vec![FunctionTool::new("a"), FunctionTool::new("b")];
        let filtered = filter_tools(&tools, &ResolvedToolChoice::Named("b".into()));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "b");
    }
}
