use anyhow::Result;
use tokenpulse_core::{QuotaFetcher, quota::{ClaudeQuotaFetcher, CodexQuotaFetcher, GeminiQuotaFetcher, AntigravityQuotaFetcher, fetch_all}};
use tokenpulse_core::config::{ConfigManager, QuotaDisplayMode};

const BAR_WIDTH: usize = 20;

fn draw_progress_bar(used_percent: f64, remaining_percent: f64, period_duration_ms: Option<i64>, resets_at: Option<chrono::DateTime<chrono::Utc>>, display_mode: &QuotaDisplayMode) -> String {
    let used_blocks = ((used_percent / 100.0) * BAR_WIDTH as f64).round() as usize;
    let used_blocks = used_blocks.min(BAR_WIDTH);
    let remaining_blocks = BAR_WIDTH - used_blocks;
    
    // Calculate expected position (vertical line marker)
    let marker_pos = if let (Some(period_ms), Some(reset_time)) = (period_duration_ms, resets_at) {
        let now = chrono::Utc::now();
        let period_start = reset_time - chrono::Duration::milliseconds(period_ms);
        let elapsed_ms = (now - period_start).num_milliseconds();
        if elapsed_ms > 0 && period_ms > 0 {
            let elapsed_fraction = elapsed_ms as f64 / period_ms as f64;
            let expected_used = elapsed_fraction * 100.0;
            let expected_blocks = ((expected_used / 100.0) * BAR_WIDTH as f64).round() as usize;
            Some(expected_blocks.min(BAR_WIDTH))
        } else {
            None
        }
    } else {
        None
    };
    
    // Build progress bar with optional marker
    let mut bar = String::new();
    bar.push('[');
    for i in 0..BAR_WIDTH {
        if marker_pos == Some(i) {
            bar.push('│');
        } else if i < used_blocks {
            bar.push('█');
        } else {
            bar.push('░');
        }
    }
    bar.push(']');
    
    match display_mode {
        QuotaDisplayMode::Used => format!("{} {:.0}% used", bar, used_percent),
        QuotaDisplayMode::Remaining => format!("{} {:.0}% left", bar, remaining_percent),
    }
}

fn calculate_pace(used_percent: f64, period_duration_ms: Option<i64>, resets_at: Option<chrono::DateTime<chrono::Utc>>) -> Option<(String, String)> {
    let period_ms = period_duration_ms?;
    let reset_time = resets_at?;
    
    let now = chrono::Utc::now();
    let period_start = reset_time - chrono::Duration::milliseconds(period_ms);
    let elapsed_ms = (now - period_start).num_milliseconds();
    
    if elapsed_ms <= 0 || now >= reset_time {
        return None;
    }
    
    let elapsed_fraction = elapsed_ms as f64 / period_ms as f64;
    let expected_usage = elapsed_fraction * 100.0;
    let deficit = used_percent - expected_usage;
    
    if deficit.abs() < 5.0 {
        return Some(("on-track".to_string(), "On track".to_string()));
    } else if deficit > 0.0 {
        let rate = used_percent / elapsed_ms as f64;
        let remaining_ms = (100.0 - used_percent) / rate;
        let eta_text = format_duration(remaining_ms as i64);
        return Some(("behind".to_string(), format!("+{:.0}% deficit, runs out in {}", deficit, eta_text)));
    } else {
        return Some(("ahead".to_string(), format!("{:.0}% under budget", deficit.abs())));
    }
}

fn format_duration(ms: i64) -> String {
    let seconds = ms / 1000;
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    
    if days > 0 {
        format!("{}d {}h", days, hours % 24)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes % 60)
    } else {
        format!("{}m", minutes)
    }
}

pub async fn run(provider: Option<String>) -> Result<()> {
    let config_manager = ConfigManager::new();
    let config = config_manager.load().unwrap_or_default();
    let display_mode = &config.display.quota_display_mode;
    let enabled_providers = config
        .providers
        .iter()
        .filter(|(_, p)| p.enabled)
        .map(|(k, _)| k.clone())
        .collect::<Vec<_>>();

    let providers: Vec<Box<dyn QuotaFetcher>> = match provider {
        Some(p) => {
            match p.as_str() {
                "claude" => vec![Box::new(ClaudeQuotaFetcher::new())],
                "codex" => vec![Box::new(CodexQuotaFetcher::new())],
                "gemini" => vec![Box::new(GeminiQuotaFetcher::new())],
                "antigravity" => vec![Box::new(AntigravityQuotaFetcher::new())],
                _ => {
                    eprintln!("Unknown provider: {}", p);
                    eprintln!("Supported providers: claude, codex, gemini, antigravity");
                    return Ok(());
                }
            }
        }
        None => {
            let mut list: Vec<Box<dyn QuotaFetcher>> = Vec::new();
            if enabled_providers.contains(&"claude".to_string()) {
                list.push(Box::new(ClaudeQuotaFetcher::new()));
            }
            if enabled_providers.contains(&"codex".to_string()) {
                list.push(Box::new(CodexQuotaFetcher::new()));
            }
            if enabled_providers.contains(&"gemini".to_string()) {
                list.push(Box::new(GeminiQuotaFetcher::new()));
            }
            if enabled_providers.contains(&"antigravity".to_string()) {
                list.push(Box::new(AntigravityQuotaFetcher::new()));
            }
            list
        }
    };

    let results = fetch_all(providers).await;

    let success_count = results.iter().filter(|r| r.is_ok()).count();
    if success_count == 0 {
        eprintln!("\nNo providers configured.\n");
        eprintln!("To use tokenpulse, you need to have at least one of these tools installed:");
        eprintln!(" - Claude Code: https://docs.anthropic.com/en/docs/claude-code");
        eprintln!(" - Codex: https://github.com/openai/codex");
        eprintln!(" - Gemini CLI: https://github.com/google-gemini/gemini-cli");
        eprintln!(" - Antigravity: https://antigravity.com\n");
        eprintln!("After installing, run the tool to login, then try again.");
        return Ok(());
    }

    for result in &results {
        match result {
            Ok(snapshot) => {
                println!("\n=== {} ===", snapshot.provider.to_uppercase());
                if let Some(ref plan) = snapshot.plan {
                    println!("Plan: {}", plan);
                }
                for window in &snapshot.windows {
                    let remaining_percent = (100.0 - window.used_percent).max(0.0);

                    println!("\n  {}", window.label);
                    println!("  {}", draw_progress_bar(window.used_percent, remaining_percent, window.period_duration_ms, window.resets_at, display_mode));

                    if let Some((status, pace_text)) = calculate_pace(window.used_percent, window.period_duration_ms, window.resets_at) {
                        let indicator = match status.as_str() {
                            "ahead" => "🟢",
                            "on-track" => "🟡",
                            "behind" => "🔴",
                            _ => "⚪",
                        };
                        println!("  {} {}", indicator, pace_text);
                    }

                    if let Some(ref resets_at) = window.resets_at {
                        let now = chrono::Utc::now();
                        let duration = resets_at.signed_duration_since(now);
                        if duration.num_seconds() > 0 {
                            println!("  Resets in {}", format_duration(duration.num_milliseconds()));
                        }
                    }
                }
                if let Some(ref credits) = snapshot.credits {
                    println!("\n  Credits: ${:.2}", credits.used);
                    if let Some(limit) = credits.limit {
                        println!("  Limit: ${:.2}", limit);
                    }
                }
            }
            Err(e) => {
                eprintln!("\nError: {}", e);
            }
        }
    }

    Ok(())
}

fn extract_provider_name(e: &anyhow::Error) -> Option<String> {
    let msg = e.to_string().to_lowercase();
    if msg.contains("claude") {
        Some("Claude Code".to_string())
    } else if msg.contains("codex") {
        Some("Codex".to_string())
    } else if msg.contains("gemini") {
        Some("Gemini".to_string())
    } else if msg.contains("antigravity") {
        Some("Antigravity".to_string())
    } else {
        None
    }
}
