use super::UsageState;
use crate::commands::quota::{display_plan, quota_display_name};
use crate::tui::theme::Theme;
use crate::tui::widgets::GradientGauge;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use tokenpulse_core::{
    config::{AccountDisplay, Config, QuotaDisplayMode},
    provider::RateWindow,
    QuotaSnapshot,
};

const FIVE_HOURS_MS: i64 = 5 * 60 * 60 * 1000;
const SEVEN_DAYS_MS: i64 = 7 * 24 * 60 * 60 * 1000;

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
        config.display.account_display,
    );
}

fn render_overview(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshots: &[&QuotaSnapshot],
    display_mode: &QuotaDisplayMode,
    theme: &Theme,
    account_display: AccountDisplay,
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
        let max_h = if snapshots[0].windows.len() > 4 {
            16
        } else {
            14
        };
        let card_area = Rect::new(area.x, area.y, area.width, area.height.min(max_h).max(8));
        render_snapshot_card(
            f,
            card_area,
            snapshots[0],
            display_mode,
            theme,
            false,
            account_display,
        );
        return;
    }

    let (small, tall): (Vec<&QuotaSnapshot>, Vec<&QuotaSnapshot>) =
        snapshots.iter().partition(|s| s.windows.len() <= 4);

    if small.is_empty() || tall.is_empty() {
        let max_h = if tall.is_empty() { 13 } else { 16 };
        render_snapshot_grid(
            f,
            area,
            snapshots,
            display_mode,
            theme,
            account_display,
            max_h,
        );
        return;
    }

    let cols = if area.width >= 110 { 2 } else { 1 };
    let small_rows = small.len().div_ceil(cols) as u16;
    let tall_rows = tall.len().div_ceil(cols) as u16;

    let small_h = (small_rows * 11).min(area.height.saturating_sub(tall_rows * 9));
    let tall_h = area.height.saturating_sub(small_h);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(small_h), Constraint::Length(tall_h)])
        .split(area);

    render_snapshot_grid(
        f,
        chunks[0],
        &small,
        display_mode,
        theme,
        account_display,
        11,
    );
    render_snapshot_grid(
        f,
        chunks[1],
        &tall,
        display_mode,
        theme,
        account_display,
        16,
    );
}

fn render_snapshot_grid(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshots: &[&QuotaSnapshot],
    display_mode: &QuotaDisplayMode,
    theme: &Theme,
    account_display: AccountDisplay,
    max_row_height: u16,
) {
    if snapshots.is_empty() || area.height == 0 || area.width == 0 {
        return;
    }

    let columns = if area.width >= 110 { 2 } else { 1 };
    let rows = snapshots.len().div_ceil(columns);
    let row_height = (area.height / rows as u16).min(max_row_height);

    let mut row_constraints = vec![Constraint::Length(row_height); rows];
    row_constraints.push(Constraint::Min(0));
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);

    for (row_idx, row_area) in row_areas.iter().enumerate().take(rows) {
        let col_constraints = vec![Constraint::Ratio(1, columns as u32); columns];
        let col_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(*row_area);

        for (col_idx, col_area) in col_areas.iter().enumerate().take(columns) {
            let index = row_idx * columns + col_idx;
            if let Some(snapshot) = snapshots.get(index) {
                render_snapshot_card(
                    f,
                    *col_area,
                    snapshot,
                    display_mode,
                    theme,
                    false,
                    account_display,
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
    account_display: AccountDisplay,
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

    let has_account_display = snapshot.account.is_some() && account_display.shows_account();
    let has_plan_display = snapshot.plan.is_some() && account_display.shows_plan();
    let show_account_row = has_account_display || has_plan_display;

    let base_windows: Vec<&RateWindow> = snapshot.windows.iter().collect();

    let fixed_rows =
        if show_account_row { 1 } else { 0 } + if snapshot.credits.is_some() { 1 } else { 0 };
    let compact = compact || inner.height < (base_windows.len() * 2 + fixed_rows) as u16;

    let lines_per_window = if compact { 1 } else { 2 };
    let available_for_windows = inner.height.saturating_sub(fixed_rows as u16);
    let max_allowed_windows = (available_for_windows as usize / lines_per_window).max(1);

    let windows = if max_allowed_windows < base_windows.len() {
        base_windows
            .into_iter()
            .take(max_allowed_windows.max(1))
            .collect::<Vec<_>>()
    } else {
        base_windows
    };

    let tags = window_tags(&windows);
    let tag_width = tags.iter().map(|t| t.chars().count()).max().unwrap_or(0);

    // Build vertical constraints with balanced spacing between sections and windows
    let add_spacing = !compact && inner.height >= (windows.len() * 3 + fixed_rows) as u16;
    let mut constraints = Vec::new();
    if show_account_row {
        constraints.push(Constraint::Length(1));
        if add_spacing {
            constraints.push(Constraint::Length(1));
        }
    }
    for (idx, _) in windows.iter().enumerate() {
        constraints.push(Constraint::Length(if compact { 1 } else { 2 }));
        if add_spacing
            && (idx + 1 < windows.len()
                || snapshot.credits.is_some()
                || !snapshot.rate_limit_reset_credits.is_empty())
        {
            constraints.push(Constraint::Length(1));
        }
    }
    if snapshot.credits.is_some() {
        constraints.push(Constraint::Length(1));
    }

    let used_lines: u16 = constraints
        .iter()
        .map(|c| match c {
            Constraint::Length(l) => *l,
            _ => 0,
        })
        .sum();
    let reset_credit_rows = inner.height.saturating_sub(used_lines) as usize;
    let reset_credit_lines = format_reset_credit_lines(snapshot, reset_credit_rows);

    for _ in &reset_credit_lines {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(0));

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let mut cursor = 0usize;
    // `PLAN: Plus · user@example.com`, cut down to what `account_display` allows.
    if show_account_row {
        let mut spans = Vec::new();
        if has_plan_display {
            if let Some(plan) = &snapshot.plan {
                spans.push(Span::styled("PLAN: ", tag_style(theme)));
                let max_plan_len = (inner.width as usize).saturating_sub(30).max(10);
                spans.push(Span::styled(
                    super::truncate(&display_plan(plan), max_plan_len),
                    Style::default().fg(theme.fg).bold(),
                ));
            }
        }
        if has_account_display {
            if let Some(account) = &snapshot.account {
                if !spans.is_empty() {
                    spans.push(Span::styled(" · ", Style::default().fg(theme.dim)));
                }
                let max_acc_len = (inner.width as usize).saturating_sub(25).max(15);
                spans.push(Span::styled(
                    super::truncate(account, max_acc_len),
                    Style::default().fg(theme.fg),
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
        if add_spacing && cursor < sections.len() {
            cursor += 1; // skip spacing line
        }
    }

    for (idx, (window, tag)) in windows.iter().zip(&tags).enumerate() {
        if cursor < sections.len() {
            let gauge_area = sections[cursor];
            cursor += 1;
            render_window_block(
                f,
                gauge_area,
                window,
                tag,
                tag_width,
                display_mode,
                theme,
                compact,
            );
        }
        if add_spacing
            && (idx + 1 < windows.len()
                || snapshot.credits.is_some()
                || !snapshot.rate_limit_reset_credits.is_empty())
            && cursor < sections.len()
        {
            cursor += 1; // skip spacing line
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

/// The tags a quota card shows for its windows: `5H`, `WEEKLY`, `WEEKLY · SONNET`.
///
/// Derived from the period and model family, so every provider's windows read
/// the same whatever its own `label` says (`Session (5h)`, `Gemini (5h)`, ...).
/// When windows carry a model family, periods before `·` are padded to the card's
/// maximum period width so the `·` separators align vertically.
fn window_tags(windows: &[&RateWindow]) -> Vec<String> {
    let raw_parts: Vec<(String, Option<String>)> = windows
        .iter()
        .map(|w| {
            let period = match w.period_duration_ms {
                Some(FIVE_HOURS_MS) => "5H".to_string(),
                Some(SEVEN_DAYS_MS) => "WEEKLY".to_string(),
                Some(ms) if ms > 0 => compact_duration(ms),
                _ => label_without_parenthetical(&w.label).to_uppercase(),
            };
            let family = match w.model_family.as_deref().map(str::to_uppercase) {
                Some(family) if family != period => Some(family),
                _ => None,
            };
            (period, family)
        })
        .collect();

    let max_period_width = raw_parts
        .iter()
        .filter(|(_, f)| f.is_some())
        .map(|(p, _)| p.chars().count())
        .max()
        .unwrap_or(0);

    raw_parts
        .into_iter()
        .map(|(period, family)| match family {
            Some(family) => format!("{period:<max_period_width$} · {family}"),
            None => period,
        })
        .collect()
}

#[cfg(test)]
fn window_tag(window: &RateWindow) -> String {
    window_tags(&[window]).pop().unwrap_or_default()
}

/// `3D`, `1H`, `30M`: the largest unit that divides the period evenly.
fn compact_duration(ms: i64) -> String {
    const UNITS: [(i64, &str); 4] = [
        (24 * 60 * 60 * 1000, "D"),
        (60 * 60 * 1000, "H"),
        (60 * 1000, "M"),
        (1000, "S"),
    ];
    UNITS
        .iter()
        .find(|(unit, _)| ms % unit == 0)
        .map(|(unit, suffix)| format!("{}{suffix}", ms / unit))
        .unwrap_or_else(|| format!("{ms}MS"))
}

/// `Premium Requests (300)` → `Premium Requests`.
fn label_without_parenthetical(label: &str) -> &str {
    let trimmed = label.trim();
    match trimmed.rfind(" (") {
        Some(start) if trimmed.ends_with(')') => trimmed[..start].trim_end(),
        _ => trimmed,
    }
}

fn tag_style(theme: &Theme) -> Style {
    Style::default().fg(theme.fg).bold()
}

fn render_window_block(
    f: &mut ratatui::Frame,
    area: Rect,
    window: &RateWindow,
    tag: &str,
    tag_width: usize,
    display_mode: &QuotaDisplayMode,
    theme: &Theme,
    compact: bool,
) {
    if area.height == 0 {
        return;
    }

    let is_fully_reset = window.used_percent <= 0.0;
    let shown_percent = quota_percent(display_mode, window.used_percent);
    let reset_str = window
        .resets_at
        .as_ref()
        .map(|time| format_reset_duration(time.signed_duration_since(chrono::Utc::now())))
        .unwrap_or_else(|| "n/a".to_string());

    let pace_result = if is_fully_reset {
        None
    } else {
        calculate_pace(window)
    };
    let gauge_color = pace_result
        .as_ref()
        .map(|(status, _, _)| theme.pace_color(status))
        .unwrap_or_else(|| theme.gauge_color(window.used_percent));

    let expected_pct = pace_result.as_ref().map(|(_, _, ep)| match display_mode {
        QuotaDisplayMode::Used => *ep,
        QuotaDisplayMode::Remaining => (100.0 - *ep).clamp(0.0, 100.0),
    });

    // Compact cards render a single line with no detail row, so the tag stays
    // in front of the bar to tell windows apart, and the percentage and reset
    // countdown stay on the bar itself.
    if compact {
        let label = super::truncate(tag, area.width.saturating_sub(18) as usize);
        let mut gauge = GradientGauge::new(&label, shown_percent)
            .color(gauge_color)
            .expected_percent(expected_pct)
            .label_width(tag_width.min(area.width.saturating_sub(30) as usize))
            .width(area.width.saturating_sub(22) as usize);
        if !is_fully_reset {
            gauge = gauge.time(&reset_str);
        }
        f.render_widget(gauge, area);
        return;
    }

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    // Top line: the bar alone across the full width (no label, no trailing
    // number); the tag and every figure live on the detail line below.
    let gauge = GradientGauge::new("", shown_percent)
        .color(gauge_color)
        .expected_percent(expected_pct)
        .label_width(0)
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

    // The tag is followed by a fixed space after the colon so figures start
    // immediately without wasting horizontal width.
    let mut detail_spans = vec![Span::styled(format!("{tag}: "), tag_style(theme))];
    if !is_fully_reset {
        detail_spans.push(Span::styled(
            format!("reset {reset_str}"),
            Style::default().fg(balance_color),
        ));
        detail_spans.push(Span::raw("   "));
    }
    detail_spans.push(Span::styled(
        format!("used {:.0}%", window.used_percent),
        Style::default().fg(balance_color),
    ));
    detail_spans.push(Span::raw("   "));
    detail_spans.push(Span::styled(
        format!("remaining {:.0}%", remaining_percent),
        Style::default().fg(balance_color),
    ));
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
    if count == 0 || max_rows < 2 {
        return Vec::new();
    }

    let mut lines = vec!["Banked reset  Expiration".to_string()];

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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tokenpulse_core::provider::RateWindow;
    use tokenpulse_core::QuotaSnapshot;

    fn sample_snapshot(provider: &str) -> QuotaSnapshot {
        QuotaSnapshot {
            provider: provider.to_string(),
            account: Some("user@example.com".to_string()),
            plan: Some("Pro".to_string()),
            windows: vec![
                RateWindow {
                    label: "5-hour Window".to_string(),
                    used_percent: 45.0,
                    resets_at: Some(chrono::Utc::now() + chrono::Duration::hours(2)),
                    period_duration_ms: Some(5 * 3600 * 1000),
                    model_family: None,
                },
                RateWindow {
                    label: "Weekly Window".to_string(),
                    used_percent: 20.0,
                    resets_at: Some(chrono::Utc::now() + chrono::Duration::days(4)),
                    period_duration_ms: Some(7 * 24 * 3600 * 1000),
                    model_family: None,
                },
            ],
            credits: None,
            rate_limit_reset_credits: vec![],
            fetched_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_render_quota_tab_half_screen() {
        let backend = TestBackend::new(95, 36);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut config = Config::default();
        let theme = Theme::default();
        let dashboard = crate::tui::views::usage::UsageDashboard { daily: vec![] };
        let mut state = UsageState::new(&dashboard, vec![]);

        state.quota_snapshots = vec![
            sample_snapshot("claude"),
            sample_snapshot("codex"),
            antigravity_snapshot(),
        ];
        for p in ["claude", "codex", "antigravity"] {
            config.providers.entry(p.to_string()).or_default().enabled = true;
        }

        terminal
            .draw(|f| {
                render_quota_tab(f, f.area(), &state, &config, &theme);
            })
            .unwrap();
    }

    #[test]
    fn test_render_quota_tab_full_screen() {
        let backend = TestBackend::new(130, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut config = Config::default();
        let theme = Theme::default();
        let dashboard = crate::tui::views::usage::UsageDashboard { daily: vec![] };
        let mut state = UsageState::new(&dashboard, vec![]);

        state.quota_snapshots = vec![
            sample_snapshot("claude"),
            sample_snapshot("codex"),
            antigravity_snapshot(),
        ];
        for p in ["claude", "codex", "antigravity"] {
            config.providers.entry(p.to_string()).or_default().enabled = true;
        }

        terminal
            .draw(|f| {
                render_quota_tab(f, f.area(), &state, &config, &theme);
            })
            .unwrap();
    }
    fn window(label: &str, period_ms: Option<i64>, family: Option<&str>, used: f64) -> RateWindow {
        RateWindow {
            label: label.to_string(),
            model_family: family.map(str::to_string),
            used_percent: used,
            resets_at: Some(chrono::Utc::now() + chrono::Duration::hours(3)),
            period_duration_ms: period_ms,
        }
    }

    fn claude_snapshot() -> QuotaSnapshot {
        QuotaSnapshot {
            provider: "claude".to_string(),
            account: Some("user@example.com".to_string()),
            plan: Some("plus".to_string()),
            windows: vec![
                window("Session (5h)", Some(FIVE_HOURS_MS), None, 30.0),
                window("Weekly (7d)", Some(SEVEN_DAYS_MS), None, 20.0),
                window("Sonnet (7d)", Some(SEVEN_DAYS_MS), Some("Sonnet"), 10.0),
            ],
            credits: None,
            rate_limit_reset_credits: vec![],
            fetched_at: chrono::Utc::now(),
        }
    }

    fn antigravity_snapshot() -> QuotaSnapshot {
        QuotaSnapshot {
            provider: "antigravity".to_string(),
            account: None,
            plan: None,
            windows: vec![
                window("Gemini (5h)", Some(FIVE_HOURS_MS), Some("Gemini"), 30.0),
                window("Gemini (7d)", Some(SEVEN_DAYS_MS), Some("Gemini"), 20.0),
                window("Claude (5h)", Some(FIVE_HOURS_MS), Some("Claude"), 10.0),
                window("Claude (7d)", Some(SEVEN_DAYS_MS), Some("Claude"), 5.0),
                window(
                    "Gemini 4 Argon (7d)",
                    Some(SEVEN_DAYS_MS),
                    Some("Gemini 4 Argon"),
                    15.0,
                ),
            ],
            credits: None,
            rate_limit_reset_credits: vec![],
            fetched_at: chrono::Utc::now(),
        }
    }

    /// The card's rows, without the left and right border columns.
    fn render_card(
        snapshot: &QuotaSnapshot,
        account_display: AccountDisplay,
        compact: bool,
    ) -> Vec<String> {
        let (width, height) = (80, 14);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let theme = Theme::default();
        terminal
            .draw(|f| {
                render_snapshot_card(
                    f,
                    f.area(),
                    snapshot,
                    &QuotaDisplayMode::Remaining,
                    &theme,
                    compact,
                    account_display,
                );
            })
            .unwrap();
        let buf = terminal.backend().buffer();
        (1..height - 1)
            .map(|y| (1..width - 1).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn is_bar_glyph(ch: char) -> bool {
        matches!(ch, '█' | '░' | '▏')
    }

    /// Each window's bar row and the detail row under it.
    fn bar_and_detail_rows(rows: &[String]) -> Vec<(&String, &String)> {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| row.chars().any(is_bar_glyph))
            .map(|(i, row)| (row, &rows[i + 1]))
            .collect()
    }

    #[test]
    fn detail_rows_start_with_tag_and_fixed_gap() {
        let rows = render_card(&claude_snapshot(), AccountDisplay::Full, false);
        let details: Vec<&String> = bar_and_detail_rows(&rows)
            .into_iter()
            .map(|(_, detail)| detail)
            .collect();

        assert!(details[0].starts_with("5H: reset "), "{:?}", details[0]);
        assert!(details[1].starts_with("WEEKLY: reset "), "{:?}", details[1]);
        assert!(
            details[2].starts_with("WEEKLY · SONNET: reset "),
            "{:?}",
            details[2]
        );
    }

    #[test]
    fn antigravity_windows_are_tagged_period_first_with_their_family() {
        let rows = render_card(&antigravity_snapshot(), AccountDisplay::Full, false);
        let details: Vec<&String> = bar_and_detail_rows(&rows)
            .into_iter()
            .map(|(_, detail)| detail)
            .collect();

        let expected = [
            "5H     · GEMINI: reset ",
            "WEEKLY · GEMINI: reset ",
            "5H     · CLAUDE: ",
            "WEEKLY · CLAUDE: reset ",
            "WEEKLY · GEMINI 4 ARGON: reset ",
        ];
        assert_eq!(details.len(), expected.len(), "{rows:#?}");
        for (detail, tag) in details.iter().zip(expected) {
            assert!(
                detail.starts_with(tag),
                "{detail:?} should start with {tag}"
            );
        }
    }

    #[test]
    fn compact_mode_keeps_the_tag_in_front_of_the_bar() {
        let rows = render_card(&claude_snapshot(), AccountDisplay::None, true);
        let bars: Vec<&String> = rows
            .iter()
            .filter(|row| row.chars().any(is_bar_glyph))
            .collect();

        assert_eq!(bars.len(), 3, "{rows:#?}");
        for (bar, tag) in bars.iter().zip(["5H", "WEEKLY", "WEEKLY · SONNET"]) {
            assert!(bar.starts_with(tag), "{bar:?} should start with {tag}");
            let after_label: String = bar
                .chars()
                .skip("WEEKLY · SONNET ".chars().count())
                .collect();
            assert!(
                after_label.starts_with(is_bar_glyph),
                "the bar follows the padded tag column: {bar:?}"
            );
        }
    }

    #[test]
    fn account_row_follows_account_display() {
        let snapshot = claude_snapshot();

        let none = render_card(&snapshot, AccountDisplay::None, false).join("\n");
        assert!(!none.contains("PLAN"), "{none}");
        assert!(!none.contains("user@example.com"), "{none}");

        let plan = render_card(&snapshot, AccountDisplay::Plan, false).join("\n");
        assert!(
            plan.contains("PLAN: Plus"),
            "plan is capitalized for display: {plan}"
        );
        assert!(!plan.contains("user@example.com"), "{plan}");

        let full = render_card(&snapshot, AccountDisplay::Full, false).join("\n");
        assert!(full.contains("PLAN: Plus · user@example.com"), "{full}");
    }

    #[test]
    fn window_tags_derive_from_period_and_family() {
        let tag = |label, period, family| window_tag(&window(label, period, family, 0.0));

        assert_eq!(tag("Session (5h)", Some(FIVE_HOURS_MS), None), "5H");
        assert_eq!(tag("Weekly (7d)", Some(SEVEN_DAYS_MS), None), "WEEKLY");
        assert_eq!(tag("Window (3d)", Some(3 * 24 * 3600 * 1000), None), "3D");
        assert_eq!(tag("Window (1h)", Some(3600 * 1000), None), "1H");
        assert_eq!(
            tag("Sonnet (7d)", Some(SEVEN_DAYS_MS), Some("Sonnet")),
            "WEEKLY · SONNET"
        );
        assert_eq!(
            tag("Opus (7d)", Some(SEVEN_DAYS_MS), Some("Opus")),
            "WEEKLY · OPUS"
        );
        assert_eq!(
            tag("Gemini (5h)", Some(FIVE_HOURS_MS), Some("Gemini")),
            "5H · GEMINI"
        );
        assert_eq!(
            tag("Claude (7d)", Some(SEVEN_DAYS_MS), Some("Claude")),
            "WEEKLY · CLAUDE"
        );
        // Unknown period: the provider label, minus its trailing parenthetical.
        assert_eq!(
            tag("Premium Requests (300)", None, None),
            "PREMIUM REQUESTS"
        );
        assert_eq!(tag("Primary window", None, None), "PRIMARY WINDOW");
        assert_eq!(tag("Gemini (monthly)", None, Some("Gemini")), "GEMINI");
    }

    #[test]
    fn fully_reset_window_suppresses_countdown_and_pace() {
        let snapshot = QuotaSnapshot {
            provider: "antigravity".to_string(),
            account: None,
            plan: None,
            windows: vec![
                window("Claude (5h)", Some(FIVE_HOURS_MS), Some("Claude"), 0.0),
                window("Claude (7d)", Some(SEVEN_DAYS_MS), Some("Claude"), 25.0),
            ],
            credits: None,
            rate_limit_reset_credits: vec![],
            fetched_at: chrono::Utc::now(),
        };

        let normal_rows = render_card(&snapshot, AccountDisplay::None, false);
        let detail_reset: Vec<&String> = normal_rows
            .iter()
            .filter(|row| row.contains("5H     · CLAUDE:"))
            .collect();
        assert_eq!(detail_reset.len(), 1);
        let detail_0 = detail_reset[0];
        assert!(
            !detail_0.contains("reset"),
            "0% used detail row should not contain reset countdown: {detail_0}"
        );
        assert!(
            detail_0.contains("used 0%"),
            "detail row should contain used 0%: {detail_0}"
        );
        assert!(
            detail_0.contains("remaining 100%"),
            "detail row should contain remaining 100%: {detail_0}"
        );

        let detail_active: Vec<&String> = normal_rows
            .iter()
            .filter(|row| row.contains("WEEKLY · CLAUDE:"))
            .collect();
        assert_eq!(detail_active.len(), 1);
        assert!(
            detail_active[0].contains("reset"),
            "active window detail row should contain reset countdown: {}",
            detail_active[0]
        );

        let compact_rows = render_card(&snapshot, AccountDisplay::None, true);
        let bar_reset: Vec<&String> = compact_rows
            .iter()
            .filter(|row| row.contains("5H     · CLAUDE"))
            .collect();
        assert_eq!(bar_reset.len(), 1);
        let bar_0 = bar_reset[0];
        assert!(
            !bar_0.contains("reset"),
            "compact bar should not contain reset countdown: {bar_0}"
        );
        assert!(
            !bar_0.contains('m') && !bar_0.contains('h') && !bar_0.contains('d'),
            "0% used compact bar should not contain time duration: {bar_0}"
        );
        assert!(
            bar_0.ends_with("100%"),
            "0% used compact bar should end with remaining percentage: {bar_0}"
        );

        let bar_active: Vec<&String> = compact_rows
            .iter()
            .filter(|row| row.contains("WEEKLY · CLAUDE"))
            .collect();
        assert_eq!(bar_active.len(), 1);
        assert!(
            bar_active[0].contains('h') || bar_active[0].contains('m'),
            "active compact bar should retain duration: {}",
            bar_active[0]
        );
    }
}
