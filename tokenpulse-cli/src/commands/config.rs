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
                _ => {
                    anyhow::bail!(
                        "Unknown setting '{}'. Available settings:\n  quota_display_mode  (used | remaining)\n  show_empty_providers  (true | false)",
                        key
                    );
                }
            }
        }
    }

    Ok(())
}
