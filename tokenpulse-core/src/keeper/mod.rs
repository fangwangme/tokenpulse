use crate::config::AgentKeeperConfig;
use chrono::{DateTime, Local, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::info;

#[derive(Debug, Clone, Copy)]
pub enum KeeperTriggerType {
    Daily,
    Weekly,
    Manual,
}

impl KeeperTriggerType {
    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "5h Daily",
            Self::Weekly => "Weekly Sync",
            Self::Manual => "Manual Ping",
        }
    }

    /// Stable identifier used as a database column value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Manual => "manual",
        }
    }

    /// Unknown values fall back to `Manual` so a hand-edited or future row
    /// still renders instead of dropping the record.
    pub fn from_db_str(value: &str) -> Self {
        match value {
            "daily" => Self::Daily,
            "weekly" => Self::Weekly,
            _ => Self::Manual,
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeeperExecutionRecord {
    pub agent: String,
    pub trigger_type: KeeperTriggerType,
    pub model: String,
    pub prompt: String,
    pub command_executed: String,
    pub timestamp: DateTime<Local>,
    pub duration_ms: u64,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub output_snippet: String,
}

/// Maximum number of characters kept from a ping's combined output.
const OUTPUT_SNIPPET_MAX_CHARS: usize = 300;

/// How long a single ping may run before it is abandoned and its child killed.
const PING_TIMEOUT_SECS: u64 = 45;

/// How long after the configured wakeup time a missed daily ping may still fire.
///
/// Without an upper bound, opening the TUI at any point later in the day fires a
/// "10:30" ping immediately, which both wastes quota and anchors the 5h session
/// window at the wrong time.
const DAILY_CATCH_UP_MINUTES: i64 = 120;

/// Escapes a value interpolated into a shell command template.
///
/// The template is trusted (users write it themselves and may use shell syntax
/// in it), but the substituted values are data. All three built-in templates
/// wrap `{prompt}` in double quotes, so a prompt containing `"` would otherwise
/// close that quote and let the rest of the prompt run as shell code.
fn escape_shell_value(value: &str) -> String {
    let stripped: String = value.chars().filter(|c| !c.is_control()).collect();

    #[cfg(not(target_os = "windows"))]
    {
        stripped
            .chars()
            .flat_map(|c| {
                let escape = matches!(c, '\\' | '"' | '$' | '`');
                escape.then_some('\\').into_iter().chain(std::iter::once(c))
            })
            .collect()
    }

    // cmd.exe has no backslash escaping; drop the characters that would let a
    // value break out of the template instead.
    #[cfg(target_os = "windows")]
    {
        stripped
            .chars()
            .filter(|c| !matches!(c, '"' | '&' | '|' | '<' | '>' | '^' | '%'))
            .collect()
    }
}

/// Formats the command string by replacing `{prompt}` and `{model}` placeholders.
pub fn format_keeper_command(template: &str, model: &str, prompt: &str) -> String {
    template
        .replace("{model}", &escape_shell_value(model))
        .replace("{prompt}", &escape_shell_value(prompt))
}

/// Collapses raw CLI output into a single line that is safe to hand to ratatui.
///
/// Two hazards are handled here. Agent CLIs emit carriage-return spinners, tabs
/// and ANSI escapes; ratatui drops `\n` but writes every other control character
/// into a buffer cell, which crossterm then prints verbatim and corrupts the
/// frame. And truncation must count characters, not bytes — slicing a `String`
/// at byte 300 panics whenever that offset lands inside a multi-byte codepoint,
/// which any CJK or emoji reply will eventually do.
fn sanitize_output_snippet(raw: &str) -> String {
    let without_ansi = strip_ansi_sequences(raw);
    let collapsed = without_ansi
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    if collapsed.chars().count() > OUTPUT_SNIPPET_MAX_CHARS {
        let truncated: String = collapsed.chars().take(OUTPUT_SNIPPET_MAX_CHARS).collect();
        format!("{truncated}...")
    } else {
        collapsed
    }
}

/// Removes ANSI CSI/OSC escape sequences so colored CLI output stays readable.
fn strip_ansi_sequences(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ ... <final byte in 0x40..=0x7e>
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... terminated by BEL or ESC \
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Two-character escape such as ESC c.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }

    out
}

/// Evaluates whether the daily 5h wakeup trigger should fire right now.
pub fn should_trigger_daily(
    daily_time_str: &str,
    last_triggered_date: Option<NaiveDate>,
    now: DateTime<Local>,
) -> bool {
    let Ok(target_time) = NaiveTime::parse_from_str(daily_time_str.trim(), "%H:%M") else {
        return false;
    };

    let today = now.date_naive();
    if let Some(last_date) = last_triggered_date {
        if last_date >= today {
            return false;
        }
    }

    let elapsed = now.time().signed_duration_since(target_time);
    elapsed >= chrono::Duration::zero()
        && elapsed <= chrono::Duration::minutes(DAILY_CATCH_UP_MINUTES)
}

/// Calculates the next daily trigger timestamp.
pub fn compute_next_daily_trigger(
    daily_time_str: &str,
    last_triggered_date: Option<NaiveDate>,
    now: DateTime<Local>,
) -> Option<DateTime<Local>> {
    let target_time = NaiveTime::parse_from_str(daily_time_str.trim(), "%H:%M").ok()?;
    let today = now.date_naive();

    let already_triggered_today = match last_triggered_date {
        Some(date) => date >= today,
        None => false,
    };

    let target_date = if already_triggered_today || now.time() >= target_time {
        today.succ_opt()?
    } else {
        today
    };

    target_date
        .and_time(target_time)
        .and_local_timezone(Local)
        .single()
}

/// Evaluates whether the Weekly auto-sync trigger should fire.
/// Fires when current time >= `resets_at + 1 minute` (buffer) and has not triggered for this cycle.
pub fn should_trigger_weekly(
    quota_resets_at: Option<DateTime<Utc>>,
    last_triggered_time: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> bool {
    let Some(resets_at) = quota_resets_at else {
        return false;
    };

    // Add 1 minute buffer after reset
    let trigger_threshold = resets_at + chrono::Duration::minutes(1);

    if now < trigger_threshold {
        return false;
    }

    match last_triggered_time {
        Some(last_time) => last_time < resets_at,
        None => true,
    }
}

/// Calculates next weekly trigger time.
pub fn compute_next_weekly_trigger(
    quota_resets_at: Option<DateTime<Utc>>,
    last_triggered_time: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let resets_at = quota_resets_at?;
    let trigger_threshold = resets_at + chrono::Duration::minutes(1);

    if let Some(last_time) = last_triggered_time {
        if last_time >= resets_at && now >= trigger_threshold {
            // Already triggered for this reset cycle
            return None;
        }
    }

    Some(trigger_threshold)
}

/// Matches a QuotaSnapshot provider name to a Keeper agent ID.
fn matches_keeper_agent(provider: &str, agent_id: &str) -> bool {
    let p = provider.to_lowercase();
    let a = agent_id.to_lowercase();
    if p == a {
        return true;
    }
    match a.as_str() {
        "antigravity" => p == "google" || p == "gemini" || p == "antigravity",
        "claude" => p == "anthropic" || p == "claude",
        "codex" => p == "openai" || p == "codex",
        _ => false,
    }
}

/// Extracts the weekly window reset time from a QuotaSnapshot.
/// Accurately recognizes "7d", "7-day", "week", "weekly", period_duration_ms >= 6 days,
/// and Antigravity's "Gemini (7d)" / "Claude (7d)" windows.
pub fn extract_weekly_reset_time(
    snapshot: &crate::provider::QuotaSnapshot,
) -> Option<DateTime<Utc>> {
    // 1. Check explicit 7d/weekly windows first
    for window in &snapshot.windows {
        let label_lower = window.label.to_lowercase();
        let is_weekly = label_lower.contains("7d")
            || label_lower.contains("7 d")
            || label_lower.contains("7-day")
            || label_lower.contains("week")
            || window
                .period_duration_ms
                .is_some_and(|ms| ms >= 6 * 24 * 60 * 60 * 1000);

        if is_weekly {
            if let Some(resets_at) = window.resets_at {
                return Some(resets_at);
            }
        }
    }

    // 2. Fallback to any window with reset_time that is not a short session window.
    //    A window that declares a sub-day period is never the weekly one, even if
    //    its label does not say "5h".
    snapshot.windows.iter().find_map(|w| {
        let label_lower = w.label.to_lowercase();
        let looks_short = label_lower.contains("5h")
            || label_lower.contains("5-hour")
            || label_lower.contains("5 hour")
            || w.period_duration_ms
                .is_some_and(|ms| ms < 24 * 60 * 60 * 1000);
        if looks_short {
            None
        } else {
            w.resets_at
        }
    })
}

/// Finds the quota snapshot backing a Keeper agent, preferring an exact
/// provider match over an alias so a future `gemini` provider cannot be picked
/// up by the `antigravity` agent just because it appears earlier in the list.
pub fn find_snapshot_for_agent<'a>(
    snapshots: &'a [crate::provider::QuotaSnapshot],
    agent_id: &str,
) -> Option<&'a crate::provider::QuotaSnapshot> {
    snapshots
        .iter()
        .find(|s| s.provider.eq_ignore_ascii_case(agent_id))
        .or_else(|| {
            snapshots
                .iter()
                .find(|s| matches_keeper_agent(&s.provider, agent_id))
        })
}

/// Asynchronously executes a CLI heartbeat command.
pub async fn execute_agent_ping(
    agent: &str,
    config: &AgentKeeperConfig,
    trigger_type: KeeperTriggerType,
) -> KeeperExecutionRecord {
    let start_instant = Instant::now();
    let timestamp = Local::now();
    let command_str = format_keeper_command(&config.command, &config.model, &config.prompt);

    info!(
        "Keeper: Executing {} ping for agent '{}' with model '{}': {}",
        trigger_type.label(),
        agent,
        config.model,
        command_str
    );

    #[cfg(target_os = "windows")]
    let (program, arg) = ("cmd", "/C");
    #[cfg(not(target_os = "windows"))]
    let (program, arg) = ("sh", "-c");

    // `kill_on_drop` matters here: on timeout the `output()` future is dropped,
    // and without it the spawned CLI keeps running detached forever.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(PING_TIMEOUT_SECS),
        tokio::process::Command::new(program)
            .arg(arg)
            .arg(&command_str)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await;

    let duration_ms = start_instant.elapsed().as_millis() as u64;

    let (success, exit_code, output_snippet) = match result {
        Ok(Ok(output)) => {
            let code = output.status.code();
            let is_success = output.status.success();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = if !stderr.trim().is_empty() {
                format!("{} {}", stdout.trim(), stderr.trim())
            } else {
                stdout.trim().to_string()
            };
            let snippet = match sanitize_output_snippet(&combined) {
                s if s.is_empty() => "Command executed (empty output)".to_string(),
                s => s,
            };
            (is_success, code, snippet)
        }
        Ok(Err(e)) => (
            false,
            None,
            sanitize_output_snippet(&format!("Execution failed: {e}")),
        ),
        Err(_) => (
            false,
            None,
            format!("Execution timed out after {PING_TIMEOUT_SECS}s"),
        ),
    };

    KeeperExecutionRecord {
        agent: agent.to_string(),
        trigger_type,
        model: config.model.clone(),
        prompt: config.prompt.clone(),
        command_executed: command_str,
        timestamp,
        duration_ms,
        success,
        exit_code,
        output_snippet,
    }
}

/// Builds a failure record for a ping that never produced one of its own.
pub fn failed_ping_record(
    agent: &str,
    config: &AgentKeeperConfig,
    trigger_type: KeeperTriggerType,
    reason: &str,
) -> KeeperExecutionRecord {
    let timestamp = Local::now();
    KeeperExecutionRecord {
        agent: agent.to_string(),
        trigger_type,
        model: config.model.clone(),
        prompt: config.prompt.clone(),
        command_executed: format_keeper_command(&config.command, &config.model, &config.prompt),
        timestamp,
        duration_ms: 0,
        success: false,
        exit_code: None,
        output_snippet: sanitize_output_snippet(reason),
    }
}

/// Last-fired bookkeeping for the scheduled triggers.
///
/// This has to outlive the process: the maps used to be in-memory only, so every
/// TUI restart after the configured wakeup time fired another daily ping.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KeeperTriggerState {
    #[serde(default)]
    pub daily_triggered: std::collections::HashMap<String, NaiveDate>,
    #[serde(default)]
    pub weekly_triggered: std::collections::HashMap<String, DateTime<Utc>>,
}

impl KeeperTriggerState {
    /// Reads the state file, falling back to empty state if it is missing or
    /// unreadable — a corrupt file must not stop the TUI from starting.
    pub fn load(path: &std::path::Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&content).unwrap_or_else(|e| {
            tracing::warn!("Failed to parse keeper state at {}: {e}", path.display());
            Self::default()
        })
    }

    /// Best-effort persist; failures are logged, never surfaced to the UI.
    pub fn save(&self, path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("Failed to create keeper state dir: {e}");
                return;
            }
        }
        match serde_json::to_string_pretty(self) {
            Ok(content) => {
                if let Err(e) = std::fs::write(path, content) {
                    tracing::warn!("Failed to write keeper state: {e}");
                }
            }
            Err(e) => tracing::warn!("Failed to serialize keeper state: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Timelike};

    #[test]
    fn test_format_keeper_command() {
        let template = "claude -p \"{prompt}\" --model {model}";
        let formatted = format_keeper_command(template, "claude-3-5-haiku-20241022", "Hello World");
        assert_eq!(
            formatted,
            "claude -p \"Hello World\" --model claude-3-5-haiku-20241022"
        );
    }

    #[test]
    fn test_sanitize_output_snippet_handles_multibyte_and_control_chars() {
        // Byte 300 lands inside a multi-byte codepoint; slicing here used to panic.
        let cjk = format!("Hello! {}", "你好，很高兴见到你。".repeat(40));
        let snippet = sanitize_output_snippet(&cjk);
        assert_eq!(snippet.chars().count(), OUTPUT_SNIPPET_MAX_CHARS + 3);
        assert!(snippet.ends_with("..."));

        // ratatui only filters `\n`; every other control char reaches the terminal.
        let noisy = "A\rB\tC\u{7}D\nE";
        let cleaned = sanitize_output_snippet(noisy);
        assert!(!cleaned.chars().any(char::is_control));
        assert_eq!(cleaned, "A B C D E");

        // ANSI colour sequences are dropped rather than shown as `[31m`.
        assert_eq!(sanitize_output_snippet("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(sanitize_output_snippet("   "), "");
    }

    #[test]
    fn test_format_keeper_command_escapes_substituted_values() {
        // A prompt containing a double quote used to close the template's quote
        // and run the remainder as shell code.
        let cmd =
            format_keeper_command("echo \"{prompt}\"", "m", "hi\"; touch /tmp/pwned; echo \"");
        assert!(!cmd.contains("; touch /tmp/pwned; echo \"\""));
        assert!(cmd.contains("\\\""));

        // Ordinary prompts are untouched.
        assert_eq!(
            format_keeper_command("claude -p \"{prompt}\" --model {model}", "haiku", "Hi"),
            "claude -p \"Hi\" --model haiku"
        );
    }

    #[test]
    fn test_daily_trigger_does_not_fire_long_after_the_window() {
        let target = "10:30";
        // Opening the TUI late at night must not fire a "10:30" ping.
        let late = Local.with_ymd_and_hms(2026, 8, 15, 23, 0, 0).unwrap();
        assert!(!should_trigger_daily(target, None, late));

        // Still fires inside the catch-up window.
        let within = Local.with_ymd_and_hms(2026, 8, 15, 12, 0, 0).unwrap();
        assert!(should_trigger_daily(target, None, within));
    }

    #[test]
    fn test_find_snapshot_prefers_exact_provider_match() {
        use crate::provider::QuotaSnapshot;

        let make = |provider: &str| QuotaSnapshot {
            provider: provider.to_string(),
            plan: None,
            account: None,
            windows: vec![],
            credits: None,
            rate_limit_reset_credits: vec![],
            fetched_at: Utc::now(),
        };

        // `gemini` aliases to the antigravity agent, but an exact match wins even
        // when the alias appears first.
        let snapshots = vec![make("gemini"), make("antigravity")];
        assert_eq!(
            find_snapshot_for_agent(&snapshots, "antigravity")
                .unwrap()
                .provider,
            "antigravity"
        );
        assert!(find_snapshot_for_agent(&snapshots, "codex").is_none());
    }

    #[test]
    fn test_keeper_trigger_state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("keeper_state.json");

        let mut state = KeeperTriggerState::default();
        state.daily_triggered.insert(
            "claude".to_string(),
            NaiveDate::from_ymd_opt(2026, 8, 15).unwrap(),
        );
        state.save(&path);

        let loaded = KeeperTriggerState::load(&path);
        assert_eq!(
            loaded.daily_triggered.get("claude"),
            Some(&NaiveDate::from_ymd_opt(2026, 8, 15).unwrap())
        );

        // Missing and corrupt files degrade to empty state instead of failing.
        assert!(
            KeeperTriggerState::load(dir.path().join("nope.json").as_path())
                .daily_triggered
                .is_empty()
        );
        std::fs::write(&path, "not json").unwrap();
        assert!(KeeperTriggerState::load(&path).daily_triggered.is_empty());
    }

    #[test]
    fn test_should_trigger_daily_logic() {
        let now = Local.with_ymd_and_hms(2026, 8, 15, 10, 35, 0).unwrap();
        let target = "10:30";

        // Haven't triggered today -> should trigger
        assert!(should_trigger_daily(target, None, now));

        // Triggered yesterday -> should trigger
        let yesterday = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
        assert!(should_trigger_daily(target, Some(yesterday), now));

        // Already triggered today -> should NOT trigger
        let today = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        assert!(!should_trigger_daily(target, Some(today), now));

        // Target is in future -> should NOT trigger
        let future_target = "11:00";
        assert!(!should_trigger_daily(future_target, None, now));
    }

    #[test]
    fn test_should_trigger_weekly_logic() {
        let reset_time = Utc.with_ymd_and_hms(2026, 8, 15, 12, 0, 0).unwrap();

        // 30 seconds after reset -> not reached threshold (1 min) yet
        let before_threshold = Utc.with_ymd_and_hms(2026, 8, 15, 12, 0, 30).unwrap();
        assert!(!should_trigger_weekly(
            Some(reset_time),
            None,
            before_threshold
        ));

        // 2 minutes after reset -> should trigger
        let after_threshold = Utc.with_ymd_and_hms(2026, 8, 15, 12, 2, 0).unwrap();
        assert!(should_trigger_weekly(
            Some(reset_time),
            None,
            after_threshold
        ));

        // Already triggered after reset_time -> should NOT trigger again
        let triggered_after = Utc.with_ymd_and_hms(2026, 8, 15, 12, 2, 30).unwrap();
        assert!(!should_trigger_weekly(
            Some(reset_time),
            Some(triggered_after),
            after_threshold
        ));
    }

    #[test]
    fn test_compute_next_daily_trigger() {
        let now = Local.with_ymd_and_hms(2026, 8, 15, 9, 0, 0).unwrap();
        let next = compute_next_daily_trigger("10:30", None, now).unwrap();
        assert_eq!(next.hour(), 10);
        assert_eq!(next.minute(), 30);
        assert_eq!(next.day(), 15);

        // If current time is after 10:30, next is tomorrow
        let late = Local.with_ymd_and_hms(2026, 8, 15, 11, 0, 0).unwrap();
        let next_tomorrow = compute_next_daily_trigger("10:30", None, late).unwrap();
        assert_eq!(next_tomorrow.day(), 16);
    }

    #[test]
    fn test_extract_weekly_reset_time_antigravity_and_standard() {
        use crate::provider::{QuotaSnapshot, RateWindow};

        let reset_time = Utc.with_ymd_and_hms(2026, 8, 20, 15, 0, 0).unwrap();

        // Antigravity snapshot with "Gemini (7d)"
        let agy_snapshot = QuotaSnapshot {
            provider: "antigravity".to_string(),
            plan: Some("Pro".to_string()),
            account: None,
            windows: vec![
                RateWindow {
                    label: "Gemini (5h)".to_string(),
                    model_family: Some("Gemini".to_string()),
                    used_percent: 10.0,
                    resets_at: Some(Utc.with_ymd_and_hms(2026, 8, 15, 14, 0, 0).unwrap()),
                    period_duration_ms: Some(5 * 60 * 60 * 1000),
                },
                RateWindow {
                    label: "Gemini (7d)".to_string(),
                    model_family: Some("Gemini".to_string()),
                    used_percent: 50.0,
                    resets_at: Some(reset_time),
                    period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
                },
            ],
            credits: None,
            rate_limit_reset_credits: vec![],
            fetched_at: Utc::now(),
        };

        assert_eq!(extract_weekly_reset_time(&agy_snapshot), Some(reset_time));
        assert!(matches_keeper_agent("antigravity", "antigravity"));
        assert!(matches_keeper_agent("google", "antigravity"));
        assert!(matches_keeper_agent("claude", "claude"));
        assert!(matches_keeper_agent("anthropic", "claude"));
        assert!(matches_keeper_agent("codex", "codex"));
        assert!(matches_keeper_agent("openai", "codex"));
    }

    fn ping_config(command: &str) -> AgentKeeperConfig {
        AgentKeeperConfig {
            session_keeper_enabled: true,
            daily_wakeup_time: "10:30".to_string(),
            weekly_keeper_enabled: true,
            command: command.to_string(),
            model: "m".to_string(),
            prompt: "p".to_string(),
        }
    }

    /// A CJK reply is over 300 bytes long well before it is 300 characters long;
    /// the old byte slice panicked on exactly this input.
    #[tokio::test]
    async fn test_ping_survives_multibyte_reply() {
        let reply = "你好，很高兴见到你。".repeat(10);
        let record = execute_agent_ping(
            "claude",
            &ping_config(&format!("printf '%s' 'Hello! {reply}'")),
            KeeperTriggerType::Manual,
        )
        .await;

        assert!(record.success);
        assert!(record.output_snippet.starts_with("Hello! 你好"));
        assert_eq!(record.output_snippet.chars().count(), 107);
    }

    #[tokio::test]
    async fn test_ping_output_is_single_line_without_control_chars() {
        let record = execute_agent_ping(
            "claude",
            &ping_config("printf 'line1\\nline2'; printf '\\x1b[31mwarn\\x1b[0m' 1>&2"),
            KeeperTriggerType::Manual,
        )
        .await;

        assert_eq!(record.output_snippet, "line1 line2 warn");
    }

    /// A prompt containing a double quote used to close the template's quoting
    /// and run whatever followed as shell code.
    #[tokio::test]
    async fn test_prompt_cannot_break_out_of_command_template() {
        let marker = std::env::temp_dir().join("tokenpulse_keeper_injection_test");
        let _ = std::fs::remove_file(&marker);

        let mut config = ping_config("echo \"{prompt}\"");
        config.prompt = format!("hi\"; touch {}; echo \"", marker.display());
        execute_agent_ping("claude", &config, KeeperTriggerType::Manual).await;

        assert!(!marker.exists(), "prompt escaped the command template");
        let _ = std::fs::remove_file(&marker);
    }

    /// Dropping the `output()` future on timeout must not leave the CLI running.
    #[tokio::test]
    async fn test_timed_out_ping_kills_its_child() {
        let marker = std::env::temp_dir().join("tokenpulse_keeper_kill_test");
        let _ = std::fs::remove_file(&marker);

        let started = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(format!("sleep 2 && touch {}", marker.display()))
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await;
        assert!(started.is_err(), "command should have timed out");

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert!(!marker.exists(), "child outlived the timeout");
        let _ = std::fs::remove_file(&marker);
    }
}
