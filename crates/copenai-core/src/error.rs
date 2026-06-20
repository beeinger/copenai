use thiserror::Error;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("cursor not authenticated: {0}")]
    CursorNotAuthenticated(String),

    #[error("cursor command failed: {0}")]
    CursorCommand(String),

    #[error("daemon already running (pid {0})")]
    DaemonAlreadyRunning(u32),

    #[error("daemon not running")]
    DaemonNotRunning,

    #[error("path outside session jail: {0}")]
    PathJail(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}
