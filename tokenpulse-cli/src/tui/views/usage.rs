use crate::tui::theme::Theme;
use crate::tui::widgets::{HeatmapMetric, StackedBarChart, YearHeatmap};
use anyhow::Result;
use chrono::{Datelike, Duration, Local, NaiveDate};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
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
use tokenpulse_core::usage::{DailyUsageRow, DashboardDay, ModelSummary, UsageSummary};

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
            UsagePage::Heatmap => "Heatmap",
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

#[derive(Debug, Clone, Copy, Default)]
struct TokenComposition {
    input: i64,
    output: i64,
    cache: i64,
    reasoning: i64,
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

    fn token_composition(&self) -> TokenComposition {
        TokenComposition {
            input: self.input_tokens,
            output: self.output_tokens,
            cache: self.cache_tokens(),
            reasoning: self.reasoning_tokens,
        }
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

    fn provider_rows(&self) -> Vec<(String, DayBreakdown)> {
        let mut rows: Vec<(String, DayBreakdown)> = self
            .providers
            .iter()
            .map(|(n, s)| (n.clone(), s.clone()))
            .collect();
        rows.sort_by(|a, b| {
            compare_breakdown(&a.1, &b.1)
                .reverse()
                .then_with(|| a.0.cmp(&b.0))
        });
        rows
    }

    fn model_rows_sorted(&self) -> Vec<(String, DayBreakdown)> {
        let mut rows: Vec<(String, DayBreakdown)> = self
            .models
            .iter()
            .map(|(n, s)| (n.clone(), s.clone()))
            .collect();
        rows.sort_by(|a, b| {
            compare_breakdown(&a.1, &b.1)
                .reverse()
                .then_with(|| a.0.cmp(&b.0))
        });
        rows
    }
}

fn compare_breakdown(left: &DayBreakdown, right: &DayBreakdown) -> std::cmp::Ordering {
    left.tokens.cmp(&right.tokens).then_with(|| {
        left.cost_usd
            .partial_cmp(&right.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn model_source(model_key: &str) -> &str {
    model_key
        .split_once(" / ")
        .map(|(source, _)| source)
        .unwrap_or("")
}

// ---------------------------------------------------------------------------
// Dashboard aggregate
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct UsageDashboard {
    daily: Vec<DailyStats>,
    total_messages: usize,
    total_sessions: usize,
    by_model: Vec<ModelSummary>,
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
            by_model: summary.by_model.clone(),
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
        selected: Option<NaiveDate>,
    ) -> Option<(NaiveDate, NaiveDate)> {
        let latest = self.latest_date()?;
        let anchor = selected.unwrap_or(latest);
        match window {
            HeatmapWindow::Recent26Weeks => {
                let end = latest;
                Some((end - Duration::days(7 * 26 - 1), end))
            }
            HeatmapWindow::Recent52Weeks => {
                let end = latest;
                Some((end - Duration::days(7 * 52 - 1), end))
            }
            HeatmapWindow::SelectedYear => {
                let start = NaiveDate::from_ymd_opt(anchor.year(), 1, 1)?;
                let end = NaiveDate::from_ymd_opt(anchor.year(), 12, 31)?;
                Some((start, end))
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
        self.by_model
            .iter()
            .filter(|m| enabled.contains(&m.source))
            .cloned()
            .collect()
    }
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
    SelectedYear,
}

impl HeatmapWindow {
    fn next(self) -> Self {
        match self {
            HeatmapWindow::Recent26Weeks => HeatmapWindow::Recent52Weeks,
            HeatmapWindow::Recent52Weeks => HeatmapWindow::SelectedYear,
            HeatmapWindow::SelectedYear => HeatmapWindow::Recent26Weeks,
        }
    }

    fn label(self) -> &'static str {
        match self {
            HeatmapWindow::Recent26Weeks => "26 Weeks",
            HeatmapWindow::Recent52Weeks => "52 Weeks",
            HeatmapWindow::SelectedYear => "Year",
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct UsageState {
    page: UsagePage,
    heatmap_metric: HeatmapMetric,
    heatmap_window: HeatmapWindow,
    selected_heatmap_date: Option<NaiveDate>,
    scroll_offset: usize,
    sort_field: SortField,
    sort_ascending: bool,
    // Source filter overlay
    show_source_filter: bool,
    source_filter_cursor: usize,
    all_sources: Vec<String>,
    enabled_sources: BTreeSet<String>,
}

impl UsageState {
    fn new(dashboard: &UsageDashboard) -> Self {
        let all_sources = dashboard.all_sources();
        let enabled_sources: BTreeSet<String> = all_sources.iter().cloned().collect();
        Self {
            page: UsagePage::Overview,
            heatmap_metric: HeatmapMetric::TotalTokens,
            heatmap_window: HeatmapWindow::SelectedYear,
            selected_heatmap_date: dashboard.latest_date(),
            scroll_offset: 0,
            sort_field: SortField::Cost,
            sort_ascending: false,
            show_source_filter: false,
            source_filter_cursor: 0,
            all_sources,
            enabled_sources,
        }
    }

    fn next_page(&mut self) {
        self.page = self.page.next();
        self.scroll_offset = 0;
    }

    fn previous_page(&mut self) {
        self.page = self.page.previous();
        self.scroll_offset = 0;
    }

    fn next_window(&mut self) {
        self.heatmap_window = self.heatmap_window.next();
    }

    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    fn scroll_down(&mut self, max: usize) {
        if self.scroll_offset < max {
            self.scroll_offset += 1;
        }
    }

    fn toggle_sort(&mut self, field: SortField) {
        if self.sort_field == field {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_field = field;
            self.sort_ascending = false;
        }
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
        }
    }

    fn is_source_enabled(&self, source: &str) -> bool {
        self.enabled_sources.contains(source)
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run(summary: UsageSummary, daily_rows: Vec<DailyUsageRow>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let theme = Theme::default();
    let dashboard = UsageDashboard::build(&summary, &daily_rows);
    let mut state = UsageState::new(&dashboard);

    loop {
        terminal.draw(|f| {
            let size = f.area();
            render_dashboard(f, size, &dashboard, &summary, &state, &theme);
            if state.show_source_filter {
                render_source_filter_overlay(f, size, &state, &theme);
            }
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // Source filter overlay intercepts all keys when open
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
                            // Toggle all
                            if state.enabled_sources.len() == state.all_sources.len() {
                                // Keep only the first source
                                state.enabled_sources.clear();
                                if let Some(first) = state.all_sources.first() {
                                    state.enabled_sources.insert(first.clone());
                                }
                            } else {
                                state.enabled_sources = state.all_sources.iter().cloned().collect();
                            }
                        }
                        _ => {}
                    }
                    continue;
                }

                match state.page {
                    UsagePage::Models => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Left | KeyCode::Char('h') => state.previous_page(),
                        KeyCode::Right | KeyCode::Char('l') => state.next_page(),
                        KeyCode::Up | KeyCode::Char('k') => state.scroll_up(),
                        KeyCode::Down | KeyCode::Char('j') => {
                            let filtered_len =
                                dashboard.filtered_models(&state.enabled_sources).len();
                            state.scroll_down(filtered_len.saturating_sub(1));
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
                        KeyCode::Char('s') => {
                            state.show_source_filter = true;
                        }
                        _ => {}
                    },
                    UsagePage::Daily => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Left | KeyCode::Char('h') => state.previous_page(),
                        KeyCode::Right | KeyCode::Char('l') => state.next_page(),
                        KeyCode::Up | KeyCode::Char('k') => state.scroll_up(),
                        KeyCode::Down | KeyCode::Char('j') => {
                            state.scroll_down(dashboard.daily.len().saturating_sub(1));
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
                            state.selected_heatmap_date =
                                dashboard.move_selection(state.selected_heatmap_date, -1);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            state.selected_heatmap_date =
                                dashboard.move_selection(state.selected_heatmap_date, 1);
                        }
                        KeyCode::Tab => {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                state.previous_page();
                            } else {
                                state.next_page();
                            }
                        }
                        KeyCode::Char('w') => state.next_window(),
                        KeyCode::Char('t') => {
                            state.heatmap_metric = HeatmapMetric::TotalTokens;
                        }
                        KeyCode::Char('c') => state.heatmap_metric = HeatmapMetric::Cost,
                        KeyCode::Char('i') => {
                            state.heatmap_metric = HeatmapMetric::InputTokens;
                        }
                        KeyCode::Char('o') => {
                            state.heatmap_metric = HeatmapMetric::OutputTokens;
                        }
                        KeyCode::Char('x') => {
                            state.heatmap_metric = HeatmapMetric::CacheTokens;
                        }
                        KeyCode::Char('m') => {
                            state.heatmap_metric = HeatmapMetric::Messages;
                        }
                        KeyCode::Char('n') => {
                            state.heatmap_metric = HeatmapMetric::Sessions;
                        }
                        KeyCode::Char('s') => {
                            state.show_source_filter = true;
                        }
                        _ => {}
                    },
                    // Overview
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
                        KeyCode::Char('s') => {
                            state.show_source_filter = true;
                        }
                        _ => {}
                    },
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
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
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(2),
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
            format!("${:.2} cost", summary.total_cost),
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
                Style::default().fg(theme.bg).bg(theme.accent).bold()
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
        UsagePage::Overview => format!(" q quit | ←→ tab{}", filter_hint),
        UsagePage::Models => {
            let dir = if state.sort_ascending { "↑" } else { "↓" };
            let field = match state.sort_field {
                SortField::Cost => "cost",
                SortField::Tokens => "tokens",
                SortField::Date => "date",
            };
            format!(
                " q quit | ←→ tab | ↑↓ scroll | c/t sort ({} {}){}",
                field, dir, filter_hint
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
                " q quit | ←→ tab | ↑↓ scroll | c/t sort ({} {}){}",
                field, dir, filter_hint
            )
        }
        UsagePage::Heatmap => format!(
            " q quit | ←→ tab | ↑↓ day | w window ({}) | t/c/i/o/x/m/n metric ({}){}",
            state.heatmap_window.label(),
            state.heatmap_metric.short_label(),
            filter_hint
        ),
    };
    let footer = Paragraph::new(help)
        .style(Style::default().fg(theme.dim))
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(footer, area);
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
        let color = theme.model_color(source);
        let style = if selected {
            Style::default().fg(theme.bg).bg(color).bold()
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

// ===========================================================================
// TAB 1: Overview
// ===========================================================================

fn render_overview_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(12)])
        .split(area);

    render_overview_chart(f, sections[0], dashboard, state, theme);
    render_overview_top_models(f, sections[1], dashboard, summary, state, theme);
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
            " Token Usage (60 days) ",
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
            for (provider, stats) in &day.providers {
                if state.is_source_enabled(provider) {
                    segments.insert(provider.as_str(), stats.tokens as f64);
                    total += stats.tokens as f64;
                }
            }
            (total, segments)
        })
        .collect();

    let chart = StackedBarChart::new(&chart_data)
        .color("claude", theme.claude)
        .color("codex", theme.codex)
        .color("opencode", theme.opencode)
        .color("gemini", theme.gemini)
        .color("pi", theme.pi)
        .color("antigravity", theme.antigravity)
        .color("copilot", theme.copilot)
        .width(sections[0].width as usize)
        .height(sections[0].height as usize);
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
        ("claude", "CLA", theme.claude),
        ("codex", "CDX", theme.codex),
        ("copilot", "COP", theme.copilot),
        ("opencode", "OCD", theme.opencode),
        ("gemini", "GEM", theme.gemini),
        ("pi", "PI", theme.pi),
        ("antigravity", "AG", theme.antigravity),
    ];
    for (name, label, color) in provider_legend {
        if state.is_source_enabled(name) {
            legend_spans.push(Span::styled(
                format!("● {}", label),
                Style::default().fg(*color),
            ));
            legend_spans.push(Span::raw(" "));
        }
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

fn render_overview_top_models(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Top Models by Cost ",
            Style::default().fg(theme.accent_soft).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let filtered = dashboard.filtered_models(&state.enabled_sources);

    if filtered.is_empty() {
        f.render_widget(
            Paragraph::new("No model data").style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    let max_rows = inner.height as usize;
    let total_cost = summary.total_cost.max(0.01);

    for (idx, model) in filtered.iter().take(max_rows.min(10)).enumerate() {
        let y = inner.y + idx as u16;
        if y >= inner.y + inner.height {
            break;
        }

        let color = theme.model_color(&model.model);
        let pct = (model.cost / total_cost * 100.0).clamp(0.0, 100.0);

        let line = Line::from(vec![
            Span::styled("● ", Style::default().fg(color)),
            Span::styled(truncate(&model.model, 30), Style::default().fg(color)),
            Span::raw("  "),
            Span::styled(format_compact(model.tokens), Style::default().fg(theme.fg)),
            Span::raw("  "),
            Span::styled(format!("${:.2}", model.cost), Style::default().fg(theme.fg)),
            Span::raw("  "),
            Span::styled(format!("{:.0}%", pct), Style::default().fg(theme.dim)),
        ]);
        f.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
    }
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
    let block = Block::default()
        .title(Span::styled(
            " Models ",
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let filtered = dashboard.filtered_models(&state.enabled_sources);

    if filtered.is_empty() {
        f.render_widget(
            Paragraph::new("No model data").style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    // Sort models
    let mut models: Vec<&ModelSummary> = filtered.iter().collect();
    match state.sort_field {
        SortField::Cost => {
            models.sort_by(|a, b| {
                a.cost
                    .partial_cmp(&b.cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if !state.sort_ascending {
                models.reverse();
            }
        }
        SortField::Tokens => {
            models.sort_by_key(|m| m.tokens);
            if !state.sort_ascending {
                models.reverse();
            }
        }
        SortField::Date => {
            // models don't have dates, fall back to cost
            models.sort_by(|a, b| {
                a.cost
                    .partial_cmp(&b.cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            if !state.sort_ascending {
                models.reverse();
            }
        }
    }

    // Header row
    let header_y = inner.y;
    let headers = ["#", "Model", "Provider", "Tokens", "Cost", "Msgs"];
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

    let header_line = Line::from(vec![
        Span::styled(
            format!("{:<4}", headers[0]),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<34}", headers[1]),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<14}", headers[2]),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<12}{}", headers[3], sort_indicator(SortField::Tokens)),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<12}{}", headers[4], sort_indicator(SortField::Cost)),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<10}", headers[5]),
            Style::default().fg(theme.accent).bold(),
        ),
    ]);
    f.render_widget(
        Paragraph::new(header_line),
        Rect::new(inner.x, header_y, inner.width, 1),
    );

    let visible_rows = inner.height.saturating_sub(1) as usize;
    let offset = state
        .scroll_offset
        .min(models.len().saturating_sub(visible_rows));

    for (i, model) in models.iter().skip(offset).take(visible_rows).enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.y + inner.height {
            break;
        }

        let rank = offset + i + 1;
        let model_color = theme.model_color(&model.model);

        let spans = vec![
            Span::styled(format!("{:<4}", rank), Style::default().fg(theme.dim)),
            Span::styled(
                format!("{:<34}", truncate(&model.model, 32)),
                Style::default().fg(model_color),
            ),
            Span::styled(
                format!("{:<14}", truncate(&model.source, 12)),
                Style::default().fg(theme.dim),
            ),
            Span::styled(
                format!("{:<12}", format_compact(model.tokens)),
                Style::default().fg(theme.fg),
            ),
            Span::styled(
                format!("${:<11.2}", model.cost),
                Style::default().fg(theme.fg),
            ),
            Span::styled(
                format!("{:<10}", format_compact(model.message_count as i64)),
                Style::default().fg(theme.fg),
            ),
        ];

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
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(8)])
        .split(area);

    // Summary header
    render_daily_summary(f, sections[0], dashboard, state, theme);

    // Table
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

    let line = Line::from(vec![
        Span::styled("Period Total ", Style::default().fg(theme.dim)),
        Span::styled(
            format!("${:.2}", total_cost),
            Style::default().fg(theme.gauge_high).bold(),
        ),
        Span::raw("    "),
        Span::styled("Avg Daily ", Style::default().fg(theme.dim)),
        Span::styled(
            format!("${:.2}", avg_daily_cost),
            Style::default().fg(theme.fg),
        ),
        Span::raw("    "),
        Span::styled("Max Daily ", Style::default().fg(theme.dim)),
        Span::styled(
            format!("${:.2}", max_daily_cost),
            Style::default().fg(theme.fg),
        ),
        Span::raw("    "),
        Span::styled(
            format!("{} active days", active_days),
            Style::default().fg(theme.dim),
        ),
    ]);
    f.render_widget(Paragraph::new(line), inner);
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
            Paragraph::new("No daily data").style(Style::default().fg(theme.dim)),
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

    // Header
    let header_y = inner.y;
    let header_line = Line::from(vec![
        Span::styled(
            format!("{:<12}", "Date"),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<10}", "Tokens"),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<10}", "Cost"),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<10}", "Input"),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<10}", "Output"),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<10}", "Cache"),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::styled(
            format!("{:<8}", "Msgs"),
            Style::default().fg(theme.accent).bold(),
        ),
    ]);
    f.render_widget(
        Paragraph::new(header_line),
        Rect::new(inner.x, header_y, inner.width, 1),
    );

    let visible_rows = inner.height.saturating_sub(1) as usize;
    let offset = state
        .scroll_offset
        .min(days.len().saturating_sub(visible_rows));

    for (i, day) in days.iter().skip(offset).take(visible_rows).enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.y + inner.height {
            break;
        }

        let is_today = day.date == today;
        let date_style = if is_today {
            Style::default().fg(Color::Yellow).bold()
        } else {
            Style::default().fg(theme.fg)
        };
        let row_style = if is_today {
            Style::default().fg(theme.fg).bg(Color::Rgb(20, 40, 20))
        } else {
            Style::default().fg(theme.fg)
        };

        let line = Line::from(vec![
            Span::styled(format!("{:<12}", day.date.format("%Y-%m-%d")), date_style),
            Span::styled(
                format!("{:<10}", format_compact(day.total_tokens)),
                row_style,
            ),
            Span::styled(
                format!("{:<10}", format!("${:.2}", day.cost_usd)),
                row_style,
            ),
            Span::styled(
                format!("{:<10}", format_compact(day.input_tokens)),
                row_style,
            ),
            Span::styled(
                format!("{:<10}", format_compact(day.output_tokens)),
                row_style,
            ),
            Span::styled(
                format!("{:<10}", format_compact(day.cache_tokens())),
                row_style,
            ),
            Span::styled(format!("{:<8}", format_compact(day.messages)), row_style),
        ]);
        f.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
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

    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(area);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(16), Constraint::Length(3)])
        .split(split[0]);

    // Heatmap grid
    let heat_title = format!(
        " Activity Grid - {} / {} ",
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
    let heat_inner = heat_block.inner(left[0]);
    f.render_widget(heat_block, left[0]);

    let palette = heatmap_palette(theme, state.heatmap_metric);
    let points = dashboard.points_in_window(
        state.heatmap_metric,
        state.heatmap_window,
        state.selected_heatmap_date,
        &state.enabled_sources,
    );
    let heatmap = YearHeatmap::new(&points, state.heatmap_metric)
        .palette(palette)
        .empty(theme.empty_heatmap)
        .selected(selected_day.as_ref().map(|day| day.date))
        .range_opt(bounds);
    f.render_widget(heatmap, heat_inner);

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
    f.render_widget(legend, left[1]);

    // Right panel: day detail
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Min(6),
        ])
        .split(split[1]);

    render_heatmap_summary_card(
        f,
        right[0],
        dashboard,
        state.heatmap_window,
        state.heatmap_metric,
        state.selected_heatmap_date,
        &state.enabled_sources,
        theme,
    );
    render_heatmap_day_detail(
        f,
        right[1],
        selected_day.as_ref(),
        state.heatmap_metric,
        theme,
    );
    render_heatmap_token_breakdown(f, right[2], selected_day.as_ref(), theme);
    render_heatmap_day_models(f, right[3], selected_day.as_ref(), theme);
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

    let lines = vec![
        Line::from(vec![
            Span::styled("Total ", Style::default().fg(theme.dim)),
            Span::styled(
                format_metric(metric, total),
                Style::default().fg(theme.accent).bold(),
            ),
            Span::raw("  "),
            Span::styled("Peak ", Style::default().fg(theme.dim)),
            Span::styled(format_metric(metric, peak), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Avg   ", Style::default().fg(theme.dim)),
            Span::styled(format_metric(metric, avg), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Active ", Style::default().fg(theme.dim)),
            Span::styled(
                format!("{} days", active_days),
                Style::default().fg(theme.fg),
            ),
            Span::raw("  "),
            Span::styled("Window ", Style::default().fg(theme.dim)),
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
    ];

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

    let lines = vec![
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
                format!("${:.2}", day.cost_usd),
                Style::default().fg(theme.fg),
            ),
            Span::raw("  "),
            Span::styled("Tokens ", Style::default().fg(theme.dim)),
            Span::styled(
                format_compact(day.total_tokens),
                Style::default().fg(theme.fg),
            ),
        ]),
        Line::from(vec![
            Span::styled("Msgs ", Style::default().fg(theme.dim)),
            Span::styled(format_compact(day.messages), Style::default().fg(theme.fg)),
            Span::raw("  "),
            Span::styled("Sess ", Style::default().fg(theme.dim)),
            Span::styled(format_compact(day.sessions), Style::default().fg(theme.fg)),
        ]),
    ];

    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }),
        inner,
    );
}

fn render_heatmap_token_breakdown(
    f: &mut ratatui::Frame,
    area: Rect,
    day: Option<&DailyStats>,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Token Mix ",
            Style::default().fg(theme.gemini).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(day) = day else {
        f.render_widget(
            Paragraph::new("No day selected").style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    };

    let comp = day.token_composition();

    // Inline colored text format
    let line1 = Line::from(vec![
        Span::styled("IN ", Style::default().fg(theme.claude).bold()),
        Span::styled(
            format_compact(comp.input),
            Style::default().fg(theme.claude),
        ),
        Span::raw("  "),
        Span::styled("OUT ", Style::default().fg(theme.codex).bold()),
        Span::styled(
            format_compact(comp.output),
            Style::default().fg(theme.codex),
        ),
    ]);
    let line2 = Line::from(vec![
        Span::styled("CACHE ", Style::default().fg(theme.gemini).bold()),
        Span::styled(
            format_compact(comp.cache),
            Style::default().fg(theme.gemini),
        ),
        Span::raw("  "),
        Span::styled("REASON ", Style::default().fg(theme.opencode).bold()),
        Span::styled(
            format_compact(comp.reasoning),
            Style::default().fg(theme.opencode),
        ),
    ]);

    // Provider breakdown
    let provider_rows = day.provider_rows();
    let mut lines = vec![line1, line2];
    for (provider, stats) in provider_rows.iter().take(2) {
        let pcolor = theme.provider_color(provider);
        lines.push(Line::from(vec![
            Span::styled("● ", Style::default().fg(pcolor)),
            Span::styled(
                format!(
                    "{} {}",
                    provider.to_uppercase(),
                    format_compact(stats.tokens)
                ),
                Style::default().fg(pcolor),
            ),
        ]));
    }

    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }),
        inner,
    );
}

fn render_heatmap_day_models(
    f: &mut ratatui::Frame,
    area: Rect,
    day: Option<&DailyStats>,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Top Models ",
            Style::default().fg(theme.codex).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(day) = day else {
        f.render_widget(
            Paragraph::new("No day selected").style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    };

    let model_rows = day.model_rows_sorted();
    if model_rows.is_empty() {
        f.render_widget(
            Paragraph::new("No models").style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    let max_rows = inner.height as usize;
    for (idx, (model_key, stats)) in model_rows.iter().take(max_rows).enumerate() {
        let y = inner.y + idx as u16;
        if y >= inner.y + inner.height {
            break;
        }

        // model_key is "source / model_id", extract model_id for coloring
        let model_name = model_key.split(" / ").nth(1).unwrap_or(model_key.as_str());
        let color = theme.model_color(model_name);
        let display = truncate(model_key, inner.width.saturating_sub(12) as usize);

        let line = Line::from(vec![
            Span::styled("● ", Style::default().fg(color)),
            Span::styled(display, Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(format_compact(stats.tokens), Style::default().fg(theme.fg)),
        ]);
        f.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

fn heatmap_palette(theme: &Theme, metric: HeatmapMetric) -> [Color; 5] {
    match PaletteMode::from_metric(metric) {
        PaletteMode::Tokens => theme.token_heatmap,
        PaletteMode::Cost => theme.cost_heatmap,
        PaletteMode::Input | PaletteMode::Output | PaletteMode::Count => theme.count_heatmap,
        PaletteMode::Cache => [
            Color::Rgb(72, 53, 33),
            Color::Rgb(88, 67, 41),
            Color::Rgb(110, 83, 48),
            Color::Rgb(144, 110, 57),
            Color::Rgb(192, 155, 83),
        ],
    }
}

fn format_metric(metric: HeatmapMetric, value: f64) -> String {
    match metric {
        HeatmapMetric::Cost => format!("${:.2}", value),
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
        assert_eq!(days[0].provider_rows().len(), 1);
        assert_eq!(days[0].model_rows_sorted().len(), 1);
    }

    #[test]
    fn heatmap_selection_uses_filtered_sources() {
        let dashboard = sample_dashboard();
        let enabled = BTreeSet::from(["codex".to_string()]);
        let selected = NaiveDate::from_ymd_opt(2026, 4, 1);

        let day = dashboard.selected_day_in_window(HeatmapWindow::SelectedYear, selected, &enabled);

        assert!(day.is_some());
        assert_eq!(day.unwrap().total_tokens, 200);
    }
}
