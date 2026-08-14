use crate::config::AgentKeeperConfig;
use chrono::{DateTime, Local, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeeperExecutionRecord {
    pub id: String,
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

/// Formats the command string by replacing `{prompt}` and `{model}` placeholders.
pub fn format_keeper_command(template: &str, model: &str, prompt: &str) -> String {
    template
        .replace("{model}", model)
        .replace("{prompt}", prompt)
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

    let current_time = now.time();
    current_time >= target_time
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
pub fn matches_keeper_agent(provider: &str, agent_id: &str) -> bool {
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
            || label_lower.contains("weekly")
            || window
                .period_duration_ms
                .map_or(false, |ms| ms >= 6 * 24 * 60 * 60 * 1000);

        if is_weekly {
            if let Some(resets_at) = window.resets_at {
                return Some(resets_at);
            }
        }
    }

    // 2. Fallback to any window with reset_time that is not a short 5h session window
    snapshot.windows.iter().find_map(|w| {
        let label_lower = w.label.to_lowercase();
        if !label_lower.contains("5h")
            && !label_lower.contains("5-hour")
            && !label_lower.contains("5 hour")
        {
            w.resets_at
        } else {
            None
        }
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

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        tokio::process::Command::new(program)
            .arg(arg)
            .arg(&command_str)
            .stdin(std::process::Stdio::null())
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
                format!("{}\n{}", stdout.trim(), stderr.trim())
            } else {
                stdout.trim().to_string()
            };
            let snippet = if combined.is_empty() {
                "Command executed (empty output)".to_string()
            } else {
                let trimmed = combined.trim();
                if trimmed.len() > 300 {
                    format!("{}...", &trimmed[..300])
                } else {
                    trimmed.to_string()
                }
            };
            (is_success, code, snippet)
        }
        Ok(Err(e)) => (false, None, format!("Execution failed: {}", e)),
        Err(_) => (false, None, "Execution timed out after 45s".to_string()),
    };

    let record_id = format!("{}-{}-{}", agent, timestamp.timestamp_millis(), duration_ms);

    KeeperExecutionRecord {
        id: record_id,
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
                    used_percent: 10.0,
                    resets_at: Some(Utc.with_ymd_and_hms(2026, 8, 15, 14, 0, 0).unwrap()),
                    period_duration_ms: Some(5 * 60 * 60 * 1000),
                },
                RateWindow {
                    label: "Gemini (7d)".to_string(),
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
}
