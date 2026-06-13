use super::{
    empty_data_message, format_compact, format_cost_compact, metric_style,
    normalized_scroll_offset, percent_delta_color, percent_delta_text, selected_row_style,
    DailyStats, OverviewMetric, SortField, UsageDashboard, UsageState,
};
use crate::tui::theme::Theme;
use chrono::{Datelike, Duration, Local, NaiveDate};
use ratatui::{
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::collections::{BTreeSet, HashMap};

pub fn render_daily_page(
    f: &mut ratatui::Frame,
    area: Rect,
    dashboard: &UsageDashboard,
    state: &UsageState,
    theme: &Theme,
) {
    render_daily_table(f, area, dashboard, state, theme);
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

    let days = sorted_daily_rows(dashboard, state);
    if days.is_empty() {
        f.render_widget(
            Paragraph::new(empty_data_message(state, "No daily data"))
                .style(Style::default().fg(theme.dim)),
            inner,
        );
        return;
    }

    let today = Local::now().date_naive();
    let show_wow = inner.width >= 100;
    let show_detail_cols = inner.width >= 80;
    // Split cache into read/write columns; abbreviate the headers when space is tight.
    let cache_split_wide = inner.width >= 90;
    let cache_col_width = if cache_split_wide { 10usize } else { 7usize };
    let (cache_r_header, cache_w_header) = if cache_split_wide {
        ("Cache R", "Cache W")
    } else {
        ("CR", "CW")
    };
    let cache_color = Color::Rgb(251, 146, 60);

    let value_by_date: HashMap<NaiveDate, f64> = days
        .iter()
        .map(|day| (day.date, daily_metric_value(day, state.daily_metric)))
        .collect();

    // Header
    let header_y = inner.y;
    let date_width = 15usize;
    let wow_width = 11usize;
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
            format!(
                "{:<date_width$}",
                format!("{}{}", "Date", sort_indicator(SortField::Date))
            ),
            Style::default().fg(theme.accent_soft).bold(),
        ),
        Span::styled(
            format!(
                "{:<10}",
                format!("{}{}", "Tokens", sort_indicator(SortField::Tokens))
            ),
            Style::default().fg(Color::Rgb(52, 211, 153)).bold(),
        ),
        Span::styled(
            format!(
                "{:<10}",
                format!("{}{}", "Cost", sort_indicator(SortField::Cost))
            ),
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
                format!("{:<cache_col_width$}", cache_r_header),
                Style::default().fg(cache_color).bold(),
            ),
            Span::styled(
                format!("{:<cache_col_width$}", cache_w_header),
                Style::default().fg(cache_color).bold(),
            ),
        ]);
    }
    header_spans.push(Span::styled(
        format!("{:<8}", "Msgs"),
        Style::default().fg(Color::Rgb(96, 165, 250)).bold(),
    ));
    if show_wow {
        header_spans.push(Span::styled(
            format!("{:<wow_width$}", state.daily_metric.daily_vs7d_header()),
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
                .bg(theme.today_bg)
                .bold()
        } else {
            Style::default().fg(theme.accent_soft)
        };
        let row_bg = if is_today { Some(theme.today_bg) } else { None };
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
                    format!(
                        "{:<cache_col_width$}",
                        format_compact(day.cache_read_tokens)
                    ),
                    selected_row_style(metric_style(cache_color, row_bg), selected, theme),
                ),
                Span::styled(
                    format!(
                        "{:<cache_col_width$}",
                        format_compact(day.cache_write_tokens)
                    ),
                    selected_row_style(metric_style(cache_color, row_bg), selected, theme),
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
            let current = daily_metric_value(day, state.daily_metric);
            let prior = value_by_date.get(&prior_date).copied();
            let wow_text = percent_delta_text(current, prior);
            let wow_color = percent_delta_color(current, prior, theme);
            spans.push(Span::styled(
                format!("{:<wow_width$}", wow_text),
                selected_row_style(metric_style(wow_color, row_bg), selected, theme),
            ));
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
}

pub fn visible_daily_rows(
    dashboard: &UsageDashboard,
    enabled: &BTreeSet<String>,
) -> Vec<DailyStats> {
    dashboard
        .filtered_daily(enabled)
        .into_iter()
        .filter(|day| day.total_tokens > 0)
        .collect()
}

/// Daily rows in the exact order the table displays them: the visible
/// (non-empty) days sorted by the active sort field/direction. The table
/// renderer and the `n` "jump to today" shortcut both rely on this so the
/// selected-row index points at the same row in both places.
pub fn sorted_daily_rows(dashboard: &UsageDashboard, state: &UsageState) -> Vec<DailyStats> {
    let mut days = visible_daily_rows(dashboard, &state.enabled_sources);
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
    days
}

pub fn daily_metric_value(day: &DailyStats, metric: OverviewMetric) -> f64 {
    match metric {
        OverviewMetric::Tokens => day.total_tokens as f64,
        OverviewMetric::Cost => day.cost_usd,
    }
}
