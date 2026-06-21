use crate::types::{ChatCompletionRequest, ChatMessage};

use super::function_tool::{from_chat_request, FunctionTool};

/// Tool result from a `role: tool` message.
#[derive(Debug, Clone)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub content: String,
}

/// Assistant tool_calls from history (for replay context).
#[derive(Debug, Clone)]
pub struct HistoryToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct ParsedChatTools {
    pub tools: Vec<FunctionTool>,
    pub tool_results: Vec<ToolResultMessage>,
    pub history_tool_calls: Vec<HistoryToolCall>,
}

/// Extract tools and tool-related messages from a chat request.
pub fn parse_chat_tools(request: &ChatCompletionRequest) -> Result<ParsedChatTools, String> {
    let tools = from_chat_request(request.tools.as_ref(), request.functions.as_ref())?;
    let mut tool_results = Vec::new();
    let mut history_tool_calls = Vec::new();

    for msg in &request.messages {
        match msg.role.as_str() {
            "tool" => {
                if let Some(result) = parse_tool_message(msg)? {
                    tool_results.push(result);
                }
            }
            "assistant" => {
                history_tool_calls.extend(parse_assistant_tool_calls(msg));
            }
            _ => {}
        }
    }

    Ok(ParsedChatTools {
        tools,
        tool_results,
        history_tool_calls,
    })
}

fn parse_tool_message(msg: &ChatMessage) -> Result<Option<ToolResultMessage>, String> {
    let tool_call_id = msg
        .tool_call_id
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "tool message missing tool_call_id".to_string())?;
    let content = msg.content_or_default();
    let text = content.as_text().unwrap_or_default();
    Ok(Some(ToolResultMessage {
        tool_call_id,
        content: text,
    }))
}

fn parse_assistant_tool_calls(msg: &ChatMessage) -> Vec<HistoryToolCall> {
    msg.tool_calls
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|tc| HistoryToolCall {
            id: tc.id,
            name: tc.function.name,
            arguments: tc.function.arguments,
        })
        .collect()
}

/// Build replay text for prior assistant tool_calls + tool results.
pub fn format_tool_history(
    tool_calls: &[HistoryToolCall],
    tool_results: &[ToolResultMessage],
) -> String {
    let mut lines = Vec::new();
    for tc in tool_calls {
        lines.push(format!(
            "Assistant tool_call id={} name={} arguments={}",
            tc.id, tc.name, tc.arguments
        ));
    }
    for tr in tool_results {
        lines.push(format!(
            "Tool result call_id={}: {}",
            tr.tool_call_id, tr.content
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatCompletionRequest, MessageContent};
    use serde_json::json;

    fn req(messages: Vec<ChatMessage>, tools: Option<serde_json::Value>) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "m".into(),
            messages,
            stream: false,
            user: None,
            metadata: None,
            temperature: None,
            max_tokens: None,
            tools,
            tool_choice: None,
            functions: None,
            function_call: None,
            parallel_tool_calls: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn parse_tools_from_request() {
        let tools = json!([{
            "type": "function",
            "function": { "name": "foo" }
        }]);
        let parsed = parse_chat_tools(&req(vec![], Some(tools))).unwrap();
        assert_eq!(parsed.tools[0].name, "foo");
    }

    #[test]
    fn parse_tool_role_message() {
        let messages = vec![ChatMessage {
            role: "tool".into(),
            name: None,
            tool_call_id: Some("call_1".into()),
            tool_calls: None,
            content: Some(MessageContent::Text("sunny".into())),
        }];
        let parsed = parse_chat_tools(&req(messages, None)).unwrap();
        assert_eq!(parsed.tool_results[0].tool_call_id, "call_1");
        assert_eq!(parsed.tool_results[0].content, "sunny");
    }
}
