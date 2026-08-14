pub mod daily;
pub mod heatmap;
pub mod keeper;
pub mod models;
pub mod overview;
pub mod quota;
pub mod settings;

use crate::tui::theme::{Theme, ThemeMode};
use crate::tui::widgets::HeatmapMetric;
use anyhow::Result;
use chrono::{Duration, Local, NaiveDate};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration as StdDuration, Instant};
use tokenpulse_core::{
    config::{Config, ConfigManager, ThemePreference},
    usage::{normalize_model_name, DailyUsageRow, DashboardDay, ModelSummary, UsageSummary},
};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePage {
    Overview,
    Models,
    Daily,
    Heatmap,
    Quota,
    Keeper,
    Settings,
}

impl UsagePage {
    pub fn all() -> [UsagePage; 7] {
        [
            UsagePage::Overview,
            UsagePage::Models,
            UsagePage::Daily,
            UsagePage::Heatmap,
            UsagePage::Quota,
            UsagePage::Keeper,
            UsagePage::Settings,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            UsagePage::Overview => "Overview",
            UsagePage::Models => "Models",
            UsagePage::Daily => "Daily",
            UsagePage::Heatmap => "Activity",
            UsagePage::Quota => "Quota",
            UsagePage::Keeper => "Keeper",
            UsagePage::Settings => "Settings",
        }
    }

    pub fn next(self) -> Self {
        let pages = Self::all();
        let idx = pages.iter().position(|p| *p == self).unwrap_or(0);
        pages[(idx + 1) % pages.len()]
    }

    pub fn previous(self) -> Self {
        let pages = Self::all();
        let idx = pages.iter().position(|p| *p == self).unwrap_or(0);
        pages[(idx + pages.len() - 1) % pages.len()]
    }
}

// ---------------------------------------------------------------------------
// Sort
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Date,
    Cost,
    Tokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverviewMetric {
    Tokens,
    Cost,
}

impl OverviewMetric {
    pub fn toggle_to_tokens(&mut self) {
        *self = OverviewMetric::Tokens;
    }

    pub fn toggle_to_cost(&mut self) {
        *self = OverviewMetric::Cost;
    }

    pub fn value_format(self) -> crate::tui::widgets::ValueFormat {
        match self {
            OverviewMetric::Tokens => crate::tui::widgets::ValueFormat::CompactNumber,
            OverviewMetric::Cost => crate::tui::widgets::ValueFormat::Currency,
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            OverviewMetric::Tokens => "tokens",
            OverviewMetric::Cost => "cost",
        }
    }

    pub fn daily_vs7d_header(self) -> &'static str {
        match self {
            OverviewMetric::Tokens => "Token vs7d",
            OverviewMetric::Cost => "Cost vs7d",
        }
    }
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct DayBreakdown {
    pub provider_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub tokens: i64,
    pub cost_usd: f64,
    pub messages: i64,
    pub sessions: i64,
}

#[derive(Debug, Clone)]
pub struct DailyStats {
    pub date: NaiveDate,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    pub messages: i64,
    pub sessions: i64,
    pub providers: HashMap<String, DayBreakdown>,
    pub models: HashMap<String, DayBreakdown>,
}

impl DailyStats {
    pub fn from_day(day: &DashboardDay) -> Option<Self> {
        Some(Self {
            date: NaiveDate::parse_from_str(&day.date, "%Y-%m-%d").ok()?,
            input_tokens: day.input_tokens,
            output_tokens: day.output_tokens,
            cache_read_tokens: day.cache_read_tokens,
            cache_write_tokens: day.cache_write_tokens,
            reasoning_tokens: day.reasoning_tokens,
            total_tokens: day.total_tokens,
            cost_usd: day.total_cost_usd,
            messages: day.message_count,
            sessions: day.session_count,
            providers: HashMap::new(),
            models: HashMap::new(),
        })
    }

    pub fn filtered(&self, enabled: &BTreeSet<String>) -> Option<Self> {
        let providers: HashMap<String, DayBreakdown> = self
            .providers
            .iter()
            .filter(|(source, _)| enabled.contains(*source))
            .map(|(source, stats)| (source.clone(), stats.clone()))
            .collect();
        if providers.is_empty() {
            return None;
        }

        let models: HashMap<String, DayBreakdown> = self
            .models
            .iter()
            .filter(|(model_key, _)| enabled.contains(model_source(model_key)))
            .map(|(model_key, stats)| (model_key.clone(), stats.clone()))
            .collect();

        let mut filtered = self.clone();
        filtered.input_tokens = providers.values().map(|row| row.input_tokens).sum();
        filtered.output_tokens = providers.values().map(|row| row.output_tokens).sum();
        filtered.cache_read_tokens = providers.values().map(|row| row.cache_read_tokens).sum();
        filtered.cache_write_tokens = providers.values().map(|row| row.cache_write_tokens).sum();
        filtered.reasoning_tokens = providers.values().map(|row| row.reasoning_tokens).sum();
        filtered.total_tokens = providers.values().map(|row| row.tokens).sum();
        filtered.cost_usd = providers.values().map(|row| row.cost_usd).sum();
        filtered.messages = providers.values().map(|row| row.messages).sum();
        filtered.sessions = providers.values().map(|row| row.sessions).sum();
        filtered.providers = providers;
        filtered.models = models;
        Some(filtered)
    }

    pub fn metric_value(&self, metric: HeatmapMetric) -> f64 {
        match metric {
            HeatmapMetric::TotalTokens => self.total_tokens as f64,
            HeatmapMetric::Cost => self.cost_usd,
        }
    }
}

pub fn model_source(model_key: &str) -> &str {
    model_key
        .split_once(" / ")
        .map(|(source, _)| source)
        .unwrap_or("")
}

pub fn model_id_from_key(model_key: &str) -> &str {
    model_key
        .split_once(" / ")
        .map(|(_, model_id)| model_id)
        .unwrap_or(model_key)
}

pub fn display_source_name(source: &str) -> &'static str {
    match source {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "copilot" => "Copilot CLI",
        "opencode" => "OpenCode",
        "gemini" => "Gemini CLI",
        "pi" => "PI",
        "antigravity" => "Antigravity",
        _ => "Unknown",
    }
}

pub fn display_source_filter_name(source: &str) -> String {
    let label = display_source_name(source);
    if label == "Unknown" {
        source.to_string()
    } else {
        label.to_string()
    }
}

pub fn format_source_list(source_csv: &str) -> String {
    let mut labels: Vec<&str> = source_csv
        .split(',')
        .map(str::trim)
        .filter(|source| !source.is_empty())
        .map(display_source_name)
        .collect();
    labels.dedup();
    if labels.is_empty() {
        "Unknown".to_string()
    } else {
        labels.join(", ")
    }
}

#[derive(Default)]
pub struct AggregatedModelSummary {
    pub providers: BTreeSet<String>,
    pub sources: BTreeSet<String>,
    pub cost: f64,
    pub tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub message_count: usize,
    pub session_count: usize,
}

#[derive(Debug, Clone)]
pub struct AgentModelGroup {
    pub source: String,
    pub total_cost_usd: f64,
    pub total_tokens: i64,
    pub models: Vec<(String, DayBreakdown)>,
}

pub struct ModelTableRow {
    pub summary: ModelSummary,
    pub last_used: Option<NaiveDate>,
}

// ---------------------------------------------------------------------------
// Dashboard aggregate
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct UsageDashboard {
    pub daily: Vec<DailyStats>,
}

impl UsageDashboard {
    pub fn build(summary: &UsageSummary, daily_rows: &[DailyUsageRow]) -> Self {
        let mut days: HashMap<String, DailyStats> = summary
            .daily
            .iter()
            .filter_map(DailyStats::from_day)
            .map(|d| (d.date.format("%Y-%m-%d").to_string(), d))
            .collect();

        for row in daily_rows {
            let Some(day) = days.get_mut(&row.date) else {
                continue;
            };

            let provider = day.providers.entry(row.source.clone()).or_default();
            provider.provider_id = row.provider_id.clone();
            provider.input_tokens += row.input_tokens;
            provider.output_tokens += row.output_tokens;
            provider.cache_read_tokens += row.cache_read_tokens;
            provider.cache_write_tokens += row.cache_write_tokens;
            provider.reasoning_tokens += row.reasoning_tokens;
            provider.tokens += row.total_tokens;
            provider.cost_usd += row.cost_usd;
            provider.messages += row.message_count;
            provider.sessions += row.session_count;

            let model_key = format!("{} / {}", row.source, row.model_id);
            let model = day.models.entry(model_key).or_default();
            model.provider_id = row.provider_id.clone();
            model.input_tokens += row.input_tokens;
            model.output_tokens += row.output_tokens;
            model.cache_read_tokens += row.cache_read_tokens;
            model.cache_write_tokens += row.cache_write_tokens;
            model.reasoning_tokens += row.reasoning_tokens;
            model.tokens += row.total_tokens;
            model.cost_usd += row.cost_usd;
            model.messages += row.message_count;
            model.sessions += row.session_count;
        }

        let mut daily: Vec<DailyStats> = days.into_values().collect();
        daily.sort_by_key(|d| d.date);

        Self { daily }
    }

    pub fn latest_date(&self) -> Option<NaiveDate> {
        self.daily.last().map(|d| d.date)
    }

    pub fn day(&self, date: NaiveDate) -> Option<&DailyStats> {
        self.daily.iter().find(|d| d.date == date)
    }

    pub fn filtered_daily(&self, enabled: &BTreeSet<String>) -> Vec<DailyStats> {
        self.daily
            .iter()
            .filter_map(|day| day.filtered(enabled))
            .collect()
    }

    pub fn move_selection(&self, selected: Option<NaiveDate>, offset: isize) -> Option<NaiveDate> {
        if self.daily.is_empty() {
            return None;
        }
        let cur = selected
            .and_then(|d| self.daily.iter().position(|day| day.date == d))
            .unwrap_or(self.daily.len().saturating_sub(1));
        let next =
            (cur as isize + offset).clamp(0, self.daily.len().saturating_sub(1) as isize) as usize;
        self.daily.get(next).map(|d| d.date)
    }

    pub fn bounds_for_fixed_window(&self) -> Option<(NaiveDate, NaiveDate)> {
        let latest = self.latest_date()?;
        let end = latest;
        Some((end - Duration::days(364), end))
    }

    pub fn days_in_fixed_window(&self, enabled: &BTreeSet<String>) -> Vec<&DailyStats> {
        let Some((start, end)) = self.bounds_for_fixed_window() else {
            return Vec::new();
        };
        self.daily
            .iter()
            .filter(|d| {
                d.date >= start
                    && d.date <= end
                    && d.providers.keys().any(|source| enabled.contains(source))
            })
            .collect()
    }

    pub fn points_in_fixed_window(
        &self,
        metric: HeatmapMetric,
        enabled: &BTreeSet<String>,
    ) -> Vec<(NaiveDate, f64)> {
        self.days_in_fixed_window(enabled)
            .into_iter()
            .filter_map(|d| {
                d.filtered(enabled)
                    .map(|filtered| (filtered.date, filtered.metric_value(metric)))
            })
            .collect()
    }

    pub fn selected_day_in_fixed_window(
        &self,
        selected: Option<NaiveDate>,
        enabled: &BTreeSet<String>,
    ) -> Option<DailyStats> {
        let (start, end) = self.bounds_for_fixed_window()?;
        let sel = selected?;
        if sel < start || sel > end {
            return None;
        }
        self.day(sel)?.filtered(enabled)
    }

    pub fn active_days_in_fixed_window(
        &self,
        metric: HeatmapMetric,
        enabled: &BTreeSet<String>,
    ) -> usize {
        self.days_in_fixed_window(enabled)
            .into_iter()
            .filter_map(|d| d.filtered(enabled))
            .filter(|d| d.metric_value(metric) > 0.0)
            .count()
    }

    pub fn longest_streak_in_fixed_window(
        &self,
        metric: HeatmapMetric,
        enabled: &BTreeSet<String>,
    ) -> usize {
        let Some((start, end)) = self.bounds_for_fixed_window() else {
            return 0;
        };
        let values: HashMap<NaiveDate, f64> = self
            .days_in_fixed_window(enabled)
            .into_iter()
            .filter_map(|d| {
                d.filtered(enabled)
                    .map(|filtered| (filtered.date, filtered.metric_value(metric)))
            })
            .collect();
        let mut cursor = start;
        let (mut current, mut best) = (0usize, 0usize);
        while cursor <= end {
            if values.get(&cursor).copied().unwrap_or(0.0) > 0.0 {
                current += 1;
                best = best.max(current);
            } else {
                current = 0;
            }
            cursor += Duration::days(1);
        }
        best
    }

    pub fn current_streak_in_fixed_window(
        &self,
        metric: HeatmapMetric,
        enabled: &BTreeSet<String>,
    ) -> usize {
        let Some((start, end)) = self.bounds_for_fixed_window() else {
            return 0;
        };
        let values: HashMap<NaiveDate, f64> = self
            .days_in_fixed_window(enabled)
            .into_iter()
            .filter_map(|d| {
                d.filtered(enabled)
                    .map(|filtered| (filtered.date, filtered.metric_value(metric)))
            })
            .collect();
        let mut cursor = end;
        let mut streak = 0usize;
        while cursor >= start {
            if values.get(&cursor).copied().unwrap_or(0.0) > 0.0 {
                streak += 1;
            } else if streak > 0 {
                break;
            }
            if cursor == start {
                break;
            }
            cursor -= Duration::days(1);
        }
        streak
    }

    pub fn recent_days(&self, limit: usize) -> Vec<&DailyStats> {
        let start = self.daily.len().saturating_sub(limit);
        self.daily[start..].iter().collect()
    }

    pub fn all_sources(&self) -> Vec<String> {
        let mut sources = BTreeSet::new();
        for day in &self.daily {
            for provider in day.providers.keys() {
                sources.insert(provider.clone());
            }
        }
        sources.into_iter().collect()
    }

    pub fn filtered_models(&self, enabled: &BTreeSet<String>) -> Vec<ModelSummary> {
        let mut aggregated: HashMap<String, AggregatedModelSummary> = HashMap::new();

        for day in &self.daily {
            for (model_key, stats) in &day.models {
                let source = model_source(model_key);
                if !enabled.contains(source) {
                    continue;
                }

                let model_id = model_id_from_key(model_key);
                let model = normalize_model_name(model_id);
                let entry = aggregated.entry(model.clone()).or_default();
                if !stats.provider_id.is_empty() {
                    entry.providers.insert(stats.provider_id.clone());
                }
                entry.sources.insert(source.to_string());
                entry.cost += stats.cost_usd;
                entry.tokens += stats.tokens;
                entry.input_tokens += stats.input_tokens;
                entry.output_tokens += stats.output_tokens;
                entry.cache_read_tokens += stats.cache_read_tokens;
                entry.cache_write_tokens += stats.cache_write_tokens;
                entry.message_count += stats.messages.max(0) as usize;
                entry.session_count += stats.sessions.max(0) as usize;
            }
        }

        let total_cost = aggregated.values().map(|model| model.cost).sum::<f64>();
        let mut models: Vec<ModelSummary> = aggregated
            .into_iter()
            .map(|(model, aggregated)| ModelSummary {
                model,
                provider: aggregated
                    .providers
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(","),
                source: aggregated.sources.into_iter().collect::<Vec<_>>().join(","),
                cost: aggregated.cost,
                tokens: aggregated.tokens,
                input_tokens: aggregated.input_tokens,
                output_tokens: aggregated.output_tokens,
                cache_tokens: aggregated.cache_read_tokens + aggregated.cache_write_tokens,
                cache_read_tokens: aggregated.cache_read_tokens,
                cache_write_tokens: aggregated.cache_write_tokens,
                message_count: aggregated.message_count,
                session_count: aggregated.session_count,
                percent: if total_cost > 0.0 {
                    aggregated.cost / total_cost * 100.0
                } else {
                    0.0
                },
            })
            .collect();

        models.sort_by(|left, right| {
            right
                .cost
                .partial_cmp(&left.cost)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.tokens.cmp(&left.tokens))
                .then_with(|| left.model.cmp(&right.model))
        });
        models
    }

    pub fn model_last_used(
        &self,
        model_name: &str,
        enabled: &BTreeSet<String>,
    ) -> Option<NaiveDate> {
        self.daily
            .iter()
            .rev()
            .find(|day| {
                day.models.iter().any(|(model_key, stats)| {
                    enabled.contains(model_source(model_key))
                        && normalize_model_name(model_id_from_key(model_key)) == model_name
                        && (stats.tokens > 0 || stats.cost_usd > 0.0 || stats.messages > 0)
                })
            })
            .map(|day| day.date)
    }
}

pub fn build_agent_model_groups(day: &DailyStats) -> Vec<AgentModelGroup> {
    let mut grouped: HashMap<String, AgentModelGroup> = HashMap::new();

    for (model_key, stats) in &day.models {
        if stats.tokens <= 0 {
            continue;
        }
        let source = model_source(model_key).to_string();
        let model_name = model_id_from_key(model_key).to_string();
        let entry = grouped
            .entry(source.clone())
            .or_insert_with(|| AgentModelGroup {
                source,
                total_cost_usd: 0.0,
                total_tokens: 0,
                models: Vec::new(),
            });
        entry.total_cost_usd += stats.cost_usd;
        entry.total_tokens += stats.tokens;
        entry.models.push((model_name, stats.clone()));
    }

    let mut groups: Vec<AgentModelGroup> = grouped.into_values().collect();
    groups.retain(|group| !group.models.is_empty());
    for group in &mut groups {
        group.models.sort_by(|left, right| {
            right
                .1
                .cost_usd
                .partial_cmp(&left.1.cost_usd)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.1.tokens.cmp(&left.1.tokens))
                .then_with(|| left.0.cmp(&right.0))
        });
    }

    groups.sort_by(|left, right| {
        right
            .total_cost_usd
            .partial_cmp(&left.total_cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.total_tokens.cmp(&left.total_tokens))
            .then_with(|| left.source.cmp(&right.source))
    });
    groups
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

fn compute_data_date_range(dashboard: &UsageDashboard) -> Option<(NaiveDate, NaiveDate)> {
    let first = dashboard.daily.first()?.date;
    let last = dashboard.daily.last()?.date;
    Some((first, last))
}

pub struct UsageState {
    pub page: UsagePage,
    pub overview_metric: OverviewMetric,
    pub daily_metric: OverviewMetric,
    pub heatmap_metric: HeatmapMetric,
    pub selected_heatmap_date: Option<NaiveDate>,
    pub selected_heatmap_legend_bucket: Option<usize>,
    pub heatmap_detail_scroll: usize,
    pub scroll_offset: usize,
    pub selected_row: usize,
    pub sort_field: SortField,
    pub sort_ascending: bool,
    pub model_filter: String,
    pub model_filter_active: bool,
    // Source filter overlay
    pub show_source_filter: bool,
    pub source_filter_cursor: usize,
    pub all_sources: Vec<String>,
    pub enabled_sources: BTreeSet<String>,
    // Help overlay
    pub show_help: bool,
    // Refresh tracking
    pub last_refreshed: Option<chrono::DateTime<Local>>,
    pub refresh_status: Option<RefreshStatus>,
    pub data_date_range: Option<(NaiveDate, NaiveDate)>,
    // Quota and Provider filter state
    pub quota_snapshots: Vec<tokenpulse_core::QuotaSnapshot>,
    pub last_refresh: Instant,
    pub usage_refresh_in_progress: bool,
    pub quota_refresh_in_progress: bool,
    // Keeper state
    pub selected_keeper_index: usize,
    pub keeper_daily_triggered: HashMap<String, NaiveDate>,
    pub keeper_weekly_triggered: HashMap<String, chrono::DateTime<chrono::Utc>>,
    pub keeper_logs: Vec<tokenpulse_core::keeper::KeeperExecutionRecord>,
    pub keeper_pings_in_progress: BTreeSet<String>,
    pub keeper_log_scroll: usize,
    pub last_keeper_check: Instant,
}

#[derive(Debug, Clone)]
pub struct RefreshStatus {
    pub message: String,
    pub level: RefreshStatusLevel,
    pub until: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshStatusLevel {
    Info,
    Success,
    Error,
}

impl UsageState {
    pub fn new(
        dashboard: &UsageDashboard,
        quota_snapshots: Vec<tokenpulse_core::QuotaSnapshot>,
    ) -> Self {
        let all_sources = dashboard.all_sources();
        let enabled_sources: BTreeSet<String> = all_sources.iter().cloned().collect();
        let today = Local::now().date_naive();
        let selected_heatmap_date = dashboard
            .day(today)
            .map(|day| day.date)
            .or_else(|| dashboard.latest_date());
        Self {
            page: UsagePage::Overview,
            overview_metric: OverviewMetric::Tokens,
            daily_metric: OverviewMetric::Cost,
            heatmap_metric: HeatmapMetric::TotalTokens,
            selected_heatmap_date,
            selected_heatmap_legend_bucket: None,
            heatmap_detail_scroll: 0,
            scroll_offset: 0,
            selected_row: 0,
            sort_field: SortField::Cost,
            sort_ascending: false,
            model_filter: String::new(),
            model_filter_active: false,
            show_source_filter: false,
            source_filter_cursor: 0,
            all_sources,
            enabled_sources,
            show_help: false,
            last_refreshed: Some(Local::now()),
            refresh_status: None,
            data_date_range: compute_data_date_range(dashboard),
            quota_snapshots,
            last_refresh: Instant::now(),
            usage_refresh_in_progress: false,
            quota_refresh_in_progress: false,
            selected_keeper_index: 0,
            keeper_daily_triggered: HashMap::new(),
            keeper_weekly_triggered: HashMap::new(),
            keeper_logs: Vec::new(),
            keeper_pings_in_progress: BTreeSet::new(),
            keeper_log_scroll: 0,
            last_keeper_check: Instant::now(),
        }
    }

    pub fn is_refreshing(&self) -> bool {
        self.usage_refresh_in_progress || self.quota_refresh_in_progress
    }

    /// Reset the shared auto-refresh countdown. Called once both the usage and
    /// quota refreshes have finished (success or failure) so the single timer
    /// always restarts from one consistent anchor.
    fn reset_timer_if_idle(&mut self) {
        if !self.is_refreshing() {
            self.last_refresh = Instant::now();
        }
    }

    pub fn next_page(&mut self) {
        self.page = self.page.next();
        self.reset_scroll();
    }

    pub fn previous_page(&mut self) {
        self.page = self.page.previous();
        self.reset_scroll();
    }

    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
        self.selected_row = 0;
        self.heatmap_detail_scroll = 0;
    }

    pub fn set_heatmap_metric(&mut self, metric: HeatmapMetric) {
        if self.heatmap_metric != metric {
            self.selected_heatmap_legend_bucket = None;
        }
        self.heatmap_metric = metric;
        self.heatmap_detail_scroll = 0;
    }

    pub fn move_selection(&mut self, total: usize, visible: usize, delta: isize) {
        if total == 0 {
            self.selected_row = 0;
            self.scroll_offset = 0;
            return;
        }

        let max_index = total.saturating_sub(1);
        self.selected_row =
            (self.selected_row as isize + delta).clamp(0, max_index as isize) as usize;
        self.sync_scroll_to_selection(total, visible);
    }

    pub fn sync_scroll_to_selection(&mut self, total: usize, visible: usize) {
        if total == 0 || visible == 0 {
            self.scroll_offset = 0;
            self.selected_row = 0;
            return;
        }

        self.selected_row = self.selected_row.min(total.saturating_sub(1));
        if self.selected_row < self.scroll_offset {
            self.scroll_offset = self.selected_row;
        } else if self.selected_row >= self.scroll_offset + visible {
            self.scroll_offset = self.selected_row + 1 - visible;
        }

        self.scroll_offset = self.scroll_offset.min(total.saturating_sub(visible));
    }

    pub fn toggle_sort(&mut self, field: SortField) {
        match field {
            SortField::Cost => self.daily_metric.toggle_to_cost(),
            SortField::Tokens => self.daily_metric.toggle_to_tokens(),
            SortField::Date => {}
        }
        if self.sort_field == field {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_field = field;
            self.sort_ascending = false;
        }
        self.reset_scroll();
    }

    pub fn toggle_source_at_cursor(&mut self) {
        if let Some(source) = self.all_sources.get(self.source_filter_cursor) {
            if self.enabled_sources.contains(source) {
                // Don't allow disabling the last source
                if self.enabled_sources.len() > 1 {
                    self.enabled_sources.remove(source);
                }
            } else {
                self.enabled_sources.insert(source.clone());
            }
            self.reset_scroll();
        }
    }

    pub fn is_source_enabled(&self, source: &str) -> bool {
        self.enabled_sources.contains(source)
    }

    pub fn set_selected_heatmap_date(&mut self, date: Option<NaiveDate>) {
        self.selected_heatmap_date = date;
        self.heatmap_detail_scroll = 0;
    }

    pub fn scroll_heatmap_detail_up(&mut self) {
        self.heatmap_detail_scroll = self.heatmap_detail_scroll.saturating_sub(1);
    }

    pub fn scroll_heatmap_detail_down(&mut self, max: usize) {
        if self.heatmap_detail_scroll < max {
            self.heatmap_detail_scroll += 1;
        }
    }

    pub fn set_refresh_status(&mut self, message: impl Into<String>, level: RefreshStatusLevel) {
        self.refresh_status = Some(RefreshStatus {
            message: message.into(),
            level,
            until: Instant::now() + StdDuration::from_secs(2),
        });
    }

    pub fn clear_expired_refresh_status(&mut self) {
        if self
            .refresh_status
            .as_ref()
            .is_some_and(|status| Instant::now() >= status.until)
        {
            self.refresh_status = None;
        }
    }
}

pub fn scrollable_item_count(dashboard: &UsageDashboard, state: &UsageState) -> usize {
    match state.page {
        UsagePage::Overview => dashboard.filtered_models(&state.enabled_sources).len(),
        UsagePage::Models => models::filtered_models_for_state(dashboard, state).len(),
        UsagePage::Daily => daily::visible_daily_rows(dashboard, &state.enabled_sources).len(),
        UsagePage::Heatmap | UsagePage::Quota | UsagePage::Keeper | UsagePage::Settings => 0,
    }
}

pub fn visible_rows_for_page(page: UsagePage, frame_area: Rect) -> usize {
    let body = dashboard_body_area(frame_area);
    match page {
        UsagePage::Overview => {
            let sections = overview::overview_sections(body);
            table_data_rows(sections[1], 2)
        }
        UsagePage::Models => table_data_rows(body, 1),
        UsagePage::Daily => table_data_rows(body, 1),
        UsagePage::Heatmap | UsagePage::Quota | UsagePage::Keeper | UsagePage::Settings => 0,
    }
}

pub fn table_data_rows(area: Rect, non_data_inner_rows: u16) -> usize {
    area.height
        .saturating_sub(2)
        .saturating_sub(non_data_inner_rows) as usize
}

pub fn move_table_selection_for_frame(
    state: &mut UsageState,
    dashboard: &UsageDashboard,
    frame_area: Rect,
    delta: isize,
) {
    let total = scrollable_item_count(dashboard, state);
    let visible = visible_rows_for_page(state.page, frame_area);
    state.move_selection(total, visible, delta);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[derive(Debug)]
#[allow(dead_code)]
pub enum TuiMessage {
    UsageReloadSuccess(UsageSummary, Vec<DailyUsageRow>),
    UsageReloadFailed(String),
    QuotaReloadSuccess(Vec<tokenpulse_core::QuotaSnapshot>, Vec<String>),
    QuotaReloadFailed(String),
    KeeperPingCompleted(tokenpulse_core::keeper::KeeperExecutionRecord),
}

fn spawn_quota_reload(
    msg_tx: tokio::sync::mpsc::Sender<TuiMessage>,
    enabled_providers: Vec<String>,
) {
    tokio::spawn(async move {
        let fetchers = crate::commands::quota::build_quota_fetchers(&enabled_providers);
        let total_fetchers = fetchers.len();
        let observed_at = chrono::Utc::now();
        let fetch_start = std::time::Instant::now();
        let results = tokenpulse_core::quota::fetch_all(fetchers).await;
        let fetch_elapsed = fetch_start.elapsed();
        info!(
            "Quota fetch completed in {} ms (fetchers count: {})",
            fetch_elapsed.as_millis(),
            total_fetchers
        );

        let mut snapshots = Vec::new();
        let mut errors = Vec::new();
        for result in results {
            match result {
                Ok(snapshot) => {
                    snapshots.push(snapshot);
                }
                Err(e) => {
                    warn!("Quota fetch failed for provider: {}", e);
                    errors.push(e.to_string());
                }
            }
        }

        if !snapshots.is_empty() {
            let snapshots_to_save = snapshots.clone();
            let save_res = tokio::task::spawn_blocking(move || {
                let cache_store = tokenpulse_core::quota::QuotaCacheStore::new();
                for snap in &snapshots_to_save {
                    let _ = cache_store.save(&snap.provider, observed_at, snap);
                }
            })
            .await;
            if let Err(_e) = save_res {
                // Ignore join error
            }
        }

        if snapshots.is_empty() && total_fetchers > 0 {
            let _ = msg_tx
                .send(TuiMessage::QuotaReloadFailed(errors.join(", ")))
                .await;
        } else {
            let _ = msg_tx
                .send(TuiMessage::QuotaReloadSuccess(snapshots, errors))
                .await;
        }
    });
}

/// When the theme preference is `auto`, re-detect the OS appearance and swap the
/// active theme if it changed. Called on each refresh (auto or manual) so the TUI
/// follows the system light/dark setting without a dedicated polling loop. Uses a
/// side-effect-free OS probe only (no OSC11), so it never disturbs the event loop's
/// stdin reader.
fn follow_system_theme(theme: &mut Theme, preference: ThemePreference) {
    if preference != ThemePreference::Auto {
        return;
    }
    if let Some(mode) = ThemeMode::detect_system_appearance() {
        if mode != theme.mode {
            *theme = Theme::new(mode);
        }
    }
}

pub fn run<F>(
    mut summary: UsageSummary,
    daily_rows: Vec<DailyUsageRow>,
    quota_snapshots: Vec<tokenpulse_core::QuotaSnapshot>,
    reload: F,
) -> Result<()>
where
    F: FnMut() -> Result<(UsageSummary, Vec<DailyUsageRow>)> + Send + 'static,
{
    let config_manager = ConfigManager::new();
    let mut config = config_manager.load().unwrap_or_default();
    let theme_preference = config.display.theme;

    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::channel::<TuiMessage>(10);
    let reload_fn_arc = Arc::new(Mutex::new(reload));

    enable_raw_mode()?;
    let mut theme = Theme::from_preference(theme_preference);
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut dashboard = UsageDashboard::build(&summary, &daily_rows);
    let mut state = UsageState::new(&dashboard, quota_snapshots);

    // Initial background reload on startup
    state.set_refresh_status("Refreshing...", RefreshStatusLevel::Info);
    state.usage_refresh_in_progress = true;
    state.quota_refresh_in_progress = config.display.refresh_quota;

    {
        let reload_clone = Arc::clone(&reload_fn_arc);
        let tx = msg_tx.clone();
        tokio::task::spawn_blocking(move || {
            let mut reload_guard = reload_clone.lock().unwrap();
            match (*reload_guard)() {
                Ok((sum, rows)) => {
                    let _ = tx.blocking_send(TuiMessage::UsageReloadSuccess(sum, rows));
                }
                Err(e) => {
                    let _ = tx.blocking_send(TuiMessage::UsageReloadFailed(e.to_string()));
                }
            }
        });
    }

    if config.display.refresh_quota {
        let enabled_providers: Vec<String> = config
            .providers
            .iter()
            .filter(|(_, p)| p.enabled)
            .map(|(k, _)| k.clone())
            .collect();
        spawn_quota_reload(msg_tx.clone(), enabled_providers);
    }

    loop {
        // Process background messages
        while let Ok(msg) = msg_rx.try_recv() {
            match msg {
                TuiMessage::UsageReloadSuccess(new_summary, new_daily_rows) => {
                    let new_dashboard = UsageDashboard::build(&new_summary, &new_daily_rows);
                    let saved_page = state.page;
                    let saved_sort_field = state.sort_field;
                    let saved_sort_ascending = state.sort_ascending;
                    let saved_model_filter = std::mem::take(&mut state.model_filter);
                    let saved_sources = state.enabled_sources.clone();
                    let saved_heatmap_date = state.selected_heatmap_date;
                    let saved_selected_row = state.selected_row;
                    summary = new_summary;
                    dashboard = new_dashboard;
                    let saved_quota_snapshots = state.quota_snapshots.clone();
                    let saved_last_refresh = state.last_refresh;
                    let saved_quota_refresh_in_progress = state.quota_refresh_in_progress;

                    state = UsageState::new(&dashboard, saved_quota_snapshots);
                    state.last_refresh = saved_last_refresh;
                    state.quota_refresh_in_progress = saved_quota_refresh_in_progress;
                    state.usage_refresh_in_progress = false;
                    state.reset_timer_if_idle();
                    state.page = saved_page;
                    state.sort_field = saved_sort_field;
                    state.sort_ascending = saved_sort_ascending;
                    state.model_filter = saved_model_filter;
                    let new_all: BTreeSet<String> = state.all_sources.iter().cloned().collect();
                    let filtered: BTreeSet<String> = saved_sources
                        .into_iter()
                        .filter(|s| new_all.contains(s))
                        .collect();
                    state.enabled_sources = if filtered.is_empty() {
                        new_all
                    } else {
                        filtered
                    };
                    state.selected_heatmap_date = saved_heatmap_date;
                    state.selected_row = saved_selected_row;
                    state.last_refreshed = Some(Local::now());

                    if !state.is_refreshing() {
                        state.set_refresh_status("Refresh complete", RefreshStatusLevel::Success);
                    }
                }
                TuiMessage::UsageReloadFailed(err) => {
                    state.usage_refresh_in_progress = false;
                    state.reset_timer_if_idle();
                    state.set_refresh_status(
                        format!("Refresh failed: {}", err),
                        RefreshStatusLevel::Error,
                    );
                }
                TuiMessage::QuotaReloadSuccess(new_snapshots, errors) => {
                    for snap in new_snapshots {
                        if let Some(existing) = state
                            .quota_snapshots
                            .iter_mut()
                            .find(|s| s.provider == snap.provider)
                        {
                            *existing = snap;
                        } else {
                            state.quota_snapshots.push(snap);
                        }
                    }
                    state.quota_refresh_in_progress = false;
                    state.reset_timer_if_idle();
                    if !errors.is_empty() {
                        state.set_refresh_status(
                            format!("Refresh failed: {}", errors.join(", ")),
                            RefreshStatusLevel::Error,
                        );
                    } else if !state.is_refreshing() {
                        state.set_refresh_status("Refresh complete", RefreshStatusLevel::Success);
                    }
                }
                TuiMessage::QuotaReloadFailed(err) => {
                    state.quota_refresh_in_progress = false;
                    state.reset_timer_if_idle();
                    state.set_refresh_status(
                        format!("Refresh failed: {}", err),
                        RefreshStatusLevel::Error,
                    );
                }
                TuiMessage::KeeperPingCompleted(record) => {
                    let agent = record.agent.clone();
                    let success = record.success;
                    let trigger_type = record.trigger_type;
                    if trigger_type == tokenpulse_core::keeper::KeeperTriggerType::Daily {
                        state
                            .keeper_daily_triggered
                            .insert(agent.clone(), Local::now().date_naive());
                    } else if trigger_type == tokenpulse_core::keeper::KeeperTriggerType::Weekly {
                        state
                            .keeper_weekly_triggered
                            .insert(agent.clone(), chrono::Utc::now());
                    }
                    let level = if success {
                        RefreshStatusLevel::Success
                    } else {
                        RefreshStatusLevel::Error
                    };
                    state.set_refresh_status(
                        format!(
                            "Keeper {}: {}",
                            keeper::keeper_agent_name(&agent),
                            if success { "Success" } else { "Failed" }
                        ),
                        level,
                    );
                    state.keeper_pings_in_progress.remove(&agent);
                    state.keeper_logs.push(record);
                }
            }
        }

        state.clear_expired_refresh_status();
        terminal.draw(|f| {
            let size = f.area();
            render_dashboard(
                f,
                size,
                &dashboard,
                &summary,
                &state,
                &theme,
                &config,
                &config_manager,
            );
            if state.show_source_filter {
                render_source_filter_overlay(f, size, &state, &theme);
            }
            if state.show_help {
                render_help_overlay(f, size, &state, &theme);
            }
        })?;

        // Auto-refresh: a single shared timer drives both scans. When it
        // elapses and nothing is in flight, kick off the usage and quota
        // refreshes together; the timer only resets once both have finished.
        let auto_secs = config.display.auto_refresh_secs;
        if auto_secs > 0
            && !state.is_refreshing()
            && state.last_refresh.elapsed().as_secs() >= auto_secs as u64
        {
            follow_system_theme(&mut theme, config.display.theme);
            state.usage_refresh_in_progress = true;
            state.quota_refresh_in_progress = config.display.refresh_quota;
            state.set_refresh_status("Auto-refreshing...", RefreshStatusLevel::Info);

            let reload_clone = Arc::clone(&reload_fn_arc);
            let tx = msg_tx.clone();
            tokio::task::spawn_blocking(move || {
                let mut reload_guard = reload_clone.lock().unwrap();
                match (*reload_guard)() {
                    Ok((sum, rows)) => {
                        let _ = tx.blocking_send(TuiMessage::UsageReloadSuccess(sum, rows));
                    }
                    Err(e) => {
                        let _ = tx.blocking_send(TuiMessage::UsageReloadFailed(e.to_string()));
                    }
                }
            });

            if config.display.refresh_quota {
                let enabled_providers: Vec<String> = config
                    .providers
                    .iter()
                    .filter(|(_, p)| p.enabled)
                    .map(|(k, _)| k.clone())
                    .collect();
                spawn_quota_reload(msg_tx.clone(), enabled_providers);
            }
        }

        // Keeper automated background check
        if config.keeper.enabled
            && state.last_keeper_check.elapsed()
                >= StdDuration::from_secs(config.keeper.check_interval_secs.max(10) as u64)
        {
            state.last_keeper_check = Instant::now();
            let now_local = Local::now();
            let now_utc = chrono::Utc::now();
            let default_agents = tokenpulse_core::config::default_keeper_agents();

            for &agent_id in keeper::KEEPER_AGENTS {
                let Some(agent_cfg) = config
                    .keeper
                    .agents
                    .get(agent_id)
                    .or_else(|| default_agents.get(agent_id))
                else {
                    continue;
                };

                // Check 5h Daily
                if agent_cfg.session_keeper_enabled
                    && !state.keeper_pings_in_progress.contains(agent_id)
                {
                    let last_triggered = state.keeper_daily_triggered.get(agent_id).copied();
                    if tokenpulse_core::keeper::should_trigger_daily(
                        &agent_cfg.daily_wakeup_time,
                        last_triggered,
                        now_local,
                    ) {
                        state
                            .keeper_daily_triggered
                            .insert(agent_id.to_string(), now_local.date_naive());
                        state.keeper_pings_in_progress.insert(agent_id.to_string());
                        let tx = msg_tx.clone();
                        let agent_cfg = agent_cfg.clone();
                        let agent_str = agent_id.to_string();
                        tokio::spawn(async move {
                            let rec = tokenpulse_core::keeper::execute_agent_ping(
                                &agent_str,
                                &agent_cfg,
                                tokenpulse_core::keeper::KeeperTriggerType::Daily,
                            )
                            .await;
                            let _ = tx.send(TuiMessage::KeeperPingCompleted(rec)).await;
                        });
                    }
                }

                // Check Weekly
                if agent_cfg.weekly_keeper_enabled
                    && !state.keeper_pings_in_progress.contains(agent_id)
                {
                    let quota_snapshot = state.quota_snapshots.iter().find(|s| {
                        tokenpulse_core::keeper::matches_keeper_agent(&s.provider, agent_id)
                    });
                    let resets_at =
                        quota_snapshot.and_then(tokenpulse_core::keeper::extract_weekly_reset_time);

                    let last_triggered = state.keeper_weekly_triggered.get(agent_id).copied();
                    if tokenpulse_core::keeper::should_trigger_weekly(
                        resets_at,
                        last_triggered,
                        now_utc,
                    ) {
                        state
                            .keeper_weekly_triggered
                            .insert(agent_id.to_string(), now_utc);
                        state.keeper_pings_in_progress.insert(agent_id.to_string());
                        let tx = msg_tx.clone();
                        let agent_cfg = agent_cfg.clone();
                        let agent_str = agent_id.to_string();
                        tokio::spawn(async move {
                            let rec = tokenpulse_core::keeper::execute_agent_ping(
                                &agent_str,
                                &agent_cfg,
                                tokenpulse_core::keeper::KeeperTriggerType::Weekly,
                            )
                            .await;
                            let _ = tx.send(TuiMessage::KeeperPingCompleted(rec)).await;
                        });
                    }
                }
            }
        }

        if event::poll(std::time::Duration::from_millis(100))? {
            let ev = event::read()?;
            if state.is_refreshing() {
                if let Event::Key(key) = &ev {
                    if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                        break;
                    }
                }
                continue;
            }
            match ev {
                Event::Key(key) => {
                    if state.show_source_filter {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('s') => {
                                state.show_source_filter = false;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                state.source_filter_cursor =
                                    state.source_filter_cursor.saturating_sub(1);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                if state.source_filter_cursor + 1 < state.all_sources.len() {
                                    state.source_filter_cursor += 1;
                                }
                            }
                            KeyCode::Char(' ') | KeyCode::Enter => {
                                state.toggle_source_at_cursor();
                            }
                            KeyCode::Char('a') => {
                                if state.enabled_sources.len() == state.all_sources.len() {
                                    state.enabled_sources.clear();
                                    if let Some(first) = state.all_sources.first() {
                                        state.enabled_sources.insert(first.clone());
                                    }
                                } else {
                                    state.enabled_sources =
                                        state.all_sources.iter().cloned().collect();
                                }
                                state.reset_scroll();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if state.show_help {
                        state.show_help = false;
                        continue;
                    }

                    if matches!(key.code, KeyCode::Char('?')) {
                        state.show_help = true;
                        continue;
                    }

                    if state.model_filter_active {
                        match key.code {
                            KeyCode::Esc | KeyCode::Enter => {
                                state.model_filter_active = false;
                            }
                            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                state.model_filter.clear();
                                state.reset_scroll();
                            }
                            KeyCode::Backspace => {
                                state.model_filter.pop();
                                state.reset_scroll();
                                state.model_filter_active = true;
                            }
                            KeyCode::Char(ch) => {
                                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                                    state.model_filter.push(ch);
                                    state.reset_scroll();
                                    state.model_filter_active = true;
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if matches!(key.code, KeyCode::Char('r'))
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        follow_system_theme(&mut theme, config.display.theme);
                        state.set_refresh_status("Refreshing...", RefreshStatusLevel::Info);
                        state.usage_refresh_in_progress = true;
                        state.quota_refresh_in_progress = config.display.refresh_quota;

                        let reload_clone = Arc::clone(&reload_fn_arc);
                        let tx = msg_tx.clone();
                        tokio::task::spawn_blocking(move || {
                            let mut reload_guard = reload_clone.lock().unwrap();
                            match (*reload_guard)() {
                                Ok((sum, rows)) => {
                                    let _ =
                                        tx.blocking_send(TuiMessage::UsageReloadSuccess(sum, rows));
                                }
                                Err(e) => {
                                    let _ = tx.blocking_send(TuiMessage::UsageReloadFailed(
                                        e.to_string(),
                                    ));
                                }
                            }
                        });

                        if config.display.refresh_quota {
                            let enabled_providers: Vec<String> = config
                                .providers
                                .iter()
                                .filter(|(_, p)| p.enabled)
                                .map(|(k, _)| k.clone())
                                .collect();
                            spawn_quota_reload(msg_tx.clone(), enabled_providers);
                        }

                        continue;
                    }

                    match state.page {
                        UsagePage::Models => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                state.model_filter.clear();
                                state.reset_scroll();
                            }
                            KeyCode::Left | KeyCode::Char('h') => state.previous_page(),
                            KeyCode::Right | KeyCode::Char('l') => state.next_page(),
                            KeyCode::Up | KeyCode::Char('k') => {
                                let frame = terminal.size()?;
                                move_table_selection_for_frame(
                                    &mut state,
                                    &dashboard,
                                    Rect::new(0, 0, frame.width, frame.height),
                                    -1,
                                );
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                let frame = terminal.size()?;
                                move_table_selection_for_frame(
                                    &mut state,
                                    &dashboard,
                                    Rect::new(0, 0, frame.width, frame.height),
                                    1,
                                );
                            }
                            KeyCode::Tab => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    state.previous_page();
                                } else {
                                    state.next_page();
                                }
                            }
                            KeyCode::Char('c') => state.toggle_sort(SortField::Cost),
                            KeyCode::Char('t') => state.toggle_sort(SortField::Tokens),
                            KeyCode::Char('d') => state.toggle_sort(SortField::Date),
                            KeyCode::Char('/') => {
                                state.model_filter_active = true;
                            }
                            KeyCode::Char('s') => {
                                state.show_source_filter = true;
                            }
                            _ => {}
                        },
                        UsagePage::Daily => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Left | KeyCode::Char('h') => state.previous_page(),
                            KeyCode::Right | KeyCode::Char('l') => state.next_page(),
                            KeyCode::Up | KeyCode::Char('k') => {
                                let frame = terminal.size()?;
                                move_table_selection_for_frame(
                                    &mut state,
                                    &dashboard,
                                    Rect::new(0, 0, frame.width, frame.height),
                                    -1,
                                );
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                let frame = terminal.size()?;
                                move_table_selection_for_frame(
                                    &mut state,
                                    &dashboard,
                                    Rect::new(0, 0, frame.width, frame.height),
                                    1,
                                );
                            }
                            KeyCode::Tab => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    state.previous_page();
                                } else {
                                    state.next_page();
                                }
                            }
                            KeyCode::Char('n') => {
                                let today = Local::now().date_naive();
                                let rows = daily::sorted_daily_rows(&dashboard, &state);
                                if let Some(idx) = rows.iter().position(|r| r.date == today) {
                                    state.selected_row = idx;
                                    let frame = terminal.size()?;
                                    let visible = visible_rows_for_page(
                                        state.page,
                                        Rect::new(0, 0, frame.width, frame.height),
                                    );
                                    state.sync_scroll_to_selection(rows.len(), visible);
                                }
                            }
                            KeyCode::Char('c') => state.toggle_sort(SortField::Cost),
                            KeyCode::Char('t') => state.toggle_sort(SortField::Tokens),
                            KeyCode::Char('d') => state.toggle_sort(SortField::Date),
                            KeyCode::Char('v') => {
                                state.daily_metric = match state.daily_metric {
                                    OverviewMetric::Tokens => OverviewMetric::Cost,
                                    OverviewMetric::Cost => OverviewMetric::Tokens,
                                };
                            }
                            KeyCode::Char('s') => {
                                state.show_source_filter = true;
                            }
                            _ => {}
                        },
                        UsagePage::Heatmap => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Left | KeyCode::Char('h') => state.previous_page(),
                            KeyCode::Right | KeyCode::Char('l') => state.next_page(),
                            KeyCode::Tab => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    state.previous_page();
                                } else {
                                    state.next_page();
                                }
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                let offset =
                                    dashboard.move_selection(state.selected_heatmap_date, -1);
                                state.set_selected_heatmap_date(offset);
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                let offset =
                                    dashboard.move_selection(state.selected_heatmap_date, 1);
                                state.set_selected_heatmap_date(offset);
                            }
                            KeyCode::PageUp => {
                                state.scroll_heatmap_detail_up();
                            }
                            KeyCode::PageDown => {
                                let frame = terminal.size()?;
                                let max = heatmap::heatmap_detail_scroll_max(
                                    &dashboard,
                                    &state,
                                    Rect::new(0, 0, frame.width, frame.height),
                                );
                                state.scroll_heatmap_detail_down(max);
                            }
                            KeyCode::Char('n') => {
                                state.set_selected_heatmap_date(Some(Local::now().date_naive()));
                            }
                            KeyCode::Char('t') => {
                                state.set_heatmap_metric(HeatmapMetric::TotalTokens)
                            }
                            KeyCode::Char('c') => state.set_heatmap_metric(HeatmapMetric::Cost),
                            KeyCode::Char('s') => {
                                state.show_source_filter = true;
                            }
                            _ => {}
                        },
                        UsagePage::Quota => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Left | KeyCode::Char('h') => state.previous_page(),
                            KeyCode::Right | KeyCode::Char('l') => state.next_page(),
                            KeyCode::Tab => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    state.previous_page();
                                } else {
                                    state.next_page();
                                }
                            }
                            _ => {}
                        },
                        UsagePage::Keeper => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Left | KeyCode::Char('h') => state.previous_page(),
                            KeyCode::Right | KeyCode::Char('l') => state.next_page(),
                            KeyCode::Tab => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if state.selected_keeper_index > 0 {
                                        state.selected_keeper_index -= 1;
                                    } else {
                                        state.selected_keeper_index =
                                            keeper::KEEPER_AGENTS.len().saturating_sub(1);
                                    }
                                } else {
                                    state.selected_keeper_index = (state.selected_keeper_index + 1)
                                        % keeper::KEEPER_AGENTS.len();
                                }
                            }
                            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                                if state.selected_keeper_index > 0 {
                                    state.selected_keeper_index -= 1;
                                } else {
                                    state.selected_keeper_index =
                                        keeper::KEEPER_AGENTS.len().saturating_sub(1);
                                }
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                state.selected_keeper_index =
                                    (state.selected_keeper_index + 1) % keeper::KEEPER_AGENTS.len();
                            }
                            KeyCode::Char('1') | KeyCode::Char('d') => {
                                let agent = keeper::KEEPER_AGENTS[state.selected_keeper_index];
                                if let Ok(new_state) =
                                    config_manager.toggle_agent_session_keeper(agent)
                                {
                                    if let Some(agent_cfg) = config.keeper.agents.get_mut(agent) {
                                        agent_cfg.session_keeper_enabled = new_state;
                                    }
                                    state.set_refresh_status(
                                        format!(
                                            "{} 5h keeper {}",
                                            keeper::keeper_agent_name(agent),
                                            if new_state { "enabled" } else { "disabled" }
                                        ),
                                        RefreshStatusLevel::Success,
                                    );
                                }
                            }
                            KeyCode::Char('2') | KeyCode::Char('w') => {
                                let agent = keeper::KEEPER_AGENTS[state.selected_keeper_index];
                                if let Ok(new_state) =
                                    config_manager.toggle_agent_weekly_keeper(agent)
                                {
                                    if let Some(agent_cfg) = config.keeper.agents.get_mut(agent) {
                                        agent_cfg.weekly_keeper_enabled = new_state;
                                    }
                                    state.set_refresh_status(
                                        format!(
                                            "{} Weekly keeper {}",
                                            keeper::keeper_agent_name(agent),
                                            if new_state { "enabled" } else { "disabled" }
                                        ),
                                        RefreshStatusLevel::Success,
                                    );
                                }
                            }
                            KeyCode::Char('p') => {
                                let agent = keeper::KEEPER_AGENTS[state.selected_keeper_index];
                                if state.keeper_pings_in_progress.contains(agent) {
                                    state.set_refresh_status(
                                        format!(
                                            "Ping for {} is already in progress...",
                                            keeper::keeper_agent_name(agent)
                                        ),
                                        RefreshStatusLevel::Info,
                                    );
                                } else {
                                    let default_agents =
                                        tokenpulse_core::config::default_keeper_agents();
                                    if let Some(agent_cfg) = config
                                        .keeper
                                        .agents
                                        .get(agent)
                                        .or_else(|| default_agents.get(agent))
                                    {
                                        let agent_cfg = agent_cfg.clone();
                                        let agent_str = agent.to_string();
                                        state.keeper_pings_in_progress.insert(agent_str.clone());
                                        state.set_refresh_status(
                                            format!(
                                                "Executing ping for {}...",
                                                keeper::keeper_agent_name(agent)
                                            ),
                                            RefreshStatusLevel::Info,
                                        );
                                        let tx = msg_tx.clone();
                                        tokio::spawn(async move {
                                            let rec = tokenpulse_core::keeper::execute_agent_ping(
                                                &agent_str,
                                                &agent_cfg,
                                                tokenpulse_core::keeper::KeeperTriggerType::Manual,
                                            )
                                            .await;
                                            let _ =
                                                tx.send(TuiMessage::KeeperPingCompleted(rec)).await;
                                        });
                                    }
                                }
                            }
                            _ => {}
                        },
                        UsagePage::Settings => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Left | KeyCode::Char('h') => state.previous_page(),
                            KeyCode::Right | KeyCode::Char('l') => state.next_page(),
                            KeyCode::Tab => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    state.previous_page();
                                } else {
                                    state.next_page();
                                }
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                let count = settings::settings_row_count(&state);
                                if state.selected_row > 0 {
                                    state.selected_row -= 1;
                                } else {
                                    state.selected_row = count.saturating_sub(1);
                                }
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                let count = settings::settings_row_count(&state);
                                if count > 0 {
                                    state.selected_row = (state.selected_row + 1) % count;
                                }
                            }
                            KeyCode::Char(' ') | KeyCode::Enter => {
                                if let Err(e) = settings::handle_settings_action(
                                    &mut state,
                                    &mut config,
                                    &config_manager,
                                    &mut theme,
                                ) {
                                    state.set_refresh_status(
                                        format!("Save error: {}", e),
                                        RefreshStatusLevel::Error,
                                    );
                                }
                            }
                            _ => {}
                        },
                        _ => match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break,
                            KeyCode::Left | KeyCode::Char('h') => state.previous_page(),
                            KeyCode::Right | KeyCode::Char('l') => state.next_page(),
                            KeyCode::Tab => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    state.previous_page();
                                } else {
                                    state.next_page();
                                }
                            }
                            _ => {}
                        },
                    }
                }
                Event::Mouse(mouse)
                    if !state.show_help
                        && !state.show_source_filter
                        && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) =>
                {
                    let frame = terminal.size()?;
                    let area = Rect::new(0, 0, frame.width, frame.height);
                    let tab_area = dashboard_tab_area(area);
                    if rect_contains(tab_area, mouse.column, mouse.row) {
                        let block = Block::default().borders(Borders::ALL);
                        let inner_area = block.inner(tab_area);
                        let pages = UsagePage::all();
                        let num_tabs = pages.len();
                        let chunks = Layout::default()
                            .direction(Direction::Horizontal)
                            .constraints(vec![Constraint::Ratio(1, num_tabs as u32); num_tabs])
                            .split(inner_area);
                        for (idx, page) in pages.iter().enumerate() {
                            if rect_contains(chunks[idx], mouse.column, mouse.row) {
                                if state.page != *page {
                                    state.page = *page;
                                    state.reset_scroll();
                                }
                                break;
                            }
                        }
                    } else if state.page == UsagePage::Heatmap {
                        let body = dashboard_body_area(area);
                        if let Some(bucket) = heatmap::heatmap_legend_bucket_at_position(
                            body,
                            mouse.column,
                            mouse.row,
                        ) {
                            state.selected_heatmap_legend_bucket = Some(bucket);
                        } else if let Some(date) = heatmap::heatmap_date_at_position(
                            area,
                            &dashboard,
                            mouse.column,
                            mouse.row,
                        ) {
                            state.set_selected_heatmap_date(Some(date));
                        }
                    }
                }
                Event::Mouse(mouse)
                    if !state.show_help
                        && !state.show_source_filter
                        && state.page == UsagePage::Heatmap
                        && matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left)) =>
                {
                    let frame_area = terminal.size()?;
                    let area = Rect::new(0, 0, frame_area.width, frame_area.height);
                    if let Some(date) =
                        heatmap::heatmap_date_at_position(area, &dashboard, mouse.column, mouse.row)
                    {
                        state.set_selected_heatmap_date(Some(date));
                    }
                }
                Event::Mouse(mouse)
                    if !state.show_help
                        && !state.show_source_filter
                        && matches!(mouse.kind, MouseEventKind::ScrollUp) =>
                {
                    if state.page == UsagePage::Keeper {
                        state.keeper_log_scroll = state.keeper_log_scroll.saturating_sub(2);
                    } else if state.page == UsagePage::Heatmap {
                        let frame = terminal.size()?;
                        let frame_area = Rect::new(0, 0, frame.width, frame.height);
                        let body = dashboard_body_area(frame_area);
                        if rect_contains(
                            heatmap::heatmap_day_panel_area(body),
                            mouse.column,
                            mouse.row,
                        ) {
                            state.scroll_heatmap_detail_up();
                        }
                    } else {
                        let frame = terminal.size()?;
                        move_table_selection_for_frame(
                            &mut state,
                            &dashboard,
                            Rect::new(0, 0, frame.width, frame.height),
                            -1,
                        );
                    }
                }
                Event::Mouse(mouse)
                    if !state.show_help
                        && !state.show_source_filter
                        && matches!(mouse.kind, MouseEventKind::ScrollDown) =>
                {
                    if state.page == UsagePage::Keeper {
                        let max_scroll = keeper::keeper_logs_total_lines(&state).saturating_sub(4);
                        state.keeper_log_scroll = (state.keeper_log_scroll + 2).min(max_scroll);
                    } else if state.page == UsagePage::Heatmap {
                        let frame = terminal.size()?;
                        let frame_area = Rect::new(0, 0, frame.width, frame.height);
                        let body = dashboard_body_area(frame_area);
                        if rect_contains(
                            heatmap::heatmap_day_panel_area(body),
                            mouse.column,
                            mouse.row,
                        ) {
                            let max = heatmap::heatmap_detail_scroll_max(
                                &dashboard,
                                &state,
                                Rect::new(0, 0, frame.width, frame.height),
                            );
                            state.scroll_heatmap_detail_down(max);
                        }
                    } else {
                        let frame = terminal.size()?;
                        move_table_selection_for_frame(
                            &mut state,
                            &dashboard,
                            Rect::new(0, 0, frame.width, frame.height),
                            1,
                        );
                    }
                }
                _ => {}
            }
        }
    }

    terminal.show_cursor()?;
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Root layout
// ---------------------------------------------------------------------------

fn render_dashboard(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
    config: &Config,
    config_manager: &ConfigManager,
) {
    f.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);

    let root = dashboard_root_sections(area);

    render_header(f, root[0], dashboard, summary, state, theme);
    render_tabs(f, root[1], state, theme);

    match state.page {
        UsagePage::Overview => {
            overview::render_overview_page(f, root[2], dashboard, summary, state, theme)
        }
        UsagePage::Models => {
            models::render_models_page(f, root[2], dashboard, summary, state, theme)
        }
        UsagePage::Daily => daily::render_daily_page(f, root[2], dashboard, state, theme),
        UsagePage::Heatmap => heatmap::render_heatmap_page(f, root[2], dashboard, state, theme),
        UsagePage::Quota => quota::render_quota_tab(f, root[2], state, config, theme),
        UsagePage::Keeper => keeper::render_keeper_tab(f, root[2], state, config, theme),
        UsagePage::Settings => {
            settings::render_settings_tab(f, root[2], state, config, config_manager, theme)
        }
    }

    render_footer(f, root[3], state, theme, config);
}

pub fn dashboard_body_area(area: Rect) -> Rect {
    dashboard_root_sections(area)[2]
}

pub fn dashboard_tab_area(area: Rect) -> Rect {
    dashboard_root_sections(area)[1]
}

fn dashboard_root_sections(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area)
}

fn render_header(
    f: &mut ratatui::Frame,
    area: Rect,
    _dashboard: &UsageDashboard,
    summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let range_str = match &state.data_date_range {
        Some((first, last)) => {
            format!("{} → {}", first.format("%Y-%m-%d"), last.format("%Y-%m-%d"))
        }
        None => "—".to_string(),
    };

    let title_spans = vec![
        Span::styled("TokenPulse", Style::default().fg(theme.accent).bold()),
        Span::raw(" "),
        Span::styled("Usage Analytics", Style::default().fg(theme.fg).bold()),
    ];

    let mut subtitle_spans = vec![
        Span::styled("Total Cost ", Style::default().fg(theme.dim)),
        Span::styled(
            format!("${:.2}", summary.total_cost),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::raw("  "),
        Span::styled("Total Tokens ", Style::default().fg(theme.dim)),
        Span::styled(
            format_int_commas(summary.total_tokens),
            Style::default().fg(Color::Rgb(52, 211, 153)).bold(),
        ),
        Span::raw("  "),
        Span::styled("Data Range ", Style::default().fg(theme.dim)),
        Span::styled(range_str, Style::default().fg(theme.fg)),
    ];

    if summary.by_provider.len() > 1 {
        subtitle_spans.push(Span::raw("  "));
        subtitle_spans.push(Span::styled("Agents ", Style::default().fg(theme.dim)));
        subtitle_spans.push(Span::styled(
            format!("{}", summary.by_provider.len()),
            Style::default().fg(theme.accent_soft).bold(),
        ));
    }

    let header_lines = vec![Line::from(title_spans), Line::from(subtitle_spans)];
    let header = Paragraph::new(header_lines).wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(header, inner);
}

fn render_tabs(f: &mut ratatui::Frame, area: Rect, state: &UsageState, theme: &Theme) {
    let pages = UsagePage::all();
    let num_tabs = pages.len();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner_area = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, num_tabs as u32); num_tabs])
        .split(inner_area);

    for (idx, page) in pages.iter().enumerate() {
        let title = page.title();
        let chunk = chunks[idx];

        let style = if *page == state.page {
            Style::default().fg(theme.on_accent).bg(theme.accent).bold()
        } else {
            Style::default().fg(theme.dim)
        };

        let paragraph = Paragraph::new(Line::from(vec![Span::styled(title, style)]))
            .alignment(Alignment::Center)
            .style(style);

        f.render_widget(paragraph, chunk);
    }
}

fn key_help(key: &'static str, desc: impl Into<String>, theme: &Theme) -> Vec<Span<'static>> {
    vec![
        Span::styled(key.to_string(), Style::default().fg(theme.accent_soft)),
        Span::styled(
            format!(" {}  ", desc.into()),
            Style::default().fg(theme.dim),
        ),
    ]
}

fn render_footer(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &UsageState,
    theme: &Theme,
    config: &Config,
) {
    let help_line = if state.is_refreshing() {
        Line::from(vec![Span::styled(
            " [LOCKED] Refreshing in progress... Please wait. Actions disabled. ",
            Style::default().fg(Color::Rgb(248, 113, 113)).bold(),
        )])
    } else {
        match state.page {
            UsagePage::Overview => {
                let mut spans = Vec::new();
                spans.push(Span::raw(" "));
                spans.extend(key_help("q", "quit", theme));
                spans.extend(key_help("r", "refresh", theme));
                spans.extend(key_help("←→", "tab", theme));
                spans.extend(key_help("↑↓", "select", theme));
                spans.extend(key_help(
                    "t/c",
                    format!(
                        "metric ({})",
                        match state.overview_metric {
                            OverviewMetric::Tokens => "tokens",
                            OverviewMetric::Cost => "cost",
                        }
                    ),
                    theme,
                ));
                spans.extend(key_help(
                    "s",
                    if state.enabled_sources.len() < state.all_sources.len() {
                        format!(
                            "filter ({}/{})",
                            state.enabled_sources.len(),
                            state.all_sources.len()
                        )
                    } else {
                        "filter".to_string()
                    },
                    theme,
                ));
                spans.extend(key_help("?", "help", theme));
                Line::from(spans)
            }
            UsagePage::Models => {
                let mut spans = Vec::new();
                spans.push(Span::raw(" "));
                spans.extend(key_help("q", "quit", theme));
                spans.extend(key_help("r", "refresh", theme));
                spans.extend(key_help("←→", "tab", theme));
                spans.extend(key_help("↑↓", "select", theme));

                let filter_desc = if state.model_filter.is_empty() {
                    "filter".to_string()
                } else {
                    format!("filter ({})", state.model_filter)
                };
                spans.extend(key_help("/", filter_desc, theme));

                let dir = if state.sort_ascending { "↑" } else { "↓" };
                let field = match state.sort_field {
                    SortField::Cost => "cost",
                    SortField::Tokens => "tokens",
                    SortField::Date => "date",
                };
                spans.extend(key_help("c/t/d", format!("sort ({field} {dir})"), theme));

                spans.extend(key_help(
                    "s",
                    if state.enabled_sources.len() < state.all_sources.len() {
                        format!(
                            "filter ({}/{})",
                            state.enabled_sources.len(),
                            state.all_sources.len()
                        )
                    } else {
                        "filter".to_string()
                    },
                    theme,
                ));
                spans.extend(key_help("?", "help", theme));
                Line::from(spans)
            }
            UsagePage::Daily => {
                let mut spans = Vec::new();
                spans.push(Span::raw(" "));
                spans.extend(key_help("q", "quit", theme));
                spans.extend(key_help("r", "refresh", theme));
                spans.extend(key_help("←→", "tab", theme));
                spans.extend(key_help("↑↓", "select", theme));
                spans.extend(key_help("n", "today", theme));

                let dir = if state.sort_ascending { "↑" } else { "↓" };
                let field = match state.sort_field {
                    SortField::Cost => "cost",
                    SortField::Tokens => "tokens",
                    SortField::Date => "date",
                };
                spans.extend(key_help("c/t/d", format!("sort ({field} {dir})"), theme));

                spans.extend(key_help(
                    "v",
                    format!("metric ({})", state.daily_metric.short_label()),
                    theme,
                ));

                spans.extend(key_help(
                    "s",
                    if state.enabled_sources.len() < state.all_sources.len() {
                        format!(
                            "filter ({}/{})",
                            state.enabled_sources.len(),
                            state.all_sources.len()
                        )
                    } else {
                        "filter".to_string()
                    },
                    theme,
                ));
                spans.extend(key_help("?", "help", theme));
                Line::from(spans)
            }
            UsagePage::Heatmap => {
                let mut spans = Vec::new();
                spans.push(Span::raw(" "));
                spans.extend(key_help("q", "quit", theme));
                spans.extend(key_help("r", "refresh", theme));
                spans.extend(key_help("←→", "tab", theme));
                spans.extend(key_help("n", "today", theme));
                spans.extend(key_help("pgup/pgdn", "detail", theme));
                spans.extend(key_help(
                    "t/c",
                    format!("metric ({})", state.heatmap_metric.short_label()),
                    theme,
                ));
                spans.extend(key_help(
                    "s",
                    if state.enabled_sources.len() < state.all_sources.len() {
                        format!(
                            "filter ({}/{})",
                            state.enabled_sources.len(),
                            state.all_sources.len()
                        )
                    } else {
                        "filter".to_string()
                    },
                    theme,
                ));
                spans.extend(key_help("?", "help", theme));
                Line::from(spans)
            }
            UsagePage::Quota => {
                let mut spans = Vec::new();
                spans.push(Span::raw(" "));
                spans.extend(key_help("q", "quit", theme));
                spans.extend(key_help("r", "refresh", theme));
                spans.extend(key_help("←→", "tab", theme));
                spans.extend(key_help("?", "help", theme));
                Line::from(spans)
            }
            UsagePage::Keeper => {
                let mut spans = Vec::new();
                spans.push(Span::raw(" "));
                spans.extend(key_help("q", "quit", theme));
                spans.extend(key_help("←→/Tab", "select", theme));
                spans.extend(key_help("1/d", "toggle 5h", theme));
                spans.extend(key_help("2/w", "toggle weekly", theme));
                spans.extend(key_help("p", "ping now", theme));
                spans.extend(key_help("?", "help", theme));
                Line::from(spans)
            }
            UsagePage::Settings => {
                let mut spans = Vec::new();
                spans.push(Span::raw(" "));
                spans.extend(key_help("q", "quit", theme));
                spans.extend(key_help("r", "refresh", theme));
                spans.extend(key_help("←→", "tab", theme));
                spans.extend(key_help("?", "help", theme));
                Line::from(spans)
            }
        }
    };

    let countdown_str = {
        let auto_secs = config.display.auto_refresh_secs;
        if auto_secs > 0 {
            // A single shared timer drives both usage and quota, so every page
            // shows the same countdown.
            let elapsed = state.last_refresh.elapsed().as_secs() as u32;
            let remaining = auto_secs.saturating_sub(elapsed);
            let m = remaining / 60;
            let s = remaining % 60;
            Some(format!("Auto-refresh in: {}m {}s", m, s))
        } else {
            None
        }
    };

    // Second line: data range + last refreshed time
    let info_line = {
        let range_str = match &state.data_date_range {
            Some((first, last)) => format!("Data: {} → {}", first, last),
            None => "Data: —".to_string(),
        };
        let refresh_str = match &state.last_refreshed {
            Some(t) => {
                if let Some(cnt) = &countdown_str {
                    format!("Refreshed: {}  |  {}", t.format("%H:%M:%S"), cnt)
                } else {
                    format!("Refreshed: {}", t.format("%H:%M:%S"))
                }
            }
            None => {
                if let Some(cnt) = &countdown_str {
                    cnt.clone()
                } else {
                    String::new()
                }
            }
        };
        let status_text = state
            .refresh_status
            .as_ref()
            .map(|status| status.message.as_str())
            .unwrap_or("");
        let reserved = if status_text.is_empty() {
            0
        } else {
            status_text.chars().count() + 4
        };
        let available = area.width.saturating_sub(reserved as u16) as usize;
        let base = if refresh_str.is_empty() {
            format!(" {}", range_str)
        } else {
            format!(" {}  |  {}", range_str, refresh_str)
        };
        truncate(&base, available.max(1))
    };

    let refresh_feedback = state.refresh_status.as_ref().map(|status| {
        let color = match status.level {
            RefreshStatusLevel::Info => theme.accent,
            RefreshStatusLevel::Success => Color::Rgb(52, 211, 153),
            RefreshStatusLevel::Error => Color::Rgb(248, 113, 113),
        };
        Paragraph::new(status.message.as_str())
            .style(Style::default().fg(color).bold())
            .alignment(Alignment::Right)
    });

    let footer_block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(theme.border));
    let footer_inner = footer_block.inner(area);
    f.render_widget(footer_block, area);

    let footer_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(footer_inner);

    f.render_widget(Paragraph::new(help_line), footer_layout[0]);

    f.render_widget(
        Paragraph::new(info_line).style(Style::default().fg(theme.dim)),
        footer_layout[1],
    );
    if let Some(feedback) = refresh_feedback {
        f.render_widget(feedback, footer_layout[1]);
    }
}

fn render_source_filter_overlay(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &UsageState,
    theme: &Theme,
) {
    let width = 46u16;
    let height = (state.all_sources.len() as u16 + 5).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(Span::styled(
            " Filter Sources ",
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines = Vec::new();
    lines.push(Line::from(Span::styled(
        "Select data sources to display in dashboard:",
        Style::default().fg(theme.fg),
    )));
    lines.push(Line::raw(""));

    for (idx, source) in state.all_sources.iter().enumerate() {
        let enabled = state.enabled_sources.contains(source);
        let active = idx == state.source_filter_cursor;
        let checkbox = if enabled { "[x]" } else { "[ ]" };
        let checkbox_style = if enabled {
            Style::default().fg(Color::Rgb(52, 211, 153))
        } else {
            Style::default().fg(theme.dim)
        };
        let label = display_source_filter_name(source);
        let line_spans = vec![
            Span::styled(
                if active { "> " } else { "  " },
                Style::default().fg(theme.accent),
            ),
            Span::styled(checkbox, checkbox_style),
            Span::raw(" "),
            Span::styled(label, Style::default().fg(theme.fg)),
        ];
        lines.push(Line::from(line_spans));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("space", Style::default().fg(theme.accent_soft).bold()),
        Span::raw(" toggle  "),
        Span::styled("a", Style::default().fg(theme.accent_soft).bold()),
        Span::raw(" toggle all  "),
        Span::styled("s/Esc", Style::default().fg(theme.accent_soft).bold()),
        Span::raw(" close"),
    ]));

    f.render_widget(Paragraph::new(lines), inner);
}

fn render_help_overlay(f: &mut ratatui::Frame, area: Rect, state: &UsageState, theme: &Theme) {
    let mut keybindings = vec![
        ("←→ / Tab", "switch tab"),
        ("↑↓ / j k", "navigate lists"),
        ("s", "toggle source filter"),
        ("r", "refresh raw project sessions"),
        ("?", "toggle this help overlay"),
        ("q / Esc", "quit dashboard"),
    ];

    match state.page {
        UsagePage::Overview => {
            keybindings.extend([
                ("t", "toggle chart view to tokens"),
                ("c", "toggle chart view to cost"),
            ]);
        }
        UsagePage::Models => {
            keybindings.extend([
                ("/", "search filter models by name"),
                ("c", "sort table by Cost value"),
                ("t", "sort table by Tokens value"),
                ("d", "sort table by Date value"),
            ]);
        }
        UsagePage::Daily => {
            keybindings.extend([
                ("n", "scroll list selection to Today"),
                ("c", "sort table by Cost value"),
                ("t", "sort table by Tokens value"),
                ("d", "sort table by Date value"),
                ("v", "toggle Wow metric cost vs tokens"),
            ]);
        }
        UsagePage::Heatmap => {
            keybindings.extend([
                ("n", "scroll list selection to Today"),
                ("t", "toggle heatmap metric to Tokens"),
                ("c", "toggle heatmap metric to Cost"),
                ("pgup/pgdn", "scroll selected day details"),
            ]);
        }
        UsagePage::Keeper => {
            keybindings.extend([
                ("1 / d", "toggle 5h daily wakeup keeper"),
                ("2 / w", "toggle weekly auto-sync keeper"),
                ("p", "trigger immediate test ping"),
                ("←→ / Tab", "switch selected agent card"),
            ]);
        }
        _ => {}
    }

    let key_col_width = keybindings.iter().map(|(k, _)| k.len()).max().unwrap_or(10) as u16 + 2;
    let desc_col_width = keybindings.iter().map(|(_, d)| d.len()).max().unwrap_or(20) as u16 + 2;
    let width = (key_col_width + desc_col_width + 4).min(area.width.saturating_sub(4));
    let height = (keybindings.len() as u16 + 4).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(Span::styled(
            " Dashboard Keybindings ",
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines = Vec::new();
    for (key, desc) in keybindings {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:width$}", key, width = key_col_width as usize),
                Style::default().fg(theme.accent),
            ),
            Span::styled(desc, Style::default().fg(theme.fg)),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![Span::styled(
        " press any key to close",
        Style::default().fg(theme.dim),
    )]));

    f.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.x + area.width && row >= area.y && row < area.y + area.height
}

pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    let mut out = String::new();
    for ch in text.chars().take(width.saturating_sub(1)) {
        out.push(ch);
    }
    out.push('…');
    out
}

pub fn format_compact(value: i64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000_000 {
        format!("{:.2} B", value as f64 / 1_000_000_000.0)
    } else if abs >= 1_000_000 {
        format!("{:.2} M", value as f64 / 1_000_000.0)
    } else if abs >= 1_000 {
        format!("{:.2} K", value as f64 / 1_000.0)
    } else {
        format!("{:.2}  ", value as f64)
    }
}

pub fn format_cost_compact(value: f64) -> String {
    let sign = if value < 0.0 { "-" } else { "" };
    let abs = value.abs();
    if abs >= 1_000.0 {
        let int_part = abs.trunc() as i64;
        let cents = ((abs.fract() * 100.0).round() as i64).abs();
        format!("{}${}.{:02}", sign, format_int_commas(int_part), cents)
    } else {
        format!("{}${:.2}", sign, abs)
    }
}

pub fn format_int_commas(value: i64) -> String {
    let raw = value.to_string();
    let digits = raw.strip_prefix('-').unwrap_or(&raw);
    let mut formatted_rev = String::with_capacity(raw.len() + raw.len() / 3);

    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            formatted_rev.push(',');
        }
        formatted_rev.push(ch);
    }

    let formatted: String = formatted_rev.chars().rev().collect();
    if raw.starts_with('-') {
        format!("-{}", formatted)
    } else {
        formatted
    }
}

pub fn sparkline_text(values: &[u64]) -> String {
    const CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = *values.iter().max().unwrap_or(&0);
    if max == 0 {
        return "▁".repeat(values.len());
    }
    values
        .iter()
        .map(|&v| {
            let idx = ((v as f64 / max as f64) * 7.0).round() as usize;
            CHARS[idx.min(7)]
        })
        .collect()
}

pub fn percent_delta_text(current: f64, prior: Option<f64>) -> String {
    match prior {
        Some(prior) if prior > 0.0 => {
            let pct = (current - prior) / prior * 100.0;
            if pct >= 0.0 {
                format!("{:>+6.1}%↑", pct)
            } else {
                format!("{:>+6.1}%↓", pct)
            }
        }
        _ => "   —  ".to_string(),
    }
}

pub fn percent_delta_color(current: f64, prior: Option<f64>, theme: &Theme) -> Color {
    match prior {
        Some(prior) if prior > 0.0 && current > prior => Color::Rgb(248, 113, 113),
        Some(prior) if prior > 0.0 && current < prior => Color::Rgb(52, 211, 153),
        _ => theme.dim,
    }
}

pub fn metric_style(color: Color, bg: Option<Color>) -> Style {
    let style = Style::default().fg(color);
    if let Some(bg) = bg {
        style.bg(bg)
    } else {
        style
    }
}

pub fn share_percent(value: f64, total: f64) -> f64 {
    if total <= 0.0 {
        return 0.0;
    }
    (value / total * 100.0).clamp(0.0, 100.0)
}

pub fn normalized_scroll_offset(
    offset: usize,
    selected: usize,
    visible: usize,
    total: usize,
) -> usize {
    if total == 0 || visible == 0 {
        return 0;
    }
    let mut normalized = offset.min(total.saturating_sub(visible));
    if selected < normalized {
        normalized = selected;
    } else if selected >= normalized + visible {
        normalized = selected + 1 - visible;
    }
    normalized.min(total.saturating_sub(visible))
}

pub fn selected_row_style(style: Style, selected: bool, theme: &Theme) -> Style {
    if selected {
        style.bg(theme.selected_bg).bold()
    } else {
        style
    }
}

pub fn empty_data_message(state: &UsageState, fallback: &str) -> String {
    if state.enabled_sources.len() < state.all_sources.len() {
        "No data for selected sources".to_string()
    } else if state.all_sources.is_empty() {
        format!(
            "{}\n\nNo sessions found. Run:\n  tokenpulse\nto parse usage data.",
            fallback
        )
    } else {
        fallback.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usage_page_navigation() {
        let pages = UsagePage::all();
        assert_eq!(pages.len(), 7);
        assert_eq!(pages[0], UsagePage::Overview);
        assert_eq!(pages[1], UsagePage::Models);
        assert_eq!(pages[2], UsagePage::Daily);
        assert_eq!(pages[3], UsagePage::Heatmap);
        assert_eq!(pages[4], UsagePage::Quota);
        assert_eq!(pages[5], UsagePage::Keeper);
        assert_eq!(pages[6], UsagePage::Settings);

        assert_eq!(UsagePage::Quota.next(), UsagePage::Keeper);
        assert_eq!(UsagePage::Keeper.next(), UsagePage::Settings);
        assert_eq!(UsagePage::Settings.next(), UsagePage::Overview);

        assert_eq!(UsagePage::Keeper.previous(), UsagePage::Quota);
        assert_eq!(UsagePage::Overview.previous(), UsagePage::Settings);

        assert_eq!(UsagePage::Keeper.title(), "Keeper");
    }
}
