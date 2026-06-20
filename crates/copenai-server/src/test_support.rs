#![cfg(any(test, feature = "test-utils"))]

use std::sync::Arc;

use copenai_acp::{mock_supervisor, MockResponse, SupervisorBackend};
use copenai_core::config::AppConfig;
use copenai_core::cursor::CursorAuth;
use copenai_core::paths::DataPaths;
use copenai_store::api_keys::ApiKeyStore;
use copenai_store::Store;

use crate::routes::router;
use crate::state::AppState;

pub struct TestHarness {
    pub state: crate::state::SharedState,
    pub api_key: String,
    pub paths: DataPaths,
}

impl TestHarness {
    pub async fn with_mock(response: MockResponse) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DataPaths::from_root(tmp.path());
        paths.ensure_layout().unwrap();
        let store = Store::open(&paths.database_file()).await.unwrap();
        let (_, secret) = ApiKeyStore::create(store.pool(), "test").await.unwrap();
        let supervisor: Arc<dyn SupervisorBackend> = mock_supervisor(response);
        let state = Arc::new(AppState::with_supervisor(
            paths.clone(),
            AppConfig::default(),
            CursorAuth::default(),
            store,
            supervisor,
        ));
        Self {
            state,
            api_key: secret,
            paths,
        }
    }

    pub fn app(&self) -> axum::Router {
        router(self.state.clone())
    }
}
