use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    ReadTextFileRequest, ReadTextFileResponse, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, ResumeSessionRequest, SelectedPermissionOutcome, SessionId,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, TextContent,
    WriteTextFileRequest, WriteTextFileResponse,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, Client, ConnectionTo};
use copenai_core::cursor::{acp_command_string, CursorAuth, CursorCommand};
use copenai_core::paths::DataPaths;
use copenai_openai::multimodal::{read_session_meta, write_session_meta, SessionMeta};
use copenai_store::conversations::ConversationStore;
use copenai_store::permissions::PermissionStore;
use futures::channel::mpsc::UnboundedSender;
use futures::StreamExt;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use crate::backend::{FinishReason, PromptStreamEvent, UsageSnapshot};
use crate::handlers::{read_text_file, write_text_file, HandlerContext};
use crate::resume::{ResumeCapabilities, ResumeMode};

pub struct WorkerConfig {
    pub conversation_id: String,
    pub workspace: PathBuf,
    pub auth: CursorAuth,
    pub model: String,
    pub auto_approve: bool,
    pub webhook_url: Option<String>,
    pub webhook_timeout_secs: u64,
    pub paths: DataPaths,
    pub pool: sqlx::SqlitePool,
    pub resume_mode: Option<ResumeMode>,
    pub existing_session_id: Option<String>,
    pub client_replay: Option<String>,
    pub idle_timeout: Duration,
}

pub enum WorkerCommand {
    Prompt {
        blocks: Vec<ContentBlock>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        prompt_chars: usize,
        stream_tx: mpsc::Sender<PromptStreamEvent>,
    },
    Shutdown,
}

struct AgentCaps {
    audio: bool,
    image: bool,
    config_option_ids: Vec<String>,
}

pub fn spawn_worker(config: WorkerConfig) -> mpsc::Sender<WorkerCommand> {
    let (cmd_tx, cmd_rx) = mpsc::channel(8);
    let sink = Arc::new(Mutex::new(None::<UnboundedSender<PromptStreamEvent>>));

    tokio::spawn(async move {
        if let Err(e) = run_worker_loop(config, cmd_rx, sink).await {
            tracing::error!(error = %e, "worker loop ended");
        }
    });

    cmd_tx
}

async fn run_worker_loop(
    config: WorkerConfig,
    mut cmd_rx: mpsc::Receiver<WorkerCommand>,
    sink: Arc<Mutex<Option<UnboundedSender<PromptStreamEvent>>>>,
) -> Result<(), String> {
    let command = acp_command_string(&config.auth, Some(&config.model));
    let agent = AcpAgent::from_str(&command).map_err(|e| e.to_string())?;
    let workspace = config.workspace.clone();
    let handler_ctx = Arc::new(HandlerContext {
        workspace: workspace.clone(),
    });
    let auto_approve = config.auto_approve;
    let webhook_url = config.webhook_url.clone();
    let webhook_timeout = Duration::from_secs(config.webhook_timeout_secs);
    let pool = config.pool.clone();
    let conv_id = config.conversation_id.clone();
    let sink_notify = sink.clone();

    Client
        .builder()
        .name("copenai")
        .on_receive_notification(
            {
                let sink_notify = sink_notify.clone();
                async move |notification: SessionNotification, _cx| {
                    match notification.update {
                        SessionUpdate::AgentMessageChunk(chunk) => {
                            if let ContentBlock::Text(t) = chunk.content {
                                let guard = sink_notify.lock().await;
                                if let Some(tx) = guard.as_ref() {
                                    let _ = tx.unbounded_send(PromptStreamEvent::Delta(t.text));
                                }
                            }
                        }
                        SessionUpdate::UsageUpdate(usage) => {
                            let guard = sink_notify.lock().await;
                            if let Some(tx) = guard.as_ref() {
                                let snap = UsageSnapshot::from_acp(usage.used, usage.size);
                                let _ = tx.unbounded_send(PromptStreamEvent::Usage(snap));
                            }
                        }
                        _ => {}
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let ctx = handler_ctx.clone();
                async move |request: ReadTextFileRequest, responder, _connection| {
                    match read_text_file(&ctx.workspace, &request.path).await {
                        Ok(content) => {
                            responder.respond(ReadTextFileResponse::new(content))?;
                        }
                        Err(e) => {
                            return Err(agent_client_protocol::Error::invalid_params()
                                .data(format!("read failed: {e}")));
                        }
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let ctx = handler_ctx.clone();
                async move |request: WriteTextFileRequest, responder, _connection| {
                    match write_text_file(&ctx.workspace, &request.path, &request.content).await {
                        Ok(()) => {
                            responder.respond(WriteTextFileResponse::new())?;
                        }
                        Err(e) => {
                            return Err(agent_client_protocol::Error::invalid_params()
                                .data(format!("write failed: {e}")));
                        }
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let pool = pool.clone();
                let conv_id = conv_id.clone();
                async move |request: RequestPermissionRequest, responder, _connection| {
                    if auto_approve {
                        let option_id = request.options.first().map(|o| o.option_id.clone());
                        if let Some(id) = option_id {
                            responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                    id,
                                )),
                            ))?;
                        } else {
                            responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Cancelled,
                            ))?;
                        }
                        return Ok(());
                    }

                    let options_json =
                        serde_json::to_string(&request.options).unwrap_or_else(|_| "[]".into());
                    let title = request
                        .tool_call
                        .fields
                        .title
                        .clone()
                        .unwrap_or_else(|| "permission".into());

                    let perm_id =
                        PermissionStore::insert_pending(&pool, &conv_id, &title, &options_json)
                            .await
                            .map_err(|e| {
                                agent_client_protocol::Error::internal_error().data(e.to_string())
                            })?;

                    if let Some(url) = &webhook_url {
                        let client = reqwest::Client::new();
                        let payload = serde_json::json!({
                            "id": perm_id,
                            "conversation_id": conv_id,
                            "tool_title": title,
                            "options": request.options,
                        });
                        let _ = tokio::time::timeout(
                            webhook_timeout,
                            client.post(url).json(&payload).send(),
                        )
                        .await;
                    }

                    let decision =
                        PermissionStore::wait_for_decision(&pool, &perm_id, webhook_timeout)
                            .await
                            .map_err(|e| {
                                agent_client_protocol::Error::internal_error().data(e.to_string())
                            })?;

                    match decision {
                        Some(option_id) => {
                            PermissionStore::resolve(&pool, &perm_id, "approved")
                                .await
                                .ok();
                            responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                    option_id,
                                )),
                            ))?;
                        }
                        None => {
                            PermissionStore::resolve(&pool, &perm_id, "cancelled")
                                .await
                                .ok();
                            responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Cancelled,
                            ))?;
                        }
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(
            agent,
            move |connection: ConnectionTo<agent_client_protocol::Agent>| {
                let workspace = workspace.clone();
                let config = WorkerConfig {
                    conversation_id: config.conversation_id.clone(),
                    workspace: config.workspace.clone(),
                    auth: config.auth.clone(),
                    model: config.model.clone(),
                    auto_approve: config.auto_approve,
                    webhook_url: config.webhook_url.clone(),
                    webhook_timeout_secs: config.webhook_timeout_secs,
                    paths: config.paths.clone(),
                    pool: config.pool.clone(),
                    resume_mode: config.resume_mode,
                    existing_session_id: config.existing_session_id.clone(),
                    client_replay: config.client_replay.clone(),
                    idle_timeout: config.idle_timeout,
                };
                let sink = sink.clone();
                async move {
                    let init = connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await?;

                    let caps = AgentCaps {
                        audio: init.agent_capabilities.prompt_capabilities.audio,
                        image: init.agent_capabilities.prompt_capabilities.image,
                        config_option_ids: Vec::new(),
                    };

                    let resume_caps = ResumeCapabilities::from_initialize(&init);
                    let (session_id, config_ids) = setup_session(
                        &connection,
                        &workspace,
                        config.resume_mode.or(Some(resume_caps.mode)),
                        config.existing_session_id.as_deref(),
                        config.client_replay.as_deref(),
                        &config,
                    )
                    .await
                    .map_err(|e| {
                        agent_client_protocol::Error::internal_error().data(e.to_string())
                    })?;
                    let mut caps = caps;
                    caps.config_option_ids = config_ids;

                    loop {
                        let cmd = tokio::time::timeout(config.idle_timeout, cmd_rx.recv()).await;
                        match cmd {
                            Ok(Some(WorkerCommand::Prompt {
                                blocks,
                                temperature,
                                max_tokens,
                                prompt_chars,
                                stream_tx,
                            })) => {
                                let stream_tx_err = stream_tx.clone();
                                let result = execute_prompt(
                                    &connection,
                                    &session_id,
                                    &sink,
                                    &config,
                                    &caps,
                                    blocks,
                                    temperature,
                                    max_tokens,
                                    prompt_chars,
                                    stream_tx,
                                )
                                .await;
                                if let Err(e) = result {
                                    let _ = stream_tx_err.send(PromptStreamEvent::Error(e)).await;
                                }
                            }
                            Ok(Some(WorkerCommand::Shutdown)) | Err(_) => {
                                info!(conv_id = %config.conversation_id, "worker idle shutdown");
                                break;
                            }
                            Ok(None) => break,
                        }
                    }
                    Ok(())
                }
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn apply_sampling_params(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    session_id: &SessionId,
    caps: &AgentCaps,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
) -> Result<(), String> {
    if let Some(temp) = temperature {
        if (temp - 1.0).abs() > f32::EPSILON && (temp).abs() > f32::EPSILON {
            if !caps.config_option_ids.iter().any(|id| id == "temperature") {
                return Err("temperature/max_tokens not supported by Cursor ACP agent".into());
            }
            connection
                .send_request(SetSessionConfigOptionRequest::new(
                    session_id.clone(),
                    "temperature",
                    temp.to_string(),
                ))
                .block_task()
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    if let Some(max) = max_tokens {
        if max > 0 {
            if !caps
                .config_option_ids
                .iter()
                .any(|id| id == "max_tokens" || id == "maxTokens")
            {
                return Err("temperature/max_tokens not supported by Cursor ACP agent".into());
            }
            let id = if caps.config_option_ids.contains(&"max_tokens".to_string()) {
                "max_tokens"
            } else {
                "maxTokens"
            };
            connection
                .send_request(SetSessionConfigOptionRequest::new(
                    session_id.clone(),
                    id,
                    max.to_string(),
                ))
                .block_task()
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn execute_prompt(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    session_id: &SessionId,
    sink: &Arc<Mutex<Option<UnboundedSender<PromptStreamEvent>>>>,
    config: &WorkerConfig,
    caps: &AgentCaps,
    blocks: Vec<ContentBlock>,
    temperature: Option<f32>,
    max_tokens: Option<u32>,
    prompt_chars: usize,
    stream_tx: mpsc::Sender<PromptStreamEvent>,
) -> Result<(), String> {
    apply_sampling_params(connection, session_id, caps, temperature, max_tokens).await?;

    for block in &blocks {
        match block {
            ContentBlock::Audio(_) if !caps.audio => {
                return Err("agent does not support audio prompts".into());
            }
            ContentBlock::Image(_) if !caps.image => {
                return Err("agent does not support image prompts".into());
            }
            _ => {}
        }
    }

    let user_text = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let (event_tx, mut event_rx) = futures::channel::mpsc::unbounded();
    {
        let mut guard = sink.lock().await;
        *guard = Some(event_tx);
    }

    connection
        .send_request(PromptRequest::new(session_id.clone(), blocks))
        .block_task()
        .await
        .map_err(|e| e.to_string())?;

    let mut collected = String::new();
    let mut usage = UsageSnapshot::default();
    // ACP may deliver AgentMessageChunk notifications after PromptRequest returns.
    // Keep sink open and drain until idle so chunks are not dropped.
    let mut idle_deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    loop {
        let wait = idle_deadline.saturating_duration_since(tokio::time::Instant::now());
        if wait.is_zero() {
            break;
        }
        match tokio::time::timeout(wait, event_rx.next()).await {
            Ok(Some(event)) => {
                idle_deadline = tokio::time::Instant::now() + Duration::from_millis(250);
                match &event {
                    PromptStreamEvent::Delta(delta) => collected.push_str(delta),
                    PromptStreamEvent::Usage(u) => usage = u.clone(),
                    _ => {}
                }
                let _ = stream_tx.send(event).await;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    {
        let mut guard = sink.lock().await;
        *guard = None;
    }

    if usage.total_tokens == 0 {
        usage = UsageSnapshot::estimate(prompt_chars, collected.len());
    }

    ConversationStore::touch(&config.pool, &config.conversation_id)
        .await
        .map_err(|e| e.to_string())?;
    append_transcript(
        &config.paths,
        &config.conversation_id,
        &user_text,
        &collected,
        &config.model,
    )
    .await;

    stream_tx
        .send(PromptStreamEvent::Done {
            finish_reason: FinishReason::Stop,
            full_text: collected.clone(),
            usage,
        })
        .await
        .map_err(|_| "stream consumer dropped".to_string())?;

    Ok(())
}

async fn setup_session(
    connection: &ConnectionTo<agent_client_protocol::Agent>,
    workspace: &PathBuf,
    resume_mode: Option<ResumeMode>,
    existing_session: Option<&str>,
    client_replay: Option<&str>,
    config: &WorkerConfig,
) -> Result<(SessionId, Vec<String>), String> {
    if let Some(session_id) = existing_session.map(str::to_string) {
        match resume_mode {
            Some(ResumeMode::Load) => {
                info!(%session_id, "session/load");
                connection
                    .send_request(LoadSessionRequest::new(session_id.clone(), workspace))
                    .block_task()
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok((session_id.into(), Vec::new()));
            }
            Some(ResumeMode::Resume) => {
                info!(%session_id, "session/resume");
                connection
                    .send_request(ResumeSessionRequest::new(session_id.clone(), workspace))
                    .block_task()
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok((session_id.into(), Vec::new()));
            }
            Some(ResumeMode::Degraded) | None => {
                warn!("degraded resume");
                let context = if let Some(replay) = client_replay.filter(|s| !s.is_empty()) {
                    replay.to_string()
                } else {
                    load_transcript_context(&config.paths, &config.conversation_id).await
                };
                let new_session = connection
                    .send_request(NewSessionRequest::new(workspace))
                    .block_task()
                    .await
                    .map_err(|e| e.to_string())?;
                if !context.is_empty() {
                    let blocks = vec![ContentBlock::Text(TextContent::new(context))];
                    let _ = connection
                        .send_request(PromptRequest::new(new_session.session_id.clone(), blocks))
                        .block_task()
                        .await;
                }
                let config_ids = config_option_ids(&new_session.config_options);
                return Ok((new_session.session_id, config_ids));
            }
        }
    }

    let new_session = connection
        .send_request(NewSessionRequest::new(workspace))
        .block_task()
        .await
        .map_err(|e| e.to_string())?;

    let session_id = new_session.session_id.clone();
    let config_ids = config_option_ids(&new_session.config_options);
    let mut session_meta = read_session_meta(&config.paths, &config.conversation_id)
        .await
        .unwrap_or(SessionMeta {
            acp_session_id: None,
            cursor_chat_id: None,
            model: None,
            turn_count: None,
        });
    session_meta.acp_session_id = Some(session_id.to_string());
    session_meta.model = Some(config.model.clone());
    if session_meta.cursor_chat_id.is_none() {
        let cmd = CursorCommand::from_auth(&config.auth);
        if let Ok(chat_id) = cmd.create_chat().await {
            session_meta.cursor_chat_id = Some(chat_id.clone());
            ConversationStore::set_cursor_chat_id(&config.pool, &config.conversation_id, &chat_id)
                .await
                .ok();
        }
    }
    let _ = write_session_meta(&config.paths, &config.conversation_id, &session_meta).await;
    ConversationStore::set_acp_session(
        &config.pool,
        &config.conversation_id,
        &session_id.to_string(),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok((session_id, config_ids))
}

fn config_option_ids(
    options: &Option<Vec<agent_client_protocol::schema::v1::SessionConfigOption>>,
) -> Vec<String> {
    options
        .as_ref()
        .map(|opts| opts.iter().map(|o| o.id.to_string()).collect())
        .unwrap_or_default()
}

async fn load_transcript_context(paths: &DataPaths, conv_id: &str) -> String {
    let dir = paths.session_transcript(conv_id);
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return String::new();
    };
    let mut lines = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(raw) = tokio::fs::read_to_string(entry.path()).await {
            lines.push(raw);
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "Previous conversation context (degraded resume):\n{}",
        lines.join("\n")
    )
}

async fn append_transcript(
    paths: &DataPaths,
    conv_id: &str,
    user: &str,
    assistant: &str,
    model: &str,
) {
    let dir = paths.session_transcript(conv_id);
    let _ = tokio::fs::create_dir_all(&dir).await;
    let file = dir.join(format!("{}.jsonl", chrono::Utc::now().timestamp_millis()));
    let line = serde_json::json!({
        "user": user,
        "assistant": assistant,
        "model": model,
    });
    let _ = tokio::fs::write(file, line.to_string()).await;
}
