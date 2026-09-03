use super::UsageState;
use crate::commands::quota::quota_provider_ids;
use crate::tui::theme::{theme_status_label, Theme};
use ratatui::{
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use tokenpulse_core::config::{Config, ConfigManager, QuotaDisplayMode};

/// The settings rows above the per-provider toggles, in display order. Both the
/// row count and the keyboard dispatch read from this, and
/// `settings_items_match_fixed_keys` pins it to `get_settings_items`.
const FIXED_SETTING_KEYS: &[&str] = &[
    "quota_display_mode",
    "show_empty_providers",
    "show_account",
    "auto_refresh_interval",
    "theme",
    "scan_antigravity",
    "refresh_quota",
    "notification_level",
    "notification_sound",
    "keeper_engine",
];

/// Sounds offered by the Settings row, in cycle order.
const SOUND_CYCLE: &[&str] = &["chime", "Hero", "Glass", "Submarine", "none"];

fn next_notification_sound(current: &str) -> String {
    let pos = SOUND_CYCLE
        .iter()
        .position(|s| s.eq_ignore_ascii_case(current))
        .map(|i| (i + 1) % SOUND_CYCLE.len())
        .unwrap_or(0);
    SOUND_CYCLE[pos].to_string()
}

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
        SettingItem {
            key: "notification_level",
            label: config
                .display
                .notification_level
                .display_label()
                .to_string(),
            value_color: match config.display.notification_level {
                tokenpulse_core::config::NotificationLevel::Off => theme.dim,
                _ => theme.gauge_low,
            },
        },
        SettingItem {
            key: "notification_sound",
            label: config.display.notification_sound.clone(),
            value_color: if config
                .display
                .notification_sound
                .eq_ignore_ascii_case(tokenpulse_core::notification::SOUND_NONE)
            {
                theme.dim
            } else {
                theme.gauge_low
            },
        },
        SettingItem {
            key: "keeper_engine",
            label: if config.keeper.enabled {
                "enabled"
            } else {
                "disabled"
            }
            .to_string(),
            value_color: if config.keeper.enabled {
                theme.gauge_low
            } else {
                theme.dim
            },
        },
    ];

    // Quota provider checkboxes. The rows come from the quota registry so they
    // are exactly the providers that have a fetcher. Usage parsing is driven by
    // `SUPPORTED_USAGE_PROVIDERS`, not by this map, and the usage view has its
    // own source filter — so nothing here decides what usage is scanned.
    for provider in quota_provider_ids() {
        let enabled = provider_enabled(config, provider);
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

/// Whether a quota provider counts as enabled.
///
/// Absent means off: quota resolution walks `config.providers`, so a key that
/// is not there is never fetched. Rendering and toggling both read this so the
/// checkbox and the keypress cannot disagree.
fn provider_enabled(config: &Config, provider: &str) -> bool {
    config
        .providers
        .get(provider)
        .map(|p| p.enabled)
        .unwrap_or(false)
}

pub fn settings_row_count(_state: &UsageState) -> usize {
    FIXED_SETTING_KEYS.len() + quota_provider_ids().len()
}

pub fn handle_settings_action(
    state: &mut UsageState,
    config: &mut Config,
    config_manager: &ConfigManager,
    theme: &mut Theme,
) -> anyhow::Result<()> {
    let idx = state.selected_row;

    // Dispatch by key rather than row number: the fixed rows and the provider
    // rows below them shift every time a setting is added, and an index-based
    // chain silently toggles the wrong setting when one of them is missed.
    match FIXED_SETTING_KEYS.get(idx).copied() {
        Some("quota_display_mode") => {
            config.display.quota_display_mode = match config.display.quota_display_mode {
                QuotaDisplayMode::Used => QuotaDisplayMode::Remaining,
                QuotaDisplayMode::Remaining => QuotaDisplayMode::Used,
            };
        }
        Some("show_empty_providers") => {
            config.display.show_empty_providers = !config.display.show_empty_providers;
        }
        Some("show_account") => config.display.show_account = !config.display.show_account,
        Some("auto_refresh_interval") => {
            config.display.auto_refresh_secs =
                next_refresh_interval(config.display.auto_refresh_secs);
        }
        Some("theme") => {
            config.display.theme = config.display.theme.next();
            *theme = Theme::from_preference(config.display.theme);
        }
        Some("scan_antigravity") => {
            config.display.scan_antigravity = !config.display.scan_antigravity;
        }
        Some("refresh_quota") => config.display.refresh_quota = !config.display.refresh_quota,
        Some("notification_level") => {
            config.display.notification_level = config.display.notification_level.next();
        }
        Some("notification_sound") => {
            config.display.notification_sound =
                next_notification_sound(&config.display.notification_sound);
            // Play the new sound so the choice can be judged by ear.
            tokenpulse_core::notification::play_alert_sound(&config.display.notification_sound);
        }
        Some("keeper_engine") => config.keeper.enabled = !config.keeper.enabled,
        _ => {
            // Quota configuration only. The usage view's source filter is
            // session state with its own key (`s`), so one keypress here must
            // not silently hide usage rows as well.
            let provider_idx = idx - FIXED_SETTING_KEYS.len();
            if let Some(&provider) = quota_provider_ids().get(provider_idx) {
                // Negate what the row displays, not what `or_default()` seeds.
                // A provider absent from the map renders `[ ]` (quota
                // resolution only sees keys that are present), while the
                // default entry is `enabled: true` — inserting then negating
                // wrote `false` and the first keypress did nothing visible.
                let enabled = provider_enabled(config, provider);
                config
                    .providers
                    .entry(provider.to_string())
                    .or_default()
                    .enabled = !enabled;
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

    let first_provider_row = items.iter().position(|i| i.key == "provider_enable");
    // Which rendered line the cursor sits on, so the view can scroll to it.
    let mut selected_line = 0usize;

    for (idx, item) in items.iter().enumerate() {
        // Name the block: these rows configure quota polling. They are not the
        // usage source filter, which lives in the usage view under `s`.
        if Some(idx) == first_provider_row {
            lines.push(Line::from(vec![
                Span::styled("Quota providers", Style::default().fg(theme.dim).bold()),
                Span::styled(
                    "  (usage sources are filtered separately)",
                    Style::default().fg(theme.dim),
                ),
            ]));
        }

        let is_selected = idx == state.selected_row;
        if is_selected {
            selected_line = lines.len();
        }
        let marker = if is_selected { ">" } else { " " };

        let mut spans = vec![
            Span::styled(marker, Style::default().fg(theme.accent)),
            Span::raw(" "),
        ];

        if item.key == "provider_enable" {
            // For providers, display as "[x] provider_id"
            let provider_name = item.label.split(' ').next().unwrap_or("");
            let enabled = provider_enabled(config, provider_name);
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

    let settings_scroll_offset =
        scroll_offset_for(selected_line, lines.len(), inner.height as usize);

    // The dashboard body is `Min(10)` between 10 rows of chrome, so on a
    // half-screen terminal the list is taller than the space it gets. Without
    // this the cursor walks off the bottom edge and toggles a row nobody can
    // see. Deliberately unwrapped: scrolling counts rendered rows, so one line
    // has to stay one row — a long config path is clipped rather than reflowed.
    let paragraph = Paragraph::new(lines).scroll((settings_scroll_offset as u16, 0));
    f.render_widget(paragraph, inner);
}

/// First line to render so `selected_line` stays on screen.
///
/// Stateless: it scrolls the minimum needed to bring the cursor into view at
/// the bottom edge, and never past the end of the list.
fn scroll_offset_for(selected_line: usize, total_lines: usize, visible: usize) -> usize {
    if visible == 0 || total_lines <= visible {
        return 0;
    }
    selected_line
        .saturating_sub(visible - 1)
        .min(total_lines - visible)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::usage::UsageDashboard;

    /// The keyboard dispatch indexes `FIXED_SETTING_KEYS` while the screen
    /// renders `get_settings_items`. If the two ever disagree, pressing space
    /// toggles a different setting than the highlighted row.
    #[test]
    fn settings_items_match_fixed_keys() {
        let dashboard = UsageDashboard { daily: vec![] };
        let state = UsageState::new(&dashboard, vec![]);
        let config = Config::default();
        let theme = Theme::new(crate::tui::theme::ThemeMode::Dark);

        let items = get_settings_items(&state, &config, &theme);
        let keys: Vec<&str> = items.iter().map(|i| i.key).collect();

        assert_eq!(&keys[..FIXED_SETTING_KEYS.len()], FIXED_SETTING_KEYS);
        assert_eq!(items.len(), settings_row_count(&state));
        assert!(keys[FIXED_SETTING_KEYS.len()..]
            .iter()
            .all(|k| *k == "provider_enable"));
    }

    /// The Settings rows are the quota registry, nothing else. `gemini` has no
    /// quota fetcher, so it must not be configurable here even though Gemini
    /// CLI usage is still parsed and shown.
    #[test]
    fn settings_provider_rows_are_exactly_the_quota_registry() {
        let dashboard = UsageDashboard { daily: vec![] };
        let state = UsageState::new(&dashboard, vec![]);
        let config = Config::default();
        let theme = Theme::new(crate::tui::theme::ThemeMode::Dark);

        let items = get_settings_items(&state, &config, &theme);
        let provider_labels: Vec<String> = items
            .iter()
            .filter(|i| i.key == "provider_enable")
            .map(|i| i.label.clone())
            .collect();

        assert_eq!(
            provider_labels,
            vec![
                "claude (enabled)",
                "codex (enabled)",
                "copilot (enabled)",
                "antigravity (enabled)",
            ]
        );
        assert!(!provider_labels.iter().any(|l| l.starts_with("gemini")));
        assert_eq!(settings_row_count(&state), FIXED_SETTING_KEYS.len() + 4);
    }

    /// One keypress used to mutate two different semantics: the persisted quota
    /// config and the session-only usage source filter. The usage view has its
    /// own filter (`s`), so this row must move the config alone.
    #[test]
    fn provider_toggle_changes_quota_config_only() {
        let dashboard = UsageDashboard { daily: vec![] };
        let mut state = UsageState::new(&dashboard, vec![]);
        // Two sources, so the old `len() > 1` removal branch would fire on the
        // disable half of the toggle and the insert branch on the enable half.
        state.enabled_sources = ["claude", "gemini"].into_iter().map(String::from).collect();
        let mut config = Config::default();
        let mut theme = Theme::new(crate::tui::theme::ThemeMode::Dark);

        let temp_dir = tempfile::tempdir().unwrap();
        let config_manager = ConfigManager::with_path(temp_dir.path().join("config.toml"));

        // The first provider row.
        state.selected_row = FIXED_SETTING_KEYS.len();
        let provider = quota_provider_ids()[0];
        let sources_before = state.enabled_sources.clone();
        assert!(config.providers.get(provider).unwrap().enabled);

        handle_settings_action(&mut state, &mut config, &config_manager, &mut theme).unwrap();

        assert!(
            !config.providers.get(provider).unwrap().enabled,
            "the quota config entry is what the row toggles"
        );
        assert_eq!(
            state.enabled_sources, sources_before,
            "the usage source filter must be untouched"
        );

        handle_settings_action(&mut state, &mut config, &config_manager, &mut theme).unwrap();
        assert!(config.providers.get(provider).unwrap().enabled);
        assert_eq!(state.enabled_sources, sources_before);
    }

    /// Renders the Settings tab into the rect the real dashboard gives it, not
    /// a full-screen area. The body sits between 10 rows of chrome, so a
    /// full-screen render flatters the layout by ~10 rows and hides exactly the
    /// overflow this tab has to survive.
    fn render_at_terminal_size(
        state: &UsageState,
        config: &Config,
        width: u16,
        height: u16,
    ) -> String {
        use ratatui::{backend::TestBackend, Terminal};

        let theme = Theme::new(crate::tui::theme::ThemeMode::Dark);
        let temp_dir = tempfile::tempdir().unwrap();
        let config_manager = ConfigManager::with_path(temp_dir.path().join("config.toml"));

        // Ask the real layout where the body goes rather than hardcoding it, so
        // this test cannot drift if the chrome changes.
        let body = super::super::dashboard_root_sections(Rect::new(0, 0, width, height))[2];
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| render_settings_tab(f, body, state, config, &config_manager, &theme))
            .unwrap();

        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The rendered tab must name the block, so a reader cannot mistake these
    /// rows for "which agents are scanned for usage".
    #[test]
    fn settings_tab_labels_the_provider_block_as_quota_providers() {
        let dashboard = UsageDashboard { daily: vec![] };
        let state = UsageState::new(&dashboard, vec![]);
        let config = Config::default();

        let rendered = render_at_terminal_size(&state, &config, 80, 40);

        assert!(rendered.contains("Quota providers"));
        for provider in quota_provider_ids() {
            assert!(
                rendered.contains(&format!("[x] {provider}")),
                "missing row for {provider}"
            );
        }
        assert!(
            !rendered.contains("] gemini"),
            "gemini has no quota fetcher and must not be a Settings row"
        );
        // The label must not claim usage is always scanned: `scan_antigravity`
        // is rendered a few rows above and can switch Antigravity usage off.
        assert!(!rendered.contains("always scanned"));
    }

    /// The dashboard body is `Min(10)` between 10 rows of chrome, so an 80x30
    /// terminal leaves the Settings list 18 rows for more lines than that. Every
    /// selectable row must still be reachable *and* visible — a cursor on a row
    /// rendered past the bottom edge toggles a setting nobody can see.
    #[test]
    fn every_settings_row_stays_visible_when_selected() {
        let dashboard = UsageDashboard { daily: vec![] };
        let config = Config::default();
        let mut state = UsageState::new(&dashboard, vec![]);

        for terminal_height in [24, 30, 40] {
            for row in 0..settings_row_count(&state) {
                state.selected_row = row;
                let rendered = render_at_terminal_size(&state, &config, 80, terminal_height);
                // Match the cursor at the start of a row, just inside the left
                // border. A bare `contains("> ")` would pass on the theme row's
                // `auto -> dark` no matter where the cursor actually went.
                assert!(
                    rendered.lines().any(|line| line.starts_with("│> ")),
                    "row {row} at height {terminal_height}: cursor scrolled off screen\n{rendered}"
                );
            }
        }
    }

    /// The specific regression: at 80x30 the last quota provider used to render
    /// past the bottom edge while remaining selectable.
    #[test]
    fn last_quota_provider_is_visible_on_a_half_screen_terminal() {
        let dashboard = UsageDashboard { daily: vec![] };
        let config = Config::default();
        let mut state = UsageState::new(&dashboard, vec![]);

        let last_provider = *quota_provider_ids().last().unwrap();
        state.selected_row = settings_row_count(&state) - 1;

        let rendered = render_at_terminal_size(&state, &config, 80, 30);
        assert!(
            rendered.contains(&format!("> [x] {last_provider}")),
            "the selected last provider row must be on screen:\n{rendered}"
        );
    }

    #[test]
    fn scroll_offset_keeps_the_cursor_in_view_without_overscrolling() {
        // Everything fits: never scroll.
        assert_eq!(scroll_offset_for(0, 10, 20), 0);
        assert_eq!(scroll_offset_for(9, 10, 20), 0);
        // Taller than the window: scroll only once the cursor passes the edge.
        assert_eq!(scroll_offset_for(0, 30, 10), 0);
        assert_eq!(scroll_offset_for(9, 30, 10), 0);
        assert_eq!(scroll_offset_for(10, 30, 10), 1);
        // Never past the end, and never divide-by-zero on a collapsed area.
        assert_eq!(scroll_offset_for(29, 30, 10), 20);
        assert_eq!(scroll_offset_for(5, 30, 0), 0);
    }

    /// A quota provider absent from `config.providers` renders as `[ ]`, because
    /// quota resolution only ever sees keys that are in the map. The toggle has
    /// to agree with that: seeding the entry from `ProviderConfig::default()`
    /// (`enabled: true`) and *then* negating writes `false` — the row was
    /// already showing `[ ]`, so the first keypress did nothing visible.
    #[test]
    fn toggling_a_provider_missing_from_config_enables_it_on_the_first_press() {
        let dashboard = UsageDashboard { daily: vec![] };
        let mut state = UsageState::new(&dashboard, vec![]);
        let mut config = Config::default();
        let mut theme = Theme::new(crate::tui::theme::ThemeMode::Dark);
        let temp_dir = tempfile::tempdir().unwrap();
        let config_manager = ConfigManager::with_path(temp_dir.path().join("config.toml"));

        let provider = quota_provider_ids()[0];
        config.providers.remove(provider);

        let theme_ro = Theme::new(crate::tui::theme::ThemeMode::Dark);
        let shown_before = get_settings_items(&state, &config, &theme_ro)
            .into_iter()
            .find(|i| i.label.starts_with(provider))
            .unwrap()
            .label;
        assert_eq!(shown_before, format!("{provider} (disabled)"));

        state.selected_row = FIXED_SETTING_KEYS.len();
        handle_settings_action(&mut state, &mut config, &config_manager, &mut theme).unwrap();

        assert!(
            config.providers.get(provider).unwrap().enabled,
            "one press on a `[ ]` row must turn it on"
        );
    }

    #[test]
    fn notification_sound_cycles_through_every_option_and_wraps() {
        let mut sound = SOUND_CYCLE[0].to_string();
        for expected in SOUND_CYCLE.iter().skip(1) {
            sound = next_notification_sound(&sound);
            assert_eq!(&sound, expected);
        }
        assert_eq!(next_notification_sound(&sound), SOUND_CYCLE[0]);
    }

    #[test]
    fn unknown_notification_sound_cycles_back_to_the_first_option() {
        assert_eq!(next_notification_sound("Bogus"), SOUND_CYCLE[0]);
    }
}
