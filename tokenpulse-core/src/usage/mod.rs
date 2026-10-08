pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod gemini;
pub mod opencode;
pub mod pi;
pub mod scanner;
pub mod store;
pub(crate) mod utils;

pub use antigravity::{detect_antigravity_connections, AntigravitySessionParser};
pub use claude::ClaudeSessionParser;
pub use codex::CodexSessionParser;
pub use copilot::CopilotSessionParser;
pub use gemini::GeminiSessionParser;
pub use opencode::OpenCodeSessionParser;
pub use pi::PiSessionParser;
pub use store::{DailyUsageRow, DateRange, UsageStore};
pub use utils::{detect_provider_from_model, normalize_model_name};

use chrono::{Datelike, Days, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardDay {
    pub date: String,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub message_count: i64,
    pub session_count: i64,
    pub intensity_tokens: u8,
    pub intensity_cost: u8,
}

impl DashboardDay {
    pub fn cache_tokens(&self) -> i64 {
        self.cache_read_tokens + self.cache_write_tokens
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRollup {
    pub label: String,
    pub start_date: String,
    pub end_date: String,
    pub total_tokens: i64,
    pub total_cost_usd: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub message_count: i64,
    pub session_count: i64,
    pub active_days: i64,
}

impl UsageRollup {
    pub fn cache_tokens(&self) -> i64 {
        self.cache_read_tokens + self.cache_write_tokens
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSummary {
    pub provider: String,
    pub cost: f64,
    pub tokens: i64,
    pub message_count: usize,
    pub session_count: usize,
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSummary {
    pub model: String,
    pub provider: String,
    pub source: String,
    pub cost: f64,
    pub tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub message_count: usize,
    pub session_count: usize,
    pub percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total_cost: f64,
    pub total_tokens: i64,
    pub message_count: usize,
    pub session_count: usize,
    pub active_days: usize,
    pub avg_daily_cost: f64,
    pub max_daily_cost: f64,
    pub avg_daily_tokens: f64,
    pub max_daily_tokens: i64,
    pub daily: Vec<DashboardDay>,
    pub weekly: Vec<UsageRollup>,
    pub monthly: Vec<UsageRollup>,
    pub by_provider: Vec<ProviderSummary>,
    pub by_model: Vec<ModelSummary>,
}

pub fn build_usage_summary_from_daily(
    mut daily: Vec<DashboardDay>,
    mut by_provider: Vec<ProviderSummary>,
    mut by_model: Vec<ModelSummary>,
    message_count: usize,
    session_count: usize,
) -> UsageSummary {
    daily.sort_by(|left, right| left.date.cmp(&right.date));
    apply_intensity_buckets(&mut daily);

    let total_cost: f64 = daily.iter().map(|day| day.total_cost_usd).sum();
    let total_tokens: i64 = daily.iter().map(|day| day.total_tokens).sum();
    let active_days = daily.len();
    let avg_daily_cost = if active_days == 0 {
        0.0
    } else {
        total_cost / active_days as f64
    };
    let max_daily_cost = daily
        .iter()
        .map(|entry| entry.total_cost_usd)
        .fold(0.0, f64::max);
    let avg_daily_tokens = if active_days == 0 {
        0.0
    } else {
        total_tokens as f64 / active_days as f64
    };
    let max_daily_tokens = daily
        .iter()
        .map(|entry| entry.total_tokens)
        .max()
        .unwrap_or_default();

    by_provider.sort_by(|left, right| right.tokens.cmp(&left.tokens));
    for provider in &mut by_provider {
        provider.percent = percent(provider.tokens, total_tokens);
    }

    by_model.sort_by(|left, right| right.tokens.cmp(&left.tokens));
    for model in &mut by_model {
        model.percent = percent(model.tokens, total_tokens);
    }

    let weekly = compute_weekly_rollups(&daily);
    let monthly = compute_monthly_rollups(&daily);

    UsageSummary {
        total_cost,
        total_tokens,
        message_count,
        session_count,
        active_days,
        avg_daily_cost,
        max_daily_cost,
        avg_daily_tokens,
        max_daily_tokens,
        daily,
        weekly,
        monthly,
        by_provider,
        by_model,
    }
}

pub fn compute_weekly_rollups(daily: &[DashboardDay]) -> Vec<UsageRollup> {
    compute_rollups(daily, weekly_group)
}

pub fn compute_monthly_rollups(daily: &[DashboardDay]) -> Vec<UsageRollup> {
    compute_rollups(daily, monthly_group)
}

fn compute_rollups<F>(daily: &[DashboardDay], group_fn: F) -> Vec<UsageRollup>
where
    F: Fn(NaiveDate) -> Option<(String, NaiveDate, NaiveDate)>,
{
    let mut groups: BTreeMap<String, UsageRollup> = BTreeMap::new();

    for day in daily {
        let Some(date) = parse_day(&day.date) else {
            continue;
        };
        let Some((label, start, end)) = group_fn(date) else {
            continue;
        };

        let entry = groups.entry(label.clone()).or_insert_with(|| UsageRollup {
            label,
            start_date: start.to_string(),
            end_date: end.to_string(),
            total_tokens: 0,
            total_cost_usd: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            message_count: 0,
            session_count: 0,
            active_days: 0,
        });

        entry.total_tokens += day.total_tokens;
        entry.total_cost_usd += day.total_cost_usd;
        entry.input_tokens += day.input_tokens;
        entry.output_tokens += day.output_tokens;
        entry.cache_read_tokens += day.cache_read_tokens;
        entry.cache_write_tokens += day.cache_write_tokens;
        entry.reasoning_tokens += day.reasoning_tokens;
        entry.message_count += day.message_count;
        entry.session_count += day.session_count;
        entry.active_days += 1;
    }

    groups.into_values().collect()
}

fn weekly_group(date: NaiveDate) -> Option<(String, NaiveDate, NaiveDate)> {
    let days_from_sunday = i64::from(date.weekday().num_days_from_sunday());
    let start = date.checked_sub_days(Days::new(days_from_sunday as u64))?;
    let end = start.checked_add_days(Days::new(6))?;
    Some((
        format!("{}..{}", start.format("%Y-%m-%d"), end.format("%Y-%m-%d")),
        start,
        end,
    ))
}

fn monthly_group(date: NaiveDate) -> Option<(String, NaiveDate, NaiveDate)> {
    let start = NaiveDate::from_ymd_opt(date.year(), date.month(), 1)?;
    let (next_year, next_month) = if date.month() == 12 {
        (date.year() + 1, 1)
    } else {
        (date.year(), date.month() + 1)
    };
    let next_start = NaiveDate::from_ymd_opt(next_year, next_month, 1)?;
    let end = next_start.checked_sub_days(Days::new(1))?;
    Some((start.format("%Y-%m").to_string(), start, end))
}

fn apply_intensity_buckets(daily: &mut [DashboardDay]) {
    apply_metric_buckets(
        daily,
        |day| day.total_tokens as f64,
        |day, bucket| {
            day.intensity_tokens = bucket;
        },
    );
    apply_metric_buckets(
        daily,
        |day| day.total_cost_usd,
        |day, bucket| {
            day.intensity_cost = bucket;
        },
    );
}

fn apply_metric_buckets<F, G>(daily: &mut [DashboardDay], value_fn: F, mut set_fn: G)
where
    F: Fn(&DashboardDay) -> f64,
    G: FnMut(&mut DashboardDay, u8),
{
    let max_value = daily.iter().map(&value_fn).fold(0.0, f64::max);
    for day in daily {
        let value = value_fn(day);
        let bucket = if value <= 0.0 || max_value <= 0.0 {
            0
        } else {
            ((value / max_value) * 4.0).ceil().clamp(1.0, 4.0) as u8
        };
        set_fn(day, bucket);
    }
}

fn percent(part: i64, total: i64) -> f64 {
    if total <= 0 {
        0.0
    } else {
        part as f64 / total as f64 * 100.0
    }
}

fn parse_day(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_usage_summary_recomputes_share_percentages() {
        let day = |date: &str, total_tokens: i64, total_cost_usd: f64| DashboardDay {
            date: date.to_string(),
            total_tokens,
            total_cost_usd,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            message_count: 1,
            session_count: 1,
            intensity_tokens: 0,
            intensity_cost: 0,
        };
        // Unsorted on purpose; two ISO weeks, one month.
        let daily = vec![
            day("2026-03-08", 200, 1.0),
            day("2026-03-01", 500, 2.5),
            day("2026-03-02", 300, 1.5),
        ];

        let summary = build_usage_summary_from_daily(
            daily,
            vec![
                ProviderSummary {
                    provider: "claude".to_string(),
                    cost: 2.0,
                    tokens: 400,
                    message_count: 2,
                    session_count: 1,
                    percent: 0.0,
                },
                ProviderSummary {
                    provider: "codex".to_string(),
                    cost: 3.0,
                    tokens: 600,
                    message_count: 3,
                    session_count: 1,
                    percent: 0.0,
                },
            ],
            vec![],
            5,
            2,
        );

        assert_eq!(summary.by_provider[0].provider, "codex");
        assert!((summary.by_provider[0].percent - 60.0).abs() < 0.001);
        assert_eq!(summary.daily[0].date, "2026-03-01");
        assert_eq!(summary.daily[0].intensity_tokens, 4);
        assert_eq!(summary.weekly.len(), 2);
        assert_eq!(summary.weekly[0].label, "2026-03-01..2026-03-07");
        assert_eq!(summary.monthly.len(), 1);
        assert_eq!(summary.monthly[0].label, "2026-03");
        assert!((summary.total_cost - 5.0).abs() < 0.001);
    }
}
