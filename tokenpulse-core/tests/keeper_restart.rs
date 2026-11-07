//! Acceptance tests for #99: weekly resets that happen while TokenPulse is not
//! running are observed after a restart.
//! Spec: `docs/specs/keeper.md` "Weekly targets" → "Baseline".

use chrono::{DateTime, Duration, TimeZone, Utc};
use tokenpulse_core::keeper::{observed_weekly_resets, KeeperTriggerState, WeeklyTarget};
use tokenpulse_core::provider::{QuotaSnapshot, RateWindow};

const SEVEN_DAYS_MS: i64 = 7 * 24 * 60 * 60 * 1000;

fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, day, hour, 0, 0).unwrap()
}

/// The first refresh after the restart.
fn restart_time() -> DateTime<Utc> {
    at(7, 4)
}

fn weekly(family: &str, used: f64, resets_at: DateTime<Utc>) -> RateWindow {
    RateWindow {
        label: format!("{family} (7d)"),
        model_family: Some(family.to_string()),
        used_percent: used,
        resets_at: Some(resets_at),
        period_duration_ms: Some(SEVEN_DAYS_MS),
    }
}

fn snapshot(windows: Vec<RateWindow>) -> QuotaSnapshot {
    QuotaSnapshot {
        provider: "antigravity".to_string(),
        plan: None,
        account: None,
        windows,
        credits: None,
        rate_limit_reset_credits: vec![],
        fetched_at: Utc::now(),
    }
}

/// Last refresh before TokenPulse was closed: every group's weekly window is used.
fn before_close() -> QuotaSnapshot {
    snapshot(vec![
        weekly("Gemini", 23.3, at(8, 5)),
        weekly("Claude", 10.6, at(10, 11)),
        weekly("Gemini 4 Argon", 65.0, at(10, 11)),
    ])
}

/// First refresh after the restart: Gemini and Gemini 4 Argon reset early while
/// TokenPulse was closed, so their `resets_at` already points at the next cycle.
/// Claude did not reset.
fn after_restart() -> QuotaSnapshot {
    let next_cycle = restart_time() + Duration::days(7);
    snapshot(vec![
        weekly("Gemini", 0.0, next_cycle),
        weekly("Claude", 10.6, at(10, 11)),
        weekly("Gemini 4 Argon", 0.0, next_cycle),
    ])
}

fn target(key: &str, group: &str) -> WeeklyTarget {
    WeeklyTarget {
        key: key.to_string(),
        group: Some(group.to_string()),
    }
}

/// Persists `state` and reads it back, as a TUI restart does.
fn restart_with(state: &KeeperTriggerState) -> (tempfile::TempDir, KeeperTriggerState) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keeper_state.json");
    state.save(&path);
    let loaded = KeeperTriggerState::load(&path);
    (dir, loaded)
}

#[test]
fn reset_while_not_running_fires_after_restart() {
    let mut state = KeeperTriggerState::default();
    state
        .weekly_triggered
        .insert("antigravity".to_string(), at(1, 5));
    state
        .weekly_baseline
        .insert("antigravity".to_string(), before_close());

    let (_dir, restarted) = restart_with(&state);

    let fired = observed_weekly_resets(
        "antigravity",
        restarted.weekly_baseline.get("antigravity"),
        &after_restart(),
        true,
        &restarted.weekly_triggered,
        restart_time(),
    );

    assert_eq!(
        fired,
        vec![
            target("antigravity", "Gemini"),
            target("antigravity:Gemini 4 Argon", "Gemini 4 Argon"),
        ],
        "both groups that reset while closed fire; Claude, which did not reset, does not"
    );
}

#[test]
fn restart_comparison_keeps_twelve_hour_guard() {
    let mut state = KeeperTriggerState::default();
    state.weekly_triggered.insert(
        "antigravity:Gemini 4 Argon".to_string(),
        restart_time() - Duration::hours(1),
    );
    state
        .weekly_baseline
        .insert("antigravity".to_string(), before_close());

    let (_dir, restarted) = restart_with(&state);

    let fired = observed_weekly_resets(
        "antigravity",
        restarted.weekly_baseline.get("antigravity"),
        &after_restart(),
        true,
        &restarted.weekly_triggered,
        restart_time(),
    );

    assert_eq!(fired, vec![target("antigravity", "Gemini")]);
}

#[test]
fn state_file_without_baseline_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keeper_state.json");
    // The format written before the baseline existed.
    std::fs::write(
        &path,
        r#"{
  "daily_triggered": { "claude": "2026-11-07" },
  "weekly_triggered": { "antigravity": "2026-11-01T05:33:08Z" }
}"#,
    )
    .unwrap();

    let loaded = KeeperTriggerState::load(&path);

    assert_eq!(
        loaded.daily_triggered.get("claude").map(|d| d.to_string()),
        Some("2026-11-07".to_string())
    );
    assert_eq!(
        loaded.weekly_triggered.get("antigravity").copied(),
        Some(Utc.with_ymd_and_hms(2026, 11, 1, 5, 33, 8).unwrap())
    );
    assert!(loaded.weekly_baseline.is_empty());

    let fired = observed_weekly_resets(
        "antigravity",
        loaded.weekly_baseline.get("antigravity"),
        &after_restart(),
        true,
        &loaded.weekly_triggered,
        restart_time(),
    );
    assert!(
        fired.is_empty(),
        "no baseline: the first refresh fires nothing"
    );
}
