use super::UsageState;
use crate::tui::theme::Theme;
use chrono::{DateTime, Local, Utc};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Row, Table},
};
use tokenpulse_core::{
    config::{default_keeper_agents, Config},
    keeper::{compute_next_daily_trigger, compute_next_weekly_trigger},
};

pub const KEEPER_AGENTS: &[&str] = &["claude", "codex", "antigravity"];

pub fn keeper_agent_name(id: &str) -> &'static str {
    match id {
        "claude" => "Claude Code",
        "codex" => "OpenAI Codex",
        "antigravity" => "Google Antigravity",
        _ => "AI Agent",
    }
}

pub fn render_keeper_tab(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &UsageState,
    config: &Config,
    theme: &Theme,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header info / Global Master Switch
            Constraint::Length(12), // Agent cards
            Constraint::Min(8),     // Logs table
        ])
        .split(area);

    render_header_bar(f, chunks[0], config, theme);
    render_agent_cards(f, chunks[1], state, config, theme);
    render_logs_panel(f, chunks[2], state, theme);
}

fn render_header_bar(f: &mut ratatui::Frame, area: Rect, config: &Config, theme: &Theme) {
    let global_enabled = config.keeper.enabled;
    let status_str = if global_enabled {
        " [ENABLED] "
    } else {
        " [DISABLED] "
    };
    let status_style = if global_enabled {
        Style::default().fg(Color::Black).bg(theme.gauge_low).bold()
    } else {
        Style::default().fg(Color::White).bg(theme.dim).bold()
    };

    let title_line = Line::from(vec![
        Span::styled(" Master Switch: ", Style::default().fg(theme.fg).bold()),
        Span::styled(status_str, status_style),
        Span::raw("   "),
        Span::styled("Hint: ", Style::default().fg(theme.accent_soft).bold()),
        Span::styled(
            "To customize wakeup time/models/commands, edit ~/.config/tokenpulse/config.toml",
            Style::default().fg(theme.dim),
        ),
    ]);

    let block = Block::default()
        .title(Span::styled(
            " Keeper Scheduled Activation Engine ",
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));

    let paragraph = Paragraph::new(title_line).block(block);
    f.render_widget(paragraph, area);
}

fn render_agent_cards(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &UsageState,
    config: &Config,
    theme: &Theme,
) {
    let columns = KEEPER_AGENTS.len();
    let col_constraints = vec![Constraint::Ratio(1, columns as u32); columns];
    let card_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(col_constraints)
        .split(area);

    let default_agents = default_keeper_agents();

    for (idx, &agent_id) in KEEPER_AGENTS.iter().enumerate() {
        let is_selected = state.selected_keeper_index == idx;
        let card_area = card_areas[idx];

        let agent_config = config
            .keeper
            .agents
            .get(agent_id)
            .or_else(|| default_agents.get(agent_id));

        let border_color = if is_selected {
            theme.accent
        } else {
            theme.border
        };

        let card_title = format!(" {} ", keeper_agent_name(agent_id));
        let title_style = if is_selected {
            Style::default().fg(theme.accent).bold()
        } else {
            Style::default().fg(theme.provider_color(agent_id)).bold()
        };

        let block = Block::default()
            .title(Span::styled(card_title, title_style))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(card_area);
        f.render_widget(block, card_area);

        let Some(agent_cfg) = agent_config else {
            continue;
        };

        // 5h / Daily switch status
        let session_icon = if agent_cfg.session_keeper_enabled {
            "[✓]"
        } else {
            "[ ]"
        };
        let session_style = if agent_cfg.session_keeper_enabled {
            Style::default().fg(theme.gauge_low).bold()
        } else {
            Style::default().fg(theme.dim)
        };

        // Weekly switch status
        let weekly_icon = if agent_cfg.weekly_keeper_enabled {
            "[✓]"
        } else {
            "[ ]"
        };
        let weekly_style = if agent_cfg.weekly_keeper_enabled {
            Style::default().fg(theme.gauge_low).bold()
        } else {
            Style::default().fg(theme.dim)
        };

        // Calculate next daily trigger
        let now_local = Local::now();
        let next_daily = compute_next_daily_trigger(
            &agent_cfg.daily_wakeup_time,
            state.keeper_daily_triggered.get(agent_id).copied(),
            now_local,
        );
        let next_daily_str = match next_daily {
            Some(dt) => dt.format("%H:%M").to_string(),
            None => "--:--".to_string(),
        };

        // Find quota snapshot for weekly reset
        let quota_snapshot = state
            .quota_snapshots
            .iter()
            .find(|s| s.provider.eq_ignore_ascii_case(agent_id));
        let resets_at = quota_snapshot.and_then(|s| {
            s.windows
                .iter()
                .find(|w| {
                    w.label.to_lowercase().contains("week")
                        || w.label.to_lowercase().contains("7-day")
                        || w.label.to_lowercase().contains("weekly")
                })
                .and_then(|w| w.resets_at)
        });

        let next_weekly = compute_next_weekly_trigger(
            resets_at,
            state.keeper_weekly_triggered.get(agent_id).copied(),
            Utc::now(),
        );
        let next_weekly_str = match next_weekly {
            Some(dt) => {
                let local_dt: DateTime<Local> = dt.into();
                local_dt.format("%m-%d %H:%M").to_string()
            }
            None => "Waiting Quota".to_string(),
        };

        // Last execution status
        let last_exec = state
            .keeper_logs
            .iter()
            .rev()
            .find(|rec| rec.agent == agent_id);

        let (last_status_str, last_status_style) = match last_exec {
            Some(rec) if rec.success => (
                format!("✓ Success ({}ms)", rec.duration_ms),
                Style::default().fg(theme.gauge_low),
            ),
            Some(rec) => (
                format!("✗ Failed ({})", rec.output_snippet),
                Style::default().fg(theme.gauge_high),
            ),
            None => (
                "Never triggered".to_string(),
                Style::default().fg(theme.dim),
            ),
        };

        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::styled(session_icon, session_style),
            Span::styled(" 5h Wakeup: ", Style::default().fg(theme.fg).bold()),
            Span::styled(
                &agent_cfg.daily_wakeup_time,
                Style::default().fg(theme.accent_soft),
            ),
            Span::styled(
                format!(" (next {})", next_daily_str),
                Style::default().fg(theme.dim),
            ),
        ]));

        lines.push(Line::from(vec![
            Span::styled(weekly_icon, weekly_style),
            Span::styled(" Weekly Sync: ", Style::default().fg(theme.fg).bold()),
            Span::styled("Auto Reset+1m", Style::default().fg(theme.accent_soft)),
        ]));
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(
                format!("Next: {}", next_weekly_str),
                Style::default().fg(theme.dim),
            ),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Model: ", Style::default().fg(theme.dim)),
            Span::styled(&agent_cfg.model, Style::default().fg(theme.fg).bold()),
        ]));

        lines.push(Line::from(vec![
            Span::styled("Last: ", Style::default().fg(theme.dim)),
            Span::styled(last_status_str, last_status_style),
        ]));

        lines.push(Line::raw(""));
        if is_selected {
            lines.push(Line::from(vec![
                Span::styled("[1/d] ", Style::default().fg(theme.accent_soft).bold()),
                Span::styled("5h ", Style::default().fg(theme.fg)),
                Span::styled("[2/w] ", Style::default().fg(theme.accent_soft).bold()),
                Span::styled("Wk ", Style::default().fg(theme.fg)),
                Span::styled("[p] ", Style::default().fg(theme.accent_soft).bold()),
                Span::styled("Ping", Style::default().fg(theme.fg)),
            ]));
        } else {
            lines.push(Line::from(vec![Span::styled(
                "Tab / ↑↓ to select",
                Style::default().fg(theme.dim),
            )]));
        }

        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, inner);
    }
}

fn render_logs_panel(f: &mut ratatui::Frame, area: Rect, state: &UsageState, theme: &Theme) {
    let block = Block::default()
        .title(Span::styled(
            " Execution History & Heartbeat Logs (Recent) ",
            Style::default().fg(theme.accent).bold(),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));

    if state.keeper_logs.is_empty() {
        let msg = Paragraph::new(
            " No keeper activations recorded yet. Press [p] on any agent to test immediate ping.",
        )
        .style(Style::default().fg(theme.dim))
        .block(block);
        f.render_widget(msg, area);
        return;
    }

    let header_cells = [
        "Time", "Agent", "Trigger", "Model", "Duration", "Status", "Snippet",
    ]
    .iter()
    .map(|&h| {
        ratatui::widgets::Cell::from(Span::styled(h, Style::default().fg(theme.accent).bold()))
    });

    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows: Vec<Row> = state
        .keeper_logs
        .iter()
        .rev()
        .take(50)
        .map(|rec| {
            let status_span = if rec.success {
                Span::styled("✓ Success", Style::default().fg(theme.gauge_low).bold())
            } else {
                Span::styled("✗ Failed", Style::default().fg(theme.gauge_high).bold())
            };

            let cells = vec![
                ratatui::widgets::Cell::from(rec.timestamp.format("%H:%M:%S").to_string()),
                ratatui::widgets::Cell::from(Span::styled(
                    rec.agent.clone(),
                    Style::default().fg(theme.provider_color(&rec.agent)).bold(),
                )),
                ratatui::widgets::Cell::from(rec.trigger_type.label()),
                ratatui::widgets::Cell::from(rec.model.clone()),
                ratatui::widgets::Cell::from(format!("{}ms", rec.duration_ms)),
                ratatui::widgets::Cell::from(status_span),
                ratatui::widgets::Cell::from(rec.output_snippet.clone()),
            ];
            Row::new(cells).style(Style::default().fg(theme.fg))
        })
        .collect();

    let widths = [
        Constraint::Length(10), // Time
        Constraint::Length(14), // Agent
        Constraint::Length(14), // Trigger
        Constraint::Length(24), // Model
        Constraint::Length(10), // Duration
        Constraint::Length(12), // Status
        Constraint::Min(20),    // Snippet
    ];

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use tokenpulse_core::keeper::{KeeperExecutionRecord, KeeperTriggerType};

    #[test]
    fn test_keeper_agent_metadata() {
        assert_eq!(keeper_agent_name("claude"), "Claude Code");
        assert_eq!(keeper_agent_name("codex"), "OpenAI Codex");
        assert_eq!(keeper_agent_name("antigravity"), "Google Antigravity");
        assert_eq!(KEEPER_AGENTS.len(), 3);
    }

    #[test]
    fn test_render_keeper_tab_smoke() {
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let config = Config::default();
        let theme = Theme::default();
        let dashboard = crate::tui::views::usage::UsageDashboard { daily: vec![] };
        let mut state = UsageState::new(&dashboard, vec![]);

        state.keeper_logs.push(KeeperExecutionRecord {
            id: "rec-1".to_string(),
            agent: "claude".to_string(),
            trigger_type: KeeperTriggerType::Daily,
            model: "claude-3-5-haiku-20241022".to_string(),
            prompt: "Hi".to_string(),
            command_executed: "claude -p \"Hi\"".to_string(),
            timestamp: Local::now(),
            duration_ms: 350,
            success: true,
            exit_code: Some(0),
            output_snippet: "Hello there".to_string(),
        });

        terminal
            .draw(|f| {
                render_keeper_tab(f, f.area(), &state, &config, &theme);
            })
            .unwrap();
    }
}
