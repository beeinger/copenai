#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    Client,
    Server,
}

impl ToolExecutionMode {
    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "server" => Self::Server,
            _ => Self::Client,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
        }
    }
}

/// Resolve mode: header > metadata.tool_execution > config default.
pub fn resolve_mode(
    config_default: &str,
    metadata_mode: Option<&str>,
    header_mode: Option<&str>,
) -> ToolExecutionMode {
    if let Some(m) = header_mode {
        return ToolExecutionMode::parse(m);
    }
    if let Some(m) = metadata_mode {
        return ToolExecutionMode::parse(m);
    }
    ToolExecutionMode::parse(config_default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_wins() {
        assert_eq!(
            resolve_mode("client", Some("server"), Some("client")),
            ToolExecutionMode::Client
        );
        assert_eq!(
            resolve_mode("client", None, Some("server")),
            ToolExecutionMode::Server
        );
    }
}
