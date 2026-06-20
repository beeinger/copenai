use std::path::{Path, PathBuf};

use base64::Engine;
use copenai_core::paths::DataPaths;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::files::{copy_to_session_assets, resolve_staged_file};
use crate::types::{ContentPart, InputAudio, MessageContent};

const MAX_DOWNLOAD_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct MappedContent {
    pub text: String,
    pub asset_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub acp_session_id: Option<String>,
    pub cursor_chat_id: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub turn_count: Option<u32>,
}

pub async fn map_message_content(
    paths: &DataPaths,
    conversation_id: &str,
    content: &MessageContent,
) -> Result<MappedContent, String> {
    let assets_dir = paths.session_assets_inbound(conversation_id);
    tokio::fs::create_dir_all(&assets_dir)
        .await
        .map_err(|e| e.to_string())?;

    let mut text_parts = Vec::new();
    let mut asset_paths = Vec::new();

    match content {
        MessageContent::Text(t) => {
            text_parts.push(t.clone());
        }
        MessageContent::Parts(parts) => {
            for part in parts {
                match part {
                    ContentPart::Text { text } => text_parts.push(text.clone()),
                    ContentPart::ImageUrl { image_url } => {
                        let path = save_image_url(&assets_dir, &image_url.url).await?;
                        asset_paths.push(path);
                    }
                    ContentPart::File { file } => {
                        let staged = resolve_staged_file(paths, &file.file_id).await?;
                        let path = copy_to_session_assets(&staged, &assets_dir).await?;
                        asset_paths.push(path);
                    }
                    ContentPart::InputFile { file_id } => {
                        let staged = resolve_staged_file(paths, file_id).await?;
                        let path = copy_to_session_assets(&staged, &assets_dir).await?;
                        asset_paths.push(path);
                    }
                    ContentPart::InputAudio { input_audio } => {
                        let path = save_input_audio(&assets_dir, input_audio).await?;
                        asset_paths.push(path);
                    }
                }
            }
        }
    }

    Ok(MappedContent {
        text: text_parts.join("\n"),
        asset_paths,
    })
}

async fn save_input_audio(assets_dir: &Path, audio: &InputAudio) -> Result<PathBuf, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(audio.data.trim())
        .map_err(|e| e.to_string())?;
    let ext = audio.format.trim().trim_start_matches('.');
    let name = format!("{}.{}", Uuid::new_v4(), ext);
    let path = assets_dir.join(name);
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(path)
}

async fn save_image_url(assets_dir: &Path, url: &str) -> Result<PathBuf, String> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (meta, data) = rest
            .split_once(',')
            .ok_or_else(|| "invalid data url".to_string())?;
        let ext = meta
            .split(';')
            .next()
            .and_then(|m| m.strip_prefix("image/"))
            .unwrap_or("bin");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| e.to_string())?;
        let name = format!("{}.{}", Uuid::new_v4(), ext);
        let path = assets_dir.join(name);
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| e.to_string())?;
        return Ok(path);
    }

    if url.starts_with("http://") || url.starts_with("https://") {
        return download_remote_image(assets_dir, url).await;
    }

    if url.starts_with("file://") {
        return Err("file:// URLs are not supported in image_url; use /v1/files upload".into());
    }

    Err(format!("unsupported image url: {url}"))
}

async fn download_remote_image(assets_dir: &Path, url: &str) -> Result<PathBuf, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("failed to download image: HTTP {}", resp.status()));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    if !content_type.starts_with("image/") {
        return Err(format!("remote URL is not an image: {content_type}"));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
        return Err("remote image exceeds size limit".into());
    }
    let ext = content_type.strip_prefix("image/").unwrap_or("bin");
    let name = format!("{}.{}", Uuid::new_v4(), ext);
    let path = assets_dir.join(name);
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(path)
}

pub async fn write_session_meta(
    paths: &DataPaths,
    conversation_id: &str,
    meta: &SessionMeta,
) -> Result<(), String> {
    let path = paths.session_meta(conversation_id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    tokio::fs::write(path, raw).await.map_err(|e| e.to_string())
}

pub async fn read_session_meta(paths: &DataPaths, conversation_id: &str) -> Option<SessionMeta> {
    let path = paths.session_meta(conversation_id);
    let raw = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn maps_text_content() {
        let paths = DataPaths::from_root(tempfile::tempdir().unwrap().path());
        let mapped = map_message_content(&paths, "c1", &MessageContent::Text("hello".into()))
            .await
            .unwrap();
        assert_eq!(mapped.text, "hello");
    }
}
