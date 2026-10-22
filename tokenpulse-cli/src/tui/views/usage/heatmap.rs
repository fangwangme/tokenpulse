use super::{
    build_agent_model_groups, empty_data_message, format_compact, format_cost_compact,
    format_int_commas, truncate, DailyStats, UsageDashboard, UsageState,
};
use crate::tui::theme::Theme;
use crate::tui::widgets::{date_at_position, HeatmapMetric, YearHeatmap};
use chrono::{Datelike, Duration, Local, NaiveDate};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::collections::BTreeSet;
use tokenpulse_core::usage::week_start;

const HEATMAP_LEGEND_BUCKETS: usize = 5;
const LEGEND_COLOR_BLOCKS_OFFSET: u16 = 43;

pub fn render_heatmap_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
) {
    render_heatmap_page_on(f, area, dashboard, state, theme, Local::now().date_naive());
}

fn render_heatmap_page_on(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
    today: NaiveDate,
) {
    let selected_day =
        dashboard.selected_day_in_fixed_window(state.selected_heatmap_date, &state.enabled_sources);
    let bounds = dashboard.bounds_for_fixed_window();

    let sections = heatmap_sections(area);

    // Heatmap grid
    let heat_title = " Usage Activity - Past 365 Days ";
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
    let points = dashboard.points_in_fixed_window(state.heatmap_metric, &state.enabled_sources);
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
            .selected_bucket(state.selected_heatmap_legend_bucket)
            .range_opt(bounds);
        f.render_widget(heatmap, heat_inner);
    }

    // Bottom panels: range summary and selected day detail
    let info = heatmap_info_sections(sections[1]);

    render_heatmap_summary_card(f, info[0], dashboard, &state.enabled_sources, theme, today);
    render_heatmap_day_detail(
        f,
        info[1],
        selected_day.as_ref(),
        state.heatmap_metric,
        state.heatmap_detail_scroll,
        theme,
    );
}

pub fn heatmap_sections(area: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(12), Constraint::Min(10)])
        .split(area)
}

pub fn heatmap_info_sections(area: Rect) -> std::rc::Rc<[Rect]> {
    if area.width >= 100 && area.height >= 10 {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(40), Constraint::Min(40)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(15), Constraint::Min(6)])
            .split(area)
    }
}

pub fn heatmap_grid_area(area: Rect) -> Rect {
    let sections = heatmap_sections(area);
    Block::default().borders(Borders::ALL).inner(sections[0])
}

pub fn heatmap_legend_bucket_at_position(area: Rect, column: u16, row: u16) -> Option<usize> {
    let sections = heatmap_sections(area);
    let heat_inner = Block::default().borders(Borders::ALL).inner(sections[0]);
    let footer_y = heat_inner.y + heat_inner.height.saturating_sub(1);
    if row != footer_y {
        return None;
    }

    let blocks_start = heat_inner.x + LEGEND_COLOR_BLOCKS_OFFSET;
    let bucket_width = 2; // width of "██" is 2
    let blocks_width = HEATMAP_LEGEND_BUCKETS as u16 * bucket_width;
    if column < blocks_start || column >= blocks_start + blocks_width {
        return None;
    }

    Some(((column - blocks_start) / bucket_width) as usize)
}

pub fn heatmap_day_panel_area(area: Rect) -> Rect {
    let sections = heatmap_sections(area);
    let info = heatmap_info_sections(sections[1]);
    info[1]
}

pub fn selected_day_detail_body_visible_rows(area: Rect, total_lines: usize) -> usize {
    selected_day_detail_content_area(area, total_lines).height as usize
}

pub fn selected_day_detail_content_area(area: Rect, total_lines: usize) -> Rect {
    if area.height > 0 && total_lines > area.height as usize {
        Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1))
    } else {
        area
    }
}

pub fn selected_day_detail_hint_area(area: Rect, total_lines: usize) -> Option<Rect> {
    if area.height > 0 && total_lines > area.height as usize {
        Some(Rect::new(
            area.x,
            area.y + area.height.saturating_sub(1),
            area.width,
            1,
        ))
    } else {
        None
    }
}

pub fn heatmap_date_at_position(
    area: Rect,
    dashboard: &UsageDashboard,
    column: u16,
    row: u16,
) -> Option<NaiveDate> {
    let body = super::dashboard_body_area(area);
    let grid_area = heatmap_grid_area(body);
    let bounds = dashboard.bounds_for_fixed_window();
    date_at_position(grid_area, bounds, column, row)
}

fn render_heatmap_summary_card(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    enabled: &BTreeSet<String>,
    theme: &Theme,
    today: NaiveDate,
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

    let days = dashboard.filtered_daily(enabled);

    let active_days = dashboard.active_days_in_fixed_window(HeatmapMetric::TotalTokens, enabled);
    let current_streak =
        dashboard.current_streak_in_fixed_window(HeatmapMetric::TotalTokens, enabled);
    let longest_streak =
        dashboard.longest_streak_in_fixed_window(HeatmapMetric::TotalTokens, enabled);

    let mut overview_rows = range_overview_rows(&days, today);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Active Days ", Style::default().fg(theme.dim)),
            Span::styled(
                format!("{} days", active_days),
                Style::default().fg(theme.fg).bold(),
            ),
        ]),
        Line::from(vec![
            Span::styled("Streak      ", Style::default().fg(theme.dim)),
            Span::styled(
                format!("{}/{}", current_streak, longest_streak),
                Style::default().fg(theme.fg).bold(),
            ),
            Span::styled(" cur/best", Style::default().fg(theme.dim)),
        ]),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Period      ", Style::default().fg(theme.dim).underlined()),
            Span::raw(" "),
            Span::styled(
                "Tokens    ",
                Style::default()
                    .fg(Color::Rgb(52, 211, 153))
                    .bold()
                    .underlined(),
            ),
            Span::raw(" "),
            Span::styled(
                "Cost      ",
                Style::default()
                    .fg(Color::Rgb(250, 204, 21))
                    .bold()
                    .underlined(),
            ),
        ]),
    ];

    // When the card is short, drop the spacer and then the rolling rows so the
    // calendar rows and Total/Average/Max stay visible before trailing rows go.
    let max_lines = inner.height as usize;
    if lines.len() + overview_rows.len() > max_lines {
        lines.remove(2);
    }
    while lines.len() + overview_rows.len() > max_lines {
        let Some(index) = overview_rows.iter().rposition(|row| row.rolling) else {
            break;
        };
        overview_rows.remove(index);
    }

    for row in &overview_rows {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:width$}", row.label, width = 12),
                Style::default().fg(theme.dim),
            ),
            Span::raw(" "),
            Span::styled(
                format!(
                    "{:width$}",
                    format_compact(row.tokens.round() as i64),
                    width = 10
                ),
                Style::default().fg(Color::Rgb(52, 211, 153)),
            ),
            Span::raw(" "),
            Span::styled(
                format_cost_compact(row.cost_usd),
                Style::default().fg(Color::Rgb(250, 204, 21)),
            ),
        ]));
    }
    lines.truncate(max_lines);

    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true }),
        inner,
    );
}

struct RangeOverviewRow {
    label: &'static str,
    tokens: f64,
    cost_usd: f64,
    rolling: bool,
}

/// Window rows in display order, with whether each is a rolling window.
const RANGE_WINDOWS: [(&str, bool); 7] = [
    ("Today", false),
    ("Week", false),
    ("Last 7 days", true),
    ("Month", false),
    ("Last 30 days", true),
    ("Year", false),
    ("Total", false),
];

fn range_overview_rows(days: &[DailyStats], today: NaiveDate) -> Vec<RangeOverviewRow> {
    let week_start = week_start(today).unwrap_or(today);
    let last7_start = today - Duration::days(6);
    let last30_start = today - Duration::days(29);

    let mut sums = [(0i64, 0.0f64); RANGE_WINDOWS.len()];
    let (mut max_tokens, mut max_cost) = (0i64, 0.0f64);
    for day in days {
        let date = day.date;
        let since = |start: NaiveDate| date >= start && date <= today;
        let in_window = [
            date == today,
            since(week_start),
            since(last7_start),
            date.year() == today.year() && date.month() == today.month(),
            since(last30_start),
            date.year() == today.year(),
            true,
        ];
        for (sum, hit) in sums.iter_mut().zip(in_window) {
            if hit {
                sum.0 += day.total_tokens;
                sum.1 += day.cost_usd;
            }
        }
        max_tokens = max_tokens.max(day.total_tokens);
        max_cost = max_cost.max(day.cost_usd);
    }

    let (total_tokens, total_cost) = sums[RANGE_WINDOWS.len() - 1];
    let day_count = days.len().max(1) as f64;
    let mut rows: Vec<RangeOverviewRow> = RANGE_WINDOWS
        .iter()
        .zip(sums)
        .map(|(&(label, rolling), (tokens, cost_usd))| RangeOverviewRow {
            label,
            tokens: tokens as f64,
            cost_usd,
            rolling,
        })
        .collect();
    rows.push(RangeOverviewRow {
        label: "Average",
        tokens: total_tokens as f64 / day_count,
        cost_usd: total_cost / day_count,
        rolling: false,
    });
    rows.push(RangeOverviewRow {
        label: "Max",
        tokens: max_tokens as f64,
        cost_usd: max_cost,
        rolling: false,
    });
    rows
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
    if area.width >= 76 && area.height >= 5 {
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

    let lines = if area.width >= 34 && area.height >= 5 {
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
                    format!("{:>8}", format_cost_compact(day.cost_usd)),
                    Style::default().fg(Color::Rgb(250, 204, 21)),
                ),
                Span::raw("  "),
                Span::styled("Tokens ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_compact(day.total_tokens)),
                    Style::default().fg(Color::Rgb(52, 211, 153)),
                ),
            ]),
            Line::from(vec![
                Span::styled("In   ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_compact(day.input_tokens)),
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                ),
                Span::raw("  "),
                Span::styled("Out    ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_compact(day.output_tokens)),
                    Style::default().fg(Color::Rgb(167, 139, 250)),
                ),
            ]),
            Line::from(vec![
                Span::styled("CR   ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_compact(day.cache_read_tokens)),
                    Style::default().fg(Color::Rgb(251, 146, 60)),
                ),
                Span::raw("  "),
                Span::styled("CW     ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_compact(day.cache_write_tokens)),
                    Style::default().fg(Color::Rgb(251, 146, 60)),
                ),
            ]),
            Line::from(vec![
                Span::styled("Msgs ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_int_commas(day.messages)),
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                ),
                Span::raw("  "),
                Span::styled("Sess   ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_int_commas(day.sessions)),
                    Style::default().fg(theme.fg),
                ),
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
                Span::styled("In ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.input_tokens),
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                ),
                Span::raw("  "),
                Span::styled("Out ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.output_tokens),
                    Style::default().fg(Color::Rgb(167, 139, 250)),
                ),
                Span::raw("  "),
                Span::styled("CR ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.cache_read_tokens),
                    Style::default().fg(Color::Rgb(251, 146, 60)),
                ),
                Span::raw("  "),
                Span::styled("CW ", Style::default().fg(theme.dim)),
                Span::styled(
                    format_compact(day.cache_write_tokens),
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

    let cost_width = 10usize.min(area.width.saturating_sub(12) as usize).max(8);
    let model_width = 24usize.min(area.width.saturating_sub((cost_width + 3) as u16) as usize);
    let model_width_inner = model_width.saturating_sub(2);
    for group in groups {
        let agent_name = super::display_source_name(&group.source);
        let agent_color = theme.provider_color(&group.source);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<model_width$}", truncate(agent_name, model_width)),
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
                    format!(
                        "{:<model_width_inner$}",
                        truncate(&model_name, model_width_inner)
                    ),
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
                    format!("{:>8}", format_compact(stats.tokens)),
                    Style::default().fg(Color::Rgb(52, 211, 153)),
                ),
                Span::raw(" "),
                Span::styled("I ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_compact(stats.input_tokens)),
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                ),
                Span::raw(" "),
                Span::styled("O ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_compact(stats.output_tokens)),
                    Style::default().fg(Color::Rgb(167, 139, 250)),
                ),
                Span::raw(" "),
                Span::styled("CR ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_compact(stats.cache_read_tokens)),
                    Style::default().fg(Color::Rgb(251, 146, 60)),
                ),
                Span::raw(" "),
                Span::styled("CW ", Style::default().fg(theme.dim)),
                Span::styled(
                    format!("{:>8}", format_compact(stats.cache_write_tokens)),
                    Style::default().fg(Color::Rgb(251, 146, 60)),
                ),
            ]));
        }
    }

    let total_lines = lines.len();
    let visible = selected_day_detail_body_visible_rows(area, total_lines);
    let offset = scroll_offset.min(total_lines.saturating_sub(visible));
    let visible_lines: Vec<Line> = lines.into_iter().skip(offset).take(visible).collect();
    let content_area = selected_day_detail_content_area(area, total_lines);
    let hint_area = selected_day_detail_hint_area(area, total_lines);

    f.render_widget(Paragraph::new(visible_lines), content_area);

    if let Some(hint_area) = hint_area {
        let hint = Line::from(vec![Span::styled(
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
        f.render_widget(Paragraph::new(hint).alignment(Alignment::Right), hint_area);
    }
}

pub fn heatmap_day_panel_line_count(day: Option<&DailyStats>) -> usize {
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

pub fn heatmap_palette(theme: &Theme, metric: HeatmapMetric) -> [Color; 5] {
    match metric {
        HeatmapMetric::TotalTokens => theme.token_heatmap,
        HeatmapMetric::Cost => theme.cost_heatmap,
    }
}

pub fn heatmap_detail_scroll_max(
    dashboard: &UsageDashboard,
    state: &UsageState,
    frame_area: Rect,
) -> usize {
    let body = super::dashboard_body_area(frame_area);
    let panel_outer = heatmap_day_panel_area(body);
    let inner = Block::default().borders(Borders::ALL).inner(panel_outer);
    let sections = selected_day_sections(inner);
    let detail_area = sections[1];
    let selected_day =
        dashboard.selected_day_in_fixed_window(state.selected_heatmap_date, &state.enabled_sources);
    let total_lines = heatmap_day_panel_line_count(selected_day.as_ref());
    let visible_lines = selected_day_detail_body_visible_rows(detail_area, total_lines);
    total_lines.saturating_sub(visible_lines)
}

fn format_metric(metric: HeatmapMetric, value: f64) -> String {
    match metric {
        HeatmapMetric::Cost => format_cost_compact(value),
        HeatmapMetric::TotalTokens => format_compact(value.round() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::views::usage::DayBreakdown;
    use ratatui::{backend::TestBackend, Terminal};

    const ROW_LABELS: [&str; 9] = [
        "Today",
        "Week",
        "Last 7 days",
        "Month",
        "Last 30 days",
        "Year",
        "Total",
        "Average",
        "Max",
    ];

    fn fixed_today() -> NaiveDate {
        // A Wednesday, so the natural week has data before and on it.
        NaiveDate::from_ymd_opt(2024, 1, 10).unwrap()
    }

    fn test_day(date: NaiveDate, tokens: i64) -> DailyStats {
        let mut day = DailyStats::empty(date);
        day.total_tokens = tokens;
        day.cost_usd = tokens as f64 / 100.0;
        day
    }

    fn sample_days() -> Vec<DailyStats> {
        vec![
            test_day(NaiveDate::from_ymd_opt(2023, 12, 11).unwrap(), 40),
            test_day(NaiveDate::from_ymd_opt(2023, 12, 12).unwrap(), 20),
            test_day(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(), 100),
            test_day(NaiveDate::from_ymd_opt(2024, 1, 3).unwrap(), 80),
            test_day(NaiveDate::from_ymd_opt(2024, 1, 7).unwrap(), 200),
            test_day(NaiveDate::from_ymd_opt(2024, 1, 8).unwrap(), 300),
            test_day(NaiveDate::from_ymd_opt(2024, 1, 9).unwrap(), 400),
            test_day(NaiveDate::from_ymd_opt(2024, 1, 10).unwrap(), 500),
        ]
    }

    fn row<'a>(rows: &'a [RangeOverviewRow], label: &str) -> &'a RangeOverviewRow {
        rows.iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| panic!("missing row {label}"))
    }

    #[test]
    fn natural_week_and_rolling_windows() {
        let rows = range_overview_rows(&sample_days(), fixed_today());

        // Monday 2024-01-08 through today; the previous Sunday stays out.
        assert_eq!(row(&rows, "Week").tokens, 1200.0);
        assert_eq!(row(&rows, "Week").cost_usd, 12.0);
        // today-6 .. today; 2024-01-03 stays out.
        assert_eq!(row(&rows, "Last 7 days").tokens, 1400.0);
        // today-29 .. today; 2023-12-11 stays out.
        assert_eq!(row(&rows, "Last 30 days").tokens, 1600.0);
        // Month stays month-to-date.
        assert_eq!(row(&rows, "Month").tokens, 1580.0);

        // On a Monday the natural week is just today.
        let monday = NaiveDate::from_ymd_opt(2024, 1, 8).unwrap();
        let rows = range_overview_rows(&sample_days(), monday);
        assert_eq!(row(&rows, "Week").tokens, 300.0);
    }

    #[test]
    fn rows_keep_contract_order() {
        let rows = range_overview_rows(&sample_days(), fixed_today());
        let labels: Vec<&str> = rows.iter().map(|row| row.label).collect();

        assert_eq!(labels, ROW_LABELS);
        // The label column must fit the longest label without shifting values.
        assert!(labels.iter().all(|label| label.chars().count() <= 12));
    }

    #[test]
    fn range_overview_card_renders_rows_and_keeps_totals_when_short() {
        let mut days = sample_days();
        // A seven-figure cost must not wrap and push the last rows out.
        let mut big = test_day(NaiveDate::from_ymd_opt(2023, 6, 1).unwrap(), 1);
        big.cost_usd = 1_000_000.0;
        days.insert(0, big);
        let days: Vec<DailyStats> = days
            .into_iter()
            .map(|mut day| {
                let breakdown = DayBreakdown {
                    tokens: day.total_tokens,
                    cost_usd: day.cost_usd,
                    ..DayBreakdown::default()
                };
                day.providers.insert("claude".to_string(), breakdown);
                day
            })
            .collect();
        let dashboard = UsageDashboard { daily: days };
        let state = UsageState::new(&dashboard, Vec::new());
        let theme = Theme::default();
        let header = format!("{:12} {:10} Cost", "Period", "Tokens");
        let week = format!("{:12} {:10} {}", "Week", "1.20 K", "$12.00");
        let max = format!("{:12} {:10} {}", "Max", "500.00  ", "$1,000,000.00");

        // (width, height, rolling rows expected to fit)
        for (width, height, rolling) in [
            (120u16, 40u16, ["Last 7 days", "Last 30 days"].as_slice()),
            (120, 35, ["Last 7 days"].as_slice()),
            (80, 45, ["Last 7 days", "Last 30 days"].as_slice()),
            (80, 40, [].as_slice()),
        ] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    render_heatmap_page_on(
                        frame,
                        super::super::dashboard_body_area(frame.area()),
                        &dashboard,
                        &state,
                        &theme,
                        fixed_today(),
                    );
                })
                .unwrap();
            let text = buffer_text(terminal.backend(), width, height);

            for expected in [header.as_str(), week.as_str(), max.as_str()] {
                assert!(
                    text.contains(expected),
                    "missing {expected:?} at {width}x{height}:\n{text}"
                );
            }
            for label in ROW_LABELS {
                let is_rolling = label.starts_with("Last ");
                assert_eq!(
                    text.contains(label),
                    !is_rolling || rolling.contains(&label),
                    "row {label} at {width}x{height}:\n{text}"
                );
            }
        }
    }

    fn buffer_text(backend: &TestBackend, width: u16, height: u16) -> String {
        let buffer = backend.buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
