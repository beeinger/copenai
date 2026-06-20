use std::sync::Arc;

use copenai_acp::{ConversationSupervisor, ResumeMode, SupervisorBackend, SupervisorConfig};
use copenai_core::config::AppConfig;
use copenai_core::cursor::CursorAuth;
use copenai_core::paths::DataPaths;
use copenai_store::Store;
use tokio::sync::RwLock;

pub struct AppState {
    pub paths: DataPaths,
    pub config: AppConfig,
    pub auth: CursorAuth,
    pub store: Store,
    pub supervisor: Arc<dyn SupervisorBackend>,
    pub resume_mode: RwLock<String>,
    pub models: RwLock<Vec<String>>,
}

impl AppState {
    pub async fn new(
        paths: DataPaths,
        config: AppConfig,
        auth: CursorAuth,
    ) -> copenai_core::Result<Self> {
        paths.ensure_layout()?;
        let store = Store::open(&paths.database_file()).await?;
        let supervisor_config = SupervisorConfig {
            paths: paths.clone(),
            config: config.clone(),
            auth: auth.clone(),
            store: store.clone(),
            resume_mode: None,
        };
        let supervisor: Arc<dyn SupervisorBackend> =
            Arc::new(ConversationSupervisor::new(supervisor_config, None));
        Ok(Self {
            paths,
            config,
            auth,
            store,
            supervisor,
            resume_mode: RwLock::new("unknown".into()),
            models: RwLock::new(vec!["composer-2.5".into(), "auto".into()]),
        })
    }

    pub fn with_supervisor(
        paths: DataPaths,
        config: AppConfig,
        auth: CursorAuth,
        store: Store,
        supervisor: Arc<dyn SupervisorBackend>,
    ) -> Self {
        Self {
            paths,
            config,
            auth,
            store,
            supervisor,
            resume_mode: RwLock::new("unknown".into()),
            models: RwLock::new(vec!["composer-2.5".into(), "auto".into()]),
        }
    }
}

pub fn parse_resume_mode(probe: &str) -> Option<ResumeMode> {
    match probe {
        "load" => Some(ResumeMode::Load),
        "resume" => Some(ResumeMode::Resume),
        "degraded" => Some(ResumeMode::Degraded),
        _ => None,
    }
}

pub type SharedState = Arc<AppState>;
