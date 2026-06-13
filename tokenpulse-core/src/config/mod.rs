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
    3
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
    pub theme: ThemePreference,
    #[serde(default)]
    pub quota_display_mode: QuotaDisplayMode,
    /// Unified auto-refresh interval (seconds) for both quota and usage in the
    /// TUI. 0 = disabled. Supported values: 0, 60, 120, 300, 600, 900.
    /// `quota_auto_refresh_secs` is accepted as an alias for migration from
    /// older configs that stored separate quota/usage intervals.
    #[serde(
        default = "default_auto_refresh_secs",
        alias = "quota_auto_refresh_secs"
    )]
    pub auto_refresh_secs: u32,
    #[serde(default = "default_true")]
    pub show_account: bool,
    #[serde(default = "default_true")]
    pub scan_antigravity: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    Auto,
    Dark,
    Light,
}

impl ThemePreference {
    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::Dark,
            Self::Dark => Self::Light,
            Self::Light => Self::Auto,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Auto => Self::Light,
            Self::Dark => Self::Auto,
            Self::Light => Self::Dark,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

impl Default for ThemePreference {
    fn default() -> Self {
        Self::Auto
    }
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
        providers.insert("copilot".to_string(), ProviderConfig::default());

        Self {
            version: 3,
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
            theme: ThemePreference::default(),
            quota_display_mode: QuotaDisplayMode::default(),
            auto_refresh_secs: default_auto_refresh_secs(),
            show_account: true,
            scan_antigravity: true,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_auto_refresh_secs() -> u32 {
    300
}

pub struct ConfigManager {
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Self {
        if let Some(path_str) = std::env::var_os("TOKENPULSE_CONFIG_PATH") {
            return Self {
                config_path: PathBuf::from(path_str),
            };
        }
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let config_dir = home.join(".local").join("share").join("tokenpulse");

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
            if let Err(e) = self.save(&config) {
                tracing::warn!("Failed to save default config: {}", e);
            }
            return Ok(config);
        }

        let content = fs::read_to_string(&self.config_path)?;
        let mut config: Config = toml::from_str(&content)?;

        if config.version < 3 {
            // v3 unified the separate quota/usage auto-refresh intervals into a
            // single `auto_refresh_secs`. The old `quota_auto_refresh_secs` value
            // is carried over via serde alias; re-saving drops the legacy keys.
            config.version = 3;
            if let Err(e) = self.save(&config) {
                tracing::warn!("Failed to save migrated config: {}", e);
            }
        }

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
        assert!(config.providers.contains_key("copilot"));
    }

    #[test]
    fn test_provider_enabled_by_default() {
        let config = Config::default();
        let claude = config.providers.get("claude").unwrap();
        assert!(claude.enabled);
    }

    #[test]
    fn test_default_has_five_providers() {
        let config = Config::default();
        assert_eq!(config.providers.len(), 5);
        for (_, provider) in &config.providers {
            assert!(provider.enabled);
        }
    }

    #[test]
    fn test_disabled_provider_filtered() {
        let mut config = Config::default();
        config.providers.get_mut("claude").unwrap().enabled = false;
        let enabled: Vec<_> = config
            .providers
            .iter()
            .filter(|(_, p)| p.enabled)
            .map(|(k, _)| k.clone())
            .collect();
        assert!(!enabled.contains(&"claude".to_string()));
        assert_eq!(enabled.len(), 4);
    }

    #[test]
    fn test_config_toml_roundtrip() {
        let config = Config::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.providers.len(), config.providers.len());
        assert_eq!(parsed.version, config.version);
    }

    #[test]
    fn test_partial_toml_fills_defaults() {
        let toml_str = r#"
version = 1

[providers.claude]
enabled = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.providers.contains_key("claude"));
        assert!(config.providers.get("claude").unwrap().enabled);
        // Other providers not in TOML are simply missing
        assert!(!config.providers.contains_key("codex"));
    }

    #[test]
    fn test_display_config_defaults() {
        let config = Config::default();
        assert!(!config.display.show_empty_providers);
        assert_eq!(config.display.theme, ThemePreference::Auto);
        assert_eq!(
            config.display.quota_display_mode,
            QuotaDisplayMode::Remaining
        );
        assert_eq!(config.display.auto_refresh_secs, 300);
        assert!(config.display.show_account);
    }

    #[test]
    fn test_auto_refresh_secs_deserializes_from_toml() {
        let toml_str = r#"
version = 3
[display]
auto_refresh_secs = 60
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.display.auto_refresh_secs, 60);
    }

    #[test]
    fn test_auto_refresh_secs_migrates_from_quota_alias() {
        let toml_str = r#"
version = 2
[display]
quota_auto_refresh_secs = 120
usage_auto_refresh_secs = 900
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        // Legacy quota interval is carried over; usage key is ignored.
        assert_eq!(config.display.auto_refresh_secs, 120);
    }

    #[test]
    fn test_theme_deserializes_from_toml() {
        let toml_str = r#"
version = 1
[display]
theme = "dark"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.display.theme, ThemePreference::Dark);
    }

    #[test]
    fn test_auto_refresh_secs_defaults_when_absent() {
        let toml_str = r#"
version = 3
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.display.auto_refresh_secs, 300);
        assert_eq!(config.display.theme, ThemePreference::Auto);
    }

    #[test]
    fn test_config_version_migration() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        // Write a legacy v2 config with separate quota/usage intervals.
        let legacy_toml = r#"
version = 2
[display]
quota_auto_refresh_secs = 120
usage_auto_refresh_secs = 900
"#;
        fs::write(&config_path, legacy_toml).unwrap();

        let manager = ConfigManager { config_path };
        let loaded = manager.load().unwrap();

        // Migrated to v3 with a single interval carried from the quota value.
        assert_eq!(loaded.version, 3);
        assert_eq!(loaded.display.auto_refresh_secs, 120);

        // The rewritten file drops the legacy keys in favor of the unified one.
        let written_content = fs::read_to_string(&manager.config_path).unwrap();
        assert!(written_content.contains("version = 3"));
        assert!(written_content.contains("auto_refresh_secs = 120"));
        assert!(!written_content.contains("quota_auto_refresh_secs"));
        assert!(!written_content.contains("usage_auto_refresh_secs"));
    }
}
