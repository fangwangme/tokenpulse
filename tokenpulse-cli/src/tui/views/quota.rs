use crate::commands::quota::{build_quota_fetchers, quota_display_name, quota_provider_info_list};
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
use std::time::{Duration, Instant};
use tokenpulse_core::{
    config::{Config, ConfigManager, QuotaDisplayMode},
    quota::{fetch_all, QuotaCacheStore},
    QuotaSnapshot, RateWindow,
};

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

fn refresh_quota_results(
    provider: Option<&str>,
    enabled_providers: &[String],
) -> Result<Vec<anyhow::Result<QuotaSnapshot>>> {
    let fetchers = build_quota_fetchers(provider, enabled_providers);
    let observed_at = chrono::Utc::now();
    let cache_store = QuotaCacheStore::new();
    let results = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(fetch_all(fetchers))
    });

    for result in &results {
        if let Ok(snapshot) = result {
            cache_store.save(&snapshot.provider, observed_at, snapshot)?;
        }
    }

    Ok(results)
}

pub fn run(
    mut results: Vec<anyhow::Result<QuotaSnapshot>>,
    initial_display_mode: QuotaDisplayMode,
    provider: Option<String>,
    initial_enabled_providers: Vec<String>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut theme = Theme::auto();
    let mut selected_tab: usize = 0;
    let mut settings_row: usize = 0;
    let config_manager = ConfigManager::new();
    let mut config = config_manager.load().unwrap_or_default();
    let mut display_mode = initial_display_mode;
    let mut enabled_providers = initial_enabled_providers;
    let mut last_refresh = Instant::now();

    loop {
        let auto_secs = config.display.quota_auto_refresh_secs;
        let refresh_countdown: Option<u64> = if auto_secs > 0 {
            let elapsed = last_refresh.elapsed().as_secs();
            Some((auto_secs as u64).saturating_sub(elapsed))
        } else {
            None
        };

        terminal.draw(|f| {
            let snapshots: Vec<&QuotaSnapshot> =
                results.iter().filter_map(|r| r.as_ref().ok()).collect();
            let tab_titles = quota_tab_titles(&snapshots);
            let size = f.area();
            f.render_widget(Block::default().style(Style::default().bg(theme.bg)), size);
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

            let settings_tab = tab_titles.len().saturating_sub(1);

            if selected_tab == 0 {
                render_overview(f, chunks[2], &snapshots, &display_mode, &theme);
            } else if selected_tab == settings_tab {
                render_settings(
                    f,
                    chunks[2],
                    &config,
                    &display_mode,
                    settings_row,
                    &config_manager,
                    &snapshots,
                    refresh_countdown,
                    &theme,
                );
            } else if let Some(snapshot) = snapshots.get(selected_tab - 1) {
                render_snapshot_card(f, chunks[2], snapshot, &display_mode, &theme, false, false);
            }

            let auto_hint = match refresh_countdown {
                None => "off".to_string(),
                Some(remaining) => format_countdown(remaining),
            };
            let footer_text = format!(
                " q quit | r refresh | a auto ({}) | b theme ({}) | m mode | e empty | space toggle | ←→ tab | {} provider{} ",
                auto_hint,
                theme.mode.label(),
                snapshots.len(),
                if snapshots.len() == 1 { "" } else { "s" }
            );
            let footer = Paragraph::new(footer_text)
                .style(Style::default().fg(theme.dim))
                .block(Block::default().borders(Borders::TOP));
            f.render_widget(footer, chunks[3]);
        })?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('r') => {
                        if let Ok(new_results) =
                            refresh_quota_results(provider.as_deref(), &enabled_providers)
                        {
                            results = new_results;
                            last_refresh = Instant::now();
                        }
                        let snapshots: Vec<&QuotaSnapshot> =
                            results.iter().filter_map(|r| r.as_ref().ok()).collect();
                        let tab_count = quota_tab_titles(&snapshots).len();
                        selected_tab = selected_tab.min(tab_count.saturating_sub(1));
                        settings_row =
                            settings_row.min(settings_row_count().saturating_sub(1));
                    }
                    KeyCode::Char('a') => {
                        config.display.quota_auto_refresh_secs =
                            next_auto_refresh_interval(config.display.quota_auto_refresh_secs);
                        let _ = config_manager.save(&config);
                        last_refresh = Instant::now();
                    }
                    KeyCode::Char('b') => {
                        theme = theme.toggled();
                    }
                    KeyCode::Char('m') => {
                        display_mode = toggle_display_mode(&display_mode);
                        config.display.quota_display_mode = display_mode.clone();
                        config_manager.save(&config)?;
                    }
                    KeyCode::Char('e') => {
                        config.display.show_empty_providers = !config.display.show_empty_providers;
                        config_manager.save(&config)?;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        let snapshots: Vec<&QuotaSnapshot> =
                            results.iter().filter_map(|r| r.as_ref().ok()).collect();
                        let tab_titles = quota_tab_titles(&snapshots);
                        if selected_tab == tab_titles.len().saturating_sub(1) && settings_row > 0 {
                            settings_row -= 1;
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        let snapshots: Vec<&QuotaSnapshot> =
                            results.iter().filter_map(|r| r.as_ref().ok()).collect();
                        let tab_titles = quota_tab_titles(&snapshots);
                        if selected_tab == tab_titles.len().saturating_sub(1) {
                            settings_row =
                                (settings_row + 1).min(settings_row_count().saturating_sub(1));
                        }
                    }
                    KeyCode::Char(' ') => {
                        let snapshots: Vec<&QuotaSnapshot> =
                            results.iter().filter_map(|r| r.as_ref().ok()).collect();
                        let tab_titles = quota_tab_titles(&snapshots);
                        if selected_tab == tab_titles.len().saturating_sub(1) {
                            if let Some(provider_id) = settings_provider_id(settings_row) {
                                let next_enabled = !is_provider_enabled(&config, provider_id);
                                let provider_config =
                                    config.providers.entry(provider_id.to_string()).or_default();
                                provider_config.enabled = next_enabled;
                                config_manager.save(&config)?;
                                enabled_providers = enabled_provider_ids(&config);
                            }
                        }
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        if selected_tab > 0 {
                            selected_tab -= 1;
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        let snapshots: Vec<&QuotaSnapshot> =
                            results.iter().filter_map(|r| r.as_ref().ok()).collect();
                        let tab_titles = quota_tab_titles(&snapshots);
                        if selected_tab + 1 < tab_titles.len() {
                            selected_tab += 1;
                        }
                    }
                    KeyCode::Tab => {
                        let snapshots: Vec<&QuotaSnapshot> =
                            results.iter().filter_map(|r| r.as_ref().ok()).collect();
                        let tab_titles = quota_tab_titles(&snapshots);
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

        // Auto-refresh check
        if auto_secs > 0 && last_refresh.elapsed().as_secs() >= auto_secs as u64 {
            if let Ok(new_results) =
                refresh_quota_results(provider.as_deref(), &enabled_providers)
            {
                results = new_results;
                last_refresh = Instant::now();
            } else {
                // Reset timer even on error to avoid tight retry loop
                last_refresh = Instant::now();
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}

fn enabled_provider_ids(config: &Config) -> Vec<String> {
    config
        .providers
        .iter()
        .filter(|(_, provider)| provider.enabled)
        .map(|(name, _)| name.clone())
        .collect()
}

fn toggle_display_mode(display_mode: &QuotaDisplayMode) -> QuotaDisplayMode {
    match display_mode {
        QuotaDisplayMode::Used => QuotaDisplayMode::Remaining,
        QuotaDisplayMode::Remaining => QuotaDisplayMode::Used,
    }
}

fn settings_row_count() -> usize {
    3 + quota_provider_info_list().len()
}

fn settings_provider_id(row: usize) -> Option<&'static str> {
    row.checked_sub(3)
        .and_then(|idx| quota_provider_info_list().get(idx).map(|info| info.id))
}

fn is_provider_enabled(config: &Config, provider: &str) -> bool {
    config
        .providers
        .get(provider)
        .map(|p| p.enabled)
        .unwrap_or(false)
}

fn quota_tab_titles(snapshots: &[&QuotaSnapshot]) -> Vec<String> {
    let mut tabs = vec!["Overview".to_string()];
    if snapshots.is_empty() {
        tabs.push("Settings".to_string());
        return tabs;
    }

    for snapshot in snapshots {
        tabs.push(snapshot.provider.clone());
    }
    tabs.push("Settings".to_string());
    tabs
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
    titles: &[String],
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
    .highlight_style(Style::default().fg(theme.on_accent).bg(theme.accent).bold())
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
            format!(" {} ", quota_display_name(&snapshot.provider)),
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

fn render_settings(
    f: &mut ratatui::Frame,
    area: Rect,
    config: &Config,
    display_mode: &QuotaDisplayMode,
    selected_row: usize,
    config_manager: &ConfigManager,
    snapshots: &[&QuotaSnapshot],
    refresh_countdown: Option<u64>,
    theme: &Theme,
) {
    let block = Block::default()
        .title(Span::styled(
            " Settings ",
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let fetched_provider_ids: std::collections::HashSet<&str> = snapshots
        .iter()
        .map(|snapshot| snapshot.provider.as_str())
        .collect();
    let mode = match display_mode {
        QuotaDisplayMode::Used => "used",
        QuotaDisplayMode::Remaining => "remaining",
    };

    let auto_value = {
        let base = auto_refresh_label(config.display.quota_auto_refresh_secs).to_string();
        match refresh_countdown {
            Some(remaining) if config.display.quota_auto_refresh_secs > 0 => {
                format!("{base} (next {})", format_countdown(remaining))
            }
            _ => base,
        }
    };

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Config file ", Style::default().fg(theme.dim)),
        Span::styled(
            config_manager.config_path().display().to_string(),
            Style::default().fg(theme.fg),
        ),
    ]));
    lines.push(Line::raw(""));
    lines.push(settings_line(
        selected_row == 0,
        "quota_display_mode",
        mode.to_string(),
        "m",
        theme,
        theme.codex,
    ));
    lines.push(settings_line(
        selected_row == 1,
        "show_empty_providers",
        if config.display.show_empty_providers {
            "true".to_string()
        } else {
            "false".to_string()
        },
        "e",
        theme,
        theme.gemini,
    ));
    lines.push(settings_line(
        selected_row == 2,
        "quota_auto_refresh_interval",
        auto_value,
        "a",
        theme,
        theme.accent_soft,
    ));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "Providers",
        Style::default().fg(theme.fg).bold(),
    )));

    for (idx, info) in quota_provider_info_list().iter().enumerate() {
        let row = idx + 3;
        let enabled = is_provider_enabled(config, info.id);
        let fetched = fetched_provider_ids.contains(info.id);
        let marker = if selected_row == row { ">" } else { " " };
        let status = if enabled { "enabled" } else { "disabled" };
        let fetched_text = if fetched { "fetched" } else { "not fetched" };
        lines.push(Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.accent)),
            Span::raw(" "),
            Span::styled(
                if enabled { "[x]" } else { "[ ]" },
                Style::default().fg(if enabled { theme.gauge_low } else { theme.dim }),
            ),
            Span::raw(" "),
            Span::styled(
                info.display_name,
                Style::default().fg(theme.provider_color(info.id)).bold(),
            ),
            Span::raw("  "),
            Span::styled(status, Style::default().fg(theme.dim)),
            Span::raw("  "),
            Span::styled(fetched_text, Style::default().fg(theme.dim)),
            Span::raw("  "),
            Span::styled(info.url, Style::default().fg(theme.dim)),
        ]));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("space", Style::default().fg(theme.accent_soft).bold()),
        Span::raw(" toggle provider  "),
        Span::styled("r", Style::default().fg(theme.accent_soft).bold()),
        Span::raw(" refresh quotas with saved settings"),
    ]));

    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(paragraph, inner);
}

fn settings_line(
    selected: bool,
    key: &'static str,
    value: String,
    shortcut: &'static str,
    theme: &Theme,
    value_color: ratatui::style::Color,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            if selected { ">" } else { " " },
            Style::default().fg(theme.accent),
        ),
        Span::raw(" "),
        Span::styled(key, Style::default().fg(theme.fg).bold()),
        Span::raw(" = "),
        Span::styled(value, Style::default().fg(value_color)),
        Span::raw("  "),
        Span::styled(shortcut, Style::default().fg(theme.accent_soft).bold()),
    ])
}

const AUTO_REFRESH_INTERVALS: &[u32] = &[0, 60, 120, 300, 600, 900];

fn next_auto_refresh_interval(current: u32) -> u32 {
    match AUTO_REFRESH_INTERVALS.iter().position(|&v| v == current) {
        Some(idx) => AUTO_REFRESH_INTERVALS[(idx + 1) % AUTO_REFRESH_INTERVALS.len()],
        // Custom interval: advance to next standard interval strictly greater than current,
        // or wrap to 0 (disabled) if already above all standard values.
        None => AUTO_REFRESH_INTERVALS
            .iter()
            .copied()
            .find(|&v| v > current)
            .unwrap_or(0),
    }
}

fn format_countdown(remaining: u64) -> String {
    let m = remaining / 60;
    let s = remaining % 60;
    if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

fn auto_refresh_label(secs: u32) -> &'static str {
    match secs {
        0 => "off",
        60 => "1m",
        120 => "2m",
        300 => "5m",
        600 => "10m",
        900 => "15m",
        _ => "custom",
    }
}
