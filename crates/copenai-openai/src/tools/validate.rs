use std::collections::HashSet;

use serde_json::Value;

use super::function_tool::FunctionTool;

pub fn validate_tool_arguments_with_schema(
    name: &str,
    arguments: &Value,
    tools: &[FunctionTool],
) -> Result<(), String> {
    let tool = tools
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| format!("unknown tool name: {name}"))?;

    if let Some(schema) = &tool.parameters {
        let compiled = jsonschema::validator_for(schema)
            .map_err(|e| format!("invalid tool schema for {name}: {e}"))?;
        if let Err(err) = compiled.validate(arguments) {
            return Err(format!("invalid arguments for {name}: {err}"));
        }
    }
    Ok(())
}

/// Validate against tool name set only (used during JSON parse before full schema check).
pub fn validate_tool_name(name: &str, tool_names: &HashSet<&str>) -> Result<(), String> {
    if !tool_names.contains(name) {
        return Err(format!("unknown tool name: {name}"));
    }
    Ok(())
}

pub fn validate_json_output(text: &str, schema: &Value) -> Result<Value, String> {
    let trimmed = text.trim();
    let value: Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("structured output is not valid JSON: {e}"))?;
    let compiled =
        jsonschema::validator_for(schema).map_err(|e| format!("invalid json_schema: {e}"))?;
    if let Err(err) = compiled.validate(&value) {
        return Err(format!("output does not match schema: {err}"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_rejects_bad_args() {
        let tools = vec![FunctionTool {
            tool_type: "function".into(),
            name: "foo".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": { "x": { "type": "string" } },
                "required": ["x"]
            })),
            strict: None,
        }];
        let args = json!({});
        assert!(validate_tool_arguments_with_schema("foo", &args, &tools).is_err());
    }
}
