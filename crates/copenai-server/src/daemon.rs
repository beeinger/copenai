use std::net::SocketAddr;
use std::process::Stdio;
use std::time::Duration;

use copenai_core::config::AppConfig;
use copenai_core::cursor::{CursorAuth, CursorCommand};
use copenai_core::daemon::{is_process_alive, parse_bind_port, read_pid, remove_pid, write_pid};
use copenai_core::paths::DataPaths;
use copenai_core::Result;
use tokio::net::TcpListener;
use tracing::info;

use crate::routes::router;
use crate::state::{AppState, SharedState};

pub async fn run_daemon(paths: DataPaths, config: AppConfig, auth: CursorAuth) -> Result<()> {
    let state = AppState::new(paths.clone(), config.clone(), auth.clone()).await?;
    let shared: SharedState = std::sync::Arc::new(state);

    refresh_models(&shared).await;
    probe_resume_mode(&shared, &auth).await;

    let addr: SocketAddr = config
        .server
        .bind
        .parse()
        .map_err(|e| copenai_core::CoreError::Config(format!("invalid bind address: {e}")))?;
    let listener = TcpListener::bind(addr).await?;
    let pid = std::process::id();
    write_pid(&paths, pid)?;
    info!(pid, %addr, "copenai listening");

    let app = router(shared.clone());
    let shutdown = shutdown_signal();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| copenai_core::CoreError::Other(e.to_string()))?;

    shared.supervisor.shutdown_all().await;
    remove_pid(&paths)?;
    Ok(())
}

pub fn spawn_daemon(paths: &DataPaths) -> Result<()> {
    if let Some(pid) = read_pid(paths)? {
        if is_process_alive(pid) {
            return Err(copenai_core::CoreError::DaemonAlreadyRunning(pid));
        }
        remove_pid(paths)?;
    }

    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--daemon")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd.spawn()?;
    Ok(())
}

pub async fn ensure_daemon_started(paths: &DataPaths, bind: &str) -> Result<u32> {
    let port = parse_bind_port(bind)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline {
        if wait_for_health(bind, Duration::from_millis(200)).await {
            if let Some(pid) = read_pid(paths)? {
                if is_process_alive(pid) {
                    return Ok(pid);
                }
            }
            return Err(copenai_core::CoreError::Other(format!(
                "health ok on port {port} but pidfile missing/stale — run `copenai stop`"
            )));
        }
        if let Some(pid) = read_pid(paths)? {
            if !is_process_alive(pid) {
                remove_pid(paths)?;
                return Err(copenai_core::CoreError::Other(
                    "daemon exited during startup — see server.log".into(),
                ));
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(copenai_core::CoreError::Other(
        "daemon startup timed out — see server.log".into(),
    ))
}

async fn refresh_models(state: &SharedState) {
    let cmd = CursorCommand::from_auth(&state.auth);
    if let Ok(models) = cmd.list_models().await {
        if !models.is_empty() {
            *state.models.write().await = models;
        }
    }
}

async fn probe_resume_mode(state: &SharedState, auth: &CursorAuth) {
    let resume_probe = copenai_acp::probe_acp_resume(auth).await;
    *state.resume_mode.write().await = resume_probe.clone();
    state
        .supervisor
        .set_resume_mode(crate::state::parse_resume_mode(&resume_probe))
        .await;
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

pub async fn wait_for_health(bind: &str, timeout: Duration) -> bool {
    let host = bind.replace("0.0.0.0", "127.0.0.1");
    let url = format!("http://{host}/health");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}
