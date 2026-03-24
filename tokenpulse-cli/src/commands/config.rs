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
    }

    Ok(())
}
