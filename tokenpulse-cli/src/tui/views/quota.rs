use crate::tui::{theme::Theme, widgets::GradientGauge};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use tokenpulse_core::QuotaSnapshot;

pub fn run(results: Vec<anyhow::Result<QuotaSnapshot>>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let theme = Theme::default();
    let snapshots: Vec<&QuotaSnapshot> = results.iter().filter_map(|r| r.as_ref().ok()).collect();

    loop {
        terminal.draw(|f| {
            let size = f.area();

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(10),
                    Constraint::Length(2),
                ])
                .split(size);

            let header = Paragraph::new("TokenPulse - Quota Overview")
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(header, chunks[0]);

            let provider_height = 8;
            let mut y_offset = 0;

            for snapshot in &snapshots {
                let provider_area = Rect::new(
                    chunks[1].x,
                    chunks[1].y + y_offset,
                    chunks[1].width,
                    provider_height.min(chunks[1].height - y_offset),
                );

                if provider_area.height < 3 {
                    break;
                }

                let provider_block = Block::default()
                    .title(format!(
                        "{} ({})",
                        snapshot.provider.to_uppercase(),
                        snapshot.plan.as_deref().unwrap_or("Unknown")
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.provider_color(&snapshot.provider)));

                f.render_widget(provider_block, provider_area);

                let inner = Rect::new(
                    provider_area.x + 1,
                    provider_area.y + 2,
                    provider_area.width.saturating_sub(2),
                    provider_area.height.saturating_sub(3),
                );

                for (i, window) in snapshot.windows.iter().enumerate() {
                    let gauge_area = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);

                    if gauge_area.y >= provider_area.y + provider_area.height - 1 {
                        break;
                    }

                    let time_str = window
                        .resets_at
                        .map(|t| {
                            let now = chrono::Utc::now();
                            let diff = t.signed_duration_since(now);
                            format!("{}h {}m", diff.num_hours(), diff.num_minutes() % 60)
                        })
                        .unwrap_or_default();

                    let gauge = GradientGauge::new(&window.label, window.used_percent)
                        .color(theme.gauge_color(window.used_percent))
                        .time(&time_str);

                    f.render_widget(gauge, gauge_area);
                }

                if let Some(ref credits) = snapshot.credits {
                    let credits_text = if credits.limit.is_some() {
                        format!(
                            "Credits ${:.2} / ${:.2}",
                            credits.used,
                            credits.limit.unwrap()
                        )
                    } else {
                        format!("Credits ${:.2} (unlimited)", credits.used)
                    };

                    let credits_area = Rect::new(
                        inner.x,
                        inner.y + snapshot.windows.len() as u16,
                        inner.width,
                        1,
                    );

                    let credits = Paragraph::new(credits_text).style(Style::default().fg(theme.fg));
                    f.render_widget(credits, credits_area);
                }

                y_offset += provider_height;
            }

            let footer_text = format!(
                "Last fetched: {} | Press q to quit, r to refresh",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );
            let footer = Paragraph::new(footer_text).style(Style::default().fg(theme.dim));
            f.render_widget(footer, chunks[2]);
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Char('r') => {}
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    Ok(())
}
