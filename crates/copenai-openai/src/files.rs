use std::path::{Path, PathBuf};

use copenai_core::paths::DataPaths;
use serde::{Deserialize, Serialize};

use crate::sse::unix_now;
use crate::types::FileObject;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedFileMeta {
    pub filename: String,
    pub content_type: String,
    pub bytes: u64,
    pub uploaded_at: String,
}

#[derive(Debug, Clone)]
pub struct StagedFile {
    pub file_id: String,
    pub path: PathBuf,
    pub meta: StagedFileMeta,
}

pub fn validate_file_id(file_id: &str) -> Result<(), String> {
    let Some(rest) = file_id.strip_prefix("file-") else {
        return Err(format!("invalid file_id: {file_id}"));
    };
    if rest.len() != 36 || uuid::Uuid::parse_str(rest).is_err() {
        return Err(format!("invalid file_id: {file_id}"));
    }
    Ok(())
}

pub async fn write_staged_meta(
    staging: &Path,
    file_id: &str,
    meta: &StagedFileMeta,
) -> Result<(), String> {
    let path = staging.join(format!("{file_id}.meta.json"));
    let raw = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    tokio::fs::write(path, raw).await.map_err(|e| e.to_string())
}

pub async fn resolve_staged_file(paths: &DataPaths, file_id: &str) -> Result<StagedFile, String> {
    validate_file_id(file_id)?;
    load_staged_file(&paths.files_staging_dir(), file_id).await
}

pub async fn copy_to_session_assets(
    staged: &StagedFile,
    assets_dir: &Path,
) -> Result<PathBuf, String> {
    tokio::fs::create_dir_all(assets_dir)
        .await
        .map_err(|e| e.to_string())?;
    let ext = Path::new(&staged.meta.filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let name = if ext.is_empty() {
        format!("{}", uuid::Uuid::new_v4())
    } else {
        format!("{}.{}", uuid::Uuid::new_v4(), ext)
    };
    let dest = assets_dir.join(name);
    tokio::fs::copy(&staged.path, &dest)
        .await
        .map_err(|e| e.to_string())?;
    Ok(dest)
}

pub fn staged_to_file_object(staged: &StagedFile) -> FileObject {
    let created_at = chrono::DateTime::parse_from_rfc3339(&staged.meta.uploaded_at)
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|_| unix_now());
    FileObject {
        id: staged.file_id.clone(),
        object: "file",
        bytes: staged.meta.bytes,
        created_at,
        filename: staged.meta.filename.clone(),
        purpose: "user_data",
    }
}

pub async fn list_staged_files(staging: &Path) -> Result<Vec<StagedFile>, String> {
    tokio::fs::create_dir_all(staging)
        .await
        .map_err(|e| e.to_string())?;
    let mut entries = tokio::fs::read_dir(staging)
        .await
        .map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".meta.json") {
            continue;
        }
        if validate_file_id(&name).is_err() {
            continue;
        }
        let staged = load_staged_file(staging, &name).await?;
        files.push(staged);
    }
    files.sort_by_key(|f| {
        chrono::DateTime::parse_from_rfc3339(&f.meta.uploaded_at)
            .map(|dt| dt.timestamp())
            .unwrap_or(0)
    });
    Ok(files)
}

pub async fn delete_staged_file(staging: &Path, file_id: &str) -> Result<(), String> {
    validate_file_id(file_id)?;
    let path = staging.join(file_id);
    if !path.is_file() {
        return Err(format!("file not found: {file_id}"));
    }
    tokio::fs::remove_file(&path)
        .await
        .map_err(|e| e.to_string())?;
    let meta_path = staging.join(format!("{file_id}.meta.json"));
    if meta_path.exists() {
        tokio::fs::remove_file(meta_path)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn load_staged_file(staging: &Path, file_id: &str) -> Result<StagedFile, String> {
    validate_file_id(file_id)?;
    let path = staging.join(file_id);
    if !path.is_file() {
        return Err(format!("file not found: {file_id}"));
    }
    let meta_path = staging.join(format!("{file_id}.meta.json"));
    let meta = if meta_path.exists() {
        let raw = tokio::fs::read_to_string(&meta_path)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())?
    } else {
        let bytes = tokio::fs::metadata(&path)
            .await
            .map_err(|e| e.to_string())?
            .len();
        StagedFileMeta {
            filename: file_id.to_string(),
            content_type: mime_guess::from_path(&path)
                .first_or_octet_stream()
                .essence_str()
                .to_string(),
            bytes,
            uploaded_at: chrono::Utc::now().to_rfc3339(),
        }
    };
    Ok(StagedFile {
        file_id: file_id.to_string(),
        path,
        meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_file_id() {
        assert!(validate_file_id("../etc/passwd").is_err());
        assert!(validate_file_id("file-not-uuid").is_err());
    }

    #[tokio::test]
    async fn list_and_delete_staged_files() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path();
        let file_id = format!("file-{}", uuid::Uuid::new_v4());
        let path = staging.join(&file_id);
        tokio::fs::create_dir_all(staging).await.unwrap();
        tokio::fs::write(&path, b"hello").await.unwrap();
        let meta = StagedFileMeta {
            filename: "hello.txt".into(),
            content_type: "text/plain".into(),
            bytes: 5,
            uploaded_at: chrono::Utc::now().to_rfc3339(),
        };
        write_staged_meta(staging, &file_id, &meta).await.unwrap();

        let listed = list_staged_files(staging).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file_id, file_id);

        delete_staged_file(staging, &file_id).await.unwrap();
        assert!(list_staged_files(staging).await.unwrap().is_empty());
        assert!(delete_staged_file(staging, &file_id).await.is_err());
    }
}
