use indexmap::IndexMap;

/// Configuration for the web search tool.
///
/// Use `Disabled` when no API key is available or web search should be turned off.
/// Use `Enabled { … }` to provide credentials and endpoint configuration.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WebSearchConfig {
    #[default]
    Disabled,
    Enabled {
        api_key: String,
        base_url: String,
        model: String,
        #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
        extra_headers: IndexMap<String, String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        alpha_test_key: Option<String>,
    },
}

impl WebSearchConfig {
    /// Returns `true` when the config is the `Enabled` variant.
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    /// Return a copy safe for returning to clients.
    ///
    /// The `api_key` is replaced with `"***REDACTED***"` and the optional
    /// extra access key field is stripped.
    pub fn redacted(&self) -> Self {
        match self {
            Self::Disabled => Self::Disabled,
            Self::Enabled {
                base_url,
                model,
                extra_headers,
                ..
            } => Self::Enabled {
                api_key: "***REDACTED***".to_string(),
                base_url: base_url.clone(),
                model: model.clone(),
                extra_headers: extra_headers.clone(),
                alpha_test_key: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default_is_disabled() {
        let config = WebSearchConfig::default();
        assert!(!config.is_enabled());
    }

    #[test]
    fn test_config_enabled() {
        let config = WebSearchConfig::Enabled {
            api_key: "test-key".to_string(),
            base_url: "https://api.example.test/v1".to_string(),
            model: "test-web-search-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
        };
        assert!(config.is_enabled());
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = WebSearchConfig::Enabled {
            api_key: "key".to_string(),
            base_url: "https://api.example.test/v1".to_string(),
            model: "test-web-search-model".to_string(),
            extra_headers: IndexMap::new(),
            alpha_test_key: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: WebSearchConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_enabled());
    }

    #[test]
    fn test_config_deserialize_from_set_options_payload() {
        let json = r#"{
            "status": "enabled",
            "api_key": "ainxt-abc123",
            "base_url": "https://api.example.test/v1",
            "model": "test-web-search-model"
        }"#;
        let config: WebSearchConfig = serde_json::from_str(json).unwrap();
        assert!(config.is_enabled());
    }
}
