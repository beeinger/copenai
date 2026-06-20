use crate::multimodal::MappedContent;
use crate::types::{ChatCompletionRequest, MessageContent};

#[derive(Debug, Clone)]
pub struct Turn {
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ParsedChat {
    pub system: String,
    pub history: Vec<Turn>,
    pub final_user_content: MessageContent,
    pub openai_user: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
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
                if let Some(text) = msg.content.as_text() {
                    if !text.is_empty() {
                        system_parts.push(text);
                    }
                }
            }
            "user" | "assistant" => {
                if let Some(text) = msg.content.as_text() {
                    history.push(Turn {
                        role: msg.role.clone(),
                        text,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(ParsedChat {
        system: system_parts.join("\n\n"),
        history,
        final_user_content: request.messages[final_user_idx].content.clone(),
        openai_user: super::conversation::openai_user_field(request),
        temperature: request.temperature,
        max_tokens: request.max_tokens,
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
        lines.push(format!("{label}: {}", turn.text));
    }

    PromptPlan {
        system_prefix: None,
        replay_transcript: Some(lines.join("\n\n")),
        needs_replay: true,
    }
}

pub fn usage_char_count(parsed: &ParsedChat, mapped: &MappedContent) -> usize {
    parsed.system.len()
        + parsed.history.iter().map(|t| t.text.len()).sum::<usize>()
        + mapped.text.len()
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
            extra: Default::default(),
        }
    }

    #[test]
    fn merges_system_and_developer() {
        let parsed = parse_chat_request(&req(vec![
            ChatMessage {
                role: "system".into(),
                content: MessageContent::Text("be concise".into()),
            },
            ChatMessage {
                role: "developer".into(),
                content: MessageContent::Text("use rust".into()),
            },
            ChatMessage {
                role: "user".into(),
                content: MessageContent::Text("hi".into()),
            },
        ]))
        .unwrap();
        assert!(parsed.system.contains("be concise"));
        assert!(parsed.system.contains("use rust"));
        assert_eq!(parsed.openai_user, Some("track-me".into()));
    }

    #[test]
    fn builds_history_replay_on_cold_session() {
        let parsed = parse_chat_request(&req(vec![
            ChatMessage {
                role: "user".into(),
                content: MessageContent::Text("first".into()),
            },
            ChatMessage {
                role: "assistant".into(),
                content: MessageContent::Text("reply".into()),
            },
            ChatMessage {
                role: "user".into(),
                content: MessageContent::Text("second".into()),
            },
        ]))
        .unwrap();
        assert_eq!(parsed.history.len(), 2);
        let plan = build_prompt_plan(&parsed, false);
        assert!(plan.needs_replay);
        assert!(plan.replay_transcript.unwrap().contains("User: first"));
    }

    #[test]
    fn hot_session_skips_replay_for_incremental_turn() {
        let parsed = parse_chat_request(&req(vec![ChatMessage {
            role: "user".into(),
            content: MessageContent::Text("only".into()),
        }]))
        .unwrap();
        let plan = build_prompt_plan(&parsed, true);
        assert!(!plan.needs_replay);
    }

    #[test]
    fn hot_session_replays_when_history_present() {
        let parsed = parse_chat_request(&req(vec![
            ChatMessage {
                role: "user".into(),
                content: MessageContent::Text("My name is Bob.".into()),
            },
            ChatMessage {
                role: "assistant".into(),
                content: MessageContent::Text("Hi Bob.".into()),
            },
            ChatMessage {
                role: "user".into(),
                content: MessageContent::Text("What is my name?".into()),
            },
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
