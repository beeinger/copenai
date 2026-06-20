use std::fs;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};
use crate::paths::DataPaths;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub cursor: CursorSection,
    #[serde(default)]
    pub server: ServerSection,
    #[serde(default)]
    pub permissions: PermissionsSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorSection {
    #[serde(default = "default_agent_bin")]
    pub agent_bin: String,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerSection {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_max_agents")]
    pub max_concurrent_agents: usize,
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionsSection {
    #[serde(default = "default_true")]
    pub auto_approve: bool,
    #[serde(default)]
    pub webhook_url: String,
    #[serde(default = "default_webhook_timeout")]
    pub webhook_timeout_secs: u64,
}

fn default_webhook_timeout() -> u64 {
    30
}

fn default_agent_bin() -> String {
    "agent".to_string()
}

fn default_bind() -> String {
    "0.0.0.0:9241".to_string()
}

fn default_max_agents() -> usize {
    32
}

fn default_idle_timeout() -> u64 {
    1800
}

fn default_true() -> bool {
    true
}

impl Default for CursorSection {
    fn default() -> Self {
        Self {
            agent_bin: default_agent_bin(),
            endpoint: String::new(),
            extra_args: Vec::new(),
        }
    }
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            max_concurrent_agents: default_max_agents(),
            idle_timeout_secs: default_idle_timeout(),
        }
    }
}

impl Default for PermissionsSection {
    fn default() -> Self {
        Self {
            auto_approve: default_true(),
            webhook_url: String::new(),
            webhook_timeout_secs: default_webhook_timeout(),
        }
    }
}

impl AppConfig {
    pub fn load(paths: &DataPaths) -> Result<Self> {
        let path = paths.config_file();
        if !path.exists() {
            let cfg = Self::default();
            cfg.save(paths)?;
            return Ok(cfg);
        }
        let raw = fs::read_to_string(&path)?;
        toml::from_str(&raw).map_err(|e| CoreError::Config(e.to_string()))
    }

    pub fn save(&self, paths: &DataPaths) -> Result<()> {
        paths.ensure_layout()?;
        let raw = toml::to_string_pretty(self).map_err(|e| CoreError::Config(e.to_string()))?;
        fs::write(paths.config_file(), raw)?;
        Ok(())
    }
}
