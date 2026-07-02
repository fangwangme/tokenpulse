use super::UsageState;
use crate::commands::quota::quota_display_name;
use crate::tui::theme::Theme;
use crate::tui::widgets::GradientGauge;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::cmp::Ordering;
use tokenpulse_core::{
    config::{Config, QuotaDisplayMode},
    provider::RateWindow,
    QuotaSnapshot,
};

pub fn render_quota_tab(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &UsageState,
    config: &Config,
    theme: &Theme,
) {
    let filtered_snapshots: Vec<&QuotaSnapshot> = state
        .quota_snapshots
        .iter()
        .filter(|s| {
            config
                .providers
                .get(&s.provider)
                .map(|p| p.enabled)
                .unwrap_or(false)
        })
        .collect();

    render_overview(
        f,
        area,
        &filtered_snapshots,
        &config.display.quota_display_mode,
        theme,
        config.display.show_account,
    );
}

fn render_overview(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshots: &[&QuotaSnapshot],
    display_mode: &QuotaDisplayMode,
    theme: &Theme,
    show_account: bool,
) {
    if snapshots.is_empty() {
        let msg = Paragraph::new("No quota data available for enabled providers")
            .style(Style::default().fg(theme.dim))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border)),
            );
        f.render_widget(msg, area);
        return;
    }

    if snapshots.len() == 1 {
        render_snapshot_card(
            f,
            area,
            snapshots[0],
            display_mode,
            theme,
            false,
            true,
            show_account,
        );
        return;
    }

    let columns = if area.width >= 110 { 2 } else { 1 };
    let rows = snapshots.len().div_ceil(columns);
    let row_constraints = vec![Constraint::Min(6); rows];
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);

    for (row_idx, row_area) in row_areas.iter().enumerate() {
        let col_constraints = vec![Constraint::Ratio(1, columns as u32); columns];
        let col_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(*row_area);

        for col_idx in 0..columns {
            let index = row_idx * columns + col_idx;
            if let Some(snapshot) = snapshots.get(index) {
                render_snapshot_card(
                    f,
                    col_areas[col_idx],
                    snapshot,
                    display_mode,
                    theme,
                    false,
                    true,
                    show_account,
                );
            }
        }
    }
}

fn render_snapshot_card(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &QuotaSnapshot,
    display_mode: &QuotaDisplayMode,
    theme: &Theme,
    compact: bool,
    overview: bool,
    show_account: bool,
) {
    let provider_color = theme.provider_color(&snapshot.provider);
    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", quota_display_name(&snapshot.provider)),
            Style::default().fg(provider_color).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let has_account_display = snapshot.account.is_some() && show_account;
    let has_plan_display = snapshot.plan.is_some() && show_account;
    let show_account_row = has_account_display || has_plan_display;

    let base_windows: Vec<&RateWindow> = if overview && snapshot.windows.len() > 4 {
        let mut sorted: Vec<&RateWindow> = snapshot.windows.iter().collect();
        sorted.sort_by(|a, b| {
            b.used_percent
                .partial_cmp(&a.used_percent)
                .unwrap_or(Ordering::Equal)
        });
        sorted.into_iter().take(4).collect()
    } else {
        snapshot.windows.iter().collect()
    };

    let fixed_rows =
        if show_account_row { 1 } else { 0 } + if snapshot.credits.is_some() { 1 } else { 0 };
    let desired_reset_credit_rows = snapshot.rate_limit_reset_credits.len();
    let compact = compact
        || inner.height < (base_windows.len() * 3 + fixed_rows + desired_reset_credit_rows) as u16;

    let lines_per_window = if compact { 1 } else { 3 };
    let min_window_rows = if base_windows.is_empty() {
        0
    } else {
        lines_per_window
    };
    let max_reset_credit_rows = if desired_reset_credit_rows == 0 {
        0
    } else {
        inner
            .height
            .saturating_sub((fixed_rows + min_window_rows) as u16)
            .max(1) as usize
    };
    let reset_credit_lines = format_reset_credit_lines(snapshot, max_reset_credit_rows);
    let reserved_lines = fixed_rows + reset_credit_lines.len();
    let available_for_windows = inner.height.saturating_sub(reserved_lines as u16);
    let max_allowed_windows = (available_for_windows as usize / lines_per_window).max(1);

    let windows = if max_allowed_windows < base_windows.len() {
        base_windows
            .into_iter()
            .take(max_allowed_windows.max(1))
            .collect::<Vec<_>>()
    } else {
        base_windows
    };

    let max_label_len = windows
        .iter()
        .map(|w| w.label.chars().count())
        .max()
        .unwrap_or(10);
    let fixed_label_width = max_label_len.min(inner.width.saturating_sub(30) as usize);

    let mut constraints = Vec::new();
    if show_account_row {
        constraints.push(Constraint::Length(1));
    }
    for _ in &windows {
        constraints.push(Constraint::Length(if compact { 1 } else { 3 }));
    }
    if snapshot.credits.is_some() {
        constraints.push(Constraint::Length(1));
    }
    for _ in &reset_credit_lines {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(0));

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let mut cursor = 0usize;
    if show_account_row {
        let mut spans = Vec::new();
        if has_account_display {
            if let Some(account) = &snapshot.account {
                let max_acc_len = (inner.width as usize).saturating_sub(25).max(15);
                let truncated_acc = super::truncate(account, max_acc_len);
                spans.push(Span::styled(
                    truncated_acc,
                    Style::default().fg(theme.fg).bold(),
                ));
            }
        }
        if has_plan_display {
            if let Some(plan) = &snapshot.plan {
                if has_account_display {
                    spans.push(Span::styled(" | ", Style::default().fg(theme.dim)));
                }
                spans.push(Span::styled("Plan: ", Style::default().fg(theme.dim)));
                let max_plan_len = (inner.width as usize).saturating_sub(30).max(10);
                let truncated_plan = super::truncate(plan, max_plan_len);
                spans.push(Span::styled(
                    truncated_plan,
                    Style::default().fg(theme.fg).bold(),
                ));
            }
        }
        if !spans.is_empty() {
            let line = Paragraph::new(Line::from(spans));
            if cursor < sections.len() {
                f.render_widget(line, sections[cursor]);
                cursor += 1;
            }
        }
    }

    for window in &windows {
        if cursor < sections.len() {
            let gauge_area = sections[cursor];
            cursor += 1;
            render_window_block(
                f,
                gauge_area,
                window,
                display_mode,
                theme,
                compact,
                fixed_label_width,
            );
        }
    }

    if let Some(credits) = &snapshot.credits {
        if cursor < sections.len() {
            let credit_text = format_credit_text(credits, display_mode);
            let line = Paragraph::new(credit_text)
                .style(Style::default().fg(theme.dim))
                .alignment(Alignment::Left);
            f.render_widget(line, sections[cursor]);
            cursor += 1;
        }
    }

    for (index, reset_text) in reset_credit_lines.into_iter().enumerate() {
        if cursor < sections.len() {
            let style = if index == 0 {
                Style::default().fg(theme.fg).bold()
            } else {
                Style::default().fg(theme.dim)
            };
            let line = Paragraph::new(reset_text)
                .style(style)
                .alignment(Alignment::Left);
            f.render_widget(line, sections[cursor]);
            cursor += 1;
        }
    }
}

fn render_window_block(
    f: &mut ratatui::Frame,
    area: Rect,
    window: &RateWindow,
    display_mode: &QuotaDisplayMode,
    theme: &Theme,
    compact: bool,
    fixed_label_width: usize,
) {
    if area.height == 0 {
        return;
    }

    let shown_percent = quota_percent(display_mode, window.used_percent);
    let label = super::truncate(&window.label, area.width.saturating_sub(18) as usize);
    let reset_str = window
        .resets_at
        .as_ref()
        .map(|time| format_reset_duration(time.signed_duration_since(chrono::Utc::now())))
        .unwrap_or_else(|| "n/a".to_string());

    let pace_result = calculate_pace(window);
    let gauge_color = pace_result
        .as_ref()
        .map(|(status, _, _)| theme.pace_color(status))
        .unwrap_or_else(|| theme.gauge_color(window.used_percent));

    let expected_pct = pace_result.as_ref().map(|(_, _, ep)| match display_mode {
        QuotaDisplayMode::Used => *ep,
        QuotaDisplayMode::Remaining => (100.0 - *ep).clamp(0.0, 100.0),
    });

    let base_gauge = GradientGauge::new(&label, shown_percent)
        .color(gauge_color)
        .expected_percent(expected_pct)
        .label_width(fixed_label_width);

    // Compact cards render a single line with no detail row, so keep the
    // percentage and reset countdown on the bar itself.
    if compact {
        let gauge = base_gauge
            .width(area.width.saturating_sub(22) as usize)
            .time(&reset_str);
        f.render_widget(gauge, area);
        return;
    }

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    // Top line: pure progress bar (no trailing number); every figure lives on
    // the detail line below so nothing is duplicated.
    let gauge = base_gauge
        .width(split[0].width as usize)
        .show_percent(false);
    f.render_widget(gauge, split[0]);

    // The detail line sits directly under the bar. render_snapshot_card gives
    // each window an extra row, so the leftover row below the detail becomes a
    // blank gap between consecutive windows (not between the bar and the text).
    let detail_area = split[1];
    if detail_area.height == 0 {
        return;
    }

    // Detail line: reset countdown + used/remaining colored by the balance
    // amount (same green/yellow/red rule as the gauge), then the pacer (which
    // carries its own at-current-rate ETA) in its pace color. When the window
    // is exhausted there is no pace to project, so the pacer is omitted.
    let balance_color = theme.gauge_color(window.used_percent);
    let remaining_percent = (100.0 - window.used_percent).max(0.0);

    let mut detail_spans = vec![
        Span::styled(
            format!("reset {reset_str}"),
            Style::default().fg(balance_color),
        ),
        Span::raw("   "),
        Span::styled(
            format!("used {:.0}%", window.used_percent),
            Style::default().fg(balance_color),
        ),
        Span::raw("   "),
        Span::styled(
            format!("remaining {:.0}%", remaining_percent),
            Style::default().fg(balance_color),
        ),
    ];
    if let Some((status, text, _)) = pace_result {
        detail_spans.push(Span::raw("   "));
        detail_spans.push(Span::styled(
            text,
            Style::default().fg(theme.pace_color(status)),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(detail_spans)), detail_area);
}

fn format_reset_duration(diff: chrono::Duration) -> String {
    let total_minutes = diff.num_minutes().max(0);
    let total_hours = total_minutes / 60;
    let days = total_hours / 24;

    if total_minutes > 24 * 60 {
        format!("{}d {}h", days, total_hours % 24)
    } else if total_hours > 0 {
        format!("{}h {}m", total_hours, total_minutes % 60)
    } else {
        format!("{}m", total_minutes)
    }
}

fn format_reset_credit_timestamp(time: &chrono::DateTime<chrono::Utc>) -> String {
    time.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn format_credit_text(
    credits: &tokenpulse_core::CreditInfo,
    display_mode: &QuotaDisplayMode,
) -> String {
    match display_mode {
        QuotaDisplayMode::Used => {
            if let Some(limit) = credits.limit {
                let percent = if limit > 0.0 {
                    (credits.used / limit * 100.0).clamp(0.0, 999.0)
                } else {
                    0.0
                };
                format!(
                    "Credits {} / {} ({:.0}%)",
                    format_credit_amount(&credits.currency, credits.used),
                    format_credit_amount(&credits.currency, limit),
                    percent
                )
            } else {
                format!(
                    "Credits {}",
                    format_credit_amount(&credits.currency, credits.used)
                )
            }
        }
        QuotaDisplayMode::Remaining => {
            if let Some(limit) = credits.limit {
                let remaining = (limit - credits.used).max(0.0);
                let percent = if limit > 0.0 {
                    (remaining / limit * 100.0).clamp(0.0, 100.0)
                } else {
                    0.0
                };
                format!(
                    "Balance {} / {} ({:.0}%)",
                    format_credit_amount(&credits.currency, remaining),
                    format_credit_amount(&credits.currency, limit),
                    percent
                )
            } else {
                format!(
                    "Balance {}",
                    format_credit_amount(&credits.currency, credits.used)
                )
            }
        }
    }
}

fn format_reset_credit_lines(snapshot: &QuotaSnapshot, max_rows: usize) -> Vec<String> {
    let count = snapshot.rate_limit_reset_credits.len();
    if count == 0 || max_rows == 0 {
        return Vec::new();
    }

    let mut lines = vec!["Banked reset  Expiration".to_string()];
    if max_rows == 1 {
        return lines;
    }

    let mut credits: Vec<&tokenpulse_core::RateLimitResetCredit> =
        snapshot.rate_limit_reset_credits.iter().collect();
    credits.sort_by_key(|credit| (credit.expires_at.is_none(), credit.expires_at));

    let format_line = |display_index: usize, credit: &tokenpulse_core::RateLimitResetCredit| {
        let expiry = credit
            .expires_at
            .map(|time| format_reset_credit_timestamp(&time))
            .unwrap_or_else(|| "expiry unknown".to_string());
        format!("#{} {expiry}", display_index + 1)
    };

    let max_credit_rows = max_rows - 1;
    if max_credit_rows >= count {
        lines.extend(
            credits
                .into_iter()
                .enumerate()
                .map(|(index, credit)| format_line(index, credit)),
        );
        return lines;
    }

    if max_credit_rows == 1 {
        lines.push(format_line(0, credits[0]));
        return lines;
    }

    let visible_credit_rows = max_credit_rows - 1;
    lines.extend(
        credits
            .iter()
            .take(visible_credit_rows)
            .enumerate()
            .map(|(index, credit)| format_line(index, credit)),
    );
    lines.push(format!(
        "+{} more reset credits",
        count - visible_credit_rows
    ));
    lines
}

fn format_credit_amount(currency: &str, value: f64) -> String {
    if currency == "USD" {
        format!("${value:.2}")
    } else if currency.is_empty() {
        format!("{value:.2}")
    } else {
        format!("{currency}{value:.2}")
    }
}

fn quota_percent(display_mode: &QuotaDisplayMode, used_percent: f64) -> f64 {
    match display_mode {
        QuotaDisplayMode::Used => used_percent,
        QuotaDisplayMode::Remaining => (100.0 - used_percent).max(0.0),
    }
}

fn calculate_pace(window: &RateWindow) -> Option<(&'static str, String, f64)> {
    let period_ms = window.period_duration_ms?;
    let reset_time = window.resets_at?;

    if window.used_percent >= 100.0 {
        return None;
    }

    let now = chrono::Utc::now();
    let period_start = reset_time - chrono::Duration::milliseconds(period_ms);
    let elapsed_ms = (now - period_start).num_milliseconds();

    if elapsed_ms <= 0 || now >= reset_time {
        return None;
    }

    let elapsed_fraction = elapsed_ms as f64 / period_ms as f64;
    let expected_usage = elapsed_fraction * 100.0;
    let deficit = window.used_percent - expected_usage;

    if deficit.abs() < 5.0 {
        Some(("on-track", "On track".to_string(), expected_usage))
    } else if deficit > 0.0 {
        let rate = window.used_percent / elapsed_ms as f64;
        if rate <= 0.0 {
            return None;
        }
        let remaining_ms = (100.0 - window.used_percent) / rate;
        Some((
            "behind",
            format!(
                "+{:.0}% pace | eta {}",
                deficit,
                format_reset_duration(chrono::Duration::milliseconds(remaining_ms as i64))
            ),
            expected_usage,
        ))
    } else {
        Some((
            "ahead",
            format!("{:.0}% under pace", deficit.abs()),
            expected_usage,
        ))
    }
}
