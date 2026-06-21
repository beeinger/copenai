use std::pin::Pin;

use futures::{Stream, StreamExt};

use crate::resume::ResumeMode;
use crate::supervisor::AgentPrompt;

#[derive(Debug, Clone, Default)]
pub struct UsageSnapshot {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl UsageSnapshot {
    pub fn from_acp(used: u64, size: u64) -> Self {
        let used = used.min(u64::from(u32::MAX)) as u32;
        let size = size.min(u64::from(u32::MAX)) as u32;
        Self {
            prompt_tokens: used,
            completion_tokens: 0,
            total_tokens: used.max(size),
        }
    }

    pub fn estimate(prompt_chars: usize, completion_chars: usize) -> Self {
        let prompt_tokens = (prompt_chars / 4).max(1) as u32;
        let completion_tokens = (completion_chars / 4) as u32;
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    Length,
    ContentFilter,
}

impl FinishReason {
    pub fn as_openai_str(&self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Length => "length",
            Self::ContentFilter => "content_filter",
        }
    }

    pub fn from_acp_stop(reason: agent_client_protocol::schema::v1::StopReason) -> Self {
        use agent_client_protocol::schema::v1::StopReason;
        match reason {
            StopReason::MaxTokens => Self::Length,
            StopReason::Refusal => Self::ContentFilter,
            StopReason::EndTurn | StopReason::MaxTurnRequests | StopReason::Cancelled => Self::Stop,
            _ => Self::Stop,
        }
    }
}

/// Cursor agent internal tool call (ACP observability, not request `tools[]`).
#[derive(Debug, Clone)]
pub enum AgentToolEventKind {
    Started,
    Updated,
}

#[derive(Debug, Clone)]
pub struct AgentToolEvent {
    pub kind: AgentToolEventKind,
    pub tool_call_id: String,
    pub title: String,
    pub status: Option<String>,
    pub raw_input: Option<serde_json::Value>,
    pub raw_output: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum PromptStreamEvent {
    Delta(String),
    ReasoningDelta(String),
    AgentToolCall(AgentToolEvent),
    Usage(UsageSnapshot),
    Done {
        finish_reason: FinishReason,
        full_text: String,
        usage: UsageSnapshot,
    },
    Error(String),
}

pub type PromptEventStream = Pin<Box<dyn Stream<Item = PromptStreamEvent> + Send>>;

#[async_trait::async_trait]
pub trait SupervisorBackend: Send + Sync {
    async fn prompt(&self, conversation_id: &str, input: AgentPrompt) -> Result<String, String>;

    async fn prompt_stream(
        &self,
        conversation_id: &str,
        input: AgentPrompt,
    ) -> Result<PromptEventStream, String>;

    async fn is_session_active(&self, conversation_id: &str) -> bool;

    async fn active_count(&self) -> usize;

    async fn shutdown_all(&self);

    async fn set_resume_mode(&self, mode: Option<ResumeMode>) {
        let _ = mode;
    }
}

pub(crate) async fn collect_stream(
    mut events: PromptEventStream,
) -> Result<(String, UsageSnapshot, FinishReason), String> {
    let mut text = String::new();
    let mut usage = UsageSnapshot::default();
    let mut finish = FinishReason::Stop;
    while let Some(event) = events.next().await {
        match event {
            PromptStreamEvent::Delta(delta) => text.push_str(&delta),
            PromptStreamEvent::ReasoningDelta(_) | PromptStreamEvent::AgentToolCall(_) => {}
            PromptStreamEvent::Usage(u) => usage = u,
            PromptStreamEvent::Done {
                finish_reason,
                full_text,
                usage: u,
            } => {
                text = full_text;
                usage = u;
                finish = finish_reason;
                break;
            }
            PromptStreamEvent::Error(e) => return Err(e),
        }
    }
    Ok((text, usage, finish))
}
