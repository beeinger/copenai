use std::fs;
use std::process::Stdio;

use serde::Deserialize;
use tokio::process::Command;

use crate::config::AppConfig;
use crate::error::{CoreError, Result};
use crate::paths::DataPaths;

#[derive(Debug, Clone, Default)]
pub struct CursorAuth {
    pub api_key: Option<String>,
    pub endpoint: Option<String>,
    pub extra_args: Vec<String>,
    pub agent_bin: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct CursorAuthStatus {
    pub status: Option<String>,
    #[serde(rename = "isAuthenticated")]
    pub is_authenticated: Option<bool>,
    #[serde(rename = "hasAccessToken")]
    pub has_access_token: Option<bool>,
}

impl CursorAuthStatus {
    pub fn is_ok(&self) -> bool {
        self.is_authenticated.unwrap_or(false)
            || self.has_access_token.unwrap_or(false)
            || self.status.as_deref() == Some("authenticated")
    }
}

pub struct CursorCommand {
    agent_bin: String,
    api_key: Option<String>,
    endpoint: Option<String>,
    extra_args: Vec<String>,
}

impl CursorCommand {
    pub fn from_auth(auth: &CursorAuth) -> Self {
        Self {
            agent_bin: auth.agent_bin.clone(),
            api_key: auth.api_key.clone(),
            endpoint: auth.endpoint.clone(),
            extra_args: auth.extra_args.clone(),
        }
    }

    pub fn base_cmd(&self) -> Command {
        let mut cmd = Command::new(&self.agent_bin);
        if let Some(key) = &self.api_key {
            cmd.arg("--api-key").arg(key);
        }
        if let Some(endpoint) = &self.endpoint {
            if !endpoint.is_empty() {
                cmd.arg("-e").arg(endpoint);
            }
        }
        for arg in &self.extra_args {
            cmd.arg(arg);
        }
        cmd
    }

    pub fn acp_spawn_argv(&self, model: Option<&str>) -> Vec<String> {
        let mut argv = vec![self.agent_bin.clone()];
        if let Some(key) = &self.api_key {
            argv.push("--api-key".into());
            argv.push(key.clone());
        }
        if let Some(endpoint) = &self.endpoint {
            if !endpoint.is_empty() {
                argv.push("-e".into());
                argv.push(endpoint.clone());
            }
        }
        argv.extend(self.extra_args.clone());
        if let Some(model) = model.filter(|m| !m.is_empty()) {
            argv.push("--model".into());
            argv.push(model.to_string());
        }
        argv.push("acp".into());
        argv
    }

    pub async fn status_json(&self) -> Result<CursorAuthStatus> {
        let output = self
            .base_cmd()
            .arg("status")
            .arg("--format")
            .arg("json")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| CoreError::CursorCommand(e.to_string()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CoreError::CursorCommand(stderr.to_string()));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str(&stdout).map_err(|e| CoreError::CursorCommand(e.to_string()))
    }

    pub async fn login(&self) -> Result<()> {
        let status = self
            .base_cmd()
            .arg("login")
            .status()
            .await
            .map_err(|e| CoreError::CursorCommand(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(CoreError::CursorCommand("login failed".into()))
        }
    }

    pub async fn logout(&self) -> Result<()> {
        let status = self
            .base_cmd()
            .arg("logout")
            .status()
            .await
            .map_err(|e| CoreError::CursorCommand(e.to_string()))?;
        if status.success() {
            Ok(())
        } else {
            Err(CoreError::CursorCommand("logout failed".into()))
        }
    }

    pub async fn create_chat(&self) -> Result<String> {
        let output = self
            .base_cmd()
            .arg("create-chat")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| CoreError::CursorCommand(e.to_string()))?;
        if !output.status.success() {
            return Err(CoreError::CursorCommand(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub async fn list_models(&self) -> Result<Vec<String>> {
        let output = self
            .base_cmd()
            .arg("models")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| CoreError::CursorCommand(e.to_string()))?;
        if !output.status.success() {
            return Err(CoreError::CursorCommand(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let models: Vec<String> = text
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with("Available") {
                    return None;
                }
                line.split(" - ").next().map(str::trim).map(str::to_string)
            })
            .collect();
        Ok(models)
    }
}

impl CursorAuth {
    pub fn resolve(paths: &DataPaths, config: &AppConfig) -> Result<Self> {
        let file_key = read_cursor_env(paths)?;
        let env_key = std::env::var("CURSOR_API_KEY").ok();
        let api_key = file_key.or(env_key);
        let endpoint = if config.cursor.endpoint.is_empty() {
            None
        } else {
            Some(config.cursor.endpoint.clone())
        };
        Ok(Self {
            api_key,
            endpoint,
            extra_args: config.cursor.extra_args.clone(),
            agent_bin: config.cursor.agent_bin.clone(),
        })
    }

    pub async fn ensure_authenticated(paths: &DataPaths, config: &AppConfig) -> Result<()> {
        let auth = Self::resolve(paths, config)?;
        let cmd = CursorCommand::from_auth(&auth);
        let status = cmd.status_json().await?;
        if status.is_ok() {
            return Ok(());
        }
        Err(CoreError::CursorNotAuthenticated(
            "run `copenai auth login` or `copenai auth api-key`".into(),
        ))
    }

    pub fn write_api_key(paths: &DataPaths, key: &str) -> Result<()> {
        paths.ensure_layout()?;
        let path = paths.cursor_env_file();
        let content = format!("CURSOR_API_KEY={key}\n");
        fs::write(&path, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn clear_api_key(paths: &DataPaths) -> Result<()> {
        let path = paths.cursor_env_file();
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

fn read_cursor_env(paths: &DataPaths) -> Result<Option<String>> {
    let path = paths.cursor_env_file();
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("CURSOR_API_KEY=") {
            let key = rest.trim().trim_matches('"').trim_matches('\'');
            if !key.is_empty() {
                return Ok(Some(key.to_string()));
            }
        }
    }
    Ok(None)
}

pub fn acp_command_string(auth: &CursorAuth, model: Option<&str>) -> String {
    CursorCommand::from_auth(auth)
        .acp_spawn_argv(model)
        .into_iter()
        .map(|s| shell_escape(&s))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_escape(s: &str) -> String {
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-:".contains(c))
    {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acp_argv_includes_model_before_acp() {
        let auth = CursorAuth::default();
        let argv = CursorCommand::from_auth(&auth).acp_spawn_argv(Some("composer-2.5"));
        let model_idx = argv
            .iter()
            .position(|a| a == "--model")
            .expect("model flag");
        let acp_idx = argv.iter().position(|a| a == "acp").expect("acp");
        assert_eq!(
            argv.get(model_idx + 1).map(String::as_str),
            Some("composer-2.5")
        );
        assert!(model_idx < acp_idx);
    }
}
