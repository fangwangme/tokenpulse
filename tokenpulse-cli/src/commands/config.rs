use crate::ConfigAction;
use anyhow::Result;
use tokenpulse_core::config::{
    ConfigManager, NotificationLevel, QuotaDisplayMode, ThemePreference,
};
use tokenpulse_core::notification::{self, QuotaRecovery, SOUND_CHIME, SOUND_NONE};

/// Rejects sound names that would silently never play, so a typo surfaces at
/// `config set` time rather than as a missing chime hours later.
fn validate_notification_sound(value: &str) -> Result<()> {
    if value.eq_ignore_ascii_case(SOUND_CHIME) || value.eq_ignore_ascii_case(SOUND_NONE) {
        return Ok(());
    }
    if std::path::Path::new(&format!("/System/Library/Sounds/{value}.aiff")).is_file() {
        return Ok(());
    }
    anyhow::bail!(
        "Invalid value '{}' for notification_sound. Expected: {} (built-in), {} (silent), \
         or the name of a sound under /System/Library/Sounds (e.g. Hero, Glass, Submarine)",
        value,
        SOUND_CHIME,
        SOUND_NONE
    )
}

pub fn run(action: ConfigAction) -> Result<()> {
    let manager = ConfigManager::new();

    match action {
        ConfigAction::Show => {
            let config = manager.load()?;
            println!("Config file: {}", manager.config_path().display());
            println!();
            println!("Providers:");
            for (name, provider) in &config.providers {
                let status = if provider.enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                println!("  {name}: {status}");
            }
            println!();
            println!("Display:");
            println!(
                "  show_empty_providers: {}",
                config.display.show_empty_providers
            );
            println!("  show_account: {}", config.display.show_account);
            println!("  theme: {}", config.display.theme.label());
            let mode_str = match config.display.quota_display_mode {
                QuotaDisplayMode::Used => "used",
                QuotaDisplayMode::Remaining => "remaining",
            };
            println!("  quota_display_mode: {}", mode_str);
            let refresh_str = match config.display.auto_refresh_secs {
                0 => "disabled".to_string(),
                s if s % 60 == 0 => format!("{} min", s / 60),
                s => format!("{}s", s),
            };
            println!("  auto_refresh_interval: {}", refresh_str);
            println!("  refresh_quota: {}", config.display.refresh_quota);
            println!(
                "  notification_level: {}",
                config.display.notification_level.label()
            );
            println!(
                "  notification_sound: {}",
                config.display.notification_sound
            );
            println!();
            println!("Keeper:");
            println!("  keeper_engine: {}", config.keeper.enabled);
            for (name, agent) in &config.keeper.agents {
                println!(
                    "  {name}: 5h={} weekly={} at {} ({})",
                    agent.session_keeper_enabled,
                    agent.weekly_keeper_enabled,
                    agent.daily_wakeup_time,
                    agent.model
                );
            }
        }
        ConfigAction::Enable { provider } => {
            manager.enable_provider(&provider)?;
            println!("Provider '{provider}' enabled");
        }
        ConfigAction::Disable { provider } => {
            manager.disable_provider(&provider)?;
            println!("Provider '{provider}' disabled");
        }
        ConfigAction::Set { setting } => {
            let (key, value) = setting.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("Expected KEY=VALUE format (e.g. quota_display_mode=used)")
            })?;

            let key = key.trim();
            let value = value.trim();

            let mut config = manager.load().unwrap_or_default();

            match key {
                "quota_display_mode" => {
                    config.display.quota_display_mode = match value {
                        "used" => QuotaDisplayMode::Used,
                        "remaining" => QuotaDisplayMode::Remaining,
                        _ => {
                            anyhow::bail!(
                                "Invalid value '{}' for quota_display_mode. Expected: used, remaining",
                                value
                            );
                        }
                    };
                    manager.save(&config)?;
                    println!("quota_display_mode = {value}");
                }
                "show_empty_providers" => {
                    config.display.show_empty_providers = match value {
                        "true" | "1" | "yes" => true,
                        "false" | "0" | "no" => false,
                        _ => {
                            anyhow::bail!(
                                "Invalid value '{}' for show_empty_providers. Expected: true, false",
                                value
                            );
                        }
                    };
                    manager.save(&config)?;
                    println!("show_empty_providers = {value}");
                }
                "show_account" => {
                    config.display.show_account = match value {
                        "true" | "1" | "yes" => true,
                        "false" | "0" | "no" => false,
                        _ => {
                            anyhow::bail!(
                                "Invalid value '{}' for show_account. Expected: true, false",
                                value
                            );
                        }
                    };
                    manager.save(&config)?;
                    println!("show_account = {value}");
                }
                "theme" => {
                    config.display.theme = match value {
                        "auto" => ThemePreference::Auto,
                        "dark" => ThemePreference::Dark,
                        "light" => ThemePreference::Light,
                        _ => {
                            anyhow::bail!(
                                "Invalid value '{}' for theme. Expected: auto, dark, light",
                                value
                            );
                        }
                    };
                    manager.save(&config)?;
                    println!("theme = {}", config.display.theme.label());
                }
                "auto_refresh_interval" => {
                    let mins: u32 = value.parse().map_err(|_| {
                        anyhow::anyhow!(
                            "Invalid value '{}' for auto_refresh_interval. Expected: 0, 1, 2, 5, 10, 15",
                            value
                        )
                    })?;
                    let secs = match mins {
                        0 => 0,
                        1 => 60,
                        2 => 120,
                        5 => 300,
                        10 => 600,
                        15 => 900,
                        _ => {
                            anyhow::bail!(
                                "Invalid value '{}' for auto_refresh_interval. Supported intervals: 0, 1, 2, 5, 10, 15 (minutes)",
                                value
                            );
                        }
                    };
                    config.display.auto_refresh_secs = secs;
                    manager.save(&config)?;
                    let label = if secs == 0 {
                        "disabled".to_string()
                    } else {
                        format!("{mins} min")
                    };
                    println!("auto_refresh_interval = {label}");
                }
                "keeper_engine" => {
                    config.keeper.enabled = match value {
                        "true" | "1" | "yes" => true,
                        "false" | "0" | "no" => false,
                        _ => {
                            anyhow::bail!(
                                "Invalid value '{}' for keeper_engine. Expected: true, false",
                                value
                            );
                        }
                    };
                    manager.save(&config)?;
                    println!("keeper_engine = {value}");
                }
                "refresh_quota" => {
                    config.display.refresh_quota = match value {
                        "true" | "1" | "yes" => true,
                        "false" | "0" | "no" => false,
                        _ => {
                            anyhow::bail!(
                                "Invalid value '{}' for refresh_quota. Expected: true, false",
                                value
                            );
                        }
                    };
                    manager.save(&config)?;
                    println!("refresh_quota = {value}");
                }
                "notification_level" => {
                    config.display.notification_level = match value {
                        "off" => tokenpulse_core::config::NotificationLevel::Off,
                        "in_app" => tokenpulse_core::config::NotificationLevel::InApp,
                        "terminal" => tokenpulse_core::config::NotificationLevel::Terminal,
                        "system" => tokenpulse_core::config::NotificationLevel::System,
                        _ => {
                            anyhow::bail!(
                                "Invalid value '{}' for notification_level. Expected: off, in_app, terminal, system",
                                value
                            );
                        }
                    };
                    manager.save(&config)?;
                    println!(
                        "notification_level = {}",
                        config.display.notification_level.label()
                    );
                }
                "notification_sound" => {
                    validate_notification_sound(value)?;
                    config.display.notification_sound = value.to_string();
                    manager.save(&config)?;
                    println!("notification_sound = {value}");
                }
                _ => {
                    anyhow::bail!(
                        "Unknown setting '{}'. Available settings:\n  quota_display_mode     (used | remaining)\n  show_empty_providers   (true | false)\n  show_account           (true | false)\n  theme                  (auto | dark | light)\n  auto_refresh_interval  (0 | 1 | 2 | 5 | 10 | 15 — minutes, 0 = disabled)\n  refresh_quota          (true | false)\n  notification_level     (off | in_app | terminal | system)\n  notification_sound     (chime | none | a name under /System/Library/Sounds, e.g. Hero)\n  keeper_engine          (true | false)",
                        key
                    );
                }
            }
        }
        ConfigAction::TestNotification => {
            let config = manager.load()?;
            let level = config.display.notification_level;
            let sound = &config.display.notification_sound;

            println!("notification_level: {}", level.label());
            println!("notification_sound: {sound}");
            if level == NotificationLevel::Off {
                println!();
                println!(
                    "Level is 'off', so nothing will fire. \
                     Try: tokenpulse config set notification_level=system"
                );
                return Ok(());
            }

            notification::notify_quota_restored(
                level,
                sound,
                &[QuotaRecovery {
                    provider: "claude".to_string(),
                    window_label: "5h".to_string(),
                    remaining_percent: 100.0,
                }],
            );
            println!();
            println!("Sent a sample notification.");
            if level == NotificationLevel::System {
                println!(
                    "No banner? Allow your terminal to post notifications in \
                     System Settings > Notifications."
                );
            }
            // Sound and banner run on background threads; give them time to
            // finish before the process exits and kills them.
            std::thread::sleep(std::time::Duration::from_millis(2500));
        }
    }

    Ok(())
}
