use std::collections::HashMap;
use std::sync::Arc;

use async_stream::stream;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{sleep, Duration};

use crate::backend::{
    FinishReason, PromptEventStream, PromptStreamEvent, SupervisorBackend, UsageSnapshot,
};
use crate::supervisor::AgentPrompt;

#[derive(Debug, Clone)]
pub enum MockResponse {
    Text(String),
    Stream(Vec<String>),
    Error(String),
    /// Text response with optional reasoning and agent-tool events before Done.
    WithEvents {
        text: String,
        reasoning: Vec<String>,
        agent_tools: Vec<(String, String)>,
    },
    /// Pop responses in order per prompt_stream call (for tool loop tests).
    Sequence(Vec<MockResponse>),
}

pub struct MockSupervisor {
    responses: RwLock<HashMap<String, MockResponse>>,
    default: RwLock<MockResponse>,
    active: Mutex<HashMap<String, String>>,
    stream_delay: Duration,
    sequence_idx: Mutex<HashMap<String, usize>>,
}

impl MockSupervisor {
    pub fn new(default: MockResponse) -> Self {
        Self {
            responses: RwLock::new(HashMap::new()),
            default: RwLock::new(default),
            active: Mutex::new(HashMap::new()),
            stream_delay: Duration::from_millis(5),
            sequence_idx: Mutex::new(HashMap::new()),
        }
    }

    pub async fn set_response(&self, conversation_id: &str, response: MockResponse) {
        self.responses
            .write()
            .await
            .insert(conversation_id.to_string(), response);
    }

    pub async fn set_default(&self, response: MockResponse) {
        *self.default.write().await = response;
    }

    async fn resolve(&self, conversation_id: &str) -> MockResponse {
        if let Some(r) = self.responses.read().await.get(conversation_id) {
            return self.resolve_one(conversation_id, r.clone()).await;
        }
        let default = self.default.read().await.clone();
        self.resolve_one(conversation_id, default).await
    }

    async fn resolve_one(&self, conversation_id: &str, response: MockResponse) -> MockResponse {
        match response {
            MockResponse::Sequence(items) if !items.is_empty() => {
                let mut idx_map = self.sequence_idx.lock().await;
                let idx = idx_map.entry(conversation_id.to_string()).or_insert(0);
                let item = items.get(*idx).cloned().unwrap_or_else(|| {
                    items
                        .last()
                        .cloned()
                        .unwrap_or(MockResponse::Text(String::new()))
                });
                *idx = (*idx + 1).min(items.len());
                item
            }
            other => other,
        }
    }
}

#[async_trait::async_trait]
impl SupervisorBackend for MockSupervisor {
    async fn prompt(&self, conversation_id: &str, input: AgentPrompt) -> Result<String, String> {
        let events = self.prompt_stream(conversation_id, input).await?;
        let mut text = String::new();
        let mut events = events;
        use futures::StreamExt;
        while let Some(event) = events.next().await {
            match event {
                PromptStreamEvent::Delta(delta) => text.push_str(&delta),
                PromptStreamEvent::Done { full_text, .. } => {
                    text = full_text;
                    break;
                }
                PromptStreamEvent::Error(e) => return Err(e),
                PromptStreamEvent::Usage(_) => {}
                PromptStreamEvent::ReasoningDelta(_) | PromptStreamEvent::AgentToolCall(_) => {}
            }
        }
        Ok(text)
    }

    async fn prompt_stream(
        &self,
        conversation_id: &str,
        input: AgentPrompt,
    ) -> Result<PromptEventStream, String> {
        let response = self.resolve(conversation_id).await;
        self.active
            .lock()
            .await
            .insert(conversation_id.to_string(), input.model.clone());

        let prompt_chars = input.usage_prompt_chars;
        let delay = self.stream_delay;

        let stream = match response {
            MockResponse::Error(e) => Box::pin(stream! {
                yield PromptStreamEvent::Error(e);
            }) as PromptEventStream,
            MockResponse::Text(text) => {
                let usage = UsageSnapshot::estimate(prompt_chars, text.len());
                Box::pin(stream! {
                    yield PromptStreamEvent::Delta(text.clone());
                    yield PromptStreamEvent::Done {
                        finish_reason: FinishReason::Stop,
                        full_text: text,
                        usage,
                    };
                })
            }
            MockResponse::Stream(chunks) => {
                let full: String = chunks.join("");
                let usage = UsageSnapshot::estimate(prompt_chars, full.len());
                Box::pin(stream! {
                    for chunk in chunks {
                        sleep(delay).await;
                        yield PromptStreamEvent::Delta(chunk);
                    }
                    yield PromptStreamEvent::Done {
                        finish_reason: FinishReason::Stop,
                        full_text: full,
                        usage,
                    };
                })
            }
            MockResponse::WithEvents {
                text,
                reasoning,
                agent_tools,
            } => {
                let usage = UsageSnapshot::estimate(prompt_chars, text.len());
                let reasoning = reasoning.clone();
                let agent_tools = agent_tools.clone();
                let text_clone = text.clone();
                Box::pin(stream! {
                    for r in reasoning {
                        yield PromptStreamEvent::ReasoningDelta(r);
                    }
                    for (id, title) in agent_tools {
                        yield PromptStreamEvent::AgentToolCall(crate::backend::AgentToolEvent {
                            kind: crate::backend::AgentToolEventKind::Started,
                            tool_call_id: id,
                            title,
                            status: Some("in_progress".into()),
                            raw_input: None,
                            raw_output: None,
                        });
                    }
                    yield PromptStreamEvent::Delta(text_clone.clone());
                    yield PromptStreamEvent::Done {
                        finish_reason: FinishReason::Stop,
                        full_text: text,
                        usage,
                    };
                })
            }
            MockResponse::Sequence(_) => {
                unreachable!("Sequence must be resolved in resolve_one before prompt_stream")
            }
        };
        Ok(stream)
    }

    async fn is_session_active(&self, conversation_id: &str) -> bool {
        self.active.lock().await.contains_key(conversation_id)
    }

    async fn active_count(&self) -> usize {
        self.active.lock().await.len()
    }

    async fn shutdown_all(&self) {
        self.active.lock().await.clear();
    }
}

pub fn mock_supervisor(default: MockResponse) -> Arc<dyn SupervisorBackend> {
    Arc::new(MockSupervisor::new(default))
}
