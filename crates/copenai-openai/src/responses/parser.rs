use super::input::InputItem;
use super::request::{ResponseCreateRequest, ResponseInput};
use crate::messages::{Turn, TurnContent};
use crate::tools::FunctionTool;
use crate::types::{ContentPart, MessageContent};

#[derive(Debug, Clone)]
pub struct ParsedResponse {
    pub system: String,
    pub history: Vec<Turn>,
    pub final_user_content: MessageContent,
    pub function_call_outputs: Vec<(String, String)>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tools: Vec<FunctionTool>,
}

pub fn parse_response_request(request: &ResponseCreateRequest) -> Result<ParsedResponse, String> {
    let items = match &request.input {
        ResponseInput::Text(t) if !t.is_empty() => {
            vec![InputItem::Message {
                role: "user".into(),
                content: super::input::InputMessageContent::Text(t.clone()),
            }]
        }
        ResponseInput::Text(_) => vec![],
        ResponseInput::Items(items) => items.clone(),
    };

    let mut system_parts = Vec::new();
    if let Some(instr) = &request.instructions {
        if !instr.is_empty() {
            system_parts.push(instr.clone());
        }
    }

    let mut history = Vec::new();
    let mut final_user = MessageContent::Text(String::new());
    let mut function_call_outputs = Vec::new();

    for item in &items {
        if let Some((call_id, output)) = item.function_call_output() {
            function_call_outputs.push((call_id.to_string(), output.to_string()));
            continue;
        }
        if item.function_call().is_some() {
            continue;
        }
        let Some(role) = item.role() else { continue };
        let Some(content) = item.to_message_content() else {
            continue;
        };
        match role {
            "system" | "developer" => {
                if let MessageContent::Text(t) = &content {
                    system_parts.push(t.clone());
                }
            }
            "user" => {
                if !matches!(final_user, MessageContent::Text(ref t) if t.is_empty()) {
                    history.push(Turn {
                        role: "user".into(),
                        content: TurnContent::Text(user_text_from_content(&std::mem::replace(
                            &mut final_user,
                            MessageContent::Text(String::new()),
                        ))),
                    });
                }
                final_user = content;
            }
            "assistant" => {
                history.push(Turn {
                    role: "assistant".into(),
                    content: TurnContent::Text(user_text_from_content(&content)),
                });
            }
            _ => {}
        }
    }

    let system = system_parts.join("\n\n");
    let tools = request.tools.clone().unwrap_or_default();

    Ok(ParsedResponse {
        system,
        history,
        final_user_content: final_user,
        function_call_outputs,
        temperature: request.temperature,
        max_tokens: request.max_output_tokens,
        tools,
    })
}

pub fn user_text_from_content(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(t) => t.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| match p {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::responses::request::ResponseInput;

    #[test]
    fn parse_simple_text_input() {
        let req = ResponseCreateRequest {
            model: "m".into(),
            input: ResponseInput::Text("hello".into()),
            instructions: None,
            stream: false,
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            metadata: None,
            truncation: None,
            include: None,
            reasoning: None,
            text: None,
            tools: None,
            tool_choice: None,
            parallel_tool_calls: None,
            previous_response_id: None,
            store: None,
            conversation: None,
        };
        let parsed = parse_response_request(&req).unwrap();
        assert_eq!(user_text_from_content(&parsed.final_user_content), "hello");
    }

    #[test]
    fn parse_multi_turn_with_output_text() {
        let json = serde_json::json!({
            "model": "composer-2.5",
            "stream": false,
            "input": [
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hi"}]},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "hello"}]},
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "again"}]}
            ]
        });
        let req: ResponseCreateRequest = serde_json::from_value(json).unwrap();
        let parsed = parse_response_request(&req).unwrap();
        assert!(parsed.history.iter().any(|t| t.role == "assistant"));
        assert_eq!(
            user_text_from_content(&parsed.final_user_content),
            "again"
        );
    }
}
