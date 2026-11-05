use anyhow::{Context, Result};
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

/// Current config schema version. `ConfigManager::load` rewrites an older file
/// in the current shape.
pub const CONFIG_VERSION: u32 = 5;

fn default_version() -> u32 {
    CONFIG_VERSION
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
    #[serde(default = "default_auto_refresh_secs")]
    pub auto_refresh_secs: u32,
    /// How much of the signed-in account the quota views show.
    #[serde(default)]
    pub account_display: AccountDisplay,
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
    /// Sound played by every notification level except `off`. Either `chime`
    /// (the built-in sound), `none` to stay silent, or the base name of a
    /// sound under `/System/Library/Sounds` such as `Hero`.
    #[serde(default = "default_notification_sound")]
    pub notification_sound: String,
}

fn default_notification_sound() -> String {
    crate::notification::SOUND_CHIME.to_string()
}

/// How far a quota recovery notification reaches. Every level except `Off`
/// plays the alert sound — the levels differ only in how far the *visual*
/// notification travels beyond the TUI.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    /// Nothing at all: no sound, no visuals.
    Off,
    /// Sound plus the in-TUI toast and ambient animation.
    InApp,
    /// Adds a terminal bell and an OSC 9 desktop notification.
    Terminal,
    /// Adds a macOS Notification Center banner.
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

/// How much of the signed-in account the quota card shows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AccountDisplay {
    /// No account row.
    None,
    /// The plan only.
    Plan,
    /// The plan and the account email.
    Full,
}

impl AccountDisplay {
    pub fn next(self) -> Self {
        match self {
            Self::None => Self::Plan,
            Self::Plan => Self::Full,
            Self::Full => Self::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Plan => "plan",
            Self::Full => "full",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "plan" => Some(Self::Plan),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    pub fn shows_plan(self) -> bool {
        self != Self::None
    }

    pub fn shows_account(self) -> bool {
        self == Self::Full
    }
}

impl Default for AccountDisplay {
    fn default() -> Self {
        Self::Full
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
        // Quota providers only — these are the ids with a quota fetcher. Usage
        // parsing never reads this map; it iterates its own supported list.
        let mut providers = HashMap::new();
        providers.insert("claude".to_string(), ProviderConfig::default());
        providers.insert("codex".to_string(), ProviderConfig::default());
        providers.insert("antigravity".to_string(), ProviderConfig::default());

        Self {
            version: CONFIG_VERSION,
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
            account_display: AccountDisplay::default(),
            scan_antigravity: true,
            refresh_quota: true,
            notification_level: NotificationLevel::default(),
            notification_sound: default_notification_sound(),
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

/// Values of the `keeper_antigravity_groups` setting, for
/// `weekly_all_groups` false and true.
pub const ANTIGRAVITY_GROUPS_PRIMARY: &str = "gemini_flash";
pub const ANTIGRAVITY_GROUPS_ALL: &str = "all";

impl KeeperConfig {
    /// `[keeper.agents.antigravity] weekly_all_groups`, false when the agent
    /// is not configured.
    pub fn antigravity_all_groups(&self) -> bool {
        self.agents
            .get("antigravity")
            .is_some_and(|agent| agent.weekly_all_groups)
    }

    /// Sets `weekly_all_groups`, adding the default Antigravity agent first
    /// when the config does not have one yet.
    pub fn set_antigravity_all_groups(&mut self, all_groups: bool) {
        self.agents
            .entry("antigravity".to_string())
            .or_insert_with(|| {
                default_keeper_agents()
                    .remove("antigravity")
                    .unwrap_or_default()
            })
            .weekly_all_groups = all_groups;
    }

    /// The `keeper_antigravity_groups` value shown for the current setting.
    pub fn antigravity_groups_label(&self) -> &'static str {
        if self.antigravity_all_groups() {
            ANTIGRAVITY_GROUPS_ALL
        } else {
            ANTIGRAVITY_GROUPS_PRIMARY
        }
    }
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
    #[serde(default = "default_prompt")]
    pub prompt: String,
    /// Antigravity only: ping every quota group on its weekly reset and on a
    /// manual ping, not just the primary (Gemini Flash) group. See
    /// `docs/specs/keeper.md` "Antigravity quota groups".
    #[serde(default)]
    pub weekly_all_groups: bool,
}

fn default_daily_wakeup_time() -> String {
    "10:30".to_string()
}

fn default_command() -> String {
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
            prompt: default_prompt(),
            weekly_all_groups: false,
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
            prompt: "Hi".to_string(),
            weekly_all_groups: false,
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
            prompt: "Hi".to_string(),
            weekly_all_groups: false,
        },
    );
    map.insert(
        "antigravity".to_string(),
        AgentKeeperConfig {
            session_keeper_enabled: true,
            daily_wakeup_time: "10:30".to_string(),
            weekly_keeper_enabled: true,
            command: "agy --model {model} --prompt \"{prompt}\"".to_string(),
            prompt: "Hi".to_string(),
            weekly_all_groups: false,
        },
    );
    map
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

    /// A manager bound to an explicit config file, bypassing the home-directory
    /// and `TOKENPULSE_CONFIG_PATH` lookup. Lets a caller (or a test) save to a
    /// path of its own without mutating process-wide environment.
    pub fn with_path(config_path: PathBuf) -> Self {
        Self { config_path }
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
        let mut config = parse_config(&content)
            .with_context(|| format!("invalid config file {}", self.config_path.display()))?;

        let default_agents = default_keeper_agents();
        let mut modified = false;

        for (name, default_cfg) in &default_agents {
            if let Some(agent_cfg) = config.keeper.agents.get_mut(name) {
                if agent_cfg.command.is_empty() {
                    agent_cfg.command = default_cfg.command.clone();
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

        if config.version < CONFIG_VERSION || modified {
            config.version = CONFIG_VERSION;
            if let Err(e) = self.save(&config) {
                tracing::warn!("Failed to save migrated config: {}", e);
            }
        }

        Ok(config)
    }

    /// Writes `config` to the file — but never over a file that does not parse.
    ///
    /// Whatever produced `config` then did not come from that file: most likely
    /// a caller fell back to defaults after loading it failed, as the TUI does
    /// at startup. Writing would silently discard everything the user had set
    /// over what may be a single typo.
    pub fn save(&self, config: &Config) -> Result<()> {
        match fs::read_to_string(&self.config_path) {
            Ok(existing) => {
                if let Err(error) = parse_config(&existing) {
                    return Err(error.context(format!(
                        "{} has an error and was left unchanged; fix or remove it, then retry",
                        self.config_path.display()
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

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
        let mut config = self.load()?;
        config
            .providers
            .entry(provider.to_string())
            .or_insert_with(ProviderConfig::default)
            .enabled = true;
        self.save(&config)
    }

    pub fn disable_provider(&self, provider: &str) -> Result<()> {
        let mut config = self.load()?;
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
        let mut config = self.load()?;
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

fn parse_config(content: &str) -> Result<Config> {
    Ok(toml::from_str(content)?)
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
    fn test_default_has_three_quota_providers() {
        let config = Config::default();
        let mut ids: Vec<&str> = config.providers.keys().map(String::as_str).collect();
        ids.sort();
        assert_eq!(ids, ["antigravity", "claude", "codex"]);
        for (_, provider) in &config.providers {
            assert!(provider.enabled);
        }
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

    fn load_written(toml_str: &str) -> (Config, String) {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        fs::write(&config_path, toml_str).unwrap();
        let manager = ConfigManager { config_path };
        let loaded = manager.load().unwrap();
        let written = fs::read_to_string(&manager.config_path).unwrap();
        (loaded, written)
    }

    #[test]
    fn account_display_cycles_none_plan_full() {
        assert_eq!(AccountDisplay::None.next(), AccountDisplay::Plan);
        assert_eq!(AccountDisplay::Plan.next(), AccountDisplay::Full);
        assert_eq!(AccountDisplay::Full.next(), AccountDisplay::None);
        for value in [
            AccountDisplay::None,
            AccountDisplay::Plan,
            AccountDisplay::Full,
        ] {
            assert_eq!(AccountDisplay::parse(value.label()), Some(value));
        }
        assert_eq!(AccountDisplay::parse("true"), None);
    }

    /// Copilot quota was removed; a config written while it existed must still
    /// load, with the stale key left in the map for the registry to ignore.
    #[test]
    fn config_with_legacy_copilot_provider_still_loads() {
        let (loaded, _) = load_written(
            r#"
version = 3
[providers.claude]
enabled = true
[providers.copilot]
enabled = true
"#,
        );

        assert!(loaded.providers.get("claude").unwrap().enabled);
        assert_eq!(loaded.version, CONFIG_VERSION);
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
        assert_eq!(
            claude.command,
            "claude -p \"{prompt}\" --model {model} --no-session-persistence"
        );

        let codex = config.keeper.agents.get("codex").unwrap();
        assert!(codex.session_keeper_enabled);
        assert_eq!(
            codex.command,
            "codex exec --skip-git-repo-check --ephemeral --model {model} \"{prompt}\""
        );

        let antigravity = config.keeper.agents.get("antigravity").unwrap();
        assert!(antigravity.session_keeper_enabled);
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

    /// One bad value plus settings worth keeping: what `config enable` and the
    /// Keeper toggles used to replace with defaults.
    const BROKEN_CONFIG: &str = r#"
version = 4

[display]
refresh_quota = false
theme = "purple"

[keeper.agents.claude]
prompt = "my custom prompt"
"#;

    #[test]
    fn changing_a_setting_never_overwrites_a_config_that_does_not_parse() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        fs::write(&config_path, BROKEN_CONFIG).unwrap();
        let manager = ConfigManager { config_path };

        let load_error = format!("{:#}", manager.load().unwrap_err());
        assert!(load_error.contains("invalid config file"), "{load_error}");
        assert!(
            load_error.contains("purple"),
            "the cause is reported: {load_error}"
        );

        assert!(manager.enable_provider("claude").is_err());
        assert!(manager.disable_provider("codex").is_err());
        assert!(manager.toggle_agent_session_keeper("claude").is_err());
        assert!(manager.toggle_agent_weekly_keeper("claude").is_err());
        // What a caller that fell back to defaults would write.
        let save_error = manager.save(&Config::default()).unwrap_err();
        assert!(
            save_error.to_string().contains("left unchanged"),
            "{save_error:#}"
        );

        assert_eq!(
            fs::read_to_string(&manager.config_path).unwrap(),
            BROKEN_CONFIG
        );
    }

    #[test]
    fn save_writes_over_a_valid_config_and_creates_a_missing_one() {
        let temp_dir = tempfile::tempdir().unwrap();
        let manager = ConfigManager {
            config_path: temp_dir.path().join("nested").join("config.toml"),
        };

        manager.save(&Config::default()).unwrap();
        let mut config = manager.load().unwrap();
        config.display.refresh_quota = false;
        manager.save(&config).unwrap();

        assert!(!manager.load().unwrap().display.refresh_quota);
    }
}
