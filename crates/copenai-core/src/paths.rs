use std::path::{Path, PathBuf};

use crate::error::{CoreError, Result};

const DEFAULT_INSTALL_DIR: &str = ".copenai";

#[derive(Debug, Clone)]
pub struct DataPaths {
    pub root: PathBuf,
}

impl DataPaths {
    pub fn resolve() -> Self {
        if let Ok(dir) = std::env::var("COPENAI_HOME") {
            return Self {
                root: PathBuf::from(dir),
            };
        }
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        Self {
            root: home.join(DEFAULT_INSTALL_DIR),
        }
    }

    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    pub fn cursor_env_file(&self) -> PathBuf {
        self.root.join("cursor.env")
    }

    pub fn database_file(&self) -> PathBuf {
        self.root.join("data").join("copenai.db")
    }

    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    pub fn pid_file(&self) -> PathBuf {
        self.run_dir().join("copenai.pid")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn server_log(&self) -> PathBuf {
        self.logs_dir().join("server.log")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("data").join("sessions")
    }

    pub fn session_dir(&self, conversation_id: &str) -> PathBuf {
        self.sessions_dir().join(conversation_id)
    }

    pub fn files_staging_dir(&self) -> PathBuf {
        self.root.join("data").join("files")
    }

    pub fn ensure_layout(&self) -> Result<()> {
        for dir in [
            self.root.join("data"),
            self.sessions_dir(),
            self.files_staging_dir(),
            self.logs_dir(),
            self.run_dir(),
        ] {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(())
    }

    pub fn session_workspace(&self, conversation_id: &str) -> PathBuf {
        self.session_dir(conversation_id).join("workspace")
    }

    pub fn session_assets_inbound(&self, conversation_id: &str) -> PathBuf {
        self.session_dir(conversation_id)
            .join("assets")
            .join("inbound")
    }

    pub fn session_transcript(&self, conversation_id: &str) -> PathBuf {
        self.session_dir(conversation_id).join("transcript")
    }

    pub fn session_meta(&self, conversation_id: &str) -> PathBuf {
        self.session_dir(conversation_id).join("meta.json")
    }

    /// Canonicalize `path` and ensure it stays under `root`.
    pub fn jail_check(root: &Path, path: &Path) -> Result<PathBuf> {
        let root = root
            .canonicalize()
            .map_err(|e| CoreError::PathJail(e.to_string()))?;
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        let canonical = joined
            .canonicalize()
            .map_err(|e| CoreError::PathJail(e.to_string()))?;
        if !canonical.starts_with(&root) {
            return Err(CoreError::PathJail(format!(
                "{} escapes {}",
                canonical.display(),
                root.display()
            )));
        }
        Ok(canonical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jail_allows_inside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ws");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("a.txt");
        std::fs::write(&file, "hi").unwrap();
        let ok = DataPaths::jail_check(&root, Path::new("a.txt")).unwrap();
        assert!(ok.ends_with("a.txt"));
    }
}
