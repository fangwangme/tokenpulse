use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub keeper: KeeperConfig,
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
    /// Whether quota refreshes are allowed. When false, the TUI skips
    /// quota fetches on startup, auto-refresh, and manual `r`, and the
    /// non-TUI text and `--json` outputs fall back to cached snapshots
    /// without contacting any quota API. The check is read at each refresh
    /// trigger, so toggling takes effect on the next refresh cycle.
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub refresh_quota: bool,
    #[serde(default)]
    pub notification_level: NotificationLevel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    Off,
    InApp,
    Terminal,
    System,
}

impl NotificationLevel {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::InApp,
            Self::InApp => Self::Terminal,
            Self::Terminal => Self::System,
            Self::System => Self::Off,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Off => Self::System,
            Self::InApp => Self::Off,
            Self::Terminal => Self::InApp,
            Self::System => Self::Terminal,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::InApp => "in_app",
            Self::Terminal => "terminal",
            Self::System => "system",
        }
    }

    pub fn display_label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::InApp => "in-app (toast)",
            Self::Terminal => "terminal",
            Self::System => "system",
        }
    }
}

impl Default for NotificationLevel {
    fn default() -> Self {
        NotificationLevel::System
    }
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
            keeper: KeeperConfig::default(),
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
            refresh_quota: true,
            notification_level: NotificationLevel::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeeperConfig {
    /// Off by default: the engine spends real quota by driving the agent CLIs,
    /// so it has to be something the user turns on rather than something an
    /// upgrade turns on for them.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_keeper_check_interval_secs")]
    pub check_interval_secs: u32,
    #[serde(default = "default_keeper_agents")]
    pub agents: HashMap<String, AgentKeeperConfig>,
}

fn default_keeper_check_interval_secs() -> u32 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentKeeperConfig {
    #[serde(default = "default_true")]
    pub session_keeper_enabled: bool,
    #[serde(default = "default_daily_wakeup_time")]
    pub daily_wakeup_time: String,
    #[serde(default = "default_true")]
    pub weekly_keeper_enabled: bool,
    #[serde(default = "default_command")]
    pub command: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_prompt")]
    pub prompt: String,
}

fn default_daily_wakeup_time() -> String {
    "10:30".to_string()
}

fn default_command() -> String {
    String::new()
}

fn default_model() -> String {
    String::new()
}

fn default_prompt() -> String {
    "Hi".to_string()
}

/// Mirrors the per-field serde defaults, so an agent the config has never seen
/// still deserializes and toggles consistently.
impl Default for AgentKeeperConfig {
    fn default() -> Self {
        Self {
            session_keeper_enabled: default_true(),
            daily_wakeup_time: default_daily_wakeup_time(),
            weekly_keeper_enabled: default_true(),
            command: default_command(),
            model: default_model(),
            prompt: default_prompt(),
        }
    }
}

pub fn default_keeper_agents() -> HashMap<String, AgentKeeperConfig> {
    let mut map = HashMap::new();
    map.insert(
        "claude".to_string(),
        AgentKeeperConfig {
            session_keeper_enabled: true,
            daily_wakeup_time: "10:30".to_string(),
            weekly_keeper_enabled: true,
            command: "claude -p \"{prompt}\" --model {model} --no-session-persistence".to_string(),
            model: "haiku".to_string(),
            prompt: "Hi".to_string(),
        },
    );
    map.insert(
        "codex".to_string(),
        AgentKeeperConfig {
            session_keeper_enabled: true,
            daily_wakeup_time: "10:30".to_string(),
            weekly_keeper_enabled: true,
            command: "codex exec --skip-git-repo-check --ephemeral --model {model} \"{prompt}\""
                .to_string(),
            model: "gpt-5.6-luna".to_string(),
            prompt: "Hi".to_string(),
        },
    );
    map.insert(
        "antigravity".to_string(),
        AgentKeeperConfig {
            session_keeper_enabled: true,
            daily_wakeup_time: "10:30".to_string(),
            weekly_keeper_enabled: true,
            command: "agy --model {model} --prompt \"{prompt}\"".to_string(),
            model: "gemini-3.7-flash-low".to_string(),
            prompt: "Hi".to_string(),
        },
    );
    map
}

/// Commands that shipped as defaults in earlier builds and should be replaced.
///
/// These are matched exactly rather than by substring: an earlier `contains("--bare")`
/// check also clobbered any hand-written command that happened to use that flag.
const LEGACY_KEEPER_COMMANDS: &[(&str, &str)] = &[
    ("claude", "claude -p \"{prompt}\" --model {model}"),
    (
        "claude",
        "claude --bare -p \"{prompt}\" --model {model} --no-session-persistence",
    ),
    ("codex", "codex exec \"{prompt}\" -m {model}"),
    ("antigravity", "agy query \"{prompt}\" --model {model}"),
];

fn is_legacy_keeper_command(agent: &str, command: &str) -> bool {
    LEGACY_KEEPER_COMMANDS
        .iter()
        .any(|(name, legacy)| *name == agent && *legacy == command.trim())
}

impl Default for KeeperConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            check_interval_secs: default_keeper_check_interval_secs(),
            agents: default_keeper_agents(),
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

    /// Where the Keeper's last-fired bookkeeping lives, alongside the config.
    pub fn keeper_state_path(&self) -> PathBuf {
        self.config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("keeper_state.json")
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

        let default_agents = default_keeper_agents();
        let mut modified = false;

        for (name, default_cfg) in &default_agents {
            if let Some(agent_cfg) = config.keeper.agents.get_mut(name) {
                if agent_cfg.command.is_empty()
                    || is_legacy_keeper_command(name, &agent_cfg.command)
                {
                    agent_cfg.command = default_cfg.command.clone();
                    modified = true;
                }
                if agent_cfg.model == "claude-3-5-haiku-20241022" {
                    agent_cfg.model = "haiku".to_string();
                    modified = true;
                }
                if agent_cfg.model == "gpt-5.6-luna-low" {
                    agent_cfg.model = "gpt-5.6-luna".to_string();
                    modified = true;
                }
            } else {
                config
                    .keeper
                    .agents
                    .insert(name.clone(), default_cfg.clone());
                modified = true;
            }
        }

        if config.version < 3 || modified {
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

    /// Flips one of an agent's keeper switches and returns its new value,
    /// seeding the entry from the built-in defaults if the config has never
    /// mentioned that agent.
    fn toggle_agent_keeper(
        &self,
        agent: &str,
        switch: fn(&mut AgentKeeperConfig) -> &mut bool,
    ) -> Result<bool> {
        let mut config = self.load().unwrap_or_default();
        let entry = config
            .keeper
            .agents
            .entry(agent.to_string())
            .or_insert_with(|| default_keeper_agents().remove(agent).unwrap_or_default());

        let flag = switch(entry);
        *flag = !*flag;
        let new_state = *flag;

        self.save(&config)?;
        Ok(new_state)
    }

    pub fn toggle_agent_session_keeper(&self, agent: &str) -> Result<bool> {
        self.toggle_agent_keeper(agent, |c| &mut c.session_keeper_enabled)
    }

    pub fn toggle_agent_weekly_keeper(&self, agent: &str) -> Result<bool> {
        self.toggle_agent_keeper(agent, |c| &mut c.weekly_keeper_enabled)
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
        assert!(config.display.scan_antigravity);
        assert!(config.display.refresh_quota);
        assert_eq!(config.display.notification_level, NotificationLevel::System);
    }

    #[test]
    fn test_notification_level_cycling() {
        let level = NotificationLevel::Off;
        assert_eq!(level.next(), NotificationLevel::InApp);
        assert_eq!(level.next().next(), NotificationLevel::Terminal);
        assert_eq!(level.next().next().next(), NotificationLevel::System);
        assert_eq!(level.next().next().next().next(), NotificationLevel::Off);

        assert_eq!(level.prev(), NotificationLevel::System);
        assert_eq!(level.prev().prev(), NotificationLevel::Terminal);
        assert_eq!(level.prev().prev().prev(), NotificationLevel::InApp);
        assert_eq!(level.prev().prev().prev().prev(), NotificationLevel::Off);
    }

    #[test]
    fn test_notification_level_labels() {
        assert_eq!(NotificationLevel::Off.label(), "off");
        assert_eq!(NotificationLevel::InApp.label(), "in_app");
        assert_eq!(NotificationLevel::Terminal.label(), "terminal");
        assert_eq!(NotificationLevel::System.label(), "system");

        assert_eq!(NotificationLevel::Off.display_label(), "off");
        assert_eq!(NotificationLevel::InApp.display_label(), "in-app (toast)");
        assert_eq!(NotificationLevel::Terminal.display_label(), "terminal");
        assert_eq!(NotificationLevel::System.display_label(), "system");
    }

    #[test]
    fn test_notification_level_serde_roundtrip() {
        for level in [
            NotificationLevel::Off,
            NotificationLevel::InApp,
            NotificationLevel::Terminal,
            NotificationLevel::System,
        ] {
            let serialized = serde_json::to_string(&level).unwrap();
            let deserialized: NotificationLevel = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, level);
        }

        let toml_str = r#"
version = 3
[display]
notification_level = "in_app"
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.display.notification_level, NotificationLevel::InApp);
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
    fn test_refresh_quota_defaults_true_when_absent() {
        let toml_str = r#"
version = 3
[display]
auto_refresh_secs = 60
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.display.refresh_quota);
    }

    #[test]
    fn test_refresh_quota_deserializes_from_toml() {
        let toml_str = r#"
version = 3
[display]
refresh_quota = false
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(!config.display.refresh_quota);
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

    #[test]
    fn test_keeper_config_defaults() {
        let config = Config::default();
        // The engine spends real quota, so it must be opt-in.
        assert!(!config.keeper.enabled);
        assert_eq!(config.keeper.check_interval_secs, 60);
        assert_eq!(config.keeper.agents.len(), 3);

        let claude = config.keeper.agents.get("claude").unwrap();
        assert!(claude.session_keeper_enabled);
        assert_eq!(claude.daily_wakeup_time, "10:30");
        assert!(claude.weekly_keeper_enabled);
        assert_eq!(claude.model, "haiku");
        assert_eq!(
            claude.command,
            "claude -p \"{prompt}\" --model {model} --no-session-persistence"
        );

        let codex = config.keeper.agents.get("codex").unwrap();
        assert!(codex.session_keeper_enabled);
        assert_eq!(codex.model, "gpt-5.6-luna");
        assert_eq!(
            codex.command,
            "codex exec --skip-git-repo-check --ephemeral --model {model} \"{prompt}\""
        );

        let antigravity = config.keeper.agents.get("antigravity").unwrap();
        assert!(antigravity.session_keeper_enabled);
        assert_eq!(antigravity.model, "gemini-3.7-flash-low");
        assert_eq!(
            antigravity.command,
            "agy --model {model} --prompt \"{prompt}\""
        );
    }

    #[test]
    fn test_keeper_toggle_helpers() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let manager = ConfigManager { config_path };

        let session_state = manager.toggle_agent_session_keeper("claude").unwrap();
        assert!(!session_state);

        let weekly_state = manager.toggle_agent_weekly_keeper("codex").unwrap();
        assert!(!weekly_state);
    }

    #[test]
    fn test_custom_keeper_command_is_not_clobbered() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        // A hand-written command that merely mentions `--bare` used to be
        // overwritten by the substring-based migration.
        let custom = r#"
version = 3
[keeper]
enabled = true
[keeper.agents.claude]
command = 'claude --bare -p "{prompt}" --model {model} --my-flag'
model = "haiku"
"#;
        fs::write(&config_path, custom).unwrap();

        let manager = ConfigManager { config_path };
        let loaded = manager.load().unwrap();

        assert_eq!(
            loaded.keeper.agents.get("claude").unwrap().command,
            "claude --bare -p \"{prompt}\" --model {model} --my-flag"
        );
    }

    #[test]
    fn test_keeper_state_path_is_next_to_config() {
        let manager = ConfigManager {
            config_path: PathBuf::from("/tmp/tp/config.toml"),
        };
        assert_eq!(
            manager.keeper_state_path(),
            PathBuf::from("/tmp/tp/keeper_state.json")
        );
    }

    #[test]
    fn test_legacy_keeper_command_migration() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let legacy_toml = r#"
version = 3
[keeper]
enabled = true
[keeper.agents.antigravity]
command = 'agy query "{prompt}" --model {model}'
model = "gemini-3.7-flash-low"
[keeper.agents.claude]
command = 'claude -p "{prompt}" --model {model}'
model = "claude-3-5-haiku-20241022"
[keeper.agents.codex]
command = 'codex exec "{prompt}" -m {model}'
model = "gpt-5.6-luna-low"
"#;
        fs::write(&config_path, legacy_toml).unwrap();

        let manager = ConfigManager { config_path };
        let loaded = manager.load().unwrap();

        let agy = loaded.keeper.agents.get("antigravity").unwrap();
        assert_eq!(agy.command, "agy --model {model} --prompt \"{prompt}\"");

        let claude = loaded.keeper.agents.get("claude").unwrap();
        assert_eq!(
            claude.command,
            "claude -p \"{prompt}\" --model {model} --no-session-persistence"
        );
        assert_eq!(claude.model, "haiku");

        let codex = loaded.keeper.agents.get("codex").unwrap();
        assert_eq!(
            codex.command,
            "codex exec --skip-git-repo-check --ephemeral --model {model} \"{prompt}\""
        );
        assert_eq!(codex.model, "gpt-5.6-luna");
    }
}
