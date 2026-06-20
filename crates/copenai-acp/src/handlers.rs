use std::path::{Path, PathBuf};
use std::sync::Arc;

use copenai_core::paths::DataPaths;

pub struct HandlerContext {
    pub workspace: PathBuf,
}

pub async fn read_text_file(workspace: &Path, path: &Path) -> Result<String, String> {
    let canonical = DataPaths::jail_check(workspace, path).map_err(|e| e.to_string())?;
    tokio::fs::read_to_string(canonical)
        .await
        .map_err(|e| e.to_string())
}

pub async fn write_text_file(workspace: &Path, path: &Path, content: &str) -> Result<(), String> {
    let canonical = DataPaths::jail_check(workspace, path).map_err(|e| e.to_string())?;
    if let Some(parent) = canonical.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    tokio::fs::write(canonical, content)
        .await
        .map_err(|e| e.to_string())
}

pub type SharedHandlerContext = Arc<HandlerContext>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn read_outside_jail_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let err = read_text_file(&ws, Path::new("../outside.txt")).await;
        assert!(err.is_err());
    }
}
