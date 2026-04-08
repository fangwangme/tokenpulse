use crate::tui::theme::Theme;
use crate::tui::widgets::{
    GradientGauge, HeatmapMetric, StackedBarChart, StyledTable, TrendSparkline, YearHeatmap,
};
use anyhow::Result;
use chrono::{Datelike, Duration, NaiveDate};
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
    widgets::{Block, Borders, Paragraph, Tabs},
    Terminal,
};
use std::collections::HashMap;
use tokenpulse_core::usage::{
    DailyUsageRow, DashboardDay, ModelSummary, ProviderSummary, UsageRollup, UsageSummary,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UsagePage {
    GitHub,
    ByDay,
    ByModel,
}

impl UsagePage {
    fn all() -> [UsagePage; 3] {
        [UsagePage::GitHub, UsagePage::ByDay, UsagePage::ByModel]
    }

    fn title(self) -> &'static str {
        match self {
            UsagePage::GitHub => "GitHub",
            UsagePage::ByDay => "By Day",
            UsagePage::ByModel => "By Model",
        }
    }

    fn next(self) -> Self {
        let pages = Self::all();
        let idx = pages.iter().position(|page| *page == self).unwrap_or(0);
        pages[(idx + 1) % pages.len()]
    }

    fn previous(self) -> Self {
        let pages = Self::all();
        let idx = pages.iter().position(|page| *page == self).unwrap_or(0);
        pages[(idx + pages.len() - 1) % pages.len()]
    }
}

#[derive(Debug, Clone, Default)]
struct DayBreakdown {
    provider_id: String,
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
    total: i64,
}

impl TokenComposition {
    fn share(self, value: i64) -> f64 {
        if self.total <= 0 {
            0.0
        } else {
            (value as f64 / self.total as f64 * 100.0).clamp(0.0, 100.0)
        }
    }
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

    fn token_composition(&self) -> TokenComposition {
        TokenComposition {
            input: self.input_tokens,
            output: self.output_tokens,
            cache: self.cache_tokens(),
            reasoning: self.reasoning_tokens,
            total: self.total_tokens,
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

    fn top_provider(&self) -> Option<(&str, &DayBreakdown)> {
        self.providers
            .iter()
            .max_by(|left, right| compare_breakdown(left.1, right.1))
            .map(|(name, stats)| (name.as_str(), stats))
    }

    fn top_model(&self) -> Option<(&str, &DayBreakdown)> {
        self.models
            .iter()
            .max_by(|left, right| compare_breakdown(left.1, right.1))
            .map(|(name, stats)| (name.as_str(), stats))
    }

    fn provider_rows(&self) -> Vec<(String, DayBreakdown)> {
        let mut rows: Vec<(String, DayBreakdown)> = self
            .providers
            .iter()
            .map(|(name, stats)| (name.clone(), stats.clone()))
            .collect();
        rows.sort_by(|left, right| {
            compare_breakdown(&left.1, &right.1)
                .reverse()
                .then_with(|| left.0.cmp(&right.0))
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

#[derive(Debug)]
struct UsageDashboard {
    daily: Vec<DailyStats>,
    total_messages: usize,
    total_sessions: usize,
    weekly: Vec<UsageRollup>,
    monthly: Vec<UsageRollup>,
    by_provider: Vec<ProviderSummary>,
    by_model: Vec<ModelSummary>,
}

impl UsageDashboard {
    fn build(summary: &UsageSummary, daily_rows: &[DailyUsageRow]) -> Self {
        let mut days: HashMap<String, DailyStats> = summary
            .daily
            .iter()
            .filter_map(DailyStats::from_day)
            .map(|day| (day.date.format("%Y-%m-%d").to_string(), day))
            .collect();

        for row in daily_rows {
            let Some(day) = days.get_mut(&row.date) else {
                continue;
            };

            let provider = day.providers.entry(row.source.clone()).or_default();
            provider.provider_id = row.provider_id.clone();
            provider.tokens += row.total_tokens;
            provider.cost_usd += row.cost_usd;
            provider.messages += row.message_count;
            provider.sessions += row.session_count;

            let model_key = format!("{} / {}", row.source, row.model_id);
            let model = day.models.entry(model_key).or_default();
            model.provider_id = row.provider_id.clone();
            model.tokens += row.total_tokens;
            model.cost_usd += row.cost_usd;
            model.messages += row.message_count;
            model.sessions += row.session_count;
        }

        let mut daily: Vec<DailyStats> = days.into_values().collect();
        daily.sort_by_key(|day| day.date);

        Self {
            daily,
            total_messages: summary.message_count,
            total_sessions: summary.session_count,
            weekly: summary.weekly.clone(),
            monthly: summary.monthly.clone(),
            by_provider: summary.by_provider.clone(),
            by_model: summary.by_model.clone(),
        }
    }

    fn latest_date(&self) -> Option<NaiveDate> {
        self.daily.last().map(|day| day.date)
    }

    fn day(&self, date: NaiveDate) -> Option<&DailyStats> {
        self.daily.iter().find(|day| day.date == date)
    }

    fn move_selection(&self, selected: Option<NaiveDate>, offset: isize) -> Option<NaiveDate> {
        if self.daily.is_empty() {
            return None;
        }

        let current_index = selected
            .and_then(|date| self.daily.iter().position(|day| day.date == date))
            .unwrap_or(self.daily.len().saturating_sub(1));

        let next_index = (current_index as isize + offset)
            .clamp(0, self.daily.len().saturating_sub(1) as isize)
            as usize;

        self.daily.get(next_index).map(|day| day.date)
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
    ) -> Vec<&DailyStats> {
        let Some((start, end)) = self.bounds_for_window(window, selected) else {
            return Vec::new();
        };

        self.daily
            .iter()
            .filter(|day| day.date >= start && day.date <= end)
            .collect()
    }

    fn points_in_window(
        &self,
        metric: HeatmapMetric,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
    ) -> Vec<(NaiveDate, f64)> {
        self.days_in_window(window, selected)
            .into_iter()
            .map(|day| (day.date, day.metric_value(metric)))
            .collect()
    }

    fn values_in_window(
        &self,
        metric: HeatmapMetric,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
    ) -> Vec<f64> {
        self.days_in_window(window, selected)
            .into_iter()
            .map(|day| day.metric_value(metric))
            .collect()
    }

    fn selected_day_in_window(
        &self,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
    ) -> Option<&DailyStats> {
        let (start, end) = self.bounds_for_window(window, selected)?;
        let selected = selected?;
        if selected < start || selected > end {
            return None;
        }
        self.day(selected)
    }

    fn active_days_in_window(
        &self,
        metric: HeatmapMetric,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
    ) -> usize {
        self.days_in_window(window, selected)
            .into_iter()
            .filter(|day| day.metric_value(metric) > 0.0)
            .count()
    }

    fn longest_streak_in_window(
        &self,
        metric: HeatmapMetric,
        window: HeatmapWindow,
        selected: Option<NaiveDate>,
    ) -> usize {
        let Some((start, end)) = self.bounds_for_window(window, selected) else {
            return 0;
        };

        let values: HashMap<NaiveDate, f64> = self
            .days_in_window(window, selected)
            .into_iter()
            .map(|day| (day.date, day.metric_value(metric)))
            .collect();

        let mut cursor = start;
        let mut current = 0usize;
        let mut best = 0usize;
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
    ) -> usize {
        let Some((start, end)) = self.bounds_for_window(window, selected) else {
            return 0;
        };

        let values: HashMap<NaiveDate, f64> = self
            .days_in_window(window, selected)
            .into_iter()
            .map(|day| (day.date, day.metric_value(metric)))
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

    fn total_token_composition(&self) -> TokenComposition {
        self.daily.iter().fold(TokenComposition::default(), |mut acc, day| {
            let composition = day.token_composition();
            acc.input += composition.input;
            acc.output += composition.output;
            acc.cache += composition.cache;
            acc.reasoning += composition.reasoning;
            acc.total += composition.total;
            acc
        })
    }
}

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

struct UsageState {
    page: UsagePage,
    heatmap_metric: HeatmapMetric,
    heatmap_window: HeatmapWindow,
    selected_heatmap_date: Option<NaiveDate>,
}

impl UsageState {
    fn new(dashboard: &UsageDashboard) -> Self {
        Self {
            page: UsagePage::GitHub,
            heatmap_metric: HeatmapMetric::TotalTokens,
            heatmap_window: HeatmapWindow::SelectedYear,
            selected_heatmap_date: dashboard.latest_date(),
        }
    }

    fn next_page(&mut self) {
        self.page = self.page.next();
    }

    fn previous_page(&mut self) {
        self.page = self.page.previous();
    }

    fn next_window(&mut self) {
        self.heatmap_window = self.heatmap_window.next();
    }
}

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
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
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
                    KeyCode::Char('t') => state.heatmap_metric = HeatmapMetric::TotalTokens,
                    KeyCode::Char('c') => state.heatmap_metric = HeatmapMetric::Cost,
                    KeyCode::Char('i') => state.heatmap_metric = HeatmapMetric::InputTokens,
                    KeyCode::Char('o') => state.heatmap_metric = HeatmapMetric::OutputTokens,
                    KeyCode::Char('x') => state.heatmap_metric = HeatmapMetric::CacheTokens,
                    KeyCode::Char('m') => state.heatmap_metric = HeatmapMetric::Messages,
                    KeyCode::Char('n') => state.heatmap_metric = HeatmapMetric::Sessions,
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}

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
        UsagePage::GitHub => render_github_page(f, root[2], dashboard, state, theme),
        UsagePage::ByDay => render_by_day_page(f, root[2], dashboard, summary, theme),
        UsagePage::ByModel => render_by_model_page(f, root[2], dashboard, summary, theme),
    }

    render_footer(f, root[3], state, theme);
}

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
            format!("{} tokens", format_compact(summary.total_tokens)),
            Style::default().fg(theme.codex),
        ),
        Span::raw("  "),
        Span::styled(
            format!("${:.2} cost", summary.total_cost),
            Style::default().fg(theme.gauge_high),
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
        Span::raw("  "),
        Span::styled(
            format!("{} active days", format_compact(summary.active_days as i64)),
            Style::default().fg(theme.gemini),
        ),
    ]);

    let header = Paragraph::new(vec![title, subtitle])
        .style(Style::default().fg(theme.fg))
        .alignment(Alignment::Left)
        .wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(header, inner);
}

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
        .select(
            pages
                .iter()
                .position(|page| *page == state.page)
                .unwrap_or(0),
        )
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

fn render_footer(f: &mut ratatui::Frame, area: Rect, state: &UsageState, theme: &Theme) {
    let help = format!(
        " q quit | ←→ page | ↑↓ day | w window ({}) | t/c/i/o/x/m/n metric ({}) ",
        state.heatmap_window.label(),
        state.heatmap_metric.short_label(),
    );
    let footer = Paragraph::new(help)
        .style(Style::default().fg(theme.dim))
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(footer, area);
}

fn render_github_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
) {
    let selected_day =
        dashboard.selected_day_in_window(state.heatmap_window, state.selected_heatmap_date);
    let bounds = dashboard.bounds_for_window(state.heatmap_window, state.selected_heatmap_date);
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
        .split(area);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(16), Constraint::Length(3)])
        .split(split[0]);

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
    let inner = heat_block.inner(left[0]);
    f.render_widget(heat_block, left[0]);

    let palette = heatmap_palette(theme, state.heatmap_metric);
    let points = dashboard.points_in_window(
        state.heatmap_metric,
        state.heatmap_window,
        state.selected_heatmap_date,
    );
    let heatmap = YearHeatmap::new(&points, state.heatmap_metric)
        .palette(palette)
        .empty(theme.empty_heatmap)
        .selected(state.selected_heatmap_date)
        .range_opt(bounds);
    f.render_widget(heatmap, inner);

    let range_label = bounds
        .map(|(start, end)| format!("{} → {}", start.format("%Y-%m-%d"), end.format("%Y-%m-%d")))
        .unwrap_or_else(|| "no range".to_string());
    let legend = Paragraph::new(Line::from(vec![
        Span::styled("low", Style::default().fg(theme.dim)),
        Span::raw("  "),
        Span::styled("░▒▓█", Style::default().fg(palette[4])),
        Span::raw("  "),
        Span::styled("high", Style::default().fg(theme.dim)),
        Span::raw("  "),
        Span::styled(
            format!(
                "{}  |  selected {}",
                range_label,
                selected_day
                    .map(|day| day.date.format("%Y-%m-%d").to_string())
                    .unwrap_or_else(|| "no selected day".to_string())
            ),
            Style::default().fg(theme.fg),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
    );
    f.render_widget(legend, left[1]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Min(12),
        ])
        .split(split[1]);

    render_heatmap_summary_card(
        f,
        right[0],
        dashboard,
        state.heatmap_window,
        state.heatmap_metric,
        state.selected_heatmap_date,
        theme,
    );
    render_selected_day_card(f, right[1], selected_day, state.heatmap_metric, theme);
    render_heatmap_breakdown_card(f, right[2], selected_day, theme);
}

fn render_heatmap_summary_card(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    window: HeatmapWindow,
    metric: HeatmapMetric,
    selected: Option<NaiveDate>,
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

    let values = dashboard.values_in_window(metric, window, selected);
    let total = values.iter().sum::<f64>();
    let avg = if values.is_empty() {
        0.0
    } else {
        total / values.len() as f64
    };
    let peak = values.iter().copied().fold(0.0, f64::max);
    let latest = values.last().copied().unwrap_or(0.0);
    let active_days = dashboard.active_days_in_window(metric, window, selected);
    let current_streak = dashboard.current_streak_in_window(metric, window, selected);
    let longest_streak = dashboard.longest_streak_in_window(metric, window, selected);

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
            Span::raw("  "),
            Span::styled("Latest", Style::default().fg(theme.dim)),
            Span::styled(format_metric(metric, latest), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Active", Style::default().fg(theme.dim)),
            Span::styled(active_days.to_string(), Style::default().fg(theme.fg)),
            Span::styled(" days", Style::default().fg(theme.dim)),
            Span::raw("  "),
            Span::styled("Window ", Style::default().fg(theme.dim)),
            Span::styled(window.label(), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Streak", Style::default().fg(theme.dim)),
            Span::styled(
                format!("{}/{}", current_streak, longest_streak),
                Style::default().fg(theme.fg),
            ),
            Span::styled(" current/best", Style::default().fg(theme.dim)),
        ]),
    ];

    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }),
        inner,
    );
}

fn render_selected_day_card(
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

    let top_provider = day
        .top_provider()
        .map(|(provider, stats)| {
            format!(
                "{} {}",
                provider.to_uppercase(),
                format_compact(stats.tokens)
            )
        })
        .unwrap_or_else(|| "n/a".to_string());
    let top_model = day
        .top_model()
        .map(|(model, stats)| format!("{} {}", truncate(model, 14), format_compact(stats.tokens)))
        .unwrap_or_else(|| "n/a".to_string());

    let lines = vec![
        Line::from(vec![
            Span::styled(
                day.date.format("%Y-%m-%d").to_string(),
                Style::default().fg(theme.opencode).bold(),
            ),
            Span::raw("  "),
            Span::styled("Value ", Style::default().fg(theme.dim)),
            Span::styled(
                format_metric(metric, day.metric_value(metric)),
                Style::default().fg(theme.fg).bold(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Cost ", Style::default().fg(theme.dim)),
            Span::styled(format!("${:.2}", day.cost_usd), Style::default().fg(theme.fg)),
            Span::raw("  "),
            Span::styled("Tokens ", Style::default().fg(theme.dim)),
            Span::styled(format_compact(day.total_tokens), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Msgs ", Style::default().fg(theme.dim)),
            Span::styled(format_compact(day.messages), Style::default().fg(theme.fg)),
            Span::raw("  "),
            Span::styled("Sess ", Style::default().fg(theme.dim)),
            Span::styled(format_compact(day.sessions), Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Top src ", Style::default().fg(theme.dim)),
            Span::styled(top_provider, Style::default().fg(theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("Top mdl ", Style::default().fg(theme.dim)),
            Span::styled(top_model, Style::default().fg(theme.fg)),
        ]),
    ];

    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }),
        inner,
    );
}

fn render_heatmap_breakdown_card(
    f: &mut ratatui::Frame,
    area: Rect,
    day: Option<&DailyStats>,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Token Mix + Sources ",
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

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(4)])
        .split(inner);

    let composition = day.token_composition();
    let mix_rows = [
        ("Input", composition.input, theme.claude),
        ("Output", composition.output, theme.codex),
        ("Cache", composition.cache, theme.gemini),
        ("Reason", composition.reasoning, theme.opencode),
    ];
    let mix_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(1); mix_rows.len() + 1])
        .split(split[0]);

    for (idx, (label, value, color)) in mix_rows.into_iter().enumerate() {
        let title = format!("{} {}", label, format_compact(value));
        let gauge = GradientGauge::new(&title, composition.share(value))
            .width(split[0].width.saturating_sub(18) as usize)
            .color(color);
        f.render_widget(gauge, mix_areas[idx]);
    }
    f.render_widget(
        Paragraph::new(format!("{} total tokens", format_compact(day.total_tokens)))
            .style(Style::default().fg(theme.dim)),
        mix_areas[mix_rows.len()],
    );

    let provider_rows = day.provider_rows();
    if provider_rows.is_empty() {
        f.render_widget(
            Paragraph::new("No provider split").style(Style::default().fg(theme.dim)),
            split[1],
        );
        return;
    }

    let provider_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            provider_rows
                .iter()
                .take(split[1].height as usize)
                .map(|_| Constraint::Length(1))
                .collect::<Vec<_>>(),
        )
        .split(split[1]);

    for (idx, (provider, stats)) in provider_rows
        .into_iter()
        .take(provider_areas.len())
        .enumerate()
    {
        let percent = if day.total_tokens <= 0 {
            0.0
        } else {
            (stats.tokens as f64 / day.total_tokens as f64 * 100.0).clamp(0.0, 100.0)
        };
        let label = format!("{} {}", provider.to_uppercase(), format_compact(stats.tokens));
        let gauge = GradientGauge::new(&label, percent)
            .width(split[1].width.saturating_sub(18) as usize)
            .color(theme.provider_color(&provider));
        f.render_widget(gauge, provider_areas[idx]);
    }
}

fn render_by_day_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    summary: &UsageSummary,
    theme: &Theme,
) {
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(11),
            Constraint::Length(10),
            Constraint::Min(10),
        ])
        .split(area);

    render_metric_row(f, sections[0], summary, dashboard, theme);

    let middle = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(sections[1]);
    render_recent_trends_card(f, middle[0], dashboard, theme);
    render_token_mix_card(
        f,
        middle[1],
        " Token Mix ",
        dashboard.total_token_composition(),
        Some(format!("{} loaded days", dashboard.daily.len())),
        theme,
    );

    let lower = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(sections[2]);
    render_provider_flow_card(f, lower[0], dashboard, theme);
    render_rollup_summary_card(f, lower[1], dashboard, theme);

    let block = Block::default()
        .title(Span::styled(
            " Daily Breakdown ",
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(sections[3]);
    f.render_widget(block, sections[3]);

    let rows = dashboard
        .daily
        .iter()
        .rev()
        .take(inner.height.saturating_sub(1) as usize)
        .map(|day| {
            vec![
                day.date.format("%Y-%m-%d").to_string(),
                format_compact(day.total_tokens),
                format!("${:.2}", day.cost_usd),
                format_compact(day.messages),
                format_compact(day.sessions),
                format_compact(day.cache_tokens()),
            ]
        })
        .collect::<Vec<_>>();

    let mut table = StyledTable::new(vec!["Date", "Tokens", "Cost", "Msgs", "Sess", "Cache"])
        .widths(vec![14, 12, 12, 10, 10, 12])
        .header_color(theme.accent);

    for row in rows {
        table = table.row(row);
    }

    f.render_widget(table, inner);
}

fn render_recent_trends_card(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Recent Trends ",
            Style::default().fg(theme.accent_soft).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let recent = dashboard.recent_days(28);
    if recent.is_empty() {
        f.render_widget(
            Paragraph::new("No recent usage data").style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(1),
        ])
        .split(inner);

    let range_text = format!(
        "{} → {} | {} active days",
        recent
            .first()
            .map(|day| day.date.format("%m-%d").to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        recent
            .last()
            .map(|day| day.date.format("%m-%d").to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        recent.iter().filter(|day| day.total_tokens > 0).count()
    );
    f.render_widget(
        Paragraph::new(range_text).style(Style::default().fg(theme.dim)),
        sections[0],
    );

    let token_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(8), Constraint::Min(10)])
        .split(sections[1]);
    f.render_widget(
        Paragraph::new("TOKENS").style(Style::default().fg(theme.gauge_mid).bold()),
        token_row[0],
    );
    let token_values = recent
        .iter()
        .map(|day| day.total_tokens as f64)
        .collect::<Vec<_>>();
    let token_max = token_values.iter().copied().fold(0.0, f64::max);
    f.render_widget(
        TrendSparkline::new(&token_values)
            .color(theme.gauge_mid)
            .empty(theme.empty_heatmap)
            .labels("0".to_string(), format_compact(token_max.round() as i64)),
        token_row[1],
    );

    let cost_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(8), Constraint::Min(10)])
        .split(sections[2]);
    f.render_widget(
        Paragraph::new("COST").style(Style::default().fg(theme.gauge_high).bold()),
        cost_row[0],
    );
    let cost_values = recent.iter().map(|day| day.cost_usd).collect::<Vec<_>>();
    let cost_max = cost_values.iter().copied().fold(0.0, f64::max);
    f.render_widget(
        TrendSparkline::new(&cost_values)
            .color(theme.gauge_high)
            .empty(theme.empty_heatmap)
            .labels("$0".to_string(), format!("${:.2}", cost_max)),
        cost_row[1],
    );

    let latest = recent.last().copied();
    let detail = latest
        .map(|day| {
            format!(
                "Latest {}  {} tokens  ${:.2}  {} msgs",
                day.date.format("%Y-%m-%d"),
                format_compact(day.total_tokens),
                day.cost_usd,
                format_compact(day.messages)
            )
        })
        .unwrap_or_else(|| "No recent day".to_string());
    f.render_widget(
        Paragraph::new(detail).style(Style::default().fg(theme.fg)),
        sections[3],
    );
}

fn render_token_mix_card(
    f: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    composition: TokenComposition,
    subtitle: Option<String>,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(theme.codex).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    let rows = [
        ("Input", composition.input, theme.claude),
        ("Output", composition.output, theme.codex),
        ("Cache", composition.cache, theme.gemini),
        ("Reason", composition.reasoning, theme.opencode),
    ];

    let labels = rows
        .iter()
        .map(|(label, value, _)| format!("{} {}", label, format_compact(*value)))
        .collect::<Vec<_>>();

    for (idx, ((_, value, color), label)) in rows.into_iter().zip(labels.iter()).enumerate() {
        let gauge = GradientGauge::new(label, composition.share(value))
            .width(inner.width.saturating_sub(18) as usize)
            .color(color);
        f.render_widget(gauge, sections[idx]);
    }

    let footer = subtitle.unwrap_or_else(|| format!("Total {}", format_compact(composition.total)));
    f.render_widget(
        Paragraph::new(format!(
            "{}  |  total {}",
            footer,
            format_compact(composition.total)
        ))
        .style(Style::default().fg(theme.dim)),
        sections[4],
    );
}

fn render_provider_flow_card(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Provider Flow 14d ",
            Style::default().fg(theme.gemini).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(4), Constraint::Length(1)])
        .split(inner);

    let recent = dashboard.recent_days(14);
    if recent.is_empty() {
        f.render_widget(
            Paragraph::new("No provider activity").style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    let chart_data = recent
        .iter()
        .map(|day| {
            let mut segments = HashMap::new();
            for (provider, stats) in &day.providers {
                segments.insert(provider.as_str(), stats.tokens as f64);
            }
            (day.total_tokens as f64, segments)
        })
        .collect::<Vec<_>>();

    let chart = StackedBarChart::new(&chart_data)
        .color("claude", theme.claude)
        .color("codex", theme.codex)
        .color("opencode", theme.opencode)
        .color("gemini", theme.gemini)
        .color("pi", theme.pi)
        .color("antigravity", theme.antigravity)
        .width(sections[0].width as usize)
        .height(sections[0].height as usize);
    f.render_widget(chart, sections[0]);

    let legend = Line::from(vec![
        Span::styled(
            recent
                .first()
                .map(|day| day.date.format("%m-%d").to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            Style::default().fg(theme.dim),
        ),
        Span::raw(" "),
        Span::styled("CLA", Style::default().fg(theme.claude).bold()),
        Span::raw(" "),
        Span::styled("CDX", Style::default().fg(theme.codex).bold()),
        Span::raw(" "),
        Span::styled("OCD", Style::default().fg(theme.opencode).bold()),
        Span::raw(" "),
        Span::styled("GEM", Style::default().fg(theme.gemini).bold()),
        Span::raw(" "),
        Span::styled(
            recent
                .last()
                .map(|day| day.date.format("%m-%d").to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            Style::default().fg(theme.dim),
        ),
    ]);
    f.render_widget(Paragraph::new(legend), sections[1]);
}

fn render_rollup_summary_card(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Rollups ",
            Style::default().fg(theme.claude).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let latest_week = dashboard.weekly.last();
    let previous_week = dashboard.weekly.iter().rev().nth(1);
    let current_month = dashboard.monthly.last();
    let previous_month = dashboard.monthly.iter().rev().nth(1);

    let lines = vec![
        render_rollup_summary_line("Week", latest_week, theme.codex),
        render_rollup_summary_line("Prev W", previous_week, theme.gemini),
        render_rollup_summary_line("Month", current_month, theme.claude),
        render_rollup_summary_line("Prev M", previous_month, theme.opencode),
    ];

    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }),
        inner,
    );
}

fn render_rollup_summary_line(
    label: &str,
    rollup: Option<&UsageRollup>,
    color: Color,
) -> Line<'static> {
    match rollup {
        Some(rollup) => Line::from(vec![
            Span::styled(format!("{:<6}", label), Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{:>6}", format_compact(rollup.total_tokens)),
                Style::default().fg(color).bold(),
            ),
            Span::raw("  "),
            Span::styled(
                format!("${:.2}", rollup.total_cost_usd),
                Style::default().fg(Color::White),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{}d", rollup.active_days),
                Style::default().fg(Color::Gray),
            ),
        ]),
        None => Line::from(vec![
            Span::styled(format!("{:<6}", label), Style::default().fg(Color::Gray)),
            Span::styled("No data", Style::default().fg(Color::Gray)),
        ]),
    }
}

fn render_by_model_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    summary: &UsageSummary,
    theme: &Theme,
) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(12)])
        .split(area);

    render_metric_row(f, split[0], summary, dashboard, theme);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(split[1]);

    render_rankings_card(
        f,
        bottom[0],
        "Provider Rank",
        &dashboard.by_provider,
        theme,
        true,
    );
    render_model_table_card(f, bottom[1], &dashboard.by_model, theme);
}

fn render_metric_row(
    f: &mut ratatui::Frame,
    area: Rect,
    summary: &UsageSummary,
    dashboard: &UsageDashboard,
    theme: &Theme,
) {
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    let latest_week = dashboard.weekly.last();
    let current_month = dashboard.monthly.last();

    render_card(
        f,
        cards[0],
        "Total Cost",
        &format!("${:.2}", summary.total_cost),
        &latest_week
            .map(|week| format!("Week ${:.2}", week.total_cost_usd))
            .unwrap_or_else(|| "No weekly data".to_string()),
        theme.gauge_high,
        theme,
    );
    render_card(
        f,
        cards[1],
        "Total Tokens",
        &format_compact(summary.total_tokens),
        &latest_week
            .map(|week| format!("Week {}", format_compact(week.total_tokens)))
            .unwrap_or_else(|| "No weekly data".to_string()),
        theme.gauge_mid,
        theme,
    );
    render_card(
        f,
        cards[2],
        "Messages",
        &format_compact(dashboard.total_messages as i64),
        &current_month
            .map(|month| format!("Month {}", format_compact(month.message_count)))
            .unwrap_or_else(|| "No monthly data".to_string()),
        theme.claude,
        theme,
    );
    render_card(
        f,
        cards[3],
        "Sessions",
        &format_compact(dashboard.total_sessions as i64),
        &current_month
            .map(|month| format!("Month {} active days", format_compact(month.active_days)))
            .unwrap_or_else(|| "No monthly data".to_string()),
        theme.opencode,
        theme,
    );
}

fn render_card(
    f: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    value: &str,
    detail: &str,
    color: Color,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(color).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));

    let content = Paragraph::new(vec![
        Line::from(Span::styled(
            value.to_string(),
            Style::default().fg(color).bold(),
        )),
        Line::from(Span::styled(
            detail.to_string(),
            Style::default().fg(Color::Gray),
        )),
    ])
    .block(block)
    .alignment(Alignment::Left)
    .wrap(ratatui::widgets::Wrap { trim: true });

    f.render_widget(content, area);
}

fn render_rankings_card(
    f: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    rankings: &[ProviderSummary],
    theme: &Theme,
    include_cost: bool,
) {
    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", title),
            Style::default().fg(theme.claude).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if rankings.is_empty() {
        f.render_widget(
            Paragraph::new("No provider data").style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    let max_rows = inner.height as usize;
    for (idx, row) in rankings.iter().take(max_rows).enumerate() {
        let y = inner.y + idx as u16;
        let label = if include_cost {
            format!(
                "{} {} | ${:.2}",
                row.provider.to_uppercase(),
                format_compact(row.tokens),
                row.cost
            )
        } else {
            format!(
                "{} {}",
                row.provider.to_uppercase(),
                format_compact(row.tokens)
            )
        };
        let label = truncate(&label, inner.width.saturating_sub(10) as usize);
        let gauge = GradientGauge::new(&label, row.percent)
            .width(inner.width.saturating_sub(2) as usize)
            .color(theme.provider_color(&row.provider));
        f.render_widget(gauge, Rect::new(inner.x, y, inner.width, 1));
    }
}

fn render_model_table_card(
    f: &mut ratatui::Frame,
    area: Rect,
    models: &[ModelSummary],
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Model Rank ",
            Style::default().fg(theme.codex).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = models
        .iter()
        .take(inner.height.saturating_sub(1) as usize)
        .map(|model| {
            vec![
                truncate(&model.source, 10),
                truncate(&model.model, 36),
                format_compact(model.tokens),
                format!("${:.2}", model.cost),
            ]
        })
        .collect::<Vec<_>>();

    let mut table = StyledTable::new(vec!["Source", "Model", "Tokens", "Cost"])
        .widths(vec![12, 38, 12, 12])
        .header_color(theme.codex);

    for row in rows {
        table = table.row(row);
    }

    f.render_widget(table, inner);
}

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
