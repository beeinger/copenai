use std::str::FromStr;

use agent_client_protocol::schema::v1::{InitializeRequest, InitializeResponse};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{AcpAgent, Client, ConnectionTo};
use copenai_core::cursor::{acp_command_string, CursorAuth};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeMode {
    Load,
    Resume,
    Degraded,
}

#[derive(Debug, Clone)]
pub struct ResumeCapabilities {
    pub mode: ResumeMode,
}

impl ResumeCapabilities {
    pub fn from_initialize(response: &InitializeResponse) -> Self {
        let caps = &response.agent_capabilities;
        if caps.load_session {
            Self {
                mode: ResumeMode::Load,
            }
        } else if caps.session_capabilities.resume.is_some() {
            Self {
                mode: ResumeMode::Resume,
            }
        } else {
            Self {
                mode: ResumeMode::Degraded,
            }
        }
    }

    pub fn as_str(self) -> &'static str {
        match self.mode {
            ResumeMode::Load => "load",
            ResumeMode::Resume => "resume",
            ResumeMode::Degraded => "degraded",
        }
    }
}

pub async fn probe_acp_resume(auth: &CursorAuth) -> String {
    let command = acp_command_string(auth, None);
    let Ok(agent) = AcpAgent::from_str(&command) else {
        return "spawn_failed".into();
    };

    let result = Client
        .builder()
        .name("copenai-doctor")
        .connect_with(
            agent,
            |connection: ConnectionTo<agent_client_protocol::Agent>| async move {
                let init = connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                Ok(ResumeCapabilities::from_initialize(&init)
                    .as_str()
                    .to_string())
            },
        )
        .await;

    match result {
        Ok(mode) => mode,
        Err(_) => "initialize_failed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degraded_when_no_caps() {
        let init = InitializeResponse::new(ProtocolVersion::V1);
        assert_eq!(
            ResumeCapabilities::from_initialize(&init).mode,
            ResumeMode::Degraded
        );
    }
}
