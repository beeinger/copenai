use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

impl FunctionTool {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            tool_type: "function".into(),
            name: name.into(),
            description: None,
            parameters: None,
            strict: None,
        }
    }
}

/// Parse Responses API `tools[]` entries (flat `name` field).
pub fn from_responses_tools(tools: &[FunctionTool]) -> Vec<FunctionTool> {
    tools.to_vec()
}

/// Parse Chat Completions `tools[]` (`{type, function:{name,...}}`) or legacy `functions[]`.
pub fn from_chat_request(
    tools: Option<&Value>,
    functions: Option<&Value>,
) -> Result<Vec<FunctionTool>, String> {
    let mut out = Vec::new();

    if let Some(tools_val) = tools {
        let arr = tools_val
            .as_array()
            .ok_or_else(|| "tools must be an array".to_string())?;
        for item in arr {
            out.push(parse_chat_tool_entry(item)?);
        }
    }

    if let Some(functions_val) = functions {
        let arr = functions_val
            .as_array()
            .ok_or_else(|| "functions must be an array".to_string())?;
        for item in arr {
            out.push(parse_legacy_function(item)?);
        }
    }

    Ok(out)
}

fn parse_chat_tool_entry(item: &Value) -> Result<FunctionTool, String> {
    let obj = item
        .as_object()
        .ok_or_else(|| "tool entry must be an object".to_string())?;

    if let Some(func) = obj.get("function") {
        return parse_legacy_function(func);
    }

    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "tool missing name".to_string())?;

    Ok(FunctionTool {
        tool_type: obj
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("function")
            .to_string(),
        name: name.to_string(),
        description: obj
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from),
        parameters: obj.get("parameters").cloned(),
        strict: obj.get("strict").and_then(|v| v.as_bool()),
    })
}

fn parse_legacy_function(item: &Value) -> Result<FunctionTool, String> {
    let obj = item
        .as_object()
        .ok_or_else(|| "function entry must be an object".to_string())?;
    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "function missing name".to_string())?;
    Ok(FunctionTool {
        tool_type: "function".into(),
        name: name.to_string(),
        description: obj
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from),
        parameters: obj.get("parameters").cloned(),
        strict: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_chat_tools_array() {
        let tools = json!([{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "weather",
                "parameters": { "type": "object" }
            }
        }]);
        let parsed = from_chat_request(Some(&tools), None).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "get_weather");
    }

    #[test]
    fn parse_legacy_functions() {
        let functions = json!([{
            "name": "foo",
            "description": "bar"
        }]);
        let parsed = from_chat_request(None, Some(&functions)).unwrap();
        assert_eq!(parsed[0].name, "foo");
    }
}
