use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::ContentBlock;
use async_stream::stream;
use copenai_core::config::AppConfig;
use copenai_core::cursor::CursorAuth;
use copenai_core::paths::DataPaths;
use copenai_openai::multimodal::{read_session_meta, MappedContent};
use copenai_store::conversations::ConversationStore;
use copenai_store::Store;
use tokio::sync::{mpsc, RwLock, Semaphore};
use tokio::time::Instant;
use tracing::info;

use crate::backend::{collect_stream, PromptEventStream, PromptStreamEvent, SupervisorBackend};
use crate::prompt::build_content_blocks;
use crate::resume::ResumeMode;
use crate::worker::{spawn_worker, WorkerCommand, WorkerConfig};

#[derive(Clone)]
pub struct SupervisorConfig {
    pub paths: DataPaths,
    pub config: AppConfig,
    pub auth: CursorAuth,
    pub store: Store,
    pub resume_mode: Option<ResumeMode>,
}

pub struct AgentPrompt {
    pub model: String,
    pub mapped: MappedContent,
    pub system_prefix: Option<String>,
    pub replay_transcript: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub usage_prompt_chars: usize,
}

impl AgentPrompt {
    pub fn prompt_char_count(&self) -> usize {
        self.usage_prompt_chars
            + self.system_prefix.as_deref().unwrap_or("").len()
            + self.replay_transcript.as_deref().unwrap_or("").len()
            + self.mapped.text.len()
    }
}

pub struct PromptStreamItem {
    pub delta: String,
    pub done: bool,
}

// Legacy type kept for API stability; prefer PromptStreamEvent.

struct ActiveConversation {
    cmd_tx: tokio::sync::mpsc::Sender<WorkerCommand>,
    model: String,
    last_active: Instant,
}

impl Clone for ActiveConversation {
    fn clone(&self) -> Self {
        Self {
            cmd_tx: self.cmd_tx.clone(),
            model: self.model.clone(),
            last_active: self.last_active,
        }
    }
}

pub struct ConversationSupervisor {
    inner: Arc<SupervisorInner>,
}

struct SupervisorInner {
    config: SupervisorConfig,
    conversations: RwLock<HashMap<String, ActiveConversation>>,
    semaphore: Semaphore,
    resume_mode: RwLock<Option<ResumeMode>>,
}

impl ConversationSupervisor {
    pub fn new(config: SupervisorConfig, resume_mode: Option<ResumeMode>) -> Self {
        let max = config.config.server.max_concurrent_agents;
        Self {
            inner: Arc::new(SupervisorInner {
                config,
                conversations: RwLock::new(HashMap::new()),
                semaphore: Semaphore::new(max),
                resume_mode: RwLock::new(resume_mode),
            }),
        }
    }

    pub async fn set_resume_mode(&self, mode: Option<ResumeMode>) {
        *self.inner.resume_mode.write().await = mode;
    }

    async fn build_blocks(input: &AgentPrompt) -> Result<Vec<ContentBlock>, String> {
        let mut blocks = Vec::new();
        if let Some(system) = &input.system_prefix {
            if !system.is_empty() {
                blocks.push(agent_client_protocol::schema::v1::ContentBlock::Text(
                    agent_client_protocol::schema::v1::TextContent::new(format!(
                        "[System instructions]\n{system}"
                    )),
                ));
            }
        }
        if let Some(replay) = &input.replay_transcript {
            if !replay.is_empty() {
                blocks.push(agent_client_protocol::schema::v1::ContentBlock::Text(
                    agent_client_protocol::schema::v1::TextContent::new(replay.clone()),
                ));
            }
        }
        let user_blocks = build_content_blocks(&input.mapped).await?;
        blocks.extend(user_blocks);
        Ok(blocks)
    }

    async fn send_prompt(
        &self,
        conversation_id: &str,
        input: AgentPrompt,
    ) -> Result<mpsc::Receiver<PromptStreamEvent>, String> {
        let blocks = Self::build_blocks(&input).await?;
        let client_replay = input.replay_transcript.clone();
        let handle = self
            .get_or_start(conversation_id, &input.model, client_replay)
            .await?;
        let (stream_tx, stream_rx) = mpsc::channel(64);
        handle
            .cmd_tx
            .send(WorkerCommand::Prompt {
                blocks,
                temperature: input.temperature,
                max_tokens: input.max_tokens,
                prompt_chars: input.prompt_char_count(),
                stream_tx,
            })
            .await
            .map_err(|_| "worker unavailable".to_string())?;
        Ok(stream_rx)
    }
}

#[async_trait::async_trait]
impl SupervisorBackend for ConversationSupervisor {
    async fn prompt(&self, conversation_id: &str, input: AgentPrompt) -> Result<String, String> {
        let stream = self.prompt_stream(conversation_id, input).await?;
        let (text, _, _) = collect_stream(stream).await?;
        Ok(text)
    }

    async fn prompt_stream(
        &self,
        conversation_id: &str,
        input: AgentPrompt,
    ) -> Result<PromptEventStream, String> {
        let mut rx = self.send_prompt(conversation_id, input).await?;
        Ok(Box::pin(stream! {
            while let Some(event) = rx.recv().await {
                yield event;
            }
        }))
    }

    async fn is_session_active(&self, conversation_id: &str) -> bool {
        self.inner
            .conversations
            .read()
            .await
            .contains_key(conversation_id)
    }

    async fn active_count(&self) -> usize {
        self.inner.conversations.read().await.len()
    }

    async fn shutdown_all(&self) {
        let mut map = self.inner.conversations.write().await;
        for (_, active) in map.drain() {
            let _ = active.cmd_tx.send(WorkerCommand::Shutdown).await;
        }
    }

    async fn set_resume_mode(&self, mode: Option<ResumeMode>) {
        ConversationSupervisor::set_resume_mode(self, mode).await;
    }
}

impl ConversationSupervisor {
    async fn evict(&self, conversation_id: &str) {
        let mut map = self.inner.conversations.write().await;
        if let Some(active) = map.remove(conversation_id) {
            let _ = active.cmd_tx.send(WorkerCommand::Shutdown).await;
        }
    }

    async fn get_or_start(
        &self,
        conversation_id: &str,
        model: &str,
        client_replay: Option<String>,
    ) -> Result<ActiveConversation, String> {
        {
            let mut map = self.inner.conversations.write().await;
            if let Some(active) = map.get_mut(conversation_id) {
                if active.model != model {
                    drop(map);
                    self.evict(conversation_id).await;
                } else {
                    active.last_active = Instant::now();
                    return Ok(ActiveConversation {
                        cmd_tx: active.cmd_tx.clone(),
                        model: active.model.clone(),
                        last_active: active.last_active,
                    });
                }
            }
        }

        let workspace = prepare_workspace(&self.inner.config.paths, conversation_id).await?;
        let workspace_rel = format!("sessions/{conversation_id}/workspace");
        let meta = read_session_meta(&self.inner.config.paths, conversation_id).await;
        let cursor_chat_id = meta.as_ref().and_then(|m| m.cursor_chat_id.clone());

        let record = ConversationStore::upsert(
            self.inner.config.store.pool(),
            conversation_id,
            &workspace_rel,
            None,
            cursor_chat_id.as_deref(),
        )
        .await
        .map_err(|e| e.to_string())?;

        let existing_session = record.acp_session_id.clone();
        let resume_model = meta.as_ref().and_then(|m| m.model.clone());
        let model_changed = resume_model.as_deref() != Some(model);
        let existing_session = if model_changed {
            None
        } else {
            existing_session
        };

        let idle = Duration::from_secs(self.inner.config.config.server.idle_timeout_secs);
        let perms = &self.inner.config.config.permissions;
        let webhook_url = if perms.webhook_url.is_empty() {
            None
        } else {
            Some(perms.webhook_url.clone())
        };
        let worker_config = WorkerConfig {
            conversation_id: conversation_id.to_string(),
            workspace,
            auth: self.inner.config.auth.clone(),
            model: model.to_string(),
            auto_approve: perms.auto_approve,
            webhook_url,
            webhook_timeout_secs: perms.webhook_timeout_secs,
            paths: self.inner.config.paths.clone(),
            pool: self.inner.config.store.pool().clone(),
            resume_mode: *self.inner.resume_mode.read().await,
            existing_session_id: existing_session,
            client_replay,
            idle_timeout: idle,
        };

        let cmd_tx = spawn_worker(worker_config);
        let active = ActiveConversation {
            cmd_tx: cmd_tx.clone(),
            model: model.to_string(),
            last_active: Instant::now(),
        };

        let conv_id = conversation_id.to_string();
        let inner_watch = Arc::clone(&self.inner);
        let store_pool = self.inner.config.store.pool().clone();
        tokio::spawn(async move {
            let _permit = inner_watch.semaphore.acquire().await.ok();
            cmd_tx.closed().await;
            inner_watch.conversations.write().await.remove(&conv_id);
            let _ = ConversationStore::set_dormant(&store_pool, &conv_id).await;
            info!(conv_id = %conv_id, "conversation dormant");
        });

        self.inner.conversations.write().await.insert(
            conversation_id.to_string(),
            ActiveConversation {
                cmd_tx: active.cmd_tx.clone(),
                model: active.model.clone(),
                last_active: active.last_active,
            },
        );

        Ok(active)
    }
}

async fn prepare_workspace(paths: &DataPaths, conversation_id: &str) -> Result<PathBuf, String> {
    let workspace = paths.session_workspace(conversation_id);
    tokio::fs::create_dir_all(&workspace)
        .await
        .map_err(|e| e.to_string())?;
    let assets = paths.session_assets_inbound(conversation_id);
    tokio::fs::create_dir_all(assets)
        .await
        .map_err(|e| e.to_string())?;
    Ok(workspace)
}
