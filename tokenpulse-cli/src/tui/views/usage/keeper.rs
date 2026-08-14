use super::UsageState;
use crate::tui::theme::Theme;
use chrono::{DateTime, Local, Utc};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
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
    config_path: &std::path::Path,
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

    render_header_bar(f, chunks[0], config, config_path, theme);
    render_agent_cards(f, chunks[1], state, config, theme);
    render_logs_panel(f, chunks[2], state, theme);
}

fn render_header_bar(
    f: &mut ratatui::Frame,
    area: Rect,
    config: &Config,
    config_path: &std::path::Path,
    theme: &Theme,
) {
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
            format!(
                "To customize wakeup time/models/commands, edit {}",
                config_path.display()
            ),
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
        let resets_at =
            tokenpulse_core::keeper::find_snapshot_for_agent(&state.quota_snapshots, agent_id)
                .and_then(tokenpulse_core::keeper::extract_weekly_reset_time);

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

        let (status_label, status_str, status_style) =
            if state.keeper_pings_in_progress.contains(agent_id) {
                (
                    "Status: ",
                    "⟳ Ping running...",
                    Style::default().fg(theme.accent_soft).bold(),
                )
            } else {
                ("Last: ", last_status_str.as_str(), last_status_style)
            };

        lines.push(Line::from(vec![
            Span::styled(status_label, Style::default().fg(theme.dim)),
            Span::styled(status_str, status_style),
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

/// Header + 4 detail rows + blank separator, as emitted by `render_logs_panel`.
pub const KEEPER_LOG_LINES_PER_RECORD: usize = 6;

pub fn keeper_logs_total_lines(state: &UsageState) -> usize {
    if state.keeper_logs.is_empty() {
        return 1;
    }
    state.keeper_logs.len() * KEEPER_LOG_LINES_PER_RECORD
}

/// Largest useful scroll offset: stop once the last record is on screen instead
/// of letting the view scroll past the end into blank space.
pub fn keeper_log_scroll_max(state: &UsageState, frame_area: Rect) -> usize {
    let visible = keeper_logs_visible_height(frame_area);
    keeper_logs_total_lines(state).saturating_sub(visible)
}

/// Height of the log panel's inner area, mirroring `render_keeper_tab`'s layout.
fn keeper_logs_visible_height(frame_area: Rect) -> usize {
    let body = super::dashboard_body_area(frame_area);
    // 3 rows of header bar + 12 rows of agent cards, then the block's borders.
    usize::from(body.height).saturating_sub(3 + 12 + 2)
}

fn render_logs_panel(f: &mut ratatui::Frame, area: Rect, state: &UsageState, theme: &Theme) {
    let title_span = if state.keeper_log_scroll > 0 {
        Span::styled(
            format!(
                " Live Execution & Heartbeat Stream (Scroll: line {}) [Mouse Scroll] ",
                state.keeper_log_scroll
            ),
            Style::default().fg(theme.accent).bold(),
        )
    } else {
        Span::styled(
            " Live Execution & Heartbeat Stream (Newest First) [Mouse Scroll] ",
            Style::default().fg(theme.accent).bold(),
        )
    };

    let block = Block::default()
        .title(title_span)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border));

    if state.keeper_logs.is_empty() {
        let msg = Paragraph::new(
            "  No keeper activations recorded yet. Press [p] on any agent to trigger an immediate test ping.",
        )
        .style(Style::default().fg(theme.dim))
        .block(block);
        f.render_widget(msg, area);
        return;
    }

    let mut log_lines: Vec<Line> = Vec::new();

    // Render in newest-first or chronological order with clear delimiters
    for rec in state.keeper_logs.iter().rev() {
        let (status_text, status_style) = if rec.success {
            (
                format!(
                    "✓ Success ({}ms, exit {})",
                    rec.duration_ms,
                    rec.exit_code.unwrap_or(0)
                ),
                Style::default().fg(theme.gauge_low).bold(),
            )
        } else {
            (
                format!(
                    "✗ Failed ({}ms, exit {})",
                    rec.duration_ms,
                    rec.exit_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "ERR".to_string())
                ),
                Style::default().fg(theme.gauge_high).bold(),
            )
        };

        // Header line: [Timestamp] [Agent] (Trigger) Status
        log_lines.push(Line::from(vec![
            Span::styled(
                format!("[{}] ", rec.timestamp.format("%Y-%m-%d %H:%M:%S")),
                Style::default().fg(theme.dim),
            ),
            Span::styled(
                format!("[{}] ", keeper_agent_name(&rec.agent)),
                Style::default().fg(theme.provider_color(&rec.agent)).bold(),
            ),
            Span::styled(
                format!("({}) ", rec.trigger_type.label()),
                Style::default().fg(theme.accent_soft),
            ),
            Span::styled(status_text, status_style),
        ]));

        // Field 1: Model
        log_lines.push(Line::from(vec![
            Span::styled("  ├─ Model:  ", Style::default().fg(theme.dim)),
            Span::styled(&rec.model, Style::default().fg(theme.fg).bold()),
        ]));

        // Field 2: Prompt
        log_lines.push(Line::from(vec![
            Span::styled("  ├─ Prompt: ", Style::default().fg(theme.dim)),
            Span::styled(
                format!("\"{}\"", rec.prompt),
                Style::default().fg(theme.accent_soft),
            ),
        ]));

        // Field 3: Command
        log_lines.push(Line::from(vec![
            Span::styled("  ├─ Cmd:    ", Style::default().fg(theme.dim)),
            Span::styled(&rec.command_executed, Style::default().fg(theme.dim)),
        ]));

        // Field 4: Reply / Output
        let reply_prefix = if rec.success {
            "  └─ Reply:  "
        } else {
            "  └─ Error:  "
        };
        let reply_style = if rec.success {
            Style::default().fg(theme.fg)
        } else {
            Style::default().fg(theme.gauge_high)
        };
        log_lines.push(Line::from(vec![
            Span::styled(reply_prefix, Style::default().fg(theme.dim)),
            Span::styled(&rec.output_snippet, reply_style),
        ]));

        // Spacing delimiter line
        log_lines.push(Line::raw(""));
    }

    let paragraph = Paragraph::new(log_lines)
        .block(block)
        .scroll((state.keeper_log_scroll as u16, 0));
    f.render_widget(paragraph, area);
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
            model: "haiku".to_string(),
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
                render_keeper_tab(
                    f,
                    f.area(),
                    &state,
                    &config,
                    std::path::Path::new("/tmp/tokenpulse/config.toml"),
                    &theme,
                );
            })
            .unwrap();
    }
}
