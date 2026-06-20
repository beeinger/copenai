use std::path::Path;

use agent_client_protocol::schema::v1::{
    AudioContent, ContentBlock, ImageContent, ResourceLink, TextContent,
};
use base64::Engine;
use copenai_openai::multimodal::MappedContent;

pub async fn build_content_blocks(mapped: &MappedContent) -> Result<Vec<ContentBlock>, String> {
    let mut blocks = Vec::new();
    if !mapped.text.is_empty() {
        blocks.push(ContentBlock::Text(TextContent::new(mapped.text.clone())));
    }
    for path in &mapped.asset_paths {
        if let Some(block) = asset_path_to_block(path).await? {
            blocks.push(block);
        }
    }
    if blocks.is_empty() {
        blocks.push(ContentBlock::Text(TextContent::new(String::new())));
    }
    Ok(blocks)
}

async fn asset_path_to_block(path: &Path) -> Result<Option<ContentBlock>, String> {
    let meta = tokio::fs::metadata(path).await.map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Ok(None);
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "txt" && path.to_string_lossy().ends_with(".url.txt") {
        let url = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| e.to_string())?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("resource")
            .to_string();
        return Ok(Some(ContentBlock::ResourceLink(ResourceLink::new(
            name,
            url.trim(),
        ))));
    }

    let mime = mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string();

    if mime.starts_with("image/") {
        let bytes = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        let uri = format!("file://{}", path.display());
        return Ok(Some(ContentBlock::Image(
            ImageContent::new(data, mime).uri(uri),
        )));
    }

    if mime.starts_with("audio/") {
        let bytes = tokio::fs::read(path).await.map_err(|e| e.to_string())?;
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        return Ok(Some(ContentBlock::Audio(AudioContent::new(data, mime))));
    }

    let uri = format!("file://{}", path.display());
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    Ok(Some(ContentBlock::ResourceLink(
        ResourceLink::new(name, uri).mime_type(mime),
    )))
}
