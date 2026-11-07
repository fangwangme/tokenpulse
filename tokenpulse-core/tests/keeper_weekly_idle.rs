//! #99: the weekly Keeper pings every target whose weekly window has not
//! started, so a reset is never missed (during downtime, after a failed ping).
//! Spec: `docs/specs/keeper.md` "Weekly targets".

use chrono::{DateTime, Duration, TimeZone, Utc};
use std::collections::{HashMap, HashSet};
use tokenpulse_core::keeper::{idle_weekly_targets, WeeklyTarget};
use tokenpulse_core::provider::{QuotaSnapshot, RateWindow};

const SEVEN_DAYS_MS: i64 = 7 * 24 * 60 * 60 * 1000;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 7, 4, 0, 0).unwrap()
}

fn weekly(label: &str, family: Option<&str>, used: f64, resets_in: Option<Duration>) -> RateWindow {
    RateWindow {
        label: label.to_string(),
        model_family: family.map(str::to_string),
        used_percent: used,
        resets_at: resets_in.map(|d| now() + d),
        period_duration_ms: Some(SEVEN_DAYS_MS),
    }
}

fn snapshot(provider: &str, windows: Vec<RateWindow>) -> QuotaSnapshot {
    QuotaSnapshot {
        provider: provider.to_string(),
        plan: None,
        account: None,
        windows,
        credits: None,
        rate_limit_reset_credits: vec![],
        fetched_at: now(),
    }
}

fn target(key: &str, group: Option<&str>) -> WeeklyTarget {
    WeeklyTarget {
        key: key.to_string(),
        group: group.map(str::to_string),
    }
}

/// Gemini has not started (0%, reset a full week away). Claude reads 0% but
/// started 5 hours ago, so its reset is fixed less than 6d20h away. Argon is in use.
fn antigravity() -> QuotaSnapshot {
    let week = Duration::days(7);
    snapshot(
        "antigravity",
        vec![
            weekly("Gemini (7d)", Some("Gemini"), 0.0, Some(week)),
            weekly(
                "Claude (7d)",
                Some("Claude"),
                0.0,
                Some(week - Duration::hours(5)),
            ),
            weekly(
                "Gemini 4 Argon (7d)",
                Some("Gemini 4 Argon"),
                3.0,
                Some(week),
            ),
        ],
    )
}

#[test]
fn fires_only_for_a_weekly_window_that_has_not_started() {
    let fired = idle_weekly_targets(
        "antigravity",
        &antigravity(),
        true,
        &HashMap::new(),
        &HashSet::new(),
        now(),
    );

    assert_eq!(fired, vec![target("antigravity", Some("Gemini"))]);
}

#[test]
fn cooldown_is_five_hours_after_success_and_one_hour_after_failure() {
    let fires = |hours_ago: i64, failed: bool| {
        let triggered = HashMap::from([(
            "antigravity".to_string(),
            now() - Duration::hours(hours_ago),
        )]);
        let failed: HashSet<String> = if failed {
            HashSet::from(["antigravity".to_string()])
        } else {
            HashSet::new()
        };
        !idle_weekly_targets(
            "antigravity",
            &antigravity(),
            true,
            &triggered,
            &failed,
            now(),
        )
        .is_empty()
    };

    assert!(!fires(4, false));
    assert!(fires(5, false));
    assert!(!fires(0, true));
    assert!(fires(1, true));
}

#[test]
fn target_without_group_follows_the_overall_weekly_window() {
    // An unused Opus sub-limit sits at 0% while the overall window is in use.
    let in_use = snapshot(
        "claude",
        vec![
            weekly("Weekly (7d)", None, 30.0, Some(Duration::days(3))),
            weekly("Opus (7d)", Some("Opus"), 0.0, Some(Duration::days(7))),
        ],
    );
    assert!(idle_weekly_targets(
        "claude",
        &in_use,
        false,
        &HashMap::new(),
        &HashSet::new(),
        now()
    )
    .is_empty());

    // A 0% window without a reset time counts as not started.
    let reset = snapshot("claude", vec![weekly("Weekly (7d)", None, 0.0, None)]);
    assert_eq!(
        idle_weekly_targets(
            "claude",
            &reset,
            false,
            &HashMap::new(),
            &HashSet::new(),
            now()
        ),
        vec![target("claude", None)]
    );
}
