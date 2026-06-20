use clap::{Parser, Subcommand};
use copenai_core::config::AppConfig;
use copenai_core::cursor::{CursorAuth, CursorCommand};
use copenai_core::daemon::{is_process_alive, read_pid, stop_daemon, tail_log, StopOutcome};
use copenai_core::paths::DataPaths;
use copenai_core::Result;
use copenai_server::daemon::{ensure_daemon_started, run_daemon, spawn_daemon};
use copenai_store::api_keys::ApiKeyStore;
use copenai_store::conversations::ConversationStore;
use copenai_store::Store;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "copenai", about = "Cursor Agent OpenAI-compatible wrapper")]
struct Cli {
    /// Run background HTTP server (internal)
    #[arg(long, hide = true)]
    daemon: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Check Rust, cursor, auth, and ACP capabilities
    Doctor,
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
    Keys {
        #[command(subcommand)]
        command: KeysCommands,
    },
    /// Start background server
    Start,
    /// Stop background server
    Stop,
    /// Server status
    Status,
    /// Tail server logs
    Logs {
        #[arg(short, long)]
        follow: bool,
    },
}

#[derive(Subcommand)]
enum AuthCommands {
    Status,
    Login,
    ApiKey {
        #[arg(long)]
        key: Option<String>,
    },
    Logout,
}

#[derive(Subcommand)]
enum KeysCommands {
    List,
    Add {
        #[arg(long, default_value = "default")]
        name: String,
    },
    Delete {
        id_or_prefix: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = DataPaths::resolve();
    paths.ensure_layout()?;
    let config = AppConfig::load(&paths)?;

    if cli.daemon {
        init_tracing(&paths, true)?;
        let auth = CursorAuth::resolve(&paths, &config)?;
        return run_daemon(paths, config, auth)
            .await
            .map_err(|e| anyhow::anyhow!(e));
    }

    init_tracing(&paths, false)?;

    match cli.command {
        None => {
            println!("copenai — run `copenai doctor` or `copenai start`");
        }
        Some(Commands::Doctor) => doctor(&paths, &config).await?,
        Some(Commands::Auth { command }) => auth_cmd(&paths, &config, command).await?,
        Some(Commands::Keys { command }) => keys_cmd(&paths, command).await?,
        Some(Commands::Start) => start_server(&paths, &config).await?,
        Some(Commands::Stop) => stop_server(&paths, &config).await?,
        Some(Commands::Status) => status_cmd(&paths, &config).await?,
        Some(Commands::Logs { follow }) => {
            tail_log(&paths.server_log(), follow, 100).map_err(|e| anyhow::anyhow!(e))?;
        }
    }

    Ok(())
}

fn init_tracing(paths: &DataPaths, daemon: bool) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if daemon {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.server_log())?;
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(file)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
    Ok(())
}

async fn doctor(paths: &DataPaths, config: &AppConfig) -> Result<()> {
    println!("copenai doctor");
    println!("  install dir: {}", paths.root.display());
    println!("  rustc: {}", rustc_version());
    println!("  agent bin: {}", config.cursor.agent_bin);

    let auth = CursorAuth::resolve(paths, config)?;
    let cmd = CursorCommand::from_auth(&auth);
    match cmd.status_json().await {
        Ok(status) => println!(
            "  cursor auth: {}",
            if status.is_ok() {
                "ok"
            } else {
                "not authenticated"
            }
        ),
        Err(e) => println!("  cursor auth: error ({e})"),
    }

    let resume = copenai_acp::probe_acp_resume(&auth).await;
    println!("  resume mode: {resume}");

    Ok(())
}

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

async fn auth_cmd(paths: &DataPaths, config: &AppConfig, command: AuthCommands) -> Result<()> {
    let auth = CursorAuth::resolve(paths, config)?;
    let cmd = CursorCommand::from_auth(&auth);
    match command {
        AuthCommands::Status => {
            let status = cmd.status_json().await?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        AuthCommands::Login => {
            cmd.login().await?;
            println!("login complete");
        }
        AuthCommands::ApiKey { key } => {
            let key = key.unwrap_or_else(|| {
                rpassword::prompt_password("Cursor API key: ").unwrap_or_default()
            });
            if key.is_empty() {
                return Err(copenai_core::CoreError::Other("empty key".into()));
            }
            CursorAuth::write_api_key(paths, &key)?;
            println!("api key stored in {}", paths.cursor_env_file().display());
        }
        AuthCommands::Logout => {
            cmd.logout().await?;
            CursorAuth::clear_api_key(paths)?;
            println!("logged out");
        }
    }
    Ok(())
}

async fn keys_cmd(paths: &DataPaths, command: KeysCommands) -> Result<()> {
    let store = Store::open(&paths.database_file()).await?;
    match command {
        KeysCommands::List => {
            for key in ApiKeyStore::list(store.pool()).await? {
                println!(
                    "{}  {}  {}  {}",
                    key.id, key.name, key.prefix, key.created_at
                );
            }
        }
        KeysCommands::Add { name } => {
            let (record, secret) = ApiKeyStore::create(store.pool(), &name).await?;
            println!("created key id={} prefix={}", record.id, record.prefix);
            println!("secret (shown once): {secret}");
        }
        KeysCommands::Delete { id_or_prefix } => {
            let deleted = ApiKeyStore::delete(store.pool(), &id_or_prefix).await?;
            println!("deleted: {deleted}");
        }
    }
    Ok(())
}

async fn start_server(paths: &DataPaths, config: &AppConfig) -> Result<()> {
    if let Err(e) = CursorAuth::ensure_authenticated(paths, config).await {
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            eprintln!("{e}");
            eprintln!("Choose: 1) login  2) api-key  3) use CURSOR_API_KEY env");
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).ok();
            match line.trim() {
                "1" => {
                    let auth = CursorAuth::resolve(paths, config)?;
                    CursorCommand::from_auth(&auth).login().await?;
                }
                "2" => {
                    let key = rpassword::prompt_password("Cursor API key: ").unwrap_or_default();
                    CursorAuth::write_api_key(paths, &key)?;
                }
                _ => {
                    if std::env::var("CURSOR_API_KEY").is_err() {
                        return Err(e);
                    }
                }
            }
        } else {
            return Err(e);
        }
    }

    spawn_daemon(paths)?;
    let pid = ensure_daemon_started(paths, &config.server.bind).await?;
    println!(
        "server started on http://{} (pid {pid})",
        config.server.bind
    );
    Ok(())
}

async fn stop_server(paths: &DataPaths, config: &AppConfig) -> Result<()> {
    let port = copenai_core::daemon::parse_bind_port(&config.server.bind)?;
    match stop_daemon(paths, port)? {
        StopOutcome::StoppedPid(pid) => println!("stopped pid {pid}"),
        StopOutcome::StoppedListeners(pids) => {
            println!(
                "stopped listener(s) on port {port}: {}",
                pids.iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        StopOutcome::NotRunning => println!("not running"),
    }
    Ok(())
}

async fn status_cmd(paths: &DataPaths, config: &AppConfig) -> Result<()> {
    let pid = read_pid(paths)?;
    let running = pid.map(is_process_alive).unwrap_or(false);
    println!("daemon: {}", if running { "running" } else { "stopped" });
    if let Some(pid) = pid {
        println!("pid: {pid}");
    }
    println!("bind: {}", config.server.bind);

    let auth = CursorAuth::resolve(paths, config)?;
    let cmd = CursorCommand::from_auth(&auth);
    let auth_ok = cmd.status_json().await.map(|s| s.is_ok()).unwrap_or(false);
    println!("cursor auth: {}", if auth_ok { "ok" } else { "missing" });

    let store = Store::open(&paths.database_file()).await?;
    let active = ConversationStore::count_active(store.pool()).await?;
    println!("active conversations (db): {active}");
    Ok(())
}
