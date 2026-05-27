use super::{
    build_agent_model_groups, empty_data_message, format_compact, format_cost_compact, truncate,
    DailyStats, UsageDashboard, UsageState,
};
use crate::tui::theme::Theme;
use crate::tui::widgets::{
    date_at_position, HeatmapMetric, YearHeatmap,
};
use chrono::{Datelike, Duration, Local, NaiveDate};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::collections::BTreeSet;

const HEATMAP_LEGEND_BUCKETS: usize = 5;

pub fn render_heatmap_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
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

    render_heatmap_summary_card(f, info[0], dashboard, &state.enabled_sources, theme);
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
        .constraints([
            Constraint::Length(12),
            Constraint::Min(10),
        ])
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
            .constraints([Constraint::Length(14), Constraint::Min(6)])
            .split(area)
    }
}

pub fn heatmap_grid_area(area: Rect) -> Rect {
    let sections = heatmap_sections(area);
    Block::default().borders(Borders::ALL).inner(sections[0])
}

pub fn heatmap_legend_bucket_at_position(
    area: Rect,
    column: u16,
    row: u16,
) -> Option<usize> {
    let sections = heatmap_sections(area);
    let heat_inner = Block::default().borders(Borders::ALL).inner(sections[0]);
    let footer_y = heat_inner.y + heat_inner.height.saturating_sub(1);
    if row != footer_y {
        return None;
    }

    let blocks_start = heat_inner.x + 43;
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
    let today = Local::now().date_naive();
    let week_start = today - Duration::days(6);

    let active_days = dashboard.active_days_in_fixed_window(HeatmapMetric::TotalTokens, enabled);
    let current_streak =
        dashboard.current_streak_in_fixed_window(HeatmapMetric::TotalTokens, enabled);
    let longest_streak =
        dashboard.longest_streak_in_fixed_window(HeatmapMetric::TotalTokens, enabled);

    // Tokens calculations
    let today_tokens = days
        .iter()
        .find(|d| d.date == today)
        .map(|d| d.total_tokens)
        .unwrap_or(0);
    let week_tokens: i64 = days
        .iter()
        .filter(|d| d.date >= week_start && d.date <= today)
        .map(|d| d.total_tokens)
        .sum();
    let month_tokens: i64 = days
        .iter()
        .filter(|d| d.date.year() == today.year() && d.date.month() == today.month())
        .map(|d| d.total_tokens)
        .sum();
    let year_tokens: i64 = days
        .iter()
        .filter(|d| d.date.year() == today.year())
        .map(|d| d.total_tokens)
        .sum();
    let total_tokens: i64 = days.iter().map(|d| d.total_tokens).sum();
    let max_tokens = days.iter().map(|d| d.total_tokens).max().unwrap_or(0);
    let avg_tokens = if days.is_empty() {
        0.0
    } else {
        total_tokens as f64 / days.len() as f64
    };

    // Cost calculations
    let today_cost = days
        .iter()
        .find(|d| d.date == today)
        .map(|d| d.cost_usd)
        .unwrap_or(0.0);
    let week_cost: f64 = days
        .iter()
        .filter(|d| d.date >= week_start && d.date <= today)
        .map(|d| d.cost_usd)
        .sum();
    let month_cost: f64 = days
        .iter()
        .filter(|d| d.date.year() == today.year() && d.date.month() == today.month())
        .map(|d| d.cost_usd)
        .sum();
    let year_cost: f64 = days
        .iter()
        .filter(|d| d.date.year() == today.year())
        .map(|d| d.cost_usd)
        .sum();
    let total_cost: f64 = days.iter().map(|d| d.cost_usd).sum();
    let max_cost = days.iter().map(|d| d.cost_usd).fold(0.0, f64::max);
    let avg_cost = if days.is_empty() {
        0.0
    } else {
        total_cost / days.len() as f64
    };

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
            Span::styled("Period     ", Style::default().fg(theme.dim).underlined()),
            Span::raw(" "),
            Span::styled(
                "Tokens      ",
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

    let mut add_row = |label: &str, tokens: f64, cost: f64, is_avg: bool| {
        let tokens_str = if is_avg {
            format_compact(tokens.round() as i64)
        } else {
            format_compact(tokens as i64)
        };
        let cost_str = format_cost_compact(cost);

        lines.push(Line::from(vec![
            Span::styled(
                format!("{:width$}", label, width = 10),
                Style::default().fg(theme.dim),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:width$}", tokens_str, width = 12),
                Style::default().fg(Color::Rgb(52, 211, 153)),
            ),
            Span::raw(" "),
            Span::styled(cost_str, Style::default().fg(Color::Rgb(250, 204, 21))),
        ]));
    };

    add_row("Today", today_tokens as f64, today_cost, false);
    add_row("Week", week_tokens as f64, week_cost, false);
    add_row("Month", month_tokens as f64, month_cost, false);
    add_row("Year", year_tokens as f64, year_cost, false);
    add_row("Total", total_tokens as f64, total_cost, false);
    add_row("Average", avg_tokens, avg_cost, true);
    add_row("Max", max_tokens as f64, max_cost, false);

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
        let agent_name = super::display_source_name(&group.source);
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
