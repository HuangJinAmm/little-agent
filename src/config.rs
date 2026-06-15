use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, de::DeserializeOwned};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub provider: ProviderKind,
    pub anthropic: Option<AnthropicConfig>,
    pub openai: Option<OpenAiConfig>,
    pub runtime: RuntimeConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnthropicConfig {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenAiConfig {
    pub model: String,
    pub api_key: String,
    pub base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    pub context_limit: usize,
    pub max_tokens: u32,
}

pub trait ConfigSection: Sized + DeserializeOwned {
    const SECTION: &'static str;

    fn from_root(root: &toml::Value) -> Result<Self> {
        let value = root
            .get(Self::SECTION)
            .cloned()
            .ok_or_else(|| anyhow!("missing config section: {}", Self::SECTION))?;

        value
            .try_into::<Self>()
            .map_err(|error| anyhow!("invalid config section {}: {}", Self::SECTION, error))
    }

    fn validate(&self) -> Result<()> {
        Ok(())
    }
}

fn optional_section<T: ConfigSection>(root: &toml::Value) -> Result<Option<T>> {
    match root.get(T::SECTION) {
        Some(_) => T::from_root(root).map(Some),
        None => Ok(None),
    }
}

impl ConfigSection for AnthropicConfig {
    const SECTION: &'static str = "anthropic";

    fn validate(&self) -> Result<()> {
        ensure!(
            !self.model.trim().is_empty(),
            "anthropic.model must not be empty"
        );
        ensure!(
            !self.api_key.trim().is_empty(),
            "anthropic.api_key must not be empty"
        );
        ensure!(
            !self.base_url.trim().is_empty(),
            "anthropic.base_url must not be empty"
        );
        Ok(())
    }
}

impl ConfigSection for OpenAiConfig {
    const SECTION: &'static str = "openai";

    fn validate(&self) -> Result<()> {
        ensure!(!self.model.trim().is_empty(), "openai.model must not be empty");
        ensure!(
            !self.base_url.trim().is_empty(),
            "openai.base_url must not be empty"
        );
        Ok(())
    }
}

impl ConfigSection for RuntimeConfig {
    const SECTION: &'static str = "runtime";

    fn validate(&self) -> Result<()> {
        ensure!(
            self.context_limit > 0,
            "runtime.context_limit must be greater than 0"
        );
        ensure!(
            self.max_tokens > 0,
            "runtime.max_tokens must be greater than 0"
        );
        Ok(())
    }
}

fn validate_selected_provider(config: &AgentConfig) -> Result<()> {
    match config.provider {
        ProviderKind::Anthropic => ensure!(
            config.anthropic.is_some(),
            "provider anthropic requires [anthropic] section"
        ),
        ProviderKind::OpenAi => ensure!(
            config.openai.is_some(),
            "provider openai requires [openai] section"
        ),
    }
    Ok(())
}

impl AgentConfig {
    pub fn active_model(&self) -> &str {
        match self.provider {
            ProviderKind::Anthropic => &self.anthropic.as_ref().expect("validated config").model,
            ProviderKind::OpenAi => &self.openai.as_ref().expect("validated config").model,
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file: {}", path.display()))?;
        let root: toml::Value = toml::from_str(&raw)
            .with_context(|| format!("failed to parse config file: {}", path.display()))?;

        let provider = root
            .get("provider")
            .cloned()
            .ok_or_else(|| anyhow!("missing config field: provider"))?
            .try_into::<ProviderKind>()
            .map_err(|error| anyhow!("invalid config field provider: {}", error))?;
        let anthropic = optional_section::<AnthropicConfig>(&root)?;
        if let Some(config) = &anthropic {
            config.validate()?;
        }
        let openai = optional_section::<OpenAiConfig>(&root)?;
        if let Some(config) = &openai {
            config.validate()?;
        }
        let runtime = RuntimeConfig::from_root(&root)?;
        runtime.validate()?;

        let config = Self {
            provider,
            anthropic,
            openai,
            runtime,
        };
        validate_selected_provider(&config)?;

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{AgentConfig, AnthropicConfig, OpenAiConfig, ProviderKind};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn write_temp_config(contents: &str, name: &str) -> PathBuf {
        let unique_id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "little-agent-config-test-{name}-{timestamp}-{unique_id}"
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("agent.toml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn loads_valid_config() {
        let path = write_temp_config(
            r#"
provider = "anthropic"

[anthropic]
model = "claude-sonnet"
api_key = "key"
base_url = "https://example.com"

[runtime]
context_limit = 50000
max_tokens = 8000
"#,
            "valid",
        );

        let config = AgentConfig::load(&path).unwrap();
        assert!(matches!(config.provider, ProviderKind::Anthropic));
        assert_eq!(config.active_model(), "claude-sonnet");
        assert_eq!(config.anthropic.as_ref().unwrap().api_key, "key");
        assert_eq!(
            config.anthropic.as_ref().unwrap().base_url,
            "https://example.com"
        );
        assert_eq!(config.runtime.context_limit, 50000);
        assert_eq!(config.runtime.max_tokens, 8000);
    }

    #[test]
    fn errors_when_section_is_missing() {
        let path = write_temp_config(
            r#"
provider = "anthropic"

[anthropic]
model = "claude-sonnet"
api_key = "key"
base_url = "https://example.com"
"#,
            "missing-runtime",
        );

        let error = AgentConfig::load(&path).unwrap_err().to_string();
        assert!(error.contains("missing config section: runtime"));
    }

    #[test]
    fn errors_when_field_type_is_invalid() {
        let path = write_temp_config(
            r#"
provider = "anthropic"

[anthropic]
model = "claude-sonnet"
api_key = "key"
base_url = "https://example.com"

[runtime]
context_limit = "large"
max_tokens = 8000
"#,
            "invalid-type",
        );

        let error = AgentConfig::load(&path).unwrap_err().to_string();
        assert!(error.contains("invalid config section runtime"));
        assert!(error.contains("context_limit"));
    }

    #[test]
    fn errors_when_runtime_values_are_zero() {
        let path = write_temp_config(
            r#"
provider = "anthropic"

[anthropic]
model = "claude-sonnet"
api_key = "key"
base_url = "https://example.com"

[runtime]
context_limit = 0
max_tokens = 0
"#,
            "invalid-runtime",
        );

        let error = AgentConfig::load(&path).unwrap_err().to_string();
        assert!(error.contains("runtime.context_limit must be greater than 0"));
    }

    #[test]
    fn loads_openai_provider_config() {
        let path = write_temp_config(
            r#"
provider = "openai"

[anthropic]
model = "claude-sonnet-4-5"
api_key = "anthropic-key"
base_url = "https://api.anthropic.com"

[openai]
model = "local-model"
api_key = ""
base_url = "http://127.0.0.1:1234/v1"

[runtime]
context_limit = 50000
max_tokens = 8000
"#,
            "openai",
        );

        let config = AgentConfig::load(&path).unwrap();
        assert!(matches!(config.provider, ProviderKind::OpenAi));
        assert_eq!(config.active_model(), "local-model");
        assert_eq!(config.openai.as_ref().unwrap().model, "local-model");
    }

    #[test]
    fn errors_when_selected_provider_section_is_missing() {
        let path = write_temp_config(
            r#"
provider = "openai"

[runtime]
context_limit = 50000
max_tokens = 8000
"#,
            "missing-openai-section",
        );

        let error = AgentConfig::load(&path).unwrap_err().to_string();
        assert!(error.contains("provider openai requires [openai] section"));
    }

    #[test]
    fn active_model_uses_selected_provider_model() {
        let config = AgentConfig {
            provider: ProviderKind::OpenAi,
            anthropic: Some(AnthropicConfig {
                model: "claude-sonnet".to_string(),
                api_key: "anthropic-key".to_string(),
                base_url: "https://api.anthropic.com".to_string(),
            }),
            openai: Some(OpenAiConfig {
                model: "local-model".to_string(),
                api_key: String::new(),
                base_url: "http://127.0.0.1:1234/v1".to_string(),
            }),
            runtime: super::RuntimeConfig {
                context_limit: 50_000,
                max_tokens: 8_000,
            },
        };

        assert_eq!(config.active_model(), "local-model");
    }
}
