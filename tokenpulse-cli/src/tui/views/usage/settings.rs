use super::UsageState;
use crate::tui::theme::{theme_status_label, Theme};
use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use tokenpulse_core::config::{Config, ConfigManager, QuotaDisplayMode};

const ALL_PROVIDERS: &[&str] = &["claude", "codex", "gemini", "antigravity", "copilot"];

pub struct SettingItem {
    pub key: &'static str,
    pub label: String,
    pub value_color: ratatui::style::Color,
}

pub fn get_settings_items(state: &UsageState, config: &Config, theme: &Theme) -> Vec<SettingItem> {
    let mode = match config.display.quota_display_mode {
        QuotaDisplayMode::Used => "used",
        QuotaDisplayMode::Remaining => "remaining",
    };

    let auto_refresh = {
        let base = refresh_label(config.display.auto_refresh_secs);
        if config.display.auto_refresh_secs > 0 {
            // A single shared timer drives both usage and quota refreshes.
            let elapsed = state.last_refresh.elapsed().as_secs() as u32;
            let remaining = config.display.auto_refresh_secs.saturating_sub(elapsed);
            let m = remaining / 60;
            let s = remaining % 60;
            format!("{} (next {}m {}s)", base, m, s)
        } else {
            base.to_string()
        }
    };
    let theme_label = theme_status_label(config.display.theme, theme.mode);

    let mut items = vec![
        SettingItem {
            key: "quota_display_mode",
            label: mode.to_string(),
            value_color: theme.codex,
        },
        SettingItem {
            key: "show_empty_providers",
            label: config.display.show_empty_providers.to_string(),
            value_color: theme.gemini,
        },
        SettingItem {
            key: "show_account",
            label: config.display.show_account.to_string(),
            value_color: theme.claude,
        },
        SettingItem {
            key: "auto_refresh_interval",
            label: auto_refresh.to_string(),
            value_color: theme.accent_soft,
        },
        SettingItem {
            key: "theme",
            label: theme_label,
            value_color: theme.antigravity,
        },
        SettingItem {
            key: "scan_antigravity",
            label: config.display.scan_antigravity.to_string(),
            value_color: theme.antigravity,
        },
        SettingItem {
            key: "refresh_quota",
            label: config.display.refresh_quota.to_string(),
            value_color: theme.accent_soft,
        },
    ];

    // Add provider checkboxes!
    let providers = ALL_PROVIDERS.to_vec();

    for provider in providers {
        let enabled = config
            .providers
            .get(provider)
            .map(|p| p.enabled)
            .unwrap_or(false);
        items.push(SettingItem {
            key: "provider_enable",
            label: format!(
                "{provider} ({})",
                if enabled { "enabled" } else { "disabled" }
            ),
            value_color: theme.provider_color(provider),
        });
    }

    items
}

fn refresh_label(secs: u32) -> &'static str {
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

const REFRESH_INTERVALS: &[u32] = &[0, 60, 120, 300, 600, 900];
fn next_refresh_interval(curr: u32) -> u32 {
    let pos = REFRESH_INTERVALS
        .iter()
        .position(|&v| v == curr)
        .unwrap_or(0);
    REFRESH_INTERVALS[(pos + 1) % REFRESH_INTERVALS.len()]
}

pub fn settings_row_count(_state: &UsageState) -> usize {
    let providers_count = ALL_PROVIDERS.len();
    7 + providers_count
}

pub fn handle_settings_action(
    state: &mut UsageState,
    config: &mut Config,
    config_manager: &ConfigManager,
    theme: &mut Theme,
) -> anyhow::Result<()> {
    let idx = state.selected_row;

    if idx == 0 {
        config.display.quota_display_mode = match config.display.quota_display_mode {
            QuotaDisplayMode::Used => QuotaDisplayMode::Remaining,
            QuotaDisplayMode::Remaining => QuotaDisplayMode::Used,
        };
    } else if idx == 1 {
        config.display.show_empty_providers = !config.display.show_empty_providers;
    } else if idx == 2 {
        config.display.show_account = !config.display.show_account;
    } else if idx == 3 {
        config.display.auto_refresh_secs = next_refresh_interval(config.display.auto_refresh_secs);
    } else if idx == 4 {
        config.display.theme = config.display.theme.next();
        *theme = Theme::from_preference(config.display.theme);
    } else if idx == 5 {
        config.display.scan_antigravity = !config.display.scan_antigravity;
    } else if idx == 6 {
        config.display.refresh_quota = !config.display.refresh_quota;
    } else {
        let provider_idx = idx - 7;
        let providers = ALL_PROVIDERS.to_vec();

        if provider_idx < providers.len() {
            if let Some(&provider) = providers.get(provider_idx) {
                let p_config = config.providers.entry(provider.to_string()).or_default();
                p_config.enabled = !p_config.enabled;

                if p_config.enabled {
                    state.enabled_sources.insert(provider.to_string());
                } else {
                    if state.enabled_sources.len() > 1 {
                        state.enabled_sources.remove(provider);
                    }
                }
            }
        }
    }

    config_manager.save(config)?;
    Ok(())
}

pub fn render_settings_tab(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &UsageState,
    config: &Config,
    config_manager: &ConfigManager,
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

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Config file ", Style::default().fg(theme.dim)),
        Span::styled(
            config_manager.config_path().display().to_string(),
            Style::default().fg(theme.fg),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Version ", Style::default().fg(theme.dim)),
        Span::styled(env!("CARGO_PKG_VERSION"), Style::default().fg(theme.fg)),
    ]));
    lines.push(Line::raw(""));

    let items = get_settings_items(state, config, theme);

    for (idx, item) in items.iter().enumerate() {
        let is_selected = idx == state.selected_row;
        let marker = if is_selected { ">" } else { " " };

        let mut spans = vec![
            Span::styled(marker, Style::default().fg(theme.accent)),
            Span::raw(" "),
        ];

        if item.key == "provider_enable" {
            // For providers, display as "[x] provider_id"
            let provider_name = item.label.split(' ').next().unwrap_or("");
            let enabled = config
                .providers
                .get(provider_name)
                .map(|p| p.enabled)
                .unwrap_or(false);
            let checkbox = if enabled { "[x]" } else { "[ ]" };
            let checkbox_style = if enabled {
                Style::default().fg(theme.gauge_low)
            } else {
                Style::default().fg(theme.dim)
            };

            spans.push(Span::styled(checkbox, checkbox_style));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                provider_name.to_string(),
                Style::default().fg(item.value_color).bold(),
            ));
        } else {
            spans.push(Span::styled(item.key, Style::default().fg(theme.fg).bold()));
            spans.push(Span::raw(" = "));
            spans.push(Span::styled(
                &item.label,
                Style::default().fg(item.value_color),
            ));
        }

        lines.push(Line::from(spans));
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(
            "space / enter",
            Style::default().fg(theme.accent_soft).bold(),
        ),
        Span::raw(" cycle / toggle selected setting"),
    ]));

    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: true });
    f.render_widget(paragraph, inner);
}
