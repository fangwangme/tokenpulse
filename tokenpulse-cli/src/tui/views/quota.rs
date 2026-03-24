use crate::tui::theme::Theme;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::Span,
    widgets::{Block, Borders, Paragraph, Tabs},
    Terminal,
};
use tokenpulse_core::QuotaSnapshot;

fn display_provider_name(provider: &str) -> &'static str {
    match provider {
        "claude" => "CLAUDE CODE",
        "gemini" => "GEMINI CLI",
        "codex" => "CODEX",
        "antigravity" => "ANTIGRAVITY",
        _ => "UNKNOWN",
    }
}

fn format_reset_duration(diff: chrono::Duration) -> String {
    let total_minutes = diff.num_minutes();
    let total_hours = diff.num_hours();
    let days = total_hours / 24;

    if total_minutes > 24 * 60 {
        format!("{}d {}h", days, total_hours % 24)
    } else if total_hours > 0 {
        format!("{}h {}m", total_hours, total_minutes % 60)
    } else {
        format!("{}m", total_minutes)
    }
}

pub fn run(results: Vec<anyhow::Result<QuotaSnapshot>>) -> Result<()> {
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
                .margin(1)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(1),
                    Constraint::Min(10),
                    Constraint::Length(1),
                ])
                .split(size);

            let title = Paragraph::new(" TokenPulse - Quota Dashboard ")
                .style(Style::default().fg(theme.fg).bold())
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(title, chunks[0]);

            let tabs_widget = Tabs::new(
                tab_titles
                    .iter()
                    .map(|t| Span::styled(*t, Style::default())),
            )
            .block(Block::default().borders(Borders::BOTTOM))
            .select(selected_tab)
            .style(Style::default().fg(theme.dim))
            .highlight_style(Style::default().fg(theme.fg).bold());
            f.render_widget(tabs_widget, chunks[1]);

            let content_area = chunks[2];

            if selected_tab == 0 {
                render_overview(f, content_area, &snapshots, &theme);
            } else {
                let idx = selected_tab - 1;
                if idx < snapshots.len() {
                    render_provider(f, content_area, snapshots[idx], &theme);
                }
            }

            let footer_text = " q: quit | ←→: switch tab | r: refresh ";
            let footer = Paragraph::new(footer_text)
                .style(Style::default().fg(theme.dim))
                .block(Block::default().borders(Borders::TOP));
            f.render_widget(footer, chunks[3]);
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('r') => {}
                    KeyCode::Left | KeyCode::Char('h') => {
                        if selected_tab > 0 {
                            selected_tab -= 1;
                        }
                    }
                    KeyCode::Right | KeyCode::Char('l') => {
                        if selected_tab < tab_titles.len() - 1 {
                            selected_tab += 1;
                        }
                    }
                    KeyCode::Tab => {
                        if key.modifiers.contains(KeyModifiers::SHIFT) {
                            if selected_tab > 0 {
                                selected_tab -= 1;
                            }
                        } else {
                            if selected_tab < tab_titles.len() - 1 {
                                selected_tab += 1;
                            }
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

fn render_overview(
    f: &mut ratatui::Frame,
    area: Rect,
    snapshots: &[&QuotaSnapshot],
    theme: &Theme,
) {
    if snapshots.is_empty() {
        let msg = Paragraph::new("No quota data available")
            .style(Style::default().fg(theme.dim))
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(msg, area);
        return;
    }

    let mut y_offset = 0u16;

    for snapshot in snapshots {
        let provider_height = (snapshot.windows.len() as u16 + 3).max(7);
        let provider_area = Rect::new(
            area.x,
            area.y + y_offset,
            area.width,
            provider_height.min(area.height.saturating_sub(y_offset)),
        );

        if provider_area.height < 3 {
            break;
        }

        let border_color = get_provider_color(&snapshot.provider);

        let title = display_provider_name(&snapshot.provider).to_string();

        let provider_block = Block::default()
            .title(Span::styled(
                title,
                Style::default().fg(border_color).bold(),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        f.render_widget(provider_block, provider_area);

        let inner = Rect::new(
            provider_area.x + 1,
            provider_area.y + 2,
            provider_area.width.saturating_sub(2),
            provider_area.height.saturating_sub(3),
        );

        for (i, window) in snapshot.windows.iter().enumerate() {
            let gauge_y = inner.y + i as u16;
            if gauge_y >= inner.y + inner.height {
                break;
            }

            let used = window.used_percent;
            let reset_str = window
                .resets_at
                .as_ref()
                .map(|t| {
                    let now = chrono::Utc::now();
                    let diff = t.signed_duration_since(now);
                    format_reset_duration(diff)
                })
                .unwrap_or_default();

            let percent_text = format!("{:.0}%", used);
            let bar_width = (inner.width as f64 * used / 100.0) as usize;

            let line = format!(
                "{:12} [{}{}] {:5} {}",
                window.label,
                "█".repeat(bar_width.min(inner.width.saturating_sub(20) as usize)),
                "░".repeat(
                    (inner.width as usize)
                        .saturating_sub(20)
                        .saturating_sub(bar_width)
                ),
                percent_text,
                reset_str
            );

            let gauge_color = get_gauge_color(used);
            let para = Paragraph::new(line).style(Style::default().fg(gauge_color));

            let gauge_area = Rect::new(inner.x, gauge_y, inner.width, 1);
            f.render_widget(para, gauge_area);
        }

        y_offset += provider_height;
    }
}

fn render_provider(f: &mut ratatui::Frame, area: Rect, snapshot: &QuotaSnapshot, theme: &Theme) {
    let border_color = get_provider_color(&snapshot.provider);

    let block = Block::default()
        .title(format!(" {} ", display_provider_name(&snapshot.provider)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    f.render_widget(block, area);

    let inner = Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(4),
    );

    let mut y_offset = 0;

    for window in &snapshot.windows {
        let y = inner.y + y_offset;
        if y >= inner.y + inner.height {
            break;
        }

        let used = window.used_percent;
        let bar_width = (inner.width.saturating_sub(25) as f64 * used / 100.0) as usize;

        let reset_str = window
            .resets_at
            .as_ref()
            .map(|t| {
                let now = chrono::Utc::now();
                let diff = t.signed_duration_since(now);
                format!("resets in {}", format_reset_duration(diff))
            })
            .unwrap_or_else(|| "no reset time".to_string());

        let line = format!(
            "{:12} {:5.0}%  {} {}",
            window.label,
            used,
            "█".repeat(bar_width),
            reset_str
        );

        let gauge_color = get_gauge_color(used);
        let para = Paragraph::new(line).style(Style::default().fg(gauge_color));

        f.render_widget(para, Rect::new(inner.x, y, inner.width, 1));
        y_offset += 2;
    }

    if let Some(ref credits) = snapshot.credits {
        let y = inner.y + y_offset;
        if y < inner.y + inner.height {
            let credit_text = if let Some(limit) = credits.limit {
                format!("Credits: ${:.2} / ${:.2}", credits.used, limit)
            } else {
                format!("Credits: ${:.2} (unlimited)", credits.used)
            };

            let para = Paragraph::new(credit_text).style(Style::default().fg(theme.dim));
            f.render_widget(para, Rect::new(inner.x, y, inner.width, 1));
        }
    }
}

fn get_provider_color(provider: &str) -> Color {
    match provider.to_lowercase().as_str() {
        "claude" => Color::Rgb(222, 115, 86),
        "codex" => Color::Rgb(116, 170, 156),
        "gemini" => Color::Rgb(66, 133, 244),
        "antigravity" => Color::Rgb(147, 51, 234),
        _ => Color::Gray,
    }
}

fn get_gauge_color(percent: f64) -> Color {
    if percent >= 90.0 {
        Color::Red
    } else if percent >= 70.0 {
        Color::Yellow
    } else if percent >= 50.0 {
        Color::Rgb(255, 165, 0)
    } else {
        Color::Green
    }
}
