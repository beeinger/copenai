/// Normalize OpenAI model id for cursor agent `--model` flag.
pub fn resolve_agent_model(requested: &str) -> String {
    let trimmed = requested.trim();
    if trimmed.is_empty() {
        "auto".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn validate_model(requested: &str, available: &[String]) -> Result<String, String> {
    let model = resolve_agent_model(requested);
    if available.is_empty() {
        return Ok(model);
    }
    if available.iter().any(|m| m == &model) {
        Ok(model)
    } else {
        Err(format!(
            "unknown model '{model}'. Use GET /v1/models for available models"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_known_model() {
        let models = vec!["composer-2.5".into(), "auto".into()];
        assert_eq!(
            validate_model("composer-2.5", &models).unwrap(),
            "composer-2.5"
        );
    }

    #[test]
    fn rejects_unknown_model() {
        let models = vec!["composer-2.5".into()];
        assert!(validate_model("not-a-model", &models).is_err());
    }
}
