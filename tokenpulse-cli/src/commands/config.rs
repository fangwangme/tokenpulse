use crate::ConfigAction;
use anyhow::Result;
use tokenpulse_core::config::ConfigManager;
use tokenpulse_core::config::QuotaDisplayMode;

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
            let mode_str = match config.display.quota_display_mode {
                QuotaDisplayMode::Used => "used",
                QuotaDisplayMode::Remaining => "remaining",
            };
            println!("  quota_display_mode: {}", mode_str);
            let refresh_str = match config.display.quota_auto_refresh_secs {
                0 => "disabled".to_string(),
                s => format!("{} min", s / 60),
            };
            println!("  quota_auto_refresh_interval: {}", refresh_str);
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
                "quota_auto_refresh_interval" => {
                    let mins: u32 = value.parse().map_err(|_| {
                        anyhow::anyhow!(
                            "Invalid value '{}' for quota_auto_refresh_interval. Expected: 0, 1, 2, 5, 10, 15",
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
                                "Invalid value '{}' for quota_auto_refresh_interval. Supported intervals: 0, 1, 2, 5, 10, 15 (minutes)",
                                value
                            );
                        }
                    };
                    config.display.quota_auto_refresh_secs = secs;
                    manager.save(&config)?;
                    let label = if secs == 0 {
                        "disabled".to_string()
                    } else {
                        format!("{mins} min")
                    };
                    println!("quota_auto_refresh_interval = {label}");
                }
                _ => {
                    anyhow::bail!(
                        "Unknown setting '{}'. Available settings:\n  quota_display_mode           (used | remaining)\n  show_empty_providers         (true | false)\n  quota_auto_refresh_interval  (0 | 1 | 2 | 5 | 10 | 15 — minutes, 0 = disabled)",
                        key
                    );
                }
            }
        }
    }

    Ok(())
}
