#![cfg(any(test, feature = "test-utils"))]

use std::sync::Arc;

use axum::Router;
use copenai_acp::{mock_supervisor, MockResponse, SupervisorBackend};
use copenai_core::config::{AppConfig, ResponsesSection};
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
    _tmp: tempfile::TempDir,
}

impl TestHarness {
    pub async fn with_mock(response: MockResponse) -> Self {
        Self::with_mock_config(response, AppConfig::default()).await
    }

    pub async fn with_mock_config(response: MockResponse, config: AppConfig) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = DataPaths::from_root(tmp.path());
        paths.ensure_layout().unwrap();
        let store = Store::open(&paths.database_file()).await.unwrap();
        let (_, secret) = ApiKeyStore::create(store.pool(), "test").await.unwrap();
        let supervisor: Arc<dyn SupervisorBackend> = mock_supervisor(response);
        let state = Arc::new(AppState::with_supervisor(
            paths.clone(),
            config,
            CursorAuth::default(),
            store,
            supervisor,
        ));
        Self {
            state,
            api_key: secret,
            paths,
            _tmp: tmp,
        }
    }

    pub async fn with_config(response: MockResponse, responses: ResponsesSection) -> Self {
        Self::with_mock_config(
            response,
            AppConfig {
                responses,
                ..Default::default()
            },
        )
        .await
    }

    pub async fn with_webhook_server(
        response: MockResponse,
        webhook: impl Fn(serde_json::Value) -> String + Send + Sync + 'static,
    ) -> (Self, String) {
        use axum::{routing::post, Json};

        let captured = Arc::new(webhook);
        let hook = captured.clone();
        let app = Router::new().route(
            "/hook",
            post(move |Json(body): Json<serde_json::Value>| {
                let hook = hook.clone();
                async move { Json(serde_json::json!({ "output": hook(body) })) }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let responses = ResponsesSection {
            tool_execution: "server".into(),
            tool_webhook: format!("http://{addr}/hook"),
            ..Default::default()
        };
        let harness = Self::with_config(response, responses).await;
        (harness, format!("http://{addr}/hook"))
    }

    pub fn app(&self) -> Router {
        router(self.state.clone())
    }
}
