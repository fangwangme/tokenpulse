use crate::tui::theme::Theme;
use crate::tui::widgets::{
    date_at_position, GradientGauge, HeatmapMetric, StackedBarChart, ValueFormat, YearHeatmap,
};
use anyhow::Result;
use chrono::{Datelike, Duration, Local, NaiveDate};
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
    widgets::{Block, Borders, Clear, Paragraph, Tabs},
    Terminal,
};
use std::collections::{BTreeSet, HashMap};
use tokenpulse_core::usage::{
    normalize_model_name, DailyUsageRow, DashboardDay, ModelSummary, UsageSummary,
};

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsagePage {
    Overview,
    Models,
    Daily,
    Heatmap,
}

impl UsagePage {
    fn all() -> [UsagePage; 4] {
        [
            UsagePage::Overview,
            UsagePage::Models,
            UsagePage::Daily,
            UsagePage::Heatmap,
        ]
    }

    fn title(self) -> &'static str {
        match self {
            UsagePage::Overview => "Overview",
            UsagePage::Models => "Models",
            UsagePage::Daily => "Daily",
            UsagePage::Heatmap => "Activity",
        }
    }

    fn next(self) -> Self {
        let pages = Self::all();
        let idx = pages.iter().position(|p| *p == self).unwrap_or(0);
        pages[(idx + 1) % pages.len()]
    }

    fn previous(self) -> Self {
        let pages = Self::all();
        let idx = pages.iter().position(|p| *p == self).unwrap_or(0);
        pages[(idx + pages.len() - 1) % pages.len()]
    }
}

// ---------------------------------------------------------------------------
// Sort
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortField {
    Date,
    Cost,
    Tokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverviewMetric {
    Tokens,
    Cost,
}

impl OverviewMetric {
    fn toggle_to_tokens(&mut self) {
        *self = OverviewMetric::Tokens;
    }

    fn toggle_to_cost(&mut self) {
        *self = OverviewMetric::Cost;
    }

    fn title(self) -> &'static str {
        match self {
            OverviewMetric::Tokens => "Token Usage (60 days)",
            OverviewMetric::Cost => "Cost Usage (60 days)",
        }
    }

    fn value_format(self) -> ValueFormat {
        match self {
            OverviewMetric::Tokens => ValueFormat::CompactNumber,
            OverviewMetric::Cost => ValueFormat::Currency,
        }
    }
}

// ---------------------------------------------------------------------------
// Data structures (kept from original)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct DayBreakdown {
    provider_id: String,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    tokens: i64,
    cost_usd: f64,
    messages: i64,
    sessions: i64,
}

#[derive(Debug, Clone)]
struct DailyStats {
    date: NaiveDate,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    reasoning_tokens: i64,
    total_tokens: i64,
    cost_usd: f64,
    messages: i64,
    sessions: i64,
    providers: HashMap<String, DayBreakdown>,
    models: HashMap<String, DayBreakdown>,
}

impl DailyStats {
    fn from_day(day: &DashboardDay) -> Option<Self> {
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

    fn cache_tokens(&self) -> i64 {
        self.cache_read_tokens + self.cache_write_tokens
    }

    fn filtered(&self, enabled: &BTreeSet<String>) -> Option<Self> {
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

    fn metric_value(&self, metric: HeatmapMetric) -> f64 {
        match metric {
            HeatmapMetric::TotalTokens => self.total_tokens as f64,
            HeatmapMetric::Cost => self.cost_usd,
            HeatmapMetric::InputTokens => self.input_tokens as f64,
            HeatmapMetric::OutputTokens => self.output_tokens as f64,
            HeatmapMetric::CacheTokens => self.cache_tokens() as f64,
            HeatmapMetric::Messages => self.messages as f64,
            HeatmapMetric::Sessions => self.sessions as f64,
        }
    }
}

fn model_source(model_key: &str) -> &str {
    model_key
        .split_once(" / ")
        .map(|(source, _)| source)
        .unwrap_or("")
}

fn model_id_from_key(model_key: &str) -> &str {
    model_key
        .split_once(" / ")
        .map(|(_, model_id)| model_id)
        .unwrap_or(model_key)
}

fn display_source_name(source: &str) -> &'static str {
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

fn format_source_list(source_csv: &str) -> String {
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
struct AggregatedModelSummary {
    providers: BTreeSet<String>,
    sources: BTreeSet<String>,
    cost: f64,
    tokens: i64,
    message_count: usize,
    session_count: usize,
}

#[derive(Debug, Clone)]
struct AgentModelGroup {
    source: String,
    total_cost_usd: f64,
    total_tokens: i64,
    models: Vec<(String, DayBreakdown)>,
}

struct ModelTableRow {
    summary: ModelSummary,
    last_used: Option<NaiveDate>,
}

// ---------------------------------------------------------------------------
// Dashboard aggregate
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct UsageDashboard {
    daily: Vec<DailyStats>,
    total_messages: usize,
    total_sessions: usize,
}

impl UsageDashboard {
    fn build(summary: &UsageSummary, daily_rows: &[DailyUsageRow]) -> Self {
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

        Self {
            daily,
            total_messages: summary.message_count,
            total_sessions: summary.session_count,
        }
    }

    fn latest_date(&self) -> Option<NaiveDate> {
        self.daily.last().map(|d| d.date)
    }

    fn day(&self, date: NaiveDate) -> Option<&DailyStats> {
        self.daily.iter().find(|d| d.date == date)
    }

    fn filtered_daily(&self, enabled: &BTreeSet<String>) -> Vec<DailyStats> {
        self.daily
            .iter()
            .filter_map(|day| day.filtered(enabled))
            .collect()
    }

    fn move_selection(&self, selected: Option<NaiveDate>, offset: isize) -> Option<NaiveDate> {
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

    fn bounds_for_window(
        &self,
        window: HeatmapWindow,
        _selected: Option<NaiveDate>,
    ) -> Option<(NaiveDate, NaiveDate)> {
        let latest = self.latest_date()?;
        match window {
            HeatmapWindow::Recent26Weeks => {
                let end = latest;
                Some((end - Duration::days(7 * 26 - 1), end))
            }
            HeatmapWindow::Recent52Weeks => {
                let end = latest;
                Some((end - Duration::days(7 * 52 - 1), end))
            }
            HeatmapWindow::Past365Days => {
                let end = latest;
                Some((end - Duration::days(364), end))
            }
        }
    }

    fn days_in_window(
        &self,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
        enabled: &BTreeSet<String>,
    ) -> Vec<&DailyStats> {
        let Some((start, end)) = self.bounds_for_window(window, selected) else {
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

    fn points_in_window(
        &self,
        metric: HeatmapMetric,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
        enabled: &BTreeSet<String>,
    ) -> Vec<(NaiveDate, f64)> {
        self.days_in_window(window, selected, enabled)
            .into_iter()
            .filter_map(|d| {
                d.filtered(enabled)
                    .map(|filtered| (filtered.date, filtered.metric_value(metric)))
            })
            .collect()
    }

    fn values_in_window(
        &self,
        metric: HeatmapMetric,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
        enabled: &BTreeSet<String>,
    ) -> Vec<f64> {
        self.days_in_window(window, selected, enabled)
            .into_iter()
            .filter_map(|d| {
                d.filtered(enabled)
                    .map(|filtered| filtered.metric_value(metric))
            })
            .collect()
    }

    fn selected_day_in_window(
        &self,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
        enabled: &BTreeSet<String>,
    ) -> Option<DailyStats> {
        let (start, end) = self.bounds_for_window(window, selected)?;
        let sel = selected?;
        if sel < start || sel > end {
            return None;
        }
        self.day(sel)?.filtered(enabled)
    }

    fn active_days_in_window(
        &self,
        metric: HeatmapMetric,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
        enabled: &BTreeSet<String>,
    ) -> usize {
        self.days_in_window(window, selected, enabled)
            .into_iter()
            .filter_map(|d| d.filtered(enabled))
            .filter(|d| d.metric_value(metric) > 0.0)
            .count()
    }

    fn longest_streak_in_window(
        &self,
        metric: HeatmapMetric,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
        enabled: &BTreeSet<String>,
    ) -> usize {
        let Some((start, end)) = self.bounds_for_window(window, selected) else {
            return 0;
        };
        let values: HashMap<NaiveDate, f64> = self
            .days_in_window(window, selected, enabled)
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

    fn current_streak_in_window(
        &self,
        metric: HeatmapMetric,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
        enabled: &BTreeSet<String>,
    ) -> usize {
        let Some((start, end)) = self.bounds_for_window(window, selected) else {
            return 0;
        };
        let values: HashMap<NaiveDate, f64> = self
            .days_in_window(window, selected, enabled)
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

    fn recent_days(&self, limit: usize) -> Vec<&DailyStats> {
        let start = self.daily.len().saturating_sub(limit);
        self.daily[start..].iter().collect()
    }

    fn all_sources(&self) -> Vec<String> {
        let mut sources = BTreeSet::new();
        for day in &self.daily {
            for provider in day.providers.keys() {
                sources.insert(provider.clone());
            }
        }
        sources.into_iter().collect()
    }

    fn filtered_models(&self, enabled: &BTreeSet<String>) -> Vec<ModelSummary> {
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

    fn model_last_used(&self, model_name: &str, enabled: &BTreeSet<String>) -> Option<NaiveDate> {
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

fn build_agent_model_groups(day: &DailyStats) -> Vec<AgentModelGroup> {
    let mut grouped: HashMap<String, AgentModelGroup> = HashMap::new();

    for (model_key, stats) in &day.models {
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
// Palette / window enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum PaletteMode {
    Tokens,
    Cost,
    Input,
    Output,
    Cache,
    Count,
}

impl PaletteMode {
    fn from_metric(metric: HeatmapMetric) -> Self {
        match metric {
            HeatmapMetric::TotalTokens => PaletteMode::Tokens,
            HeatmapMetric::Cost => PaletteMode::Cost,
            HeatmapMetric::InputTokens => PaletteMode::Input,
            HeatmapMetric::OutputTokens => PaletteMode::Output,
            HeatmapMetric::CacheTokens => PaletteMode::Cache,
            HeatmapMetric::Messages | HeatmapMetric::Sessions => PaletteMode::Count,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeatmapWindow {
    Recent26Weeks,
    Recent52Weeks,
    Past365Days,
}

impl HeatmapWindow {
    fn next(self) -> Self {
        match self {
            HeatmapWindow::Recent26Weeks => HeatmapWindow::Recent52Weeks,
            HeatmapWindow::Recent52Weeks => HeatmapWindow::Past365Days,
            HeatmapWindow::Past365Days => HeatmapWindow::Recent26Weeks,
        }
    }

    fn label(self) -> &'static str {
        match self {
            HeatmapWindow::Recent26Weeks => "Past 26 Weeks",
            HeatmapWindow::Recent52Weeks => "Past 52 Weeks",
            HeatmapWindow::Past365Days => "Past 365 Days",
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

fn compute_data_date_range(dashboard: &UsageDashboard) -> Option<(NaiveDate, NaiveDate)> {
    let first = dashboard.daily.first()?.date;
    let last = dashboard.daily.last()?.date;
    Some((first, last))
}

struct UsageState {
    page: UsagePage,
    overview_metric: OverviewMetric,
    heatmap_metric: HeatmapMetric,
    heatmap_window: HeatmapWindow,
    selected_heatmap_date: Option<NaiveDate>,
    heatmap_detail_scroll: usize,
    scroll_offset: usize,
    selected_row: usize,
    sort_field: SortField,
    sort_ascending: bool,
    model_filter: String,
    model_filter_active: bool,
    // Source filter overlay
    show_source_filter: bool,
    source_filter_cursor: usize,
    all_sources: Vec<String>,
    enabled_sources: BTreeSet<String>,
    // Help overlay
    show_help: bool,
    // Refresh tracking
    last_refreshed: Option<chrono::DateTime<Local>>,
    data_date_range: Option<(NaiveDate, NaiveDate)>,
    // Budget
    monthly_budget_usd: Option<f64>,
}

impl UsageState {
    fn new(dashboard: &UsageDashboard, monthly_budget_usd: Option<f64>) -> Self {
        let all_sources = dashboard.all_sources();
        let enabled_sources: BTreeSet<String> = all_sources.iter().cloned().collect();
        Self {
            page: UsagePage::Overview,
            overview_metric: OverviewMetric::Tokens,
            heatmap_metric: HeatmapMetric::TotalTokens,
            heatmap_window: HeatmapWindow::Past365Days,
            selected_heatmap_date: dashboard.latest_date(),
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
            data_date_range: compute_data_date_range(dashboard),
            monthly_budget_usd,
        }
    }

    fn next_page(&mut self) {
        self.page = self.page.next();
        self.reset_scroll();
    }

    fn previous_page(&mut self) {
        self.page = self.page.previous();
        self.reset_scroll();
    }

    fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
        self.selected_row = 0;
        self.heatmap_detail_scroll = 0;
    }

    fn next_window(&mut self) {
        self.heatmap_window = self.heatmap_window.next();
        self.heatmap_detail_scroll = 0;
    }

    fn move_selection(&mut self, total: usize, visible: usize, delta: isize) {
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

    fn sync_scroll_to_selection(&mut self, total: usize, visible: usize) {
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

    fn toggle_sort(&mut self, field: SortField) {
        if self.sort_field == field {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_field = field;
            self.sort_ascending = false;
        }
        self.reset_scroll();
    }

    fn toggle_source_at_cursor(&mut self) {
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

    fn is_source_enabled(&self, source: &str) -> bool {
        self.enabled_sources.contains(source)
    }

    fn set_selected_heatmap_date(&mut self, date: Option<NaiveDate>) {
        self.selected_heatmap_date = date;
        self.heatmap_detail_scroll = 0;
    }

    fn scroll_heatmap_detail_up(&mut self) {
        self.heatmap_detail_scroll = self.heatmap_detail_scroll.saturating_sub(1);
    }

    fn scroll_heatmap_detail_down(&mut self, max: usize) {
        if self.heatmap_detail_scroll < max {
            self.heatmap_detail_scroll += 1;
        }
    }
}

fn scrollable_item_count(dashboard: &UsageDashboard, state: &UsageState) -> usize {
    match state.page {
        UsagePage::Overview => dashboard.filtered_models(&state.enabled_sources).len(),
        UsagePage::Models => filtered_models_for_state(dashboard, state).len(),
        UsagePage::Daily => dashboard.filtered_daily(&state.enabled_sources).len(),
        UsagePage::Heatmap => 0,
    }
}

fn visible_rows_for_page(page: UsagePage, frame_area: Rect) -> usize {
    let body = dashboard_body_area(frame_area);
    match page {
        UsagePage::Overview => {
            let sections = overview_sections(body);
            table_data_rows(sections[1], 2)
        }
        UsagePage::Models => table_data_rows(body, 1),
        UsagePage::Daily => {
            let sections = daily_sections(body);
            table_data_rows(sections[1], 1)
        }
        UsagePage::Heatmap => 0,
    }
}

fn table_data_rows(area: Rect, non_data_inner_rows: u16) -> usize {
    area.height
        .saturating_sub(2)
        .saturating_sub(non_data_inner_rows) as usize
}

fn move_table_selection_for_frame(
    state: &mut UsageState,
    dashboard: &UsageDashboard,
    frame_area: Rect,
    delta: isize,
) {
    let total = scrollable_item_count(dashboard, state);
    let visible = visible_rows_for_page(state.page, frame_area);
    state.move_selection(total, visible, delta);
}

fn heatmap_detail_scroll_max(
    dashboard: &UsageDashboard,
    state: &UsageState,
    frame_area: Rect,
) -> usize {
    let body = dashboard_body_area(frame_area);
    let detail_area = heatmap_day_panel_area(body);
    let selected_day = dashboard.selected_day_in_window(
        state.heatmap_window,
        state.selected_heatmap_date,
        &state.enabled_sources,
    );
    let total_lines = heatmap_day_panel_line_count(selected_day.as_ref());
    let visible_lines = detail_area.height.saturating_sub(2) as usize;
    total_lines.saturating_sub(visible_lines)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run<F>(
    mut summary: UsageSummary,
    daily_rows: Vec<DailyUsageRow>,
    monthly_budget_usd: Option<f64>,
    mut reload: F,
) -> Result<()>
where
    F: FnMut() -> Result<(UsageSummary, Vec<DailyUsageRow>)>,
{
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let mut theme = Theme::auto();
    let mut dashboard = UsageDashboard::build(&summary, &daily_rows);
    let mut state = UsageState::new(&dashboard, monthly_budget_usd);

    loop {
        terminal.draw(|f| {
            let size = f.area();
            render_dashboard(f, size, &dashboard, &summary, &state, &theme);
            if state.show_source_filter {
                render_source_filter_overlay(f, size, &state, &theme);
            }
            if state.show_help {
                render_help_overlay(f, size, &state, &theme);
            }
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
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
                        // any key closes help
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

                    if matches!(key.code, KeyCode::Char('b'))
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        theme = theme.toggled();
                        continue;
                    }

                    if matches!(key.code, KeyCode::Char('r'))
                        && !key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        if let Ok((new_summary, new_daily_rows)) = reload() {
                            let new_dashboard =
                                UsageDashboard::build(&new_summary, &new_daily_rows);
                            let saved_page = state.page;
                            let saved_sort_field = state.sort_field;
                            let saved_sort_ascending = state.sort_ascending;
                            let saved_model_filter = std::mem::take(&mut state.model_filter);
                            let saved_sources = state.enabled_sources.clone();
                            let saved_heatmap_date = state.selected_heatmap_date;
                            let saved_selected_row = state.selected_row;
                            summary = new_summary;
                            dashboard = new_dashboard;
                            state = UsageState::new(&dashboard, state.monthly_budget_usd);
                            state.page = saved_page;
                            state.sort_field = saved_sort_field;
                            state.sort_ascending = saved_sort_ascending;
                            state.model_filter = saved_model_filter;
                            let new_all: BTreeSet<String> =
                                state.all_sources.iter().cloned().collect();
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
                            KeyCode::Char('c') => state.toggle_sort(SortField::Cost),
                            KeyCode::Char('t') => state.toggle_sort(SortField::Tokens),
                            KeyCode::Char('d') => state.toggle_sort(SortField::Date),
                            KeyCode::Char('T') => {
                                let today = Local::now().date_naive();
                                let mut days = dashboard.filtered_daily(&state.enabled_sources);
                                match state.sort_field {
                                    SortField::Date => days.sort_by_key(|d| d.date),
                                    SortField::Cost => days.sort_by(|a, b| {
                                        a.cost_usd
                                            .partial_cmp(&b.cost_usd)
                                            .unwrap_or(std::cmp::Ordering::Equal)
                                    }),
                                    SortField::Tokens => days.sort_by_key(|d| d.total_tokens),
                                }
                                if !state.sort_ascending {
                                    days.reverse();
                                }
                                if let Some(idx) = days.iter().position(|d| d.date == today) {
                                    let frame = terminal.size()?;
                                    let visible = visible_rows_for_page(
                                        UsagePage::Daily,
                                        Rect::new(0, 0, frame.width, frame.height),
                                    );
                                    state.selected_row = idx;
                                    state.sync_scroll_to_selection(days.len(), visible);
                                }
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
                            KeyCode::Up | KeyCode::Char('k') => {
                                state.set_selected_heatmap_date(
                                    dashboard.move_selection(state.selected_heatmap_date, -1),
                                );
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                state.set_selected_heatmap_date(
                                    dashboard.move_selection(state.selected_heatmap_date, 1),
                                );
                            }
                            KeyCode::Tab => {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    state.previous_page();
                                } else {
                                    state.next_page();
                                }
                            }
                            KeyCode::PageUp => state.scroll_heatmap_detail_up(),
                            KeyCode::PageDown => {
                                let frame = terminal.size()?;
                                state.scroll_heatmap_detail_down(heatmap_detail_scroll_max(
                                    &dashboard,
                                    &state,
                                    Rect::new(0, 0, frame.width, frame.height),
                                ));
                            }
                            KeyCode::Char('w') => state.next_window(),
                            KeyCode::Char('t') => {
                                state.heatmap_metric = HeatmapMetric::TotalTokens;
                                state.heatmap_detail_scroll = 0;
                            }
                            KeyCode::Char('c') => {
                                state.heatmap_metric = HeatmapMetric::Cost;
                                state.heatmap_detail_scroll = 0;
                            }
                            KeyCode::Char('i') => {
                                state.heatmap_metric = HeatmapMetric::InputTokens;
                                state.heatmap_detail_scroll = 0;
                            }
                            KeyCode::Char('o') => {
                                state.heatmap_metric = HeatmapMetric::OutputTokens;
                                state.heatmap_detail_scroll = 0;
                            }
                            KeyCode::Char('x') => {
                                state.heatmap_metric = HeatmapMetric::CacheTokens;
                                state.heatmap_detail_scroll = 0;
                            }
                            KeyCode::Char('m') => {
                                state.heatmap_metric = HeatmapMetric::Messages;
                                state.heatmap_detail_scroll = 0;
                            }
                            KeyCode::Char('n') => {
                                state.heatmap_metric = HeatmapMetric::Sessions;
                                state.heatmap_detail_scroll = 0;
                            }
                            KeyCode::Char('s') => {
                                state.show_source_filter = true;
                            }
                            KeyCode::Char('T') => {
                                state.set_selected_heatmap_date(Some(Local::now().date_naive()));
                            }
                            _ => {}
                        },
                        UsagePage::Overview => match key.code {
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
                            KeyCode::Char('s') => {
                                state.show_source_filter = true;
                            }
                            KeyCode::Char('t') => state.overview_metric.toggle_to_tokens(),
                            KeyCode::Char('c') => state.overview_metric.toggle_to_cost(),
                            _ => {}
                        },
                    }
                }
                Event::Mouse(mouse)
                    if !state.show_help
                        && !state.show_source_filter
                        && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                        && mouse.row >= 4
                        && mouse.row <= 6 =>
                {
                    // Tab bar click — header=4 rows, tab bar=3 rows (rows 4-6)
                    let pages = UsagePage::all();
                    let mut x = 1u16; // skip left border
                    for page in pages.iter() {
                        let title = format!(" {} ", page.title());
                        let end_x = x + title.len() as u16;
                        if mouse.column >= x && mouse.column < end_x {
                            if state.page != *page {
                                state.page = *page;
                                state.reset_scroll();
                            }
                            break;
                        }
                        x = end_x;
                    }
                }
                Event::Mouse(mouse)
                    if !state.show_source_filter
                        && state.page == UsagePage::Heatmap
                        && matches!(
                            mouse.kind,
                            MouseEventKind::Down(MouseButton::Left)
                                | MouseEventKind::Drag(MouseButton::Left)
                        ) =>
                {
                    let frame_area = terminal.size()?;
                    let area = Rect::new(0, 0, frame_area.width, frame_area.height);
                    if let Some(date) =
                        heatmap_date_at_position(area, &dashboard, &state, mouse.column, mouse.row)
                    {
                        state.set_selected_heatmap_date(Some(date));
                    }
                }
                Event::Mouse(mouse)
                    if !state.show_source_filter
                        && matches!(mouse.kind, MouseEventKind::ScrollUp) =>
                {
                    if state.page == UsagePage::Heatmap {
                        let frame = terminal.size()?;
                        let frame_area = Rect::new(0, 0, frame.width, frame.height);
                        let body = dashboard_body_area(frame_area);
                        if rect_contains(heatmap_day_panel_area(body), mouse.column, mouse.row) {
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
                    if !state.show_source_filter
                        && matches!(mouse.kind, MouseEventKind::ScrollDown) =>
                {
                    if state.page == UsagePage::Heatmap {
                        let frame = terminal.size()?;
                        let frame_area = Rect::new(0, 0, frame.width, frame.height);
                        let body = dashboard_body_area(frame_area);
                        if rect_contains(heatmap_day_panel_area(body), mouse.column, mouse.row) {
                            state.scroll_heatmap_detail_down(heatmap_detail_scroll_max(
                                &dashboard, &state, frame_area,
                            ));
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
) {
    f.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);

    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(f, root[0], dashboard, summary, state, theme);
    render_tabs(f, root[1], state, theme);

    match state.page {
        UsagePage::Overview => render_overview_page(f, root[2], dashboard, summary, state, theme),
        UsagePage::Models => {
            render_models_page(f, root[2], dashboard, summary, state, theme);
        }
        UsagePage::Daily => {
            render_daily_page(f, root[2], dashboard, state, theme);
        }
        UsagePage::Heatmap => render_heatmap_page(f, root[2], dashboard, state, theme),
    }

    render_footer(f, root[3], state, theme);
}

fn dashboard_body_area(area: Rect) -> Rect {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area)[2]
}

fn overview_sections(area: Rect) -> std::rc::Rc<[Rect]> {
    let chart_height = (area.height.saturating_mul(2) / 3).max(8).min(area.height);
    let model_height = area.height.saturating_sub(chart_height);

    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(chart_height),
            Constraint::Length(model_height),
        ])
        .split(area)
}

fn overview_sections_with_budget(area: Rect, budget_height: u16) -> std::rc::Rc<[Rect]> {
    let remaining = area.height.saturating_sub(budget_height);
    let chart_height = (remaining.saturating_mul(2) / 3).max(8).min(remaining);
    let model_height = remaining.saturating_sub(chart_height);

    if budget_height > 0 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(chart_height),
                Constraint::Length(budget_height),
                Constraint::Length(model_height),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(chart_height),
                Constraint::Length(model_height),
            ])
            .split(area)
    }
}

fn daily_sections(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(8)])
        .split(area)
}

fn heatmap_grid_area(area: Rect) -> Rect {
    let sections = heatmap_sections(area);

    Block::default().borders(Borders::ALL).inner(sections[0])
}

fn heatmap_day_panel_area(area: Rect) -> Rect {
    let sections = heatmap_sections(area);
    let info = heatmap_info_sections(sections[2]);

    info[1]
}

fn heatmap_sections(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(12),
            Constraint::Length(3),
            Constraint::Min(10),
        ])
        .split(area)
}

fn heatmap_info_sections(area: Rect) -> std::rc::Rc<[Rect]> {
    if area.width >= 100 && area.height >= 10 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(40), Constraint::Min(40)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(8), Constraint::Min(6)])
            .split(area)
    }
}

fn heatmap_date_at_position(
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    column: u16,
    row: u16,
) -> Option<NaiveDate> {
    let body = dashboard_body_area(area);
    let grid_area = heatmap_grid_area(body);
    let bounds = dashboard.bounds_for_window(state.heatmap_window, state.selected_heatmap_date);
    date_at_position(grid_area, bounds, column, row)
}

fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x && column < area.x + area.width && row >= area.y && row < area.y + area.height
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn render_header(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let title = Line::from(vec![
        Span::styled("TokenPulse", Style::default().fg(theme.accent).bold()),
        Span::raw(" "),
        Span::styled("Usage Dashboard", Style::default().fg(theme.fg).bold()),
        Span::raw("  "),
        Span::styled(
            format!("{} view", state.page.title()),
            Style::default().fg(theme.dim),
        ),
    ]);

    let subtitle = Line::from(vec![
        Span::styled(
            format!("{} cost", format_cost_compact(summary.total_cost)),
            Style::default().fg(theme.gauge_high),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} tokens", format_compact(summary.total_tokens)),
            Style::default().fg(theme.codex),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{} messages",
                format_compact(dashboard.total_messages as i64)
            ),
            Style::default().fg(theme.claude),
        ),
        Span::raw("  "),
        Span::styled(
            format!(
                "{} sessions",
                format_compact(dashboard.total_sessions as i64)
            ),
            Style::default().fg(theme.opencode),
        ),
    ]);

    let header = Paragraph::new(vec![title, subtitle])
        .style(Style::default().fg(theme.fg))
        .alignment(Alignment::Left)
        .wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(header, inner);
}

// ---------------------------------------------------------------------------
// Tabs
// ---------------------------------------------------------------------------

fn render_tabs(f: &mut ratatui::Frame, area: Rect, state: &UsageState, theme: &Theme) {
    let pages = UsagePage::all();
    let titles = pages.iter().map(|page| {
        Span::styled(
            format!(" {} ", page.title()),
            if *page == state.page {
                Style::default().fg(theme.on_accent).bg(theme.accent).bold()
            } else {
                Style::default().fg(theme.dim)
            },
        )
    });

    let tabs = Tabs::new(titles)
        .select(pages.iter().position(|p| *p == state.page).unwrap_or(0))
        .divider(Span::raw(""))
        .highlight_style(Style::default())
        .style(Style::default())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border)),
        );

    f.render_widget(tabs, area);
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

fn render_footer(f: &mut ratatui::Frame, area: Rect, state: &UsageState, theme: &Theme) {
    let filter_hint = if state.enabled_sources.len() < state.all_sources.len() {
        format!(
            " | s filter ({}/{})",
            state.enabled_sources.len(),
            state.all_sources.len()
        )
    } else {
        " | s filter".to_string()
    };
    let help = match state.page {
        UsagePage::Overview => format!(
            " q quit | r refresh | b theme ({}) | ←→ tab | ↑↓ select | t/c metric ({}) | ? help{}",
            theme.mode.label(),
            match state.overview_metric {
                OverviewMetric::Tokens => "tokens",
                OverviewMetric::Cost => "cost",
            },
            filter_hint
        ),
        UsagePage::Models => {
            let dir = if state.sort_ascending { "↑" } else { "↓" };
            let field = match state.sort_field {
                SortField::Cost => "cost",
                SortField::Tokens => "tokens",
                SortField::Date => "date",
            };
            let filter = if state.model_filter.is_empty() {
                String::new()
            } else {
                format!(" | / filter ({})", state.model_filter)
            };
            format!(
                " q quit | r refresh | b theme ({}) | ←→ tab | ↑↓ select | / filter | ctrl+l clear | c/t/d sort ({} {}) | ? help{}{}",
                theme.mode.label(), field, dir, filter, filter_hint
            )
        }
        UsagePage::Daily => {
            let dir = if state.sort_ascending { "↑" } else { "↓" };
            let field = match state.sort_field {
                SortField::Cost => "cost",
                SortField::Tokens => "tokens",
                SortField::Date => "date",
            };
            format!(
                " q quit | r refresh | b theme ({}) | ←→ tab | ↑↓ select | T today | c/t/d sort ({} {}) | ? help{}",
                theme.mode.label(), field, dir, filter_hint
            )
        }
        UsagePage::Heatmap => format!(
            " q quit | r refresh | b theme ({}) | ←→ tab | ↑↓ day | T today | pgup/pgdn detail | w window ({}) | t/c/i/o/x/m/n metric ({}) | ? help{}",
            theme.mode.label(),
            state.heatmap_window.label(),
            state.heatmap_metric.short_label(),
            filter_hint
        ),
    };

    // Second line: data range + last refreshed time
    let info_line = {
        let range_str = match &state.data_date_range {
            Some((first, last)) => format!("Data: {} → {}", first, last),
            None => "Data: —".to_string(),
        };
        let refresh_str = match &state.last_refreshed {
            Some(t) => format!("Refreshed: {}", t.format("%H:%M:%S")),
            None => String::new(),
        };
        if refresh_str.is_empty() {
            format!(" {}", range_str)
        } else {
            format!(" {}  |  {}", range_str, refresh_str)
        }
    };

    let footer_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Length(1)])
        .split(area);

    let footer1 = Paragraph::new(help)
        .style(Style::default().fg(theme.dim))
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(footer1, footer_chunks[0]);

    let footer2 = Paragraph::new(info_line).style(Style::default().fg(theme.dim));
    f.render_widget(footer2, footer_chunks[1]);
}

// ---------------------------------------------------------------------------
// Source filter overlay
// ---------------------------------------------------------------------------

fn render_source_filter_overlay(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &UsageState,
    theme: &Theme,
) {
    let height = (state.all_sources.len() as u16 + 4).min(area.height.saturating_sub(4));
    let width = 36u16.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Source Filter ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled(" ↑↓ ", Style::default().fg(theme.dim)),
        Span::styled("navigate", Style::default().fg(theme.dim)),
        Span::raw("  "),
        Span::styled("⏎/space", Style::default().fg(theme.dim)),
        Span::raw(" "),
        Span::styled("toggle", Style::default().fg(theme.dim)),
    ]));

    for (i, source) in state.all_sources.iter().enumerate() {
        let enabled = state.is_source_enabled(source);
        let selected = i == state.source_filter_cursor;
        let checkbox = if enabled { "[✓]" } else { "[ ]" };
        let color = theme.provider_color(source);
        let style = if selected {
            Style::default().fg(theme.on_accent).bg(color).bold()
        } else if enabled {
            Style::default().fg(color)
        } else {
            Style::default().fg(theme.dim)
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", checkbox), style),
            Span::styled(source.clone(), style),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" a ", Style::default().fg(theme.dim)),
        Span::styled("toggle all", Style::default().fg(theme.dim)),
        Span::raw("  "),
        Span::styled("s/esc", Style::default().fg(theme.dim)),
        Span::raw(" "),
        Span::styled("close", Style::default().fg(theme.dim)),
    ]));

    let para = Paragraph::new(lines).style(Style::default().fg(theme.fg));
    f.render_widget(para, inner);
}

// ---------------------------------------------------------------------------
// Help overlay
// ---------------------------------------------------------------------------

fn render_help_overlay(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &UsageState,
    theme: &Theme,
) {
    let keybindings: &[(&str, &str)] = match state.page {
        UsagePage::Overview => &[
            ("←→", "switch tab"),
            ("↑↓", "select model"),
            ("t / c", "toggle tokens / cost metric"),
            ("r", "refresh data"),
            ("s", "source filter"),
            ("b", "toggle theme"),
            ("q / Esc", "quit"),
        ],
        UsagePage::Models => &[
            ("←→", "switch tab"),
            ("↑↓", "select model"),
            ("c / t / d", "sort by cost / tokens / date"),
            ("/", "filter models"),
            ("Ctrl+L", "clear filter"),
            ("r", "refresh data"),
            ("s", "source filter"),
            ("b", "toggle theme"),
            ("q / Esc", "quit"),
        ],
        UsagePage::Daily => &[
            ("←→", "switch tab"),
            ("↑↓", "select row"),
            ("T", "jump to today"),
            ("c / t / d", "sort by cost / tokens / date"),
            ("r", "refresh data"),
            ("s", "source filter"),
            ("b", "toggle theme"),
            ("q / Esc", "quit"),
        ],
        UsagePage::Heatmap => &[
            ("←→", "switch tab"),
            ("↑↓", "select day"),
            ("T", "jump to today"),
            ("PgUp / PgDn", "scroll detail panel"),
            ("w", "cycle time window"),
            ("t / c / i / o / x / m / n", "switch metric"),
            ("r", "refresh data"),
            ("s", "source filter"),
            ("b", "toggle theme"),
            ("q / Esc", "quit"),
        ],
    };

    let key_col_width = keybindings.iter().map(|(k, _)| k.len()).max().unwrap_or(10) as u16 + 2;
    let desc_col_width = keybindings.iter().map(|(_, d)| d.len()).max().unwrap_or(20) as u16 + 2;
    let width = (key_col_width + desc_col_width + 4).min(area.width.saturating_sub(4));
    let height = (keybindings.len() as u16 + 4).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    f.render_widget(Clear, popup);

    let tab_name = match state.page {
        UsagePage::Overview => "Overview",
        UsagePage::Models => "Models",
        UsagePage::Daily => "Daily",
        UsagePage::Heatmap => "Activity",
    };
    let block = Block::default()
        .title(format!(" Keybindings — {} ", tab_name))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines: Vec<Line> = Vec::new();
    for (key, desc) in keybindings {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:width$}", key, width = key_col_width as usize),
                Style::default().fg(theme.accent),
            ),
            Span::styled(*desc, Style::default().fg(theme.fg)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![Span::styled(
        " any key to close",
        Style::default().fg(theme.dim),
    )]));

    let para = Paragraph::new(lines).style(Style::default().fg(theme.fg));
    f.render_widget(para, inner);
}

fn render_overview_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
) {
    let budget_height = if state.monthly_budget_usd.is_some() { 3u16 } else { 0u16 };
    let sections = overview_sections_with_budget(area, budget_height);

    render_overview_chart(f, sections[0], dashboard, state, theme);
    if budget_height > 0 {
        render_budget_gauge(f, sections[1], summary, state, theme);
    }
    render_overview_top_models(f, sections[if budget_height > 0 { 2 } else { 1 }], dashboard, summary, state, theme);
}

fn render_overview_chart(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", state.overview_metric.title()),
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let recent = dashboard.recent_days(60);
    if recent.is_empty() {
        f.render_widget(
            Paragraph::new("No usage data").style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(1)])
        .split(inner);

    let chart_data: Vec<(f64, HashMap<&str, f64>)> = recent
        .iter()
        .map(|day| {
            let mut segments = HashMap::new();
            let mut total = 0.0;
            for (model_key, stats) in &day.models {
                let source = model_source(model_key);
                if !state.is_source_enabled(source) {
                    continue;
                }
                let company = theme.company_key_for(
                    model_id_from_key(model_key),
                    Some(stats.provider_id.as_str()),
                );
                let value = match state.overview_metric {
                    OverviewMetric::Tokens => stats.tokens as f64,
                    OverviewMetric::Cost => stats.cost_usd,
                };
                *segments.entry(company).or_insert(0.0) += value;
                total += value;
            }
            (total, segments)
        })
        .collect();

    if chart_data.iter().all(|(total, _)| *total <= 0.0) {
        f.render_widget(
            Paragraph::new(empty_data_message(state, "No usage data"))
                .style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    let chart = StackedBarChart::new(&chart_data)
        .color("openai", theme.company_color("openai"))
        .color("google", theme.company_color("google"))
        .color("anthropic", theme.company_color("anthropic"))
        .color("other", theme.company_color("other"))
        .value_format(state.overview_metric.value_format());
    f.render_widget(chart, sections[0]);

    // Legend: only show enabled sources
    let mut legend_spans = vec![Span::styled(
        recent
            .first()
            .map(|d| d.date.format("%m-%d").to_string())
            .unwrap_or_default(),
        Style::default().fg(theme.dim),
    )];
    legend_spans.push(Span::raw("  "));

    let provider_legend: &[(&str, &str, Color)] = &[
        ("anthropic", "Anthropic", theme.company_color("anthropic")),
        ("openai", "OpenAI", theme.company_color("openai")),
        ("google", "Google", theme.company_color("google")),
        ("other", "Others", theme.company_color("other")),
    ];
    for (_, label, color) in provider_legend {
        legend_spans.push(Span::styled(
            format!("● {}", label),
            Style::default().fg(*color),
        ));
        legend_spans.push(Span::raw(" "));
    }
    legend_spans.push(Span::raw(" "));
    legend_spans.push(Span::styled(
        recent
            .last()
            .map(|d| d.date.format("%m-%d").to_string())
            .unwrap_or_default(),
        Style::default().fg(theme.dim),
    ));

    f.render_widget(Paragraph::new(Line::from(legend_spans)), sections[1]);
}

fn render_budget_gauge(
    f: &mut ratatui::Frame,
    area: Rect,
    summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
) {
    let budget = match state.monthly_budget_usd {
        Some(b) if b > 0.0 => b,
        _ => return,
    };

    let now = Local::now().date_naive();
    let month_start = NaiveDate::from_ymd_opt(now.year(), now.month(), 1).unwrap_or(now);
    let days_elapsed = (now - month_start).num_days().max(1) as f64;
    let days_in_month = {
        let next = if now.month() == 12 {
            NaiveDate::from_ymd_opt(now.year() + 1, 1, 1)
        } else {
            NaiveDate::from_ymd_opt(now.year(), now.month() + 1, 1)
        };
        next.map(|d| (d - month_start).num_days() as f64)
            .unwrap_or(30.0)
    };

    let month_cost = summary.total_cost;
    let avg_daily = month_cost / days_elapsed;
    let projected = avg_daily * days_in_month;
    let pct = (month_cost / budget * 100.0).min(200.0);

    let color = if pct >= 100.0 {
        theme.gauge_high
    } else if pct >= 80.0 {
        theme.gauge_mid
    } else {
        theme.gauge_low
    };

    let projected_str = if projected > budget {
        format!(
            " | Projected: {} (+{} over)",
            format_cost_compact(projected),
            format_cost_compact(projected - budget)
        )
    } else {
        format!(
            " | Projected: {} ({} left)",
            format_cost_compact(projected),
            format_cost_compact(budget - projected)
        )
    };

    let label = format!(
        "Monthly Budget  {} / {}{}",
        format_cost_compact(month_cost),
        format_cost_compact(budget),
        projected_str
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let gauge = GradientGauge::new(&label, pct.min(100.0))
        .width(inner.width.saturating_sub(2) as usize)
        .color(color);
    f.render_widget(gauge, inner);
}

fn render_overview_top_models(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    _summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
) {
    let filtered = dashboard.filtered_models(&state.enabled_sources);
    let total_rows = filtered.len();
    let block = Block::default()
        .title(Span::styled(
            format!(
                " Top Models ({}) {} ",
                total_rows,
                scroll_window_label(
                    state.scroll_offset,
                    overview_model_visible_rows(area.height),
                    total_rows
                )
            ),
            Style::default().fg(theme.accent_soft).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if filtered.is_empty() {
        f.render_widget(
            Paragraph::new(empty_data_message(state, "No model data"))
                .style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    let data_rows_visible = inner.height as usize;
    let data_rows_visible = data_rows_visible.saturating_sub(2);
    let selected_row = state.selected_row.min(filtered.len().saturating_sub(1));
    let offset = normalized_scroll_offset(
        state.scroll_offset,
        selected_row,
        data_rows_visible,
        filtered.len(),
    );
    let total_cost = filtered.iter().map(|model| model.cost).sum::<f64>();
    let total_width = inner.width as usize;
    let pct_width = 7usize;
    let cost_width = 8usize;
    let tokens_width = 9usize;
    let fixed_width = tokens_width + cost_width + pct_width + 4;
    let remaining = total_width.saturating_sub(fixed_width);
    let agent_width = (remaining * 2 / 5).clamp(26, 40);
    let model_width = total_width
        .saturating_sub(agent_width + fixed_width)
        .clamp(18, 36);

    let mut lines = Vec::with_capacity(data_rows_visible + 2);
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<model_width$}", "Model"),
            Style::default().fg(theme.accent_soft).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:<agent_width$}", "Agent"),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>tokens_width$}", "Tokens"),
            Style::default().fg(Color::Rgb(52, 211, 153)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>cost_width$}", "Cost"),
            Style::default().fg(Color::Rgb(250, 204, 21)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>pct_width$}", "%"),
            Style::default().fg(Color::Rgb(96, 165, 250)).bold(),
        ),
    ]));

    for (row_idx, model) in filtered
        .iter()
        .enumerate()
        .skip(offset)
        .take(data_rows_visible)
    {
        let provider_hint = model.provider.split(',').next();
        let color = theme.model_color_for(&model.model, provider_hint);
        let pct = cost_share_percent(model.cost, total_cost);
        let selected = row_idx == selected_row;
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<model_width$}", truncate(&model.model, model_width)),
                selected_row_style(Style::default().fg(color), selected, theme),
            ),
            Span::raw(" "),
            Span::styled(
                format!(
                    "{:<agent_width$}",
                    truncate(&format_source_list(&model.source), agent_width)
                ),
                selected_row_style(Style::default().fg(theme.accent), selected, theme),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:>tokens_width$}", format_compact(model.tokens)),
                selected_row_style(
                    Style::default().fg(Color::Rgb(52, 211, 153)),
                    selected,
                    theme,
                ),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:>cost_width$}", format_cost_compact(model.cost)),
                selected_row_style(
                    Style::default().fg(Color::Rgb(250, 204, 21)),
                    selected,
                    theme,
                ),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:>pct_width$}", format!("{:.2}%", pct)),
                selected_row_style(
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                    selected,
                    theme,
                ),
            ),
        ]));
    }

    lines.push(Line::from(vec![Span::styled(
        overview_scroll_hint(offset, data_rows_visible, total_rows),
        Style::default().fg(theme.dim),
    )]));

    f.render_widget(Paragraph::new(lines), inner);
}

// ===========================================================================
// TAB 2: Models
// ===========================================================================

fn render_models_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    _summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
) {
    let title = if state.model_filter.is_empty() {
        " Models ".to_string()
    } else {
        format!(" Models /{} ", state.model_filter)
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let filtered = filtered_model_rows_for_state(dashboard, state);

    if filtered.is_empty() {
        let empty = if state.model_filter.is_empty() {
            empty_data_message(state, "No model data")
        } else {
            format!("No models match /{}", state.model_filter)
        };
        f.render_widget(
            Paragraph::new(empty).style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    // Sort models
    let mut models: Vec<&ModelTableRow> = filtered.iter().collect();
    match state.sort_field {
        SortField::Cost => {
            models.sort_by(|a, b| {
                a.summary
                    .cost
                    .partial_cmp(&b.summary.cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.summary.model.cmp(&b.summary.model))
            });
            if !state.sort_ascending {
                models.reverse();
            }
        }
        SortField::Tokens => {
            models.sort_by(|a, b| {
                a.summary
                    .tokens
                    .cmp(&b.summary.tokens)
                    .then_with(|| a.summary.model.cmp(&b.summary.model))
            });
            if !state.sort_ascending {
                models.reverse();
            }
        }
        SortField::Date => {
            models.sort_by(|a, b| {
                a.last_used
                    .cmp(&b.last_used)
                    .then_with(|| a.summary.model.cmp(&b.summary.model))
            });
            if !state.sort_ascending {
                models.reverse();
            }
        }
    }

    // Header row
    let header_y = inner.y;
    let total_width = inner.width as usize;
    let rank_width = 4usize;
    let cost_width = 8usize;
    let pct_width = 7usize;
    let msg_width = 8usize;
    let tokens_width = 9usize;
    let show_last = total_width >= 92;
    let last_width = if show_last { 11usize } else { 0usize };
    let show_sparkline = total_width >= 110;
    let sparkline_width = if show_sparkline { 9usize } else { 0usize };
    let show_efficiency = total_width >= 130;
    let cost_per_msg_width = if show_efficiency { 10usize } else { 0usize };
    let tok_per_msg_width = if show_efficiency { 9usize } else { 0usize };
    let agent_width = (total_width / 4).clamp(22, 36);
    let model_width = total_width
        .saturating_sub(
            rank_width
                + agent_width
                + tokens_width
                + cost_width
                + 1
                + pct_width
                + 1
                + msg_width
                + last_width
                + sparkline_width
                + cost_per_msg_width
                + tok_per_msg_width,
        )
        .clamp(12, 40);
    let total_cost = models.iter().map(|m| m.summary.cost).sum::<f64>();

    // Build per-model 7-day token sparkline data
    let today = Local::now().date_naive();
    let model_sparklines: HashMap<String, Vec<u64>> = if show_sparkline {
        let last_7: Vec<NaiveDate> = (0..7).map(|i| today - Duration::days(6 - i)).collect();
        let mut map: HashMap<String, Vec<u64>> = HashMap::new();
        for day_stat in &dashboard.daily {
            let day_idx = last_7.iter().position(|&d| d == day_stat.date);
            let Some(idx) = day_idx else { continue };
            for (model_key, breakdown) in &day_stat.models {
                let norm = normalize_model_name(model_id_from_key(model_key));
                let entry = map.entry(norm).or_insert_with(|| vec![0u64; 7]);
                entry[idx] += breakdown.tokens.max(0) as u64;
            }
        }
        map
    } else {
        HashMap::new()
    };

    let headers = ["#", "Model", "Agent", "Tokens", "Cost", "%", "Msgs", "Last", "Trend", "$/Msg", "T/Msg"];
    let sort_indicator = |field: SortField| -> &str {
        if state.sort_field == field {
            if state.sort_ascending {
                " ↑"
            } else {
                " ↓"
            }
        } else {
            ""
        }
    };

    let mut header_spans = vec![
        Span::styled(
            format!("{:<rank_width$}", headers[0]),
            Style::default().fg(theme.dim).bold(),
        ),
        Span::styled(
            format!("{:<model_width$}", headers[1]),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<agent_width$}", headers[2]),
            Style::default().fg(theme.accent_soft).bold(),
        ),
        Span::styled(
            format!(
                "{:<tokens_width$}",
                format!("{}{}", headers[3], sort_indicator(SortField::Tokens))
            ),
            Style::default().fg(Color::Rgb(52, 211, 153)).bold(),
        ),
        Span::styled(
            format!(
                "{:<cost_width$}",
                format!("{}{}", headers[4], sort_indicator(SortField::Cost))
            ),
            Style::default().fg(Color::Rgb(250, 204, 21)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:<pct_width$}", headers[5]),
            Style::default().fg(Color::Rgb(96, 165, 250)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:<msg_width$}", headers[6]),
            Style::default().fg(Color::Rgb(96, 165, 250)).bold(),
        ),
    ];
    if show_last {
        header_spans.push(Span::styled(
            format!(
                "{:<last_width$}",
                format!("{}{}", headers[7], sort_indicator(SortField::Date))
            ),
            Style::default().fg(theme.dim).bold(),
        ));
    }
    if show_sparkline {
        header_spans.push(Span::styled(
            format!("{:<sparkline_width$}", headers[8]),
            Style::default().fg(Color::Rgb(52, 211, 153)).bold(),
        ));
    }
    if show_efficiency {
        header_spans.push(Span::styled(
            format!("{:<cost_per_msg_width$}", headers[9]),
            Style::default().fg(Color::Rgb(250, 204, 21)).bold(),
        ));
        header_spans.push(Span::styled(
            format!("{:<tok_per_msg_width$}", headers[10]),
            Style::default().fg(Color::Rgb(52, 211, 153)).bold(),
        ));
    }
    let header_line = Line::from(header_spans);
    f.render_widget(
        Paragraph::new(header_line),
        Rect::new(inner.x, header_y, inner.width, 1),
    );

    let visible_rows = inner.height.saturating_sub(1) as usize;
    let selected_row = state.selected_row.min(models.len().saturating_sub(1));
    let offset = normalized_scroll_offset(
        state.scroll_offset,
        selected_row,
        visible_rows,
        models.len(),
    );

    for (i, row) in models.iter().skip(offset).take(visible_rows).enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.y + inner.height {
            break;
        }

        let rank = offset + i + 1;
        let selected = rank - 1 == selected_row;
        let model = &row.summary;
        let model_color = theme.model_color_for(&model.model, model.provider.split(',').next());
        let pct = cost_share_percent(model.cost, total_cost);

        let mut spans = vec![
            Span::styled(
                format!("{:<rank_width$}", rank),
                selected_row_style(Style::default().fg(theme.dim), selected, theme),
            ),
            Span::styled(
                format!("{:<model_width$}", truncate(&model.model, model_width)),
                selected_row_style(Style::default().fg(model_color), selected, theme),
            ),
            Span::styled(
                format!(
                    "{:<agent_width$}",
                    truncate(&format_source_list(&model.source), agent_width)
                ),
                selected_row_style(Style::default().fg(theme.accent_soft), selected, theme),
            ),
            Span::styled(
                format!("{:<tokens_width$}", format_compact(model.tokens)),
                selected_row_style(
                    Style::default().fg(Color::Rgb(52, 211, 153)),
                    selected,
                    theme,
                ),
            ),
            Span::styled(
                format!("{:<cost_width$}", format_cost_compact(model.cost)),
                selected_row_style(
                    Style::default().fg(Color::Rgb(250, 204, 21)),
                    selected,
                    theme,
                ),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:<pct_width$}", format!("{:.1}%", pct)),
                selected_row_style(
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                    selected,
                    theme,
                ),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:<msg_width$}", format_compact(model.message_count as i64)),
                selected_row_style(
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                    selected,
                    theme,
                ),
            ),
        ];
        if show_last {
            let last = row
                .last_used
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "n/a".to_string());
            spans.push(Span::styled(
                format!("{:<last_width$}", last),
                selected_row_style(Style::default().fg(theme.dim), selected, theme),
            ));
        }
        if show_sparkline {
            let trend = model_sparklines
                .get(&model.model)
                .map(|vals| sparkline_text(vals))
                .unwrap_or_else(|| "▁".repeat(7));
            spans.push(Span::styled(
                format!("{:<sparkline_width$}", trend),
                selected_row_style(
                    Style::default().fg(Color::Rgb(52, 211, 153)),
                    selected,
                    theme,
                ),
            ));
        }
        if show_efficiency {
            let cost_per_msg = if model.message_count > 0 {
                format_cost_compact(model.cost / model.message_count as f64)
            } else {
                "—".to_string()
            };
            let tok_per_msg = if model.message_count > 0 {
                format_compact(model.tokens / model.message_count as i64)
            } else {
                "—".to_string()
            };
            spans.push(Span::styled(
                format!("{:<cost_per_msg_width$}", cost_per_msg),
                selected_row_style(
                    Style::default().fg(Color::Rgb(250, 204, 21)),
                    selected,
                    theme,
                ),
            ));
            spans.push(Span::styled(
                format!("{:<tok_per_msg_width$}", tok_per_msg),
                selected_row_style(
                    Style::default().fg(Color::Rgb(52, 211, 153)),
                    selected,
                    theme,
                ),
            ));
        }

        let line = Line::from(spans);
        f.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
    }
}

// ===========================================================================
// TAB 3: Daily
// ===========================================================================

fn render_daily_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
) {
    let sections = daily_sections(area);

    render_daily_summary(f, sections[0], dashboard, state, theme);
    render_daily_table(f, sections[1], dashboard, state, theme);
}

fn render_daily_summary(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let days = dashboard.filtered_daily(&state.enabled_sources);
    let total_cost: f64 = days.iter().map(|day| day.cost_usd).sum();
    let avg_daily_cost = if days.is_empty() {
        0.0
    } else {
        total_cost / days.len() as f64
    };
    let max_daily_cost = days.iter().map(|day| day.cost_usd).fold(0.0, f64::max);
    let active_days = days.iter().filter(|day| day.total_tokens > 0).count();

    let today = Local::now().date_naive();
    let week_start = today - Duration::days(6);
    let today_cost = days
        .iter()
        .find(|day| day.date == today)
        .map(|day| day.cost_usd)
        .unwrap_or(0.0);
    let week_cost: f64 = days
        .iter()
        .filter(|day| day.date >= week_start && day.date <= today)
        .map(|day| day.cost_usd)
        .sum();
    let month_cost: f64 = days
        .iter()
        .filter(|day| day.date.year() == today.year() && day.date.month() == today.month())
        .map(|day| day.cost_usd)
        .sum();

    let mut lines = vec![Line::from(vec![
        Span::styled("Period Total ", Style::default().fg(theme.dim)),
        Span::styled(
            format_cost_compact(total_cost),
            Style::default().fg(theme.gauge_high).bold(),
        ),
        Span::raw("    "),
        Span::styled("Avg Daily ", Style::default().fg(theme.dim)),
        Span::styled(
            format_cost_compact(avg_daily_cost),
            Style::default().fg(theme.fg),
        ),
        Span::raw("    "),
        Span::styled("Max Daily ", Style::default().fg(theme.dim)),
        Span::styled(
            format_cost_compact(max_daily_cost),
            Style::default().fg(theme.fg),
        ),
        Span::raw("    "),
        Span::styled(
            format!("{} active days", active_days),
            Style::default().fg(theme.dim),
        ),
    ])];

    if inner.height >= 2 {
        lines.push(Line::from(vec![
            Span::styled("Today ", Style::default().fg(theme.dim)),
            Span::styled(
                format_cost_compact(today_cost),
                Style::default().fg(theme.accent).bold(),
            ),
            Span::raw("    "),
            Span::styled("This Week ", Style::default().fg(theme.dim)),
            Span::styled(format_cost_compact(week_cost), Style::default().fg(theme.fg)),
            Span::raw("    "),
            Span::styled("This Month ", Style::default().fg(theme.dim)),
            Span::styled(format_cost_compact(month_cost), Style::default().fg(theme.fg)),
        ]));
    }

    f.render_widget(Paragraph::new(lines), inner);
}

fn render_daily_table(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Daily Breakdown ",
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut days = dashboard.filtered_daily(&state.enabled_sources);
    if days.is_empty() {
        f.render_widget(
            Paragraph::new(empty_data_message(state, "No daily data"))
                .style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }
    // Sort daily data
    match state.sort_field {
        SortField::Date => {
            days.sort_by_key(|d| d.date);
            if !state.sort_ascending {
                days.reverse();
            }
        }
        SortField::Cost => {
            days.sort_by(|a, b| {
                a.cost_usd
                    .partial_cmp(&b.cost_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if !state.sort_ascending {
                days.reverse();
            }
        }
        SortField::Tokens => {
            days.sort_by_key(|d| d.total_tokens);
            if !state.sort_ascending {
                days.reverse();
            }
        }
    }

    let today = Local::now().date_naive();
    let show_wow = inner.width >= 100;
    let show_detail_cols = inner.width >= 80;

    // Build cost-by-date map for WoW calculation
    let cost_by_date: std::collections::HashMap<NaiveDate, f64> = days
        .iter()
        .map(|d| (d.date, d.cost_usd))
        .collect();

    // Header
    let header_y = inner.y;
    let date_width = 15usize;
    let mut header_spans = vec![
        Span::styled(
            format!("{:<date_width$}", "Date"),
            Style::default().fg(theme.accent_soft).bold(),
        ),
        Span::styled(
            format!("{:<10}", "Tokens"),
            Style::default().fg(Color::Rgb(52, 211, 153)).bold(),
        ),
        Span::styled(
            format!("{:<10}", "Cost"),
            Style::default().fg(Color::Rgb(250, 204, 21)).bold(),
        ),
    ];
    if show_detail_cols {
        header_spans.extend([
            Span::styled(
                format!("{:<10}", "Input"),
                Style::default().fg(Color::Rgb(96, 165, 250)).bold(),
            ),
            Span::styled(
                format!("{:<10}", "Output"),
                Style::default().fg(Color::Rgb(167, 139, 250)).bold(),
            ),
            Span::styled(
                format!("{:<10}", "Cache"),
                Style::default().fg(Color::Rgb(251, 146, 60)).bold(),
            ),
        ]);
    }
    header_spans.push(Span::styled(
        format!("{:<8}", "Msgs"),
        Style::default().fg(Color::Rgb(96, 165, 250)).bold(),
    ));
    if show_wow {
        header_spans.push(Span::styled(
            format!("{:<9}", "WoW"),
            Style::default().fg(theme.dim).bold(),
        ));
    }
    let header_line = Line::from(header_spans);
    f.render_widget(
        Paragraph::new(header_line),
        Rect::new(inner.x, header_y, inner.width, 1),
    );

    let visible_rows = inner.height.saturating_sub(1) as usize;
    let selected_row = state.selected_row.min(days.len().saturating_sub(1));
    let offset =
        normalized_scroll_offset(state.scroll_offset, selected_row, visible_rows, days.len());

    for (i, day) in days.iter().skip(offset).take(visible_rows).enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.y + inner.height {
            break;
        }

        let is_today = day.date == today;
        let selected = offset + i == selected_row;
        let date_style = if is_today {
            Style::default()
                .fg(theme.accent_soft)
                .bg(Color::Rgb(20, 40, 20))
                .bold()
        } else {
            Style::default().fg(theme.accent_soft)
        };
        let row_bg = if is_today {
            Some(Color::Rgb(20, 40, 20))
        } else {
            None
        };
        let date_text =
            if state.sort_field == SortField::Date && day.date.weekday() == chrono::Weekday::Mon {
                format!("┄ {}", day.date.format("%Y-%m-%d"))
            } else {
                format!("  {}", day.date.format("%Y-%m-%d"))
            };

        let mut spans = vec![
            Span::styled(
                format!("{:<date_width$}", date_text),
                selected_row_style(date_style, selected, theme),
            ),
            Span::styled(
                format!("{:<10}", format_compact(day.total_tokens)),
                selected_row_style(
                    metric_style(Color::Rgb(52, 211, 153), row_bg),
                    selected,
                    theme,
                ),
            ),
            Span::styled(
                format!("{:<10}", format_cost_compact(day.cost_usd)),
                selected_row_style(
                    metric_style(Color::Rgb(250, 204, 21), row_bg),
                    selected,
                    theme,
                ),
            ),
        ];
        if show_detail_cols {
            spans.extend([
                Span::styled(
                    format!("{:<10}", format_compact(day.input_tokens)),
                    selected_row_style(
                        metric_style(Color::Rgb(96, 165, 250), row_bg),
                        selected,
                        theme,
                    ),
                ),
                Span::styled(
                    format!("{:<10}", format_compact(day.output_tokens)),
                    selected_row_style(
                        metric_style(Color::Rgb(167, 139, 250), row_bg),
                        selected,
                        theme,
                    ),
                ),
                Span::styled(
                    format!("{:<10}", format_compact(day.cache_tokens())),
                    selected_row_style(
                        metric_style(Color::Rgb(251, 146, 60), row_bg),
                        selected,
                        theme,
                    ),
                ),
            ]);
        }
        spans.push(Span::styled(
            format!("{:<8}", format_compact(day.messages)),
            selected_row_style(
                metric_style(Color::Rgb(96, 165, 250), row_bg),
                selected,
                theme,
            ),
        ));
        if show_wow {
            let prior_date = day.date - Duration::days(7);
            let wow_text = match cost_by_date.get(&prior_date) {
                Some(&prior) if prior > 0.0 => {
                    let pct = (day.cost_usd - prior) / prior * 100.0;
                    if pct >= 0.0 {
                        format!("{:>+6.1}%↑", pct)
                    } else {
                        format!("{:>+6.1}%↓", pct)
                    }
                }
                _ => "   —  ".to_string(),
            };
            let wow_color = match cost_by_date.get(&prior_date) {
                Some(&prior) if prior > 0.0 && day.cost_usd > prior => Color::Rgb(248, 113, 113),
                Some(&prior) if prior > 0.0 && day.cost_usd < prior => Color::Rgb(52, 211, 153),
                _ => theme.dim,
            };
            spans.push(Span::styled(
                format!("{:<9}", wow_text),
                selected_row_style(metric_style(wow_color, row_bg), selected, theme),
            ));
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
}

// ===========================================================================
// TAB 4: Heatmap
// ===========================================================================

fn render_heatmap_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
) {
    let selected_day = dashboard.selected_day_in_window(
        state.heatmap_window,
        state.selected_heatmap_date,
        &state.enabled_sources,
    );
    let bounds = dashboard.bounds_for_window(state.heatmap_window, state.selected_heatmap_date);

    let sections = heatmap_sections(area);

    // Heatmap grid
    let heat_title = format!(
        " Usage Activity - {} / {} ",
        state.heatmap_window.label(),
        state.heatmap_metric.label()
    );
    let heat_block = Block::default()
        .title(Span::styled(
            heat_title,
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let heat_inner = heat_block.inner(sections[0]);
    f.render_widget(heat_block, sections[0]);

    let palette = heatmap_palette(theme, state.heatmap_metric);
    let points = dashboard.points_in_window(
        state.heatmap_metric,
        state.heatmap_window,
        state.selected_heatmap_date,
        &state.enabled_sources,
    );
    if points.is_empty() {
        f.render_widget(
            Paragraph::new(empty_data_message(state, "No activity data"))
                .style(Style::default().fg(theme.dim)),
            heat_inner,
        );
    } else {
        let heatmap = YearHeatmap::new(&points, state.heatmap_metric)
            .palette(palette)
            .empty(theme.empty_heatmap)
            .background(theme.heatmap_bg)
            .border(Some(theme.heatmap_border))
            .selected(selected_day.as_ref().map(|day| day.date))
            .range_opt(bounds);
        f.render_widget(heatmap, heat_inner);
    }

    // Legend bar
    let range_label = bounds
        .map(|(s, e)| format!("{} → {}", s.format("%Y-%m-%d"), e.format("%Y-%m-%d")))
        .unwrap_or_else(|| "no range".to_string());
    let legend = Paragraph::new(Line::from(vec![
        Span::styled("low", Style::default().fg(theme.dim)),
        Span::raw("  "),
        Span::styled("░▒▓█", Style::default().fg(palette[4])),
        Span::raw("  "),
        Span::styled("high", Style::default().fg(theme.dim)),
        Span::raw("  "),
        Span::styled(range_label, Style::default().fg(theme.fg)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(legend, sections[1]);

    // Bottom panels: range summary and selected day detail
    let info = heatmap_info_sections(sections[2]);

    render_heatmap_summary_card(
        f,
        info[0],
        dashboard,
        state.heatmap_window,
        state.heatmap_metric,
        state.selected_heatmap_date,
        &state.enabled_sources,
        theme,
    );
    render_heatmap_day_detail(
        f,
        info[1],
        selected_day.as_ref(),
        state.heatmap_metric,
        state.heatmap_detail_scroll,
        theme,
    );
}

fn render_heatmap_summary_card(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    window: HeatmapWindow,
    metric: HeatmapMetric,
    selected: Option<NaiveDate>,
    enabled: &BTreeSet<String>,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Range Overview ",
            Style::default().fg(theme.accent_soft).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let values = dashboard.values_in_window(metric, window, selected, enabled);
    let total = values.iter().sum::<f64>();
    let avg = if values.is_empty() {
        0.0
    } else {
        total / values.len() as f64
    };
    let peak = values.iter().copied().fold(0.0, f64::max);
    let active_days = dashboard.active_days_in_window(metric, window, selected, enabled);
    let current_streak = dashboard.current_streak_in_window(metric, window, selected, enabled);
    let longest_streak = dashboard.longest_streak_in_window(metric, window, selected, enabled);

    // Global cost stats for the cost overview rows
    let all_days = dashboard.filtered_daily(enabled);
    let today = Local::now().date_naive();
    let week_start = today - Duration::days(6);
    let today_cost = all_days
        .iter()
        .find(|day| day.date == today)
        .map(|day| day.cost_usd)
        .unwrap_or(0.0);
    let week_cost: f64 = all_days
        .iter()
        .filter(|day| day.date >= week_start && day.date <= today)
        .map(|day| day.cost_usd)
        .sum();
    let month_cost: f64 = all_days
        .iter()
        .filter(|day| day.date.year() == today.year() && day.date.month() == today.month())
        .map(|day| day.cost_usd)
        .sum();
    let all_time_cost: f64 = all_days.iter().map(|day| day.cost_usd).sum();

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Total  ", Style::default().fg(theme.dim)),
            Span::styled(
                format_metric(metric, total),
                Style::default().fg(theme.accent).bold(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Peak   ", Style::default().fg(theme.dim)),
            Span::styled(format_metric(metric, peak), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Avg    ", Style::default().fg(theme.dim)),
            Span::styled(format_metric(metric, avg), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Active ", Style::default().fg(theme.dim)),
            Span::styled(
                format!("{} days", active_days),
                Style::default().fg(theme.fg),
            ),
        ]),
        Line::from(vec![
            Span::styled("Range  ", Style::default().fg(theme.dim)),
            Span::styled(window.label(), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Streak ", Style::default().fg(theme.dim)),
            Span::styled(
                format!("{}/{}", current_streak, longest_streak),
                Style::default().fg(theme.fg),
            ),
            Span::styled(" cur/best", Style::default().fg(theme.dim)),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Today  ", Style::default().fg(theme.dim)),
            Span::styled(
                format_cost_compact(today_cost),
                Style::default().fg(theme.accent).bold(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Week   ", Style::default().fg(theme.dim)),
            Span::styled(format_cost_compact(week_cost), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Month  ", Style::default().fg(theme.dim)),
            Span::styled(format_cost_compact(month_cost), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("All    ", Style::default().fg(theme.dim)),
            Span::styled(
                format_cost_compact(all_time_cost),
                Style::default().fg(theme.fg),
            ),
        ]),
    ];

    // Trim trailing lines that don't fit
    let max_lines = inner.height as usize;
    lines.truncate(max_lines);

    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }),
        inner,
    );
}

fn render_heatmap_day_detail(
    f: &mut ratatui::Frame,
    area: Rect,
    day: Option<&DailyStats>,
    metric: HeatmapMetric,
    scroll_offset: usize,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Selected Day ",
            Style::default().fg(theme.opencode).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(day) = day else {
        f.render_widget(
            Paragraph::new("No selected day").style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    };

    let sections = selected_day_sections(inner);
    render_selected_day_overview(f, sections[0], day, metric, theme);
    render_selected_day_agent_detail(f, sections[1], day, scroll_offset, theme);
}

fn selected_day_sections(area: Rect) -> std::rc::Rc<[Rect]> {
    if area.width >= 76 && area.height >= 6 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(34), Constraint::Min(32)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(4)])
            .split(area)
    }
}

fn render_selected_day_overview(
    f: &mut ratatui::Frame,
    area: Rect,
    day: &DailyStats,
    metric: HeatmapMetric,
    theme: &Theme,
) {
    if area.height == 0 {
        return;
    }

    let lines = if area.width >= 34 && area.height >= 4 {
        vec![
            Line::from(vec![
                Span::styled(
                    day.date.format("%Y-%m-%d").to_string(),
                    Style::default().fg(theme.opencode).bold(),
                ),
                Span::raw("  "),
                Span::styled(
                    format_metric(metric, day.metric_value(metric)),
                    Style::default().fg(theme.fg).bold(),
                ),
            ]),
            Line::from(vec![
                Span::styled("Cost ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_cost_compact(day.cost_usd),
                    Style::default().fg(Color::Rgb(250, 204, 21)),
                ),
                Span::raw("  "),
                Span::styled("Tokens ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.total_tokens),
                    Style::default().fg(Color::Rgb(52, 211, 153)),
                ),
            ]),
            Line::from(vec![
                Span::styled("I ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.input_tokens),
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                ),
                Span::raw("  "),
                Span::styled("O ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.output_tokens),
                    Style::default().fg(Color::Rgb(167, 139, 250)),
                ),
                Span::raw("  "),
                Span::styled("C ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.cache_tokens()),
                    Style::default().fg(Color::Rgb(251, 146, 60)),
                ),
            ]),
            Line::from(vec![
                Span::styled("Reason ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.reasoning_tokens),
                    Style::default().fg(theme.opencode),
                ),
                Span::raw("  "),
                Span::styled("Msgs ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.messages),
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                ),
                Span::raw("  "),
                Span::styled("Sess ", Style::default().fg(theme.dim)),
                Span::styled(format_compact(day.sessions), Style::default().fg(theme.fg)),
            ]),
        ]
    } else {
        vec![
            Line::from(vec![
                Span::styled(
                    day.date.format("%Y-%m-%d").to_string(),
                    Style::default().fg(theme.opencode).bold(),
                ),
                Span::raw("  "),
                Span::styled(
                    format_metric(metric, day.metric_value(metric)),
                    Style::default().fg(theme.fg).bold(),
                ),
                Span::raw("  "),
                Span::styled("Cost ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_cost_compact(day.cost_usd),
                    Style::default().fg(Color::Rgb(250, 204, 21)),
                ),
            ]),
            Line::from(vec![
                Span::styled("T ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.total_tokens),
                    Style::default().fg(Color::Rgb(52, 211, 153)),
                ),
                Span::raw("  "),
                Span::styled("I ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.input_tokens),
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                ),
                Span::raw("  "),
                Span::styled("O ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.output_tokens),
                    Style::default().fg(Color::Rgb(167, 139, 250)),
                ),
                Span::raw("  "),
                Span::styled("C ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.cache_tokens()),
                    Style::default().fg(Color::Rgb(251, 146, 60)),
                ),
            ]),
        ]
    };

    f.render_widget(Paragraph::new(lines), area);
}

fn render_selected_day_agent_detail(
    f: &mut ratatui::Frame,
    area: Rect,
    day: &DailyStats,
    scroll_offset: usize,
    theme: &Theme,
) {
    let groups = build_agent_model_groups(day);
    let mut lines = vec![Line::from(vec![Span::styled(
        "Agent / Model Cost",
        Style::default().fg(theme.accent_soft).bold(),
    )])];

    let cost_width = 8usize.min(area.width.saturating_sub(10) as usize);
    let model_width = area.width.saturating_sub((cost_width + 3) as u16) as usize;
    for group in groups {
        let agent_name = display_source_name(&group.source);
        let agent_color = theme.provider_color(&group.source);
        lines.push(Line::from(vec![
            Span::styled(
                truncate(agent_name, model_width),
                Style::default().fg(agent_color).bold(),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:>cost_width$}", format_cost_compact(group.total_cost_usd)),
                Style::default().fg(Color::Rgb(250, 204, 21)).bold(),
            ),
        ]));

        for (model_name, stats) in group.models {
            let model_color = theme.model_color_for(&model_name, Some(stats.provider_id.as_str()));
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    truncate(&model_name, model_width.saturating_sub(2)),
                    Style::default().fg(model_color),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{:>cost_width$}", format_cost_compact(stats.cost_usd)),
                    Style::default().fg(Color::Rgb(250, 204, 21)),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default()),
                Span::styled("T ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(stats.tokens),
                    Style::default().fg(Color::Rgb(52, 211, 153)),
                ),
                Span::raw(" "),
                Span::styled("I ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(stats.input_tokens),
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                ),
                Span::raw(" "),
                Span::styled("O ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(stats.output_tokens),
                    Style::default().fg(Color::Rgb(167, 139, 250)),
                ),
                Span::raw(" "),
                Span::styled("C ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(stats.cache_read_tokens + stats.cache_write_tokens),
                    Style::default().fg(Color::Rgb(251, 146, 60)),
                ),
            ]));
        }
    }

    let total_lines = lines.len();
    let visible = area.height as usize;
    let offset = scroll_offset.min(total_lines.saturating_sub(visible));
    let mut visible_lines: Vec<Line> = lines.into_iter().skip(offset).take(visible).collect();
    if total_lines > visible && !visible_lines.is_empty() {
        let last = visible_lines.len().saturating_sub(1);
        visible_lines[last] = Line::from(vec![Span::styled(
            format!(
                "{}{} detail {}-{} / {}",
                if offset > 0 { "↑" } else { " " },
                if offset + visible < total_lines {
                    "↓"
                } else {
                    " "
                },
                offset + 1,
                (offset + visible).min(total_lines),
                total_lines
            ),
            Style::default().fg(theme.dim),
        )]);
    }

    f.render_widget(Paragraph::new(visible_lines), area);
}

fn heatmap_day_panel_line_count(day: Option<&DailyStats>) -> usize {
    let Some(day) = day else {
        return 1;
    };

    let groups = build_agent_model_groups(day);
    let mut lines = 1usize;
    for group in groups {
        lines += 1 + (group.models.len() * 2);
    }
    lines
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

fn heatmap_palette(theme: &Theme, metric: HeatmapMetric) -> [Color; 5] {
    match PaletteMode::from_metric(metric) {
        PaletteMode::Tokens => theme.token_heatmap,
        PaletteMode::Cost => theme.cost_heatmap,
        PaletteMode::Input | PaletteMode::Output | PaletteMode::Count => theme.count_heatmap,
        PaletteMode::Cache => theme.cache_heatmap,
    }
}

fn filtered_models_for_state(dashboard: &UsageDashboard, state: &UsageState) -> Vec<ModelSummary> {
    let models = dashboard.filtered_models(&state.enabled_sources);
    let query = state.model_filter.trim().to_ascii_lowercase();
    if query.is_empty() {
        return models;
    }

    models
        .into_iter()
        .filter(|model| {
            model.model.to_ascii_lowercase().contains(&query)
                || model.provider.to_ascii_lowercase().contains(&query)
                || model.source.to_ascii_lowercase().contains(&query)
                || format_source_list(&model.source)
                    .to_ascii_lowercase()
                    .contains(&query)
        })
        .collect()
}

fn filtered_model_rows_for_state(
    dashboard: &UsageDashboard,
    state: &UsageState,
) -> Vec<ModelTableRow> {
    filtered_models_for_state(dashboard, state)
        .into_iter()
        .map(|summary| {
            let last_used = dashboard.model_last_used(&summary.model, &state.enabled_sources);
            ModelTableRow { summary, last_used }
        })
        .collect()
}

fn empty_data_message(state: &UsageState, fallback: &str) -> String {
    if state.enabled_sources.len() < state.all_sources.len() {
        "No data for selected sources".to_string()
    } else if state.all_sources.is_empty() {
        format!(
            "{}\n\nNo sessions found. Run:\n  tokenpulse usage --reload\nto parse usage data.",
            fallback
        )
    } else {
        fallback.to_string()
    }
}

fn format_metric(metric: HeatmapMetric, value: f64) -> String {
    match metric {
        HeatmapMetric::Cost => format_cost_compact(value),
        HeatmapMetric::TotalTokens
        | HeatmapMetric::InputTokens
        | HeatmapMetric::OutputTokens
        | HeatmapMetric::CacheTokens
        | HeatmapMetric::Messages
        | HeatmapMetric::Sessions => format_compact(value.round() as i64),
    }
}

fn format_compact(value: i64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000_000 {
        format!("{:.1}B", value as f64 / 1_000_000_000.0)
    } else if abs >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if abs >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

/// Format a USD cost value compactly: $0.42, $1.2K, $3.4M.
fn format_cost_compact(value: f64) -> String {
    if value >= 1_000_000.0 {
        format!("${:.1}M", value / 1_000_000.0)
    } else if value >= 1_000.0 {
        format!("${:.1}K", value / 1_000.0)
    } else {
        format!("${:.2}", value)
    }
}

/// Render a slice of values as a 7-character block sparkline.
fn sparkline_text(values: &[u64]) -> String {
    const CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = *values.iter().max().unwrap_or(&0);
    if max == 0 {
        return "▁".repeat(values.len());
    }
    values
        .iter()
        .map(|&v| {
            let idx = ((v as f64 / max as f64) * 7.0) as usize;
            CHARS[idx.min(7)]
        })
        .collect()
}

fn truncate(text: &str, width: usize) -> String {
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

fn metric_style(color: Color, bg: Option<Color>) -> Style {
    let style = Style::default().fg(color);
    if let Some(bg) = bg {
        style.bg(bg)
    } else {
        style
    }
}

fn overview_model_visible_rows(area_height: u16) -> usize {
    area_height.saturating_sub(3) as usize
}

fn cost_share_percent(cost: f64, total_cost: f64) -> f64 {
    if total_cost <= 0.0 {
        return 0.0;
    }

    (cost / total_cost * 100.0).clamp(0.0, 100.0)
}

fn normalized_scroll_offset(offset: usize, selected: usize, visible: usize, total: usize) -> usize {
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

fn selected_row_style(style: Style, selected: bool, theme: &Theme) -> Style {
    if selected {
        style.bg(theme.selected_bg).bold()
    } else {
        style
    }
}

fn scroll_window_label(offset: usize, visible: usize, total: usize) -> String {
    if total == 0 {
        return String::new();
    }
    let start = offset + 1;
    let end = (offset + visible).min(total).max(start);
    format!("{}-{}", start, end)
}

fn overview_scroll_hint(offset: usize, visible: usize, total: usize) -> String {
    if total <= visible || visible == 0 {
        return format!("{} models", total);
    }
    let up = if offset > 0 { "↑" } else { " " };
    let down = if offset + visible < total { "↓" } else { " " };
    format!(
        "{}{} scroll {}-{} / {}",
        up,
        down,
        offset + 1,
        (offset + visible).min(total),
        total
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenpulse_core::usage::UsageRollup;

    fn sample_dashboard() -> UsageDashboard {
        let summary = UsageSummary {
            total_cost: 3.0,
            total_tokens: 300,
            message_count: 3,
            session_count: 3,
            active_days: 1,
            avg_daily_cost: 3.0,
            max_daily_cost: 3.0,
            avg_daily_tokens: 300.0,
            max_daily_tokens: 300,
            daily: vec![DashboardDay {
                date: "2026-04-01".to_string(),
                total_tokens: 300,
                total_cost_usd: 3.0,
                input_tokens: 120,
                output_tokens: 140,
                cache_read_tokens: 30,
                cache_write_tokens: 10,
                reasoning_tokens: 0,
                message_count: 3,
                session_count: 3,
                intensity_tokens: 1,
                intensity_cost: 1,
            }],
            weekly: Vec::<UsageRollup>::new(),
            monthly: Vec::<UsageRollup>::new(),
            by_provider: Vec::new(),
            by_model: vec![
                ModelSummary {
                    model: "claude-1".to_string(),
                    provider: "anthropic".to_string(),
                    source: "claude".to_string(),
                    cost: 1.0,
                    tokens: 100,
                    message_count: 1,
                    session_count: 1,
                    percent: 33.0,
                },
                ModelSummary {
                    model: "gpt-5".to_string(),
                    provider: "openai".to_string(),
                    source: "codex".to_string(),
                    cost: 2.0,
                    tokens: 200,
                    message_count: 2,
                    session_count: 2,
                    percent: 67.0,
                },
            ],
        };
        let daily_rows = vec![
            DailyUsageRow {
                date: "2026-04-01".to_string(),
                source: "claude".to_string(),
                provider_id: "anthropic".to_string(),
                model_id: "claude-1".to_string(),
                input_tokens: 40,
                output_tokens: 50,
                cache_read_tokens: 5,
                cache_write_tokens: 5,
                reasoning_tokens: 0,
                total_tokens: 100,
                cost_usd: 1.0,
                message_count: 1,
                session_count: 1,
            },
            DailyUsageRow {
                date: "2026-04-01".to_string(),
                source: "codex".to_string(),
                provider_id: "openai".to_string(),
                model_id: "gpt-5".to_string(),
                input_tokens: 80,
                output_tokens: 90,
                cache_read_tokens: 25,
                cache_write_tokens: 5,
                reasoning_tokens: 0,
                total_tokens: 200,
                cost_usd: 2.0,
                message_count: 2,
                session_count: 2,
            },
        ];

        UsageDashboard::build(&summary, &daily_rows)
    }

    #[test]
    fn filtered_daily_recomputes_day_totals_from_enabled_sources() {
        let dashboard = sample_dashboard();
        let enabled = BTreeSet::from(["codex".to_string()]);

        let days = dashboard.filtered_daily(&enabled);

        assert_eq!(days.len(), 1);
        assert_eq!(days[0].total_tokens, 200);
        assert_eq!(days[0].cost_usd, 2.0);
        assert_eq!(days[0].input_tokens, 80);
        assert_eq!(days[0].output_tokens, 90);
        assert_eq!(days[0].cache_tokens(), 30);
        assert_eq!(days[0].messages, 2);
        assert_eq!(days[0].providers.len(), 1);
        assert_eq!(days[0].models.len(), 1);
    }

    #[test]
    fn heatmap_selection_uses_filtered_sources() {
        let dashboard = sample_dashboard();
        let enabled = BTreeSet::from(["codex".to_string()]);
        let selected = NaiveDate::from_ymd_opt(2026, 4, 1);

        let day = dashboard.selected_day_in_window(HeatmapWindow::Past365Days, selected, &enabled);

        assert!(day.is_some());
        assert_eq!(day.unwrap().total_tokens, 200);
    }

    #[test]
    fn heatmap_past_365_days_is_rolling_not_calendar_year() {
        let dashboard = sample_dashboard();

        let bounds = dashboard
            .bounds_for_window(HeatmapWindow::Past365Days, None)
            .unwrap();

        assert_eq!(bounds.0, NaiveDate::from_ymd_opt(2025, 4, 2).unwrap());
        assert_eq!(bounds.1, NaiveDate::from_ymd_opt(2026, 4, 1).unwrap());
    }

    #[test]
    fn agent_model_groups_roll_up_costs_per_source() {
        let dashboard = sample_dashboard();
        let day = dashboard
            .day(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap())
            .unwrap();

        let groups = build_agent_model_groups(day);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].source, "codex");
        assert_eq!(groups[0].total_cost_usd, 2.0);
        assert_eq!(groups[0].models.len(), 1);
        assert_eq!(groups[0].models[0].0, "gpt-5");
        assert_eq!(groups[0].models[0].1.cost_usd, 2.0);
    }

    #[test]
    fn overview_visible_rows_uses_model_table_section() {
        let rows = visible_rows_for_page(UsagePage::Overview, Rect::new(0, 0, 120, 40));

        assert!(rows > 0);
    }

    #[test]
    fn cost_share_percent_uses_actual_small_nonzero_total() {
        assert_eq!(cost_share_percent(0.005, 0.005), 100.0);
        assert_eq!(cost_share_percent(0.005, 0.0), 0.0);
    }
}
