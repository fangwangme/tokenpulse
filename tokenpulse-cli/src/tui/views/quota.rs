use crate::tui::theme::Theme;
use crate::tui::widgets::GradientGauge;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs},
    Terminal,
};
use std::cmp::Ordering;
use tokenpulse_core::{config::QuotaDisplayMode, QuotaSnapshot, RateWindow};

fn display_provider_name(provider: &str) -> &'static str {
    match provider {
        "claude" => "CLAUDE CODE",
        "gemini" => "GEMINI CLI",
        "codex" => "CODEX",
        "copilot" => "GITHUB COPILOT",
        "antigravity" => "ANTIGRAVITY",
        _ => "UNKNOWN",
    }
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

fn quota_percent(display_mode: &QuotaDisplayMode, used_percent: f64) -> f64 {
    match display_mode {
        QuotaDisplayMode::Used => used_percent,
        QuotaDisplayMode::Remaining => (100.0 - used_percent).max(0.0),
    }
}

fn quota_suffix(display_mode: &QuotaDisplayMode) -> &'static str {
    match display_mode {
        QuotaDisplayMode::Used => "used",
        QuotaDisplayMode::Remaining => "left",
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

pub fn run(
    results: Vec<anyhow::Result<QuotaSnapshot>>,
    display_mode: QuotaDisplayMode,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let theme = Theme::default();
    let snapshots: Vec<&QuotaSnapshot> = results.iter().filter_map(|r| r.as_ref().ok()).collect();

    let mut selected_tab: usize = 0;
    let tab_titles: Vec<&str> = if snapshots.is_empty() {
        vec!["Overview"]
    } else {
        let mut tabs = vec!["Overview"];
        for s in &snapshots {
            tabs.push(s.provider.as_str());
        }
        tabs
    };

    loop {
        terminal.draw(|f| {
            let size = f.area();
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(4),
                    Constraint::Length(3),
                    Constraint::Min(12),
                    Constraint::Length(2),
                ])
                .split(size);

            render_header(f, chunks[0], &snapshots, &display_mode, &theme);
            render_tabs(f, chunks[1], &tab_titles, selected_tab, &theme);

            if selected_tab == 0 {
                render_overview(f, chunks[2], &snapshots, &display_mode, &theme);
            } else if let Some(snapshot) = snapshots.get(selected_tab - 1) {
                render_snapshot_card(f, chunks[2], snapshot, &display_mode, &theme, false, false);
            }

            let footer_text = format!(
                " q quit | ←→ tab | mode {} | {} provider{} ",
                quota_suffix(&display_mode),
                snapshots.len(),
                if snapshots.len() == 1 { "" } else { "s" }
            );
            let footer = Paragraph::new(footer_text)
                .style(Style::default().fg(theme.dim))
                .block(Block::default().borders(Borders::TOP));
            f.render_widget(footer, chunks[3]);
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Left | KeyCode::Char('h') => {
                        if selected_tab > 0 {
                            selected_tab -= 1;
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        if selected_tab + 1 < tab_titles.len() {
                            selected_tab += 1;
                        }
                    }
                    KeyCode::Tab => {
                        if key.modifiers.contains(KeyModifiers::SHIFT) {
                            if selected_tab > 0 {
                                selected_tab -= 1;
                            }
                        } else if selected_tab + 1 < tab_titles.len() {
                            selected_tab += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}

fn render_header(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshots: &[&QuotaSnapshot],
    display_mode: &QuotaDisplayMode,
    theme: &Theme,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let last_fetch = snapshots
        .iter()
        .map(|snapshot| snapshot.fetched_at)
        .max()
        .map(|ts| ts.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "no data".to_string());

    let lines = vec![
        Line::from(vec![
            Span::styled("TokenPulse", Style::default().fg(theme.accent).bold()),
            Span::raw(" "),
            Span::styled("Quota Dashboard", Style::default().fg(theme.fg).bold()),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{} mode", quota_suffix(display_mode)),
                Style::default().fg(theme.codex),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} providers", snapshots.len()),
                Style::default().fg(theme.gemini),
            ),
            Span::raw("  "),
            Span::styled(
                format!("last fetch {}", last_fetch),
                Style::default().fg(theme.dim),
            ),
        ]),
    ];

    let header = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(header, inner);
}

fn render_tabs(
    f: &mut ratatui::Frame,
    area: Rect,
    titles: &[&str],
    selected_tab: usize,
    theme: &Theme,
) {
    let tabs = Tabs::new(titles.iter().map(|t| {
        Span::styled(
            format!(" {} ", t.to_uppercase()),
            Style::default().fg(theme.dim),
        )
    }))
    .select(selected_tab)
    .divider(Span::raw(""))
    .highlight_style(Style::default().fg(theme.bg).bg(theme.accent).bold())
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border)),
    );

    f.render_widget(tabs, area);
}

fn render_overview(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshots: &[&QuotaSnapshot],
    display_mode: &QuotaDisplayMode,
    theme: &Theme,
) {
    if snapshots.is_empty() {
        let msg = Paragraph::new("No quota data available")
            .style(Style::default().fg(theme.dim))
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(msg, area);
        return;
    }

    if snapshots.len() == 1 {
        render_snapshot_card(f, area, snapshots[0], display_mode, theme, false, true);
        return;
    }

    let columns = if area.width >= 110 { 2 } else { 1 };
    let rows = snapshots.len().div_ceil(columns);
    let row_constraints = vec![Constraint::Ratio(1, rows as u32); rows];
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
                    true,
                    true,
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
) {
    let provider_color = theme.provider_color(&snapshot.provider);
    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", display_provider_name(&snapshot.provider)),
            Style::default().fg(provider_color).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // In overview mode, show only the most important windows (max 3)
    let windows: Vec<&RateWindow> = if overview && snapshot.windows.len() > 3 {
        // Pick windows with highest usage (most critical) up to 3
        let mut sorted: Vec<&RateWindow> = snapshot.windows.iter().collect();
        sorted.sort_by(|a, b| {
            b.used_percent
                .partial_cmp(&a.used_percent)
                .unwrap_or(Ordering::Equal)
        });
        sorted.into_iter().take(3).collect()
    } else {
        snapshot.windows.iter().collect()
    };

    // Compute max label width across all windows for alignment
    let max_label_len = windows
        .iter()
        .map(|w| {
            let label_text = format!("{} {}", w.label, quota_suffix(display_mode));
            label_text.chars().count()
        })
        .max()
        .unwrap_or(10);
    let fixed_label_width = max_label_len.min(inner.width.saturating_sub(30) as usize);

    let mut constraints = Vec::new();
    if snapshot.plan.is_some() {
        constraints.push(Constraint::Length(1));
    }
    for _ in &windows {
        constraints.push(Constraint::Length(2));
    }
    if snapshot.credits.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Min(0));

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let mut cursor = 0usize;
    if let Some(plan) = &snapshot.plan {
        let line = Paragraph::new(Line::from(vec![
            Span::styled("Plan ", Style::default().fg(theme.dim)),
            Span::styled(plan.clone(), Style::default().fg(theme.fg).bold()),
        ]));
        f.render_widget(line, sections[cursor]);
        cursor += 1;
    }

    for window in &windows {
        let gauge_area = sections[cursor];
        cursor += 1;
        render_window_block(
            f,
            gauge_area,
            snapshot,
            window,
            display_mode,
            theme,
            compact,
            fixed_label_width,
        );
    }

    if let Some(credits) = &snapshot.credits {
        let credit_text = if let Some(limit) = credits.limit {
            let percent = if limit > 0.0 {
                (credits.used / limit * 100.0).clamp(0.0, 999.0)
            } else {
                0.0
            };
            format!(
                "Credits {}{:.2} / {}{:.2} ({:.0}%)",
                credits.currency, credits.used, credits.currency, limit, percent
            )
        } else {
            format!(
                "Credits {}{:.2} (unlimited)",
                credits.currency, credits.used
            )
        };
        let line = Paragraph::new(credit_text)
            .style(Style::default().fg(theme.dim))
            .alignment(Alignment::Left);
        f.render_widget(line, sections[cursor]);
        cursor += 1;
    }

    if !compact && cursor < sections.len() {
        let footer = Paragraph::new(format!(
            "Fetched {}",
            snapshot.fetched_at.format("%Y-%m-%d %H:%M UTC")
        ))
        .style(Style::default().fg(theme.dim));
        f.render_widget(footer, sections[cursor]);
    }
}

fn render_window_block(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshot: &QuotaSnapshot,
    window: &RateWindow,
    display_mode: &QuotaDisplayMode,
    theme: &Theme,
    compact: bool,
    fixed_label_width: usize,
) {
    if area.height == 0 {
        return;
    }

    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    let shown_percent = quota_percent(display_mode, window.used_percent);
    let label = truncate(
        &format!("{} {}", window.label, quota_suffix(display_mode)),
        area.width.saturating_sub(18) as usize,
    );
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

    let gauge = GradientGauge::new(&label, shown_percent)
        .width(area.width.saturating_sub(22) as usize)
        .color(gauge_color)
        .time(&reset_str)
        .expected_percent(expected_pct)
        .label_width(fixed_label_width);
    f.render_widget(gauge, split[0]);

    if split[1].height == 0 {
        return;
    }

    let pace = pace_result
        .map(|(status, text, _)| Span::styled(text, Style::default().fg(theme.pace_color(status))))
        .unwrap_or_else(|| Span::styled("No pace data", Style::default().fg(theme.dim)));

    let primary = match display_mode {
        QuotaDisplayMode::Used => format!("{:.0}% used", window.used_percent),
        QuotaDisplayMode::Remaining => format!("{:.0}% left", shown_percent),
    };
    let secondary = match display_mode {
        QuotaDisplayMode::Used => format!("{:.0}% left", 100.0 - window.used_percent),
        QuotaDisplayMode::Remaining => format!("{:.0}% used", window.used_percent),
    };

    let detail = if compact {
        Line::from(vec![
            Span::styled(
                primary,
                Style::default().fg(theme.provider_color(&snapshot.provider)),
            ),
            Span::raw("  "),
            pace,
        ])
    } else {
        Line::from(vec![
            Span::styled(
                primary,
                Style::default().fg(theme.provider_color(&snapshot.provider)),
            ),
            Span::raw("  "),
            Span::styled(secondary, Style::default().fg(theme.fg)),
            Span::raw("  "),
            pace,
        ])
    };

    let paragraph = Paragraph::new(detail).style(Style::default().fg(theme.dim));
    f.render_widget(paragraph, split[1]);
}
