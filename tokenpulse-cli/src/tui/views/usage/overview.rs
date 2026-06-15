use super::{
    empty_data_message, format_compact, format_cost_compact, format_source_list, model_id_from_key,
    model_source, normalized_scroll_offset, selected_row_style, share_percent, truncate,
    OverviewMetric, UsageDashboard, UsageState,
};
use crate::tui::theme::Theme;
use crate::tui::widgets::StackedBarChart;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::collections::HashMap;
use tokenpulse_core::usage::{ModelSummary, UsageSummary};

pub fn render_overview_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
) {
    let sections = overview_sections(area);
    render_overview_chart(f, sections[0], dashboard, state, theme);
    render_overview_top_models(f, sections[1], dashboard, summary, state, theme);
}

pub fn overview_sections(area: Rect) -> std::rc::Rc<[Rect]> {
    let chart_height = (area.height.saturating_mul(3) / 5).max(8).min(area.height);
    let model_height = area.height.saturating_sub(chart_height);

    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(chart_height),
            Constraint::Length(model_height),
        ])
        .split(area)
}

fn render_overview_chart(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
) {
    let temp_block = Block::default().borders(Borders::ALL);
    let inner_temp = temp_block.inner(area);
    let y_axis_width = 7u16;
    let chart_width = inner_temp.width.saturating_sub(y_axis_width + 1) as usize;

    let bar_width = if chart_width < 60 {
        1
    } else if chart_width < 150 {
        2
    } else {
        3
    };
    let limit = chart_width / bar_width;

    let recent = dashboard.recent_days(limit);
    let displayed_days = recent.len();

    let metric_name = match state.overview_metric {
        OverviewMetric::Tokens => "Token Usage",
        OverviewMetric::Cost => "Cost Usage",
    };
    let title = format!(" {} ({} days) ", metric_name, displayed_days);

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

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

    // X-axis date ticks (oldest first); the widget shows the first/last plus a
    // few evenly spaced dates in between to make bars easy to locate.
    let x_labels: Vec<String> = recent
        .iter()
        .map(|day| day.date.format("%m-%d").to_string())
        .collect();

    let chart = StackedBarChart::new(&chart_data, bar_width)
        .color("openai", theme.company_color("openai"))
        .color("google", theme.company_color("google"))
        .color("anthropic", theme.company_color("anthropic"))
        .color("other", theme.company_color("other"))
        .value_format(state.overview_metric.value_format())
        .x_labels(&x_labels);
    f.render_widget(chart, sections[0]);

    // Legend: provider colors only — dates now live on the chart's X axis.
    let provider_legend: &[(&str, &str, Color)] = &[
        ("anthropic", "Anthropic", theme.company_color("anthropic")),
        ("openai", "OpenAI", theme.company_color("openai")),
        ("google", "Google", theme.company_color("google")),
        ("other", "Others", theme.company_color("other")),
    ];
    let mut legend_spans = Vec::with_capacity(provider_legend.len() * 2);
    for (_, label, color) in provider_legend {
        legend_spans.push(Span::styled(
            format!("● {}", label),
            Style::default().fg(*color),
        ));
        legend_spans.push(Span::raw("  "));
    }

    f.render_widget(Paragraph::new(Line::from(legend_spans)), sections[1]);
}

fn render_overview_top_models(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    _summary: &UsageSummary,
    state: &UsageState,
    theme: &Theme,
) {
    let mut filtered = dashboard.filtered_models(&state.enabled_sources);
    sort_overview_models(&mut filtered, state.overview_metric);
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
    let total_width = inner.width as usize;
    let share_total = filtered
        .iter()
        .map(|entry| overview_model_metric_value(entry, state.overview_metric))
        .sum::<f64>();
    let pct_width = 7usize;
    let cost_width = 8usize;
    let tokens_width = 9usize;
    let fixed_width = tokens_width + cost_width + pct_width + 4;
    let available_for_both = total_width.saturating_sub(fixed_width);
    let (model_width, agent_width) = if available_for_both < 32 {
        let m_w = 22usize.min(available_for_both);
        let a_w = available_for_both.saturating_sub(m_w);
        (m_w, a_w)
    } else {
        let a_w = (available_for_both * 3 / 5).clamp(10, 40);
        let m_w = available_for_both.saturating_sub(a_w);
        if m_w < 22 {
            (22usize, available_for_both.saturating_sub(22))
        } else {
            (m_w, a_w)
        }
    };

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
        let pct = share_percent(
            overview_model_metric_value(model, state.overview_metric),
            share_total,
        );
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

fn overview_model_visible_rows(area_height: u16) -> usize {
    area_height.saturating_sub(3) as usize
}

fn sort_overview_models(models: &mut [ModelSummary], metric: OverviewMetric) {
    models.sort_by(|left, right| match metric {
        OverviewMetric::Tokens => right
            .tokens
            .cmp(&left.tokens)
            .then_with(|| {
                right
                    .cost
                    .partial_cmp(&left.cost)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| left.model.cmp(&right.model)),
        OverviewMetric::Cost => right
            .cost
            .partial_cmp(&left.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.tokens.cmp(&left.tokens))
            .then_with(|| left.model.cmp(&right.model)),
    });
}

fn overview_model_metric_value(model: &ModelSummary, metric: OverviewMetric) -> f64 {
    match metric {
        OverviewMetric::Tokens => model.tokens as f64,
        OverviewMetric::Cost => model.cost,
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
