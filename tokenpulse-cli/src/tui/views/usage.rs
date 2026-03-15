use crate::tui::theme::Theme;
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
use tokenpulse_core::{usage::UsageSummary, UnifiedMessage};

pub fn run(messages: Vec<UnifiedMessage>, summary: UsageSummary) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let theme = Theme::default();
    let mut current_tab = 0;
    let mut _scroll_offset = 0u16;

    loop {
        terminal.draw(|f| {
            let size = f.area();

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(10),
                    Constraint::Length(8),
                    Constraint::Min(5),
                    Constraint::Length(2),
                ])
                .split(size);

            let header = Paragraph::new("TokenPulse - Usage Dashboard")
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(header, chunks[0]);

            // Daily cost chart (simplified)
            let daily_block = Block::default()
                .title("Daily Cost (Last 14 Days)")
                .borders(Borders::ALL);
            f.render_widget(daily_block, chunks[1]);

            // Provider breakdown
            let provider_block = Block::default()
                .title("Provider Breakdown")
                .borders(Borders::ALL);
            f.render_widget(provider_block, chunks[2]);

            let provider_area = Rect::new(
                chunks[2].x + 1,
                chunks[2].y + 2,
                chunks[2].width.saturating_sub(2),
                chunks[2].height.saturating_sub(3),
            );

            for (i, prov) in summary.by_provider.iter().enumerate() {
                if i as u16 >= provider_area.height {
                    break;
                }

                let line = format!(
                    "{} {:.1}% ${:.2}",
                    prov.provider.to_uppercase(),
                    prov.percent,
                    prov.cost
                );

                let para = Paragraph::new(line)
                    .style(Style::default().fg(theme.provider_color(&prov.provider)));

                let row_area = Rect::new(
                    provider_area.x,
                    provider_area.y + i as u16,
                    provider_area.width,
                    1,
                );
                f.render_widget(para, row_area);
            }

            // Details area
            let details_block = Block::default()
                .title(match current_tab {
                    0 => "Token Details",
                    1 => "Daily",
                    2 => "Models",
                    3 => "Sessions",
                    _ => "Details",
                })
                .borders(Borders::ALL);
            f.render_widget(details_block, chunks[3]);

            // Tabs
            let tabs = Paragraph::new(" [Overview] [Daily] [Models] [Sessions]")
                .style(Style::default().fg(theme.dim));
            f.render_widget(tabs, chunks[4]);

            // Footer
            let footer = Paragraph::new(format!(
                "Total: ${:.2} | {} messages | Press q to quit, <-/-> tabs, j/k scroll",
                summary.total_cost,
                messages.len()
            ))
            .style(Style::default().fg(theme.dim));
            f.render_widget(
                footer,
                Rect::new(chunks[4].x, chunks[4].y, chunks[4].width, 1),
            );
        })?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Left => {
                        if current_tab > 0 {
                            current_tab -= 1;
                        }
                    }
                    KeyCode::Right => {
                        if current_tab < 3 {
                            current_tab += 1;
                        }
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        _scroll_offset = _scroll_offset.saturating_add(1);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        _scroll_offset = _scroll_offset.saturating_sub(1);
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
