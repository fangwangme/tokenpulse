use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub display: DisplayConfig,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    #[serde(default)]
    pub show_empty_providers: bool,
    #[serde(default)]
    pub quota_display_mode: QuotaDisplayMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum QuotaDisplayMode {
    Used,
    Remaining,
}

impl Default for QuotaDisplayMode {
    fn default() -> Self {
        QuotaDisplayMode::Remaining
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut providers = HashMap::new();
        providers.insert("claude".to_string(), ProviderConfig::default());
        providers.insert("codex".to_string(), ProviderConfig::default());
        providers.insert("gemini".to_string(), ProviderConfig::default());
        providers.insert("antigravity".to_string(), ProviderConfig::default());

        Self {
            version: 1,
            providers,
            display: DisplayConfig::default(),
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: None,
        }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            show_empty_providers: false,
            quota_display_mode: QuotaDisplayMode::default(),
        }
    }
}

fn default_true() -> bool {
    true
}

pub struct ConfigManager {
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let config_dir = home.join(".config").join("tokenpulse");

        Self {
            config_path: config_dir.join("config.toml"),
        }
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    pub fn exists(&self) -> bool {
        self.config_path.exists()
    }

    pub fn load(&self) -> Result<Config> {
        if !self.config_path.exists() {
            let config = Config::default();
            // Write default config so users can discover and edit it
            let _ = self.save(&config);
            return Ok(config);
        }

        let content = fs::read_to_string(&self.config_path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn save(&self, config: &Config) -> Result<()> {
        if let Some(parent) = self.config_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }

        let content = toml::to_string_pretty(config)?;
        fs::write(&self.config_path, content)?;
        Ok(())
    }

    pub fn is_provider_enabled(&self, provider: &str) -> bool {
        match self.load() {
            Ok(config) => config
                .providers
                .get(provider)
                .map(|p| p.enabled)
                .unwrap_or(true),
            Err(_) => true,
        }
    }

    pub fn get_enabled_providers(&self) -> Vec<String> {
        match self.load() {
            Ok(config) => config
                .providers
                .iter()
                .filter(|(_, p)| p.enabled)
                .map(|(k, _)| k.clone())
                .collect(),
            Err(_) => vec![
                "claude".to_string(),
                "codex".to_string(),
                "gemini".to_string(),
                "antigravity".to_string(),
            ],
        }
    }

    pub fn enable_provider(&self, provider: &str) -> Result<()> {
        let mut config = self.load().unwrap_or_default();
        config
            .providers
            .entry(provider.to_string())
            .or_insert_with(ProviderConfig::default)
            .enabled = true;
        self.save(&config)
    }

    pub fn disable_provider(&self, provider: &str) -> Result<()> {
        let mut config = self.load().unwrap_or_default();
        if let Some(p) = config.providers.get_mut(provider) {
            p.enabled = false;
        }
        self.save(&config)
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(config.providers.contains_key("claude"));
        assert!(config.providers.contains_key("codex"));
        assert!(config.providers.contains_key("gemini"));
        assert!(config.providers.contains_key("antigravity"));
    }

    #[test]
    fn test_provider_enabled_by_default() {
        let config = Config::default();
        let claude = config.providers.get("claude").unwrap();
        assert!(claude.enabled);
    }
}
