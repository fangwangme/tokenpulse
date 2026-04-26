use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, MultiSelect, Select};
use std::collections::HashMap;
use tokenpulse_core::auth::detect_providers;
use tokenpulse_core::config::{
    Config, ConfigManager, DisplayConfig, ProviderConfig, QuotaDisplayMode,
};

/// Supported providers — init selection is based on this list,
/// not on what's detected locally. Detection is only for hints.
const SUPPORTED_PROVIDERS: &[(&str, &str)] = &[
    ("claude", "Claude Code"),
    ("codex", "Codex"),
    ("gemini", "Gemini"),
    ("antigravity", "Antigravity"),
];

pub fn run(use_defaults: bool) -> Result<()> {
    let config_manager = ConfigManager::new();

    println!("\nWelcome to TokenPulse! Let's set up your configuration.\n");
    println!("Detecting installed providers...");

    let detected = detect_providers();

    // Show detection status for each supported provider
    for (name, display_name) in SUPPORTED_PROVIDERS {
        let det = detected.iter().find(|d| d.name == *name);
        let (icon, hint) = match det {
            Some(d) if d.detected => ("\u{2713}", d.credential_hint.as_str()),
            _ => ("\u{2717}", "not detected"),
        };
        let padding = 14usize.saturating_sub(display_name.len());
        println!(
            "  {} {}{} ({})",
            icon,
            display_name,
            " ".repeat(padding.max(1)),
            hint
        );
    }
    println!();

    let (selected_names, display_mode) = if use_defaults {
        // Auto-enable providers whose credentials were detected
        let auto_enabled: Vec<&str> = SUPPORTED_PROVIDERS
            .iter()
            .filter(|(name, _)| detected.iter().any(|d| d.name == *name && d.detected))
            .map(|(name, _)| *name)
            .collect();

        if auto_enabled.is_empty() {
            println!("No providers detected. All providers will be disabled.");
            println!("Install a provider and run `tokenpulse init` again.\n");
        } else {
            println!("Auto-enabling {} detected provider(s).", auto_enabled.len());
        }
        (auto_enabled, QuotaDisplayMode::Remaining)
    } else {
        // Interactive: show all supported providers with detection hints
        let items: Vec<String> = SUPPORTED_PROVIDERS
            .iter()
            .map(|(name, display_name)| {
                let is_detected = detected.iter().any(|d| d.name == *name && d.detected);
                let status = if is_detected { "detected" } else { "not found" };
                format!("{} ({})", display_name, status)
            })
            .collect();

        let defaults: Vec<bool> = SUPPORTED_PROVIDERS
            .iter()
            .map(|(name, _)| detected.iter().any(|d| d.name == *name && d.detected))
            .collect();

        let selections = MultiSelect::with_theme(&ColorfulTheme::default())
            .with_prompt("Select providers to enable")
            .items(&items)
            .defaults(&defaults)
            .interact()?;

        let selected: Vec<&str> = selections
            .iter()
            .map(|&i| SUPPORTED_PROVIDERS[i].0)
            .collect();

        let display_options = &[
            "Show remaining percentage (e.g., \"55% left\")",
            "Show used percentage (e.g., \"45% used\")",
        ];

        let display_selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Display preference")
            .items(display_options)
            .default(0)
            .interact()?;

        let mode = match display_selection {
            0 => QuotaDisplayMode::Remaining,
            _ => QuotaDisplayMode::Used,
        };

        (selected, mode)
    };

    // Build config from supported providers list
    let mut providers = HashMap::new();
    for (name, _) in SUPPORTED_PROVIDERS {
        let enabled = selected_names.contains(name);
        providers.insert(
            name.to_string(),
            ProviderConfig {
                enabled,
                path: None,
            },
        );
    }

    let config = Config {
        version: 1,
        providers,
        display: DisplayConfig {
            show_empty_providers: false,
            quota_display_mode: display_mode,
            quota_auto_refresh_secs: DisplayConfig::default().quota_auto_refresh_secs,
            monthly_budget_usd: None,
        },
    };

    config_manager.save(&config)?;

    println!(
        "\nConfiguration saved to {}\n",
        config_manager.config_path().display()
    );
    println!("Run `tokenpulse quota` to check your usage!");

    Ok(())
}
