use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::function_tool::FunctionTool;
use super::validate::{validate_tool_arguments_with_schema, validate_tool_name};
use crate::responses::output::new_call_id;

/// Parsed tool call extracted from agent structured output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedFunctionCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

/// Detect structured tool calls from agent text output.
pub fn detect_tool_calls(
    text: &str,
    tools: &[FunctionTool],
) -> Result<Vec<ParsedFunctionCall>, String> {
    if tools.is_empty() {
        return Ok(vec![]);
    }

    let trimmed = text.trim();
    let json_value = extract_json_payload(trimmed)?;
    let calls = parse_tool_calls_from_json(&json_value, tools)?;
    Ok(calls)
}

pub fn parse_and_validate_calls(
    text: &str,
    tools: &[FunctionTool],
) -> Result<Vec<ParsedFunctionCall>, String> {
    let calls = detect_tool_calls(text, tools)?;
    for call in &calls {
        validate_tool_arguments_with_schema(&call.name, &call.arguments, tools)?;
    }
    Ok(calls)
}

fn extract_json_payload(text: &str) -> Result<Value, String> {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return Ok(v);
    }
    if let Some(start) = text.find("```json") {
        let rest = &text[start + 7..];
        if let Some(end) = rest.find("```") {
            let inner = rest[..end].trim();
            if let Ok(v) = serde_json::from_str(inner) {
                return Ok(v);
            }
        }
    }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        if start < end {
            let slice = &text[start..=end];
            if let Ok(v) = serde_json::from_str(slice) {
                return Ok(v);
            }
        }
    }
    Ok(Value::Null)
}

fn parse_tool_calls_from_json(
    value: &Value,
    tools: &[FunctionTool],
) -> Result<Vec<ParsedFunctionCall>, String> {
    if value.is_null() {
        return Ok(vec![]);
    }

    let tool_names: HashSet<&str> = tools.iter().map(|t| t.name.as_str()).collect();

    let mut calls = Vec::new();

    let items = match value {
        Value::Object(map) if map.contains_key("tool_calls") => {
            map.get("tool_calls").and_then(|v| v.as_array())
        }
        Value::Object(map) if map.contains_key("name") => {
            if let Some(call) = single_call_from_object(map, &tool_names)? {
                calls.push(call);
            }
            return Ok(calls);
        }
        Value::Array(arr) => Some(arr),
        _ => None,
    };

    if let Some(arr) = items {
        for item in arr {
            if let Some(obj) = item.as_object() {
                if let Some(call) = single_call_from_object(obj, &tool_names)? {
                    calls.push(call);
                }
            }
        }
    }

    Ok(calls)
}

fn single_call_from_object(
    obj: &serde_json::Map<String, Value>,
    tool_names: &HashSet<&str>,
) -> Result<Option<ParsedFunctionCall>, String> {
    let name = obj
        .get("name")
        .or_else(|| obj.get("function").and_then(|f| f.get("name")))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "tool call missing name".to_string())?;

    if !tool_names.contains(name) {
        return Err(format!("unknown tool name: {name}"));
    }

    let arguments = obj
        .get("arguments")
        .or_else(|| obj.get("function").and_then(|f| f.get("arguments")))
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    let args_value = match &arguments {
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::String(s.clone())),
        other => other.clone(),
    };

    validate_tool_name(name, tool_names)?;

    let call_id = obj
        .get("call_id")
        .or_else(|| obj.get("id"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(new_call_id);

    Ok(Some(ParsedFunctionCall {
        call_id,
        name: name.to_string(),
        arguments: args_value,
    }))
}

pub fn ensure_call_ids(calls: &mut [ParsedFunctionCall]) {
    for call in calls.iter_mut() {
        if call.call_id.is_empty() {
            call.call_id = new_call_id();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_single_call() {
        let tools = vec![FunctionTool::new("get_weather")];
        let text = r#"{"name": "get_weather", "arguments": {"location": "Boston"}}"#;
        let calls = detect_tool_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_weather");
    }

    #[test]
    fn detect_tool_calls_array() {
        let tools = vec![FunctionTool::new("a"), FunctionTool::new("b")];
        let text = r#"{"tool_calls": [{"name": "a", "arguments": {}}, {"name": "b", "arguments": {}}]}"#;
        let calls = detect_tool_calls(text, &tools).unwrap();
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn unknown_name_errors() {
        let tools = vec![FunctionTool::new("foo")];
        let text = r#"{"name": "bar", "arguments": {}}"#;
        assert!(detect_tool_calls(text, &tools).is_err());
    }
}
