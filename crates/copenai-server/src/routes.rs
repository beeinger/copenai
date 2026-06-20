use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use copenai_acp::{AgentPrompt, PromptStreamEvent};
use copenai_openai::types::{FileDeleted, FileList, OpenAiErrorBody};
use copenai_openai::{messages::usage_char_count, StreamEvent, Usage};
use copenai_store::api_keys::ApiKeyStore;
use copenai_store::permissions::PermissionStore;
use futures::StreamExt;
use tower_http::trace::TraceLayer;

use crate::state::SharedState;

pub fn router(state: SharedState) -> Router {
    let api = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/v1/files", get(list_files).post(upload_file))
        .route(
            "/v1/files/{file_id}",
            get(get_file_meta).delete(delete_file),
        )
        .route("/v1/files/{file_id}/content", get(get_file_content))
        .route("/v1/permissions/pending", get(list_pending_permissions))
        .route("/v1/permissions/{id}/respond", post(respond_permission))
        .layer(middleware::from_fn_with_state(state.clone(), bearer_auth));

    Router::new()
        .route("/health", get(health))
        .merge(api)
        .fallback(not_implemented)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn bearer_auth(State(state): State<SharedState>, req: Request<Body>, next: Next) -> Response {
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let Some(token) = auth_header.and_then(|h| h.strip_prefix("Bearer ")) else {
        return error_response(StatusCode::UNAUTHORIZED, "missing bearer token");
    };
    match ApiKeyStore::validate(state.store.pool(), token).await {
        Ok(true) => next.run(req).await,
        Ok(false) => error_response(StatusCode::UNAUTHORIZED, "invalid api key"),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

async fn health(State(state): State<SharedState>) -> impl IntoResponse {
    let cursor_cmd = copenai_core::cursor::CursorCommand::from_auth(&state.auth);
    let cursor_ok = cursor_cmd
        .status_json()
        .await
        .map(|s| s.is_ok())
        .unwrap_or(false);
    let active = state.supervisor.active_count().await;
    let resume_mode = state.resume_mode.read().await.clone();
    Json(serde_json::json!({
        "status": "ok",
        "cursor_authenticated": cursor_ok,
        "active_conversations": active,
        "resume_mode": resume_mode,
    }))
}

async fn list_models(State(state): State<SharedState>) -> impl IntoResponse {
    let models = state.models.read().await.clone();
    let data: Vec<copenai_openai::types::ModelObject> = models
        .into_iter()
        .map(|id| copenai_openai::types::ModelObject {
            id,
            object: "model",
            created: copenai_openai::sse::unix_now(),
            owned_by: "cursor",
        })
        .collect();
    Json(copenai_openai::types::ModelList {
        object: "list",
        data,
    })
}

fn prompt_error_response(e: &str) -> Response {
    if e.contains("temperature/max_tokens not supported") || e.contains("does not support") {
        error_response(StatusCode::BAD_REQUEST, e)
    } else {
        error_response(StatusCode::INTERNAL_SERVER_ERROR, e)
    }
}

async fn chat_completions(
    State(state): State<SharedState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<copenai_openai::types::ChatCompletionRequest>,
) -> Response {
    if request.has_tool_fields() {
        return error_response(
            StatusCode::NOT_IMPLEMENTED,
            "tool calling not supported; Cursor ACP does not expose OpenAI function protocol",
        );
    }

    let conv_header = headers
        .get("x-conversation-id")
        .and_then(|v| v.to_str().ok());
    let conversation_id = copenai_openai::resolve_conversation_id(&request, conv_header);

    let models = state.models.read().await.clone();
    let model = match copenai_openai::validate_model(&request.model, &models) {
        Ok(m) => m,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
    };

    let parsed = match copenai_openai::parse_chat_request(&request) {
        Ok(p) => p,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
    };

    let session_hot = state.supervisor.is_session_active(&conversation_id).await;
    let plan = copenai_openai::build_prompt_plan(&parsed, session_hot);

    let mapped = match copenai_openai::map_message_content(
        &state.paths,
        &conversation_id,
        &parsed.final_user_content,
    )
    .await
    {
        Ok(m) => m,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &e),
    };

    let prompt = AgentPrompt {
        model: model.clone(),
        mapped: mapped.clone(),
        system_prefix: plan.system_prefix,
        replay_transcript: plan.replay_transcript,
        temperature: parsed.temperature,
        max_tokens: parsed.max_tokens,
        usage_prompt_chars: usage_char_count(&parsed, &mapped),
    };

    if request.stream {
        let completion_id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
        match state
            .supervisor
            .prompt_stream(&conversation_id, prompt)
            .await
        {
            Ok(events) => {
                let mapped_events = events.map(map_stream_event);
                let stream = copenai_openai::live_sse_stream(completion_id, model, mapped_events);
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(stream))
                    .unwrap()
            }
            Err(e) => prompt_error_response(&e),
        }
    } else {
        match state
            .supervisor
            .prompt_stream(&conversation_id, prompt)
            .await
        {
            Ok(mut events) => {
                let mut content = String::new();
                let mut usage = Usage::default();
                let mut finish_reason = "stop".to_string();
                while let Some(event) = events.next().await {
                    match event {
                        PromptStreamEvent::Delta(delta) => content.push_str(&delta),
                        PromptStreamEvent::Usage(u) => {
                            usage = Usage {
                                prompt_tokens: u.prompt_tokens,
                                completion_tokens: u.completion_tokens,
                                total_tokens: u.total_tokens,
                            };
                        }
                        PromptStreamEvent::Done {
                            full_text,
                            usage: u,
                            finish_reason: fr,
                        } => {
                            content = full_text;
                            usage = Usage {
                                prompt_tokens: u.prompt_tokens,
                                completion_tokens: u.completion_tokens,
                                total_tokens: u.total_tokens,
                            };
                            finish_reason = fr.as_openai_str().to_string();
                            break;
                        }
                        PromptStreamEvent::Error(e) => return prompt_error_response(&e),
                    }
                }
                let response = copenai_openai::completion_response(
                    &format!("chatcmpl-{}", uuid::Uuid::new_v4()),
                    &model,
                    &content,
                    &finish_reason,
                    usage,
                );
                (StatusCode::OK, Json(response)).into_response()
            }
            Err(e) => prompt_error_response(&e),
        }
    }
}

fn map_stream_event(event: PromptStreamEvent) -> StreamEvent {
    match event {
        PromptStreamEvent::Delta(d) => StreamEvent::Delta(d),
        PromptStreamEvent::Usage(u) => StreamEvent::Usage(Usage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        }),
        PromptStreamEvent::Done {
            finish_reason,
            usage,
            ..
        } => StreamEvent::Done {
            finish_reason: finish_reason.as_openai_str().to_string(),
            usage: Usage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens,
            },
        },
        PromptStreamEvent::Error(e) => StreamEvent::Error(e),
    }
}

#[derive(serde::Deserialize)]
struct PendingQuery {
    conversation_id: Option<String>,
}

async fn list_pending_permissions(
    State(state): State<SharedState>,
    Query(q): Query<PendingQuery>,
) -> Response {
    match PermissionStore::list_pending(state.store.pool(), q.conversation_id.as_deref()).await {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(serde::Deserialize)]
struct PermissionRespondBody {
    option_id: Option<String>,
    cancel: Option<bool>,
}

async fn respond_permission(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(body): Json<PermissionRespondBody>,
) -> Response {
    let cancel = body.cancel.unwrap_or(false);
    match PermissionStore::respond(state.store.pool(), &id, body.option_id.as_deref(), cancel).await
    {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response(),
        Ok(false) => error_response(
            StatusCode::NOT_FOUND,
            "permission request not found or already resolved",
        ),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(serde::Deserialize, Default)]
struct ListFilesQuery {
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    after: Option<String>,
}

async fn list_files(State(state): State<SharedState>, Query(q): Query<ListFilesQuery>) -> Response {
    let _ = q.purpose;
    let staging = state.paths.files_staging_dir();
    match copenai_openai::files::list_staged_files(&staging).await {
        Ok(mut files) => {
            let desc = q.order.as_deref() != Some("asc");
            if desc {
                files.reverse();
            }
            if let Some(after) = q.after.as_deref().filter(|s| !s.is_empty()) {
                if let Some(pos) = files.iter().position(|f| f.file_id == after) {
                    files = files.into_iter().skip(pos + 1).collect();
                } else {
                    files.clear();
                }
            }
            let limit = q.limit.unwrap_or(10_000).clamp(1, 10_000) as usize;
            let has_more = files.len() > limit;
            files.truncate(limit);
            let objects: Vec<_> = files
                .iter()
                .map(copenai_openai::files::staged_to_file_object)
                .collect();
            let first_id = objects.first().map(|o| o.id.clone()).unwrap_or_default();
            let last_id = objects.last().map(|o| o.id.clone()).unwrap_or_default();
            (
                StatusCode::OK,
                Json(FileList {
                    object: "list",
                    data: objects,
                    first_id,
                    last_id,
                    has_more,
                }),
            )
                .into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn delete_file(State(state): State<SharedState>, Path(file_id): Path<String>) -> Response {
    if copenai_openai::validate_file_id(&file_id).is_err() {
        return error_response(StatusCode::BAD_REQUEST, "invalid file_id");
    }
    let staging = state.paths.files_staging_dir();
    match copenai_openai::files::delete_staged_file(&staging, &file_id).await {
        Ok(()) => (
            StatusCode::OK,
            Json(FileDeleted {
                id: file_id,
                object: "file",
                deleted: true,
            }),
        )
            .into_response(),
        Err(e) if e.contains("not found") => {
            error_response(StatusCode::NOT_FOUND, "file not found")
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn upload_file(
    State(state): State<SharedState>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let staging = state.paths.files_staging_dir();
    if let Err(e) = tokio::fs::create_dir_all(&staging).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    if let Ok(Some(field)) = multipart.next_field().await {
        let name = field.file_name().unwrap_or("upload").to_string();
        let content_type = field
            .content_type()
            .map(|m| m.to_string())
            .unwrap_or_else(|| {
                mime_guess::from_path(&name)
                    .first_or_octet_stream()
                    .essence_str()
                    .to_string()
            });
        let data = match field.bytes().await {
            Ok(b) => b,
            Err(e) => return error_response(StatusCode::BAD_REQUEST, &e.to_string()),
        };
        let file_id = format!("file-{}", uuid::Uuid::new_v4());
        let path = staging.join(&file_id);
        if tokio::fs::write(&path, &data).await.is_err() {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "write failed");
        }
        let meta = copenai_openai::StagedFileMeta {
            filename: name.clone(),
            content_type,
            bytes: data.len() as u64,
            uploaded_at: chrono::Utc::now().to_rfc3339(),
        };
        if copenai_openai::files::write_staged_meta(&staging, &file_id, &meta)
            .await
            .is_err()
        {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "meta write failed");
        }
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "id": file_id,
                "object": "file",
                "bytes": data.len(),
                "filename": name,
            })),
        )
            .into_response();
    }
    error_response(StatusCode::BAD_REQUEST, "no file field")
}

async fn get_file_meta(State(state): State<SharedState>, Path(file_id): Path<String>) -> Response {
    if copenai_openai::validate_file_id(&file_id).is_err() {
        return error_response(StatusCode::BAD_REQUEST, "invalid file_id");
    }
    match copenai_openai::resolve_staged_file(&state.paths, &file_id).await {
        Ok(staged) => (
            StatusCode::OK,
            Json(copenai_openai::files::staged_to_file_object(&staged)),
        )
            .into_response(),
        Err(e) if e.contains("not found") => {
            error_response(StatusCode::NOT_FOUND, "file not found")
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn get_file_content(
    State(state): State<SharedState>,
    Path(file_id): Path<String>,
) -> Response {
    if copenai_openai::validate_file_id(&file_id).is_err() {
        return error_response(StatusCode::BAD_REQUEST, "invalid file_id");
    }
    let path = state.paths.files_staging_dir().join(&file_id);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/octet-stream")],
            bytes,
        )
            .into_response(),
        Err(_) => error_response(StatusCode::NOT_FOUND, "file not found"),
    }
}

async fn not_implemented() -> Response {
    error_response(StatusCode::NOT_IMPLEMENTED, "endpoint not implemented")
}

fn error_response(status: StatusCode, message: &str) -> Response {
    let body = OpenAiErrorBody {
        error: copenai_openai::types::OpenAiErrorDetail {
            message: message.to_string(),
            error_type: status
                .canonical_reason()
                .unwrap_or("error")
                .to_lowercase()
                .replace(' ', "_"),
            code: Some(status.as_u16().to_string()),
        },
    };
    (status, Json(body)).into_response()
}
