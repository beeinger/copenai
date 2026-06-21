use crate::multimodal::MappedContent;
use crate::tools::{
    filter_tools, format_tool_history, from_chat_choice, parse_chat_tools, FunctionTool,
    ResolvedToolChoice,
};
use crate::types::{ChatCompletionRequest, MessageContent};

#[derive(Debug, Clone)]
pub enum TurnContent {
    Text(String),
    Parts(crate::types::MessageContent),
}

impl TurnContent {
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(t) => t.clone(),
            Self::Parts(c) => c.as_text().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Turn {
    pub role: String,
    pub content: TurnContent,
}

#[derive(Debug, Clone)]
pub struct ParsedChat {
    pub system: String,
    pub history: Vec<Turn>,
    pub final_user_content: MessageContent,
    pub openai_user: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub tools: Vec<FunctionTool>,
    pub tool_choice: ResolvedToolChoice,
    pub tool_results: Vec<(String, String)>,
    pub parallel_tool_calls: bool,
}

#[derive(Debug, Clone)]
pub struct PromptPlan {
    pub system_prefix: Option<String>,
    pub replay_transcript: Option<String>,
    pub needs_replay: bool,
}

pub fn parse_chat_request(request: &ChatCompletionRequest) -> Result<ParsedChat, String> {
    if request.messages.is_empty() {
        return Err("messages must not be empty".into());
    }

    let chat_tools = parse_chat_tools(request)?;
    let tool_choice =
        from_chat_choice(request.tool_choice.as_ref(), request.function_call.as_ref());
    let tools = filter_tools(&chat_tools.tools, &tool_choice);

    let final_user_idx = request
        .messages
        .iter()
        .rposition(|m| m.role == "user")
        .ok_or_else(|| "messages must include a user turn".to_string())?;

    let mut system_parts = Vec::new();
    let mut history = Vec::new();

    for (idx, msg) in request.messages.iter().enumerate() {
        if idx == final_user_idx {
            break;
        }
        match msg.role.as_str() {
            "system" | "developer" => {
                if let Some(text) = msg.content.as_ref().and_then(|c| c.as_text()) {
                    if !text.is_empty() {
                        system_parts.push(text);
                    }
                }
            }
            "user" | "assistant" => {
                let content = msg.content_or_default();
                let turn_content = if let Some(text) = content.as_text() {
                    TurnContent::Text(text)
                } else {
                    TurnContent::Parts(content)
                };
                history.push(Turn {
                    role: msg.role.clone(),
                    content: turn_content,
                });
            }
            "tool" => {}
            _ => {}
        }
    }

    let tool_history = format_tool_history(
        &chat_tools.history_tool_calls,
        &chat_tools
            .tool_results
            .iter()
            .map(|t| crate::tools::ToolResultMessage {
                tool_call_id: t.tool_call_id.clone(),
                content: t.content.clone(),
            })
            .collect::<Vec<_>>(),
    );
    if !tool_history.is_empty() {
        system_parts.push(tool_history);
    }

    let tool_results: Vec<(String, String)> = chat_tools
        .tool_results
        .into_iter()
        .map(|t| (t.tool_call_id, t.content))
        .collect();

    Ok(ParsedChat {
        system: system_parts.join("\n\n"),
        history,
        final_user_content: request.messages[final_user_idx].content_or_default(),
        openai_user: super::conversation::openai_user_field(request),
        temperature: request.temperature,
        max_tokens: request.max_tokens,
        tools,
        tool_choice,
        tool_results,
        parallel_tool_calls: request.parallel_tool_calls(),
    })
}

pub fn build_prompt_plan(parsed: &ParsedChat, session_hot: bool) -> PromptPlan {
    // Hot ACP session: skip replay only for incremental turns (no prefix history).
    // OpenAI messages[] is authoritative when the client sends prior turns.
    if session_hot && parsed.history.is_empty() {
        return PromptPlan {
            system_prefix: if parsed.system.is_empty() {
                None
            } else {
                Some(parsed.system.clone())
            },
            replay_transcript: None,
            needs_replay: false,
        };
    }

    if parsed.history.is_empty() && parsed.system.is_empty() {
        return PromptPlan {
            system_prefix: None,
            replay_transcript: None,
            needs_replay: false,
        };
    }

    let mut lines = Vec::new();
    if !parsed.system.is_empty() {
        lines.push(format!("[System]\n{}", parsed.system));
    }
    for turn in &parsed.history {
        let label = if turn.role == "assistant" {
            "Assistant"
        } else {
            "User"
        };
        lines.push(format!("{label}: {}", turn.content.as_text()));
    }

    PromptPlan {
        system_prefix: None,
        replay_transcript: Some(lines.join("\n\n")),
        needs_replay: true,
    }
}

pub fn usage_char_count(parsed: &ParsedChat, mapped: &MappedContent) -> usize {
    parsed.system.len()
        + parsed
            .history
            .iter()
            .map(|t| t.content.as_text().len())
            .sum::<usize>()
        + mapped.text.len()
}

/// Trim history to fit within char budget (truncation: auto).
pub fn truncate_history(history: &mut Vec<Turn>, budget: usize) {
    let mut total: usize = history.iter().map(|t| t.content.as_text().len()).sum();
    while total > budget && !history.is_empty() {
        let removed = history.remove(0);
        total = total.saturating_sub(removed.content.as_text().len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;

    fn req(messages: Vec<ChatMessage>) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "composer-2.5".into(),
            messages,
            stream: false,
            user: Some("track-me".into()),
            metadata: None,
            temperature: None,
            max_tokens: None,
            tools: None,
            tool_choice: None,
            functions: None,
            function_call: None,
            parallel_tool_calls: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn merges_system_and_developer() {
        let parsed = parse_chat_request(&req(vec![
            ChatMessage {
                role: "system".into(),
                name: None,
                tool_call_id: None,
                tool_calls: None,
                content: Some(MessageContent::Text("be concise".into())),
            },
            ChatMessage {
                role: "developer".into(),
                name: None,
                tool_call_id: None,
                tool_calls: None,
                content: Some(MessageContent::Text("use rust".into())),
            },
            ChatMessage {
                role: "user".into(),
                name: None,
                tool_call_id: None,
                tool_calls: None,
                content: Some(MessageContent::Text("hi".into())),
            },
        ]))
        .unwrap();
        assert!(parsed.system.contains("be concise"));
        assert!(parsed.system.contains("use rust"));
        assert_eq!(parsed.openai_user, Some("track-me".into()));
    }

    fn user_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: "user".into(),
            name: None,
            tool_call_id: None,
            tool_calls: None,
            content: Some(MessageContent::Text(text.into())),
        }
    }

    fn assistant_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: "assistant".into(),
            name: None,
            tool_call_id: None,
            tool_calls: None,
            content: Some(MessageContent::Text(text.into())),
        }
    }

    #[test]
    fn builds_history_replay_on_cold_session() {
        let parsed = parse_chat_request(&req(vec![
            user_msg("first"),
            assistant_msg("reply"),
            user_msg("second"),
        ]))
        .unwrap();
        assert_eq!(parsed.history.len(), 2);
        let plan = build_prompt_plan(&parsed, false);
        assert!(plan.needs_replay);
        assert!(plan.replay_transcript.unwrap().contains("User: first"));
    }

    #[test]
    fn hot_session_skips_replay_for_incremental_turn() {
        let parsed = parse_chat_request(&req(vec![user_msg("only")])).unwrap();
        let plan = build_prompt_plan(&parsed, true);
        assert!(!plan.needs_replay);
    }

    #[test]
    fn hot_session_replays_when_history_present() {
        let parsed = parse_chat_request(&req(vec![
            user_msg("My name is Bob."),
            assistant_msg("Hi Bob."),
            user_msg("What is my name?"),
        ]))
        .unwrap();
        let plan = build_prompt_plan(&parsed, true);
        assert!(plan.needs_replay);
        let transcript = plan.replay_transcript.unwrap();
        assert!(transcript.contains("User: My name is Bob."));
        assert!(transcript.contains("Assistant: Hi Bob."));
    }

    #[test]
    fn empty_messages_rejected() {
        assert!(parse_chat_request(&req(vec![])).is_err());
    }
}
