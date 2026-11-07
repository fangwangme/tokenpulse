//! Acceptance tests for #97: Antigravity weekly Keeper targets per quota group.
//! Spec: `docs/specs/keeper.md` "Weekly targets" and "Antigravity quota groups".

use chrono::{DateTime, TimeZone, Utc};
use std::collections::{HashMap, HashSet};
use tokenpulse_core::keeper::{
    extract_weekly_reset_time, idle_weekly_targets, select_antigravity_group_model, weekly_targets,
    WeeklyTarget,
};
use tokenpulse_core::provider::{QuotaSnapshot, RateWindow};

const FIVE_HOURS_MS: i64 = 5 * 60 * 60 * 1000;
const SEVEN_DAYS_MS: i64 = 7 * 24 * 60 * 60 * 1000;

fn reset(day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, day, 5, 30, 0).unwrap()
}

fn window(family: &str, period_ms: i64, used: f64, resets_at: DateTime<Utc>) -> RateWindow {
    let suffix = if period_ms == SEVEN_DAYS_MS {
        "7d"
    } else {
        "5h"
    };
    RateWindow {
        label: format!("{family} ({suffix})"),
        model_family: Some(family.to_string()),
        used_percent: used,
        resets_at: Some(resets_at),
        period_duration_ms: Some(period_ms),
    }
}

/// The three-group snapshot the quota fetcher produces, Gemini sorted first.
/// Each group's weekly window resets at a different time.
fn antigravity_snapshot(gemini_7d: f64, claude_7d: f64, argon_7d: f64) -> QuotaSnapshot {
    QuotaSnapshot {
        provider: "antigravity".to_string(),
        plan: None,
        account: None,
        windows: vec![
            window("Gemini", FIVE_HOURS_MS, 10.0, reset(5)),
            window("Gemini", SEVEN_DAYS_MS, gemini_7d, reset(8)),
            window("Claude", FIVE_HOURS_MS, 0.0, reset(5)),
            window("Claude", SEVEN_DAYS_MS, claude_7d, reset(10)),
            window("Gemini 4 Argon", SEVEN_DAYS_MS, argon_7d, reset(11)),
        ],
        credits: None,
        rate_limit_reset_credits: vec![],
        fetched_at: Utc::now(),
    }
}

fn target(key: &str, group: Option<&str>) -> WeeklyTarget {
    WeeklyTarget {
        key: key.to_string(),
        group: group.map(str::to_string),
    }
}

fn groups() -> Vec<String> {
    ["Gemini", "Claude", "Gemini 4 Argon"]
        .into_iter()
        .map(String::from)
        .collect()
}

/// What `agy models` prints: `<id>\t<label>` per line.
const AGY_MODELS: &str = "\
gemini-4-argon-medium\tGemini 4 Argon (Medium)
gemini-4-argon-high\tGemini 4 Argon (High)
gemini-4-argon-low\tGemini 4 Argon (Low)
gemini-3.8-flash-high\tGemini 3.8 Flash (High)
gemini-3.8-flash-medium\tGemini 3.8 Flash (Medium)
gemini-3.8-flash-low\tGemini 3.8 Flash (Low)
claude-opus-5-5-low\tClaude Opus 5.5 (Low)
claude-opus-5-5-medium\tClaude Opus 5.5 (Medium)
claude-opus-5-5-high\tClaude Opus 5.5 (High)
claude-sonnet-5-5-low\tClaude Sonnet 5.5 (Low)
claude-sonnet-5-5-medium\tClaude Sonnet 5.5 (Medium)
claude-sonnet-5-5-high\tClaude Sonnet 5.5 (High)
gpt-oss-120b-medium\tGPT-OSS 120B (Medium)
";

#[test]
fn default_mode_targets_only_primary_group() {
    let snapshot = antigravity_snapshot(5.0, 10.0, 2.0);
    assert_eq!(
        weekly_targets("antigravity", Some(&snapshot), false),
        vec![target("antigravity", Some("Gemini"))]
    );
}

/// A moment one hour into the 7d cycle that ends at `resets_at`, when a window
/// still at 0% counts as not started.
fn just_reset(resets_at: DateTime<Utc>) -> DateTime<Utc> {
    resets_at - chrono::Duration::days(7) + chrono::Duration::hours(1)
}

fn idle(snapshot: &QuotaSnapshot, all_groups: bool, now: DateTime<Utc>) -> Vec<WeeklyTarget> {
    idle_weekly_targets(
        "antigravity",
        snapshot,
        all_groups,
        &HashMap::new(),
        &HashSet::new(),
        now,
    )
}

#[test]
fn default_mode_ignores_other_group_reset() {
    // Only the Claude group's weekly window rolled over.
    let claude_reset = antigravity_snapshot(5.0, 0.0, 2.0);
    assert!(
        idle(&claude_reset, false, just_reset(reset(10))).is_empty(),
        "a Claude-group reset must not fire the Gemini Flash ping"
    );

    // The primary group's own reset still fires it.
    let gemini_reset = antigravity_snapshot(0.0, 10.0, 2.0);
    assert_eq!(
        idle(&gemini_reset, false, just_reset(reset(8))),
        vec![target("antigravity", Some("Gemini"))]
    );
}

#[test]
fn all_groups_mode_targets_every_weekly_family() {
    let snapshot = antigravity_snapshot(5.0, 10.0, 2.0);
    assert_eq!(
        weekly_targets("antigravity", Some(&snapshot), true),
        vec![
            target("antigravity", Some("Gemini")),
            target("antigravity:Claude", Some("Claude")),
            target("antigravity:Gemini 4 Argon", Some("Gemini 4 Argon")),
        ]
    );

    // No snapshot yet: a single primary target without a group.
    assert_eq!(
        weekly_targets("antigravity", None, true),
        vec![target("antigravity", None)]
    );
}

#[test]
fn group_reset_time_and_idle_check_are_per_group() {
    let snapshot = antigravity_snapshot(5.0, 10.0, 2.0);
    assert_eq!(
        extract_weekly_reset_time(&snapshot, Some("Gemini")),
        Some(reset(8))
    );
    assert_eq!(
        extract_weekly_reset_time(&snapshot, Some("Claude")),
        Some(reset(10))
    );
    assert_eq!(
        extract_weekly_reset_time(&snapshot, Some("Gemini 4 Argon")),
        Some(reset(11))
    );
    assert_eq!(extract_weekly_reset_time(&snapshot, Some("Unknown")), None);

    let argon_reset = antigravity_snapshot(5.0, 10.0, 0.0);
    assert_eq!(
        idle(&argon_reset, true, just_reset(reset(11))),
        vec![target("antigravity:Gemini 4 Argon", Some("Gemini 4 Argon"))]
    );
}

#[test]
fn other_agents_keep_single_target() {
    let claude = QuotaSnapshot {
        provider: "claude".to_string(),
        plan: None,
        account: None,
        windows: vec![
            window("Opus", SEVEN_DAYS_MS, 10.0, reset(9)),
            window("Sonnet", SEVEN_DAYS_MS, 10.0, reset(9)),
        ],
        credits: None,
        rate_limit_reset_credits: vec![],
        fetched_at: Utc::now(),
    };
    for all_groups in [false, true] {
        assert_eq!(
            weekly_targets("claude", Some(&claude), all_groups),
            vec![target("claude", None)]
        );
        assert_eq!(
            weekly_targets("codex", None, all_groups),
            vec![target("codex", None)]
        );
    }
}

#[test]
fn selects_cheapest_lowest_thinking_model_per_group() {
    let groups = groups();
    assert_eq!(
        select_antigravity_group_model(AGY_MODELS, "Claude", &groups).unwrap(),
        "claude-sonnet-5-5-low"
    );
    assert_eq!(
        select_antigravity_group_model(AGY_MODELS, "Gemini 4 Argon", &groups).unwrap(),
        "gemini-4-argon-low"
    );
    // Argon ids start with `gemini` too, but belong to the longer slug.
    assert_eq!(
        select_antigravity_group_model(AGY_MODELS, "Gemini", &groups).unwrap(),
        "gemini-3.8-flash-low"
    );

    // No `-low` variant: the model without a level suffix comes next.
    let no_low = "\
gemini-4-carbon-high\tGemini 4 Carbon (High)
gemini-4-carbon\tGemini 4 Carbon
gemini-4-carbon-medium\tGemini 4 Carbon (Medium)
";
    let carbon_groups = vec!["Gemini".to_string(), "Gemini 4 Carbon".to_string()];
    assert_eq!(
        select_antigravity_group_model(no_low, "Gemini 4 Carbon", &carbon_groups).unwrap(),
        "gemini-4-carbon"
    );

    // Newest version wins within the cheapest tier.
    let versions = "\
claude-sonnet-4-6-low\tClaude Sonnet 4.6 (Low)
claude-sonnet-5-5-medium\tClaude Sonnet 5.5 (Medium)
claude-opus-6-low\tClaude Opus 6 (Low)
";
    assert_eq!(
        select_antigravity_group_model(versions, "Claude", &groups).unwrap(),
        "claude-sonnet-5-5-medium"
    );
}

#[test]
fn group_without_matching_model_is_an_error() {
    let groups = vec![
        "Gemini".to_string(),
        "Claude".to_string(),
        "Gemini 4 Carbon".to_string(),
    ];
    assert!(select_antigravity_group_model(AGY_MODELS, "Gemini 4 Carbon", &groups).is_err());
    assert!(select_antigravity_group_model("not a model listing", "Claude", &groups).is_err());
}
