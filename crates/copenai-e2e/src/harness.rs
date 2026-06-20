use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::{Mutex, Once};
use std::time::Duration;

use copenai_core::config::AppConfig;
use copenai_core::cursor::{CursorAuth, CursorCommand};
use copenai_core::paths::DataPaths;
use copenai_server::routes::router;
use copenai_server::state::AppState;
use copenai_store::api_keys::ApiKeyStore;
use tokio::net::TcpListener;

static SKIP_BANNER: Once = Once::new();
static SKIP_COUNT: AtomicUsize = AtomicUsize::new(0);
static CACHED_SKIP: Mutex<Option<Option<E2eSkip>>> = Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum E2eSkip {
    GateNotSet,
    AgentNotAuthenticated,
    HarnessStart(String),
}

impl E2eSkip {
    pub fn message(&self) -> String {
        match self {
            Self::GateNotSet => {
                "COPENAI_E2E is not set to 1 (run: export COPENAI_E2E=1 cargo test -p copenai-e2e -- --ignored)"
                    .into()
            }
            Self::AgentNotAuthenticated => {
                "Cursor agent not authenticated — run `copenai auth login` or `agent login`, \
                 or set CURSOR_API_KEY"
                    .into()
            }
            Self::HarnessStart(e) => format!("failed to start e2e server: {e}"),
        }
    }
}

impl std::fmt::Display for E2eSkip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

pub struct E2eEnv {
    pub base_url: String,
    /// copenai wrapper API key (`sk-copenai-...`), not Cursor credentials.
    pub api_key: String,
    pub paths: DataPaths,
    pub _guard: tempfile::TempDir,
}

impl E2eEnv {
    /// Why live E2E would not run right now (`None` = prerequisites OK).
    pub async fn skip_reason() -> Option<E2eSkip> {
        Self::cached_skip().await
    }

    /// Print status to the terminal even when libtest captures stderr on passing tests.
    pub async fn notify_status() {
        let report = Self::status_report().await;
        console_eprintln("");
        console_eprintln(&report);
        console_eprintln("");
    }

    /// Human-readable status for the always-on `live_e2e_status` test.
    pub async fn status_report() -> String {
        match Self::skip_reason().await {
            None => [
                "live e2e: READY",
                "  prerequisites satisfied",
                "  run: COPENAI_E2E=1 cargo test -p copenai-e2e -- --ignored --test-threads=1 --show-output",
            ]
            .join("\n"),
            Some(skip) => {
                format!(
                    "live e2e: SKIPPED\n  reason: {skip}\n  \
                     (ignored tests will pass without running; use --show-output to see per-test SKIP lines)"
                )
            }
        }
    }

    /// Start live E2E server when `COPENAI_E2E=1` and Cursor agent is authenticated.
    pub async fn try_start() -> Option<Self> {
        if let Some(skip) = Self::skip_reason().await {
            Self::log_skip(&skip);
            return None;
        }
        match Self::start_inner().await {
            Ok(env) => Some(env),
            Err(e) => {
                let skip = E2eSkip::HarnessStart(e.to_string());
                Self::log_skip(&skip);
                None
            }
        }
    }

    /// Log that a specific ignored test returned early (call from test harness).
    pub fn log_test_skip(test: &str) {
        if let Ok(guard) = CACHED_SKIP.lock() {
            if let Some(Some(skip)) = guard.as_ref() {
                eprintln!("SKIP {test}: {skip}");
                SKIP_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    pub async fn start() -> anyhow::Result<Self> {
        if let Some(skip) = Self::skip_reason().await {
            anyhow::bail!("live e2e skipped: {skip}");
        }
        Self::start_inner().await
    }

    fn log_skip(skip: &E2eSkip) {
        SKIP_BANNER.call_once(|| {
            console_eprintln("");
            console_eprintln("=== live e2e skipped ===");
            console_eprintln(&format!("  {skip}"));
            console_eprintln("  (use --show-output on `cargo test` to see SKIP lines per test)");
            console_eprintln("");
        });
    }

    async fn cached_skip() -> Option<E2eSkip> {
        {
            let guard = CACHED_SKIP.lock().expect("skip cache poisoned");
            if let Some(cached) = guard.as_ref() {
                return cached.clone();
            }
        }

        let skip = Self::compute_skip().await;
        let mut guard = CACHED_SKIP.lock().expect("skip cache poisoned");
        *guard = Some(skip.clone());
        skip
    }

    async fn compute_skip() -> Option<E2eSkip> {
        if std::env::var("COPENAI_E2E").ok().as_deref() != Some("1") {
            return Some(E2eSkip::GateNotSet);
        }
        if !cursor_agent_ready().await {
            return Some(E2eSkip::AgentNotAuthenticated);
        }
        None
    }

    async fn start_inner() -> anyhow::Result<Self> {
        let guard = tempfile::tempdir()?;
        let paths = DataPaths::from_root(guard.path());
        paths.ensure_layout()?;

        if let Ok(key) = std::env::var("CURSOR_API_KEY") {
            if !key.is_empty() {
                std::fs::write(paths.cursor_env_file(), format!("CURSOR_API_KEY={key}\n"))?;
            }
        }

        let mut config = AppConfig::default();
        config.server.bind = "127.0.0.1:0".into();
        config.server.idle_timeout_secs = 600;

        let auth = CursorAuth::resolve(&paths, &config)?;

        let state = AppState::new(paths.clone(), config, auth).await?;
        let store = state.store.clone();
        let (_, secret) = ApiKeyStore::create(store.pool(), "e2e").await?;
        let shared = Arc::new(state);

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        let app = router(shared);
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        wait_health(&format!("http://{addr}")).await?;

        Ok(Self {
            base_url: format!("http://{addr}"),
            api_key: secret,
            paths,
            _guard: guard,
        })
    }

    pub fn client(&self) -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("client")
    }
}

/// True when `agent` is on PATH and reports authenticated (login session or api-key).
pub async fn cursor_agent_ready() -> bool {
    let paths = DataPaths::from_root(std::env::temp_dir());
    let config = AppConfig::default();
    let Ok(auth) = CursorAuth::resolve(&paths, &config) else {
        return false;
    };
    let cmd = CursorCommand::from_auth(&auth);
    match cmd.status_json().await {
        Ok(status) => status.is_ok(),
        Err(_) => false,
    }
}

pub fn e2e_enabled() -> bool {
    std::env::var("COPENAI_E2E").ok().as_deref() == Some("1")
}

/// Write to the real terminal when available; libtest captures plain stderr on pass.
fn console_eprintln(msg: &str) {
    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        if let Ok(mut tty) = OpenOptions::new().write(true).open("/dev/tty") {
            let _ = writeln!(tty, "{msg}");
            return;
        }
    }
    eprintln!("{msg}");
}

async fn wait_health(base: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while tokio::time::Instant::now() < deadline {
        if let Ok(resp) = client.get(format!("{base}/health")).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    anyhow::bail!("health check timed out")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn gate_not_set_reports_skip_reason() {
        std::env::remove_var("COPENAI_E2E");
        // Reset cache for this test process.
        *CACHED_SKIP.lock().expect("lock") = None;
        let reason = E2eEnv::skip_reason().await;
        assert_eq!(reason, Some(E2eSkip::GateNotSet));
    }
}
