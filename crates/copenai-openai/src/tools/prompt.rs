use serde_json::Value;

use super::function_tool::FunctionTool;

pub fn build_tool_system_prompt(tools: &[FunctionTool]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    let defs: Vec<Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            })
        })
        .collect();
    format!(
        "You have access to the following tools. When you need to call a tool, respond ONLY with valid JSON in one of these forms:\n\
        {{\"tool_calls\": [{{\"name\": \"<tool_name>\", \"arguments\": {{...}}}}]}}\n\
        or {{\"name\": \"<tool_name>\", \"arguments\": {{...}}}}\n\
        Do not include markdown unless wrapping in ```json fences.\n\
        Available tools:\n{}",
        serde_json::to_string_pretty(&defs).unwrap_or_default()
    )
}

pub fn build_json_schema_prompt(schema: &Value) -> String {
    format!(
        "Respond with JSON matching this schema (no markdown, raw JSON only):\n{}",
        serde_json::to_string_pretty(schema).unwrap_or_default()
    )
}

pub fn build_schema_retry_prompt(schema: &Value, error: &str) -> String {
    format!(
        "Your previous response did not match the required JSON schema.\n\
        Error: {error}\n\
        Respond again with valid JSON matching this schema (no markdown):\n{}",
        serde_json::to_string_pretty(schema).unwrap_or_default()
    )
}
