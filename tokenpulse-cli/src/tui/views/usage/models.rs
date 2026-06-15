use super::{
    empty_data_message, format_compact, format_cost_compact, format_source_list, model_id_from_key,
    model_source, normalized_scroll_offset, selected_row_style, share_percent, sparkline_text,
    truncate, ModelTableRow, SortField, UsageDashboard, UsageState,
};
use crate::tui::theme::Theme;
use chrono::{Duration, Local, NaiveDate};
use ratatui::{
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::collections::HashMap;
use tokenpulse_core::usage::{normalize_model_name, ModelSummary, UsageSummary};

pub fn render_models_page(
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
    let input_width = 9usize;
    let output_width = 9usize;
    // Split cache into read/write columns; abbreviate the headers when space is tight.
    let cache_split_wide = total_width >= 100;
    let cache_read_width = if cache_split_wide { 9usize } else { 7usize };
    let cache_write_width = if cache_split_wide { 9usize } else { 7usize };
    let (cache_r_header, cache_w_header) = if cache_split_wide {
        ("Cache R", "Cache W")
    } else {
        ("CR", "CW")
    };
    let pct_width = 7usize;
    let msg_width = 8usize;
    let tokens_width = 9usize;
    let show_last = total_width >= 120;
    let last_width = if show_last { 11usize } else { 0usize };
    let show_sparkline = total_width >= 144;
    let sparkline_width = if show_sparkline { 15usize } else { 0usize };
    let last_spacer_width = if show_last { 1usize } else { 0usize };
    let trend_spacer_width = if show_sparkline { 2usize } else { 0usize };
    let total_spacers = 10usize + last_spacer_width + trend_spacer_width;
    let available_for_both = total_width
        .saturating_sub(
            rank_width
                + tokens_width
                + cost_width
                + input_width
                + output_width
                + cache_read_width
                + cache_write_width
                + pct_width
                + msg_width
                + last_width
                + sparkline_width
                + total_spacers,
        );
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
    let share_total = model_table_share_total(models.iter().copied(), state.sort_field);

    // Build per-model 7-day token sparkline data
    let today = dashboard
        .latest_date()
        .unwrap_or_else(|| Local::now().date_naive());
    let model_sparklines: HashMap<String, Vec<u64>> = if show_sparkline {
        let last_7: Vec<NaiveDate> = (0..7).map(|i| today - Duration::days(6 - i)).collect();
        let mut map: HashMap<String, Vec<u64>> = HashMap::new();
        for day_stat in &dashboard.daily {
            let day_idx = last_7.iter().position(|&d| d == day_stat.date);
            let Some(idx) = day_idx else { continue };
            for (model_key, breakdown) in &day_stat.models {
                if !state.is_source_enabled(model_source(model_key)) {
                    continue;
                }
                let norm = normalize_model_name(model_id_from_key(model_key));
                let entry = map.entry(norm).or_insert_with(|| vec![0u64; 7]);
                entry[idx] += breakdown.tokens.max(0) as u64;
            }
        }
        map
    } else {
        HashMap::new()
    };

    let headers = [
        "#", "Model", "Agent", "Tokens", "Cost", "Input", "Output", "Cache", "%", "Msgs", "Last",
        "Trend",
    ];
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
        Span::raw(" "),
        Span::styled(
            format!("{:<model_width$}", headers[1]),
            Style::default().fg(theme.accent).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:<agent_width$}", headers[2]),
            Style::default().fg(theme.accent_soft).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!(
                "{:>tokens_width$}",
                format!("{}{}", headers[3], sort_indicator(SortField::Tokens))
            ),
            Style::default().fg(Color::Rgb(52, 211, 153)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!(
                "{:>cost_width$}",
                format!("{}{}", headers[4], sort_indicator(SortField::Cost))
            ),
            Style::default().fg(Color::Rgb(250, 204, 21)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>input_width$}", headers[5]),
            Style::default().fg(Color::Rgb(96, 165, 250)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>output_width$}", headers[6]),
            Style::default().fg(Color::Rgb(167, 139, 250)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>cache_read_width$}", cache_r_header),
            Style::default().fg(Color::Rgb(251, 146, 60)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>cache_write_width$}", cache_w_header),
            Style::default().fg(Color::Rgb(251, 146, 60)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>pct_width$}", headers[8]),
            Style::default().fg(Color::Rgb(96, 165, 250)).bold(),
        ),
        Span::raw(" "),
        Span::styled(
            format!("{:>msg_width$}", headers[9]),
            Style::default().fg(Color::Rgb(96, 165, 250)).bold(),
        ),
    ];
    if show_last {
        header_spans.push(Span::raw(" "));
        header_spans.push(Span::styled(
            format!(
                "{:>last_width$}",
                format!("{}{}", headers[10], sort_indicator(SortField::Date))
            ),
            Style::default().fg(theme.dim).bold(),
        ));
    }
    if show_sparkline {
        header_spans.push(Span::raw("  "));
        header_spans.push(Span::styled(
            format!("{:<sparkline_width$}", headers[11]),
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
        let pct = model_share_percent(model, share_total, state.sort_field);

        let mut spans = vec![
            Span::styled(
                format!("{:<rank_width$}", rank),
                selected_row_style(Style::default().fg(theme.dim), selected, theme),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:<model_width$}", truncate(&model.model, model_width)),
                selected_row_style(Style::default().fg(model_color), selected, theme),
            ),
            Span::raw(" "),
            Span::styled(
                format!(
                    "{:<agent_width$}",
                    truncate(&format_source_list(&model.source), agent_width)
                ),
                selected_row_style(Style::default().fg(theme.accent_soft), selected, theme),
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
                format!("{:>input_width$}", format_compact(model.input_tokens)),
                selected_row_style(
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                    selected,
                    theme,
                ),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:>output_width$}", format_compact(model.output_tokens)),
                selected_row_style(
                    Style::default().fg(Color::Rgb(167, 139, 250)),
                    selected,
                    theme,
                ),
            ),
            Span::raw(" "),
            Span::styled(
                format!(
                    "{:>cache_read_width$}",
                    format_compact(model.cache_read_tokens)
                ),
                selected_row_style(
                    Style::default().fg(Color::Rgb(251, 146, 60)),
                    selected,
                    theme,
                ),
            ),
            Span::raw(" "),
            Span::styled(
                format!(
                    "{:>cache_write_width$}",
                    format_compact(model.cache_write_tokens)
                ),
                selected_row_style(
                    Style::default().fg(Color::Rgb(251, 146, 60)),
                    selected,
                    theme,
                ),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:>pct_width$}", format!("{:.1}%", pct)),
                selected_row_style(
                    Style::default().fg(Color::Rgb(96, 165, 250)),
                    selected,
                    theme,
                ),
            ),
            Span::raw(" "),
            Span::styled(
                format!("{:>msg_width$}", format_compact(model.message_count as i64)),
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
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("{:>last_width$}", last),
                selected_row_style(Style::default().fg(theme.dim), selected, theme),
            ));
        }
        if show_sparkline {
            spans.push(Span::raw("  "));
            let trend_key = normalize_model_name(&model.model);
            let trend = model_sparklines
                .get(&trend_key)
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
        let line = Line::from(spans);
        f.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
    }
}

pub fn filtered_models_for_state(
    dashboard: &UsageDashboard,
    state: &UsageState,
) -> Vec<ModelSummary> {
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

pub fn filtered_model_rows_for_state(
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

pub fn model_table_share_total<'a>(
    rows: impl IntoIterator<Item = &'a ModelTableRow>,
    sort_field: SortField,
) -> f64 {
    match sort_field {
        SortField::Tokens => rows.into_iter().map(|row| row.summary.tokens as f64).sum(),
        SortField::Cost | SortField::Date => rows.into_iter().map(|row| row.summary.cost).sum(),
    }
}

pub fn model_share_percent(model: &ModelSummary, total: f64, sort_field: SortField) -> f64 {
    let value = match sort_field {
        SortField::Tokens => model.tokens as f64,
        SortField::Cost | SortField::Date => model.cost,
    };
    share_percent(value, total)
}
