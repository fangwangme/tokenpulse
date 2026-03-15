use anyhow::Result;
use tokenpulse_core::{QuotaFetcher, QuotaSnapshot, quota::{ClaudeQuotaFetcher, CodexQuotaFetcher, fetch_all}};
use crate::tui;

pub async fn run(provider: Option<String>, json: bool) -> Result<()> {
    let providers: Vec<Box<dyn QuotaFetcher>> = match provider {
        Some(p) => {
            match p.as_str() {
                "claude" => vec![Box::new(ClaudeQuotaFetcher::new())],
                "codex" => vec![Box::new(CodexQuotaFetcher::new())],
                _ => {
                    eprintln!("Unknown provider: {}", p);
                    eprintln!("Supported providers: claude, codex");
                    return Ok(());
                }
            }
        }
        None => vec![
            Box::new(ClaudeQuotaFetcher::new()),
            Box::new(CodexQuotaFetcher::new()),
        ],
    };

    let results = fetch_all(providers).await;

    // Check if all failed (likely no config)
    let success_count = results.iter().filter(|r| r.is_ok()).count();
    if success_count == 0 {
        eprintln!("\nNo providers configured.\n");
        eprintln!("To use tokenpulse, you need to have at least one of these tools installed:");
        eprintln!("  - Claude Code: https://docs.anthropic.com/en/docs/claude-code");
        eprintln!("  - Codex: https://github.com/openai/codex\n");
        eprintln!("After installing, run 'claude' or 'codex' to login, then try again.");
        return Ok(());
    }

    if json {
        let snapshots: Vec<&QuotaSnapshot> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
        println!("{}", serde_json::to_string_pretty(&snapshots)?);
        return Ok(());
    }

    // For non-TUI mode, print simple output
    for result in &results {
        match result {
            Ok(snapshot) => {
                println!("\n=== {} ===", snapshot.provider.to_uppercase());
                if let Some(ref plan) = snapshot.plan {
                    println!("Plan: {}", plan);
                }
                for window in &snapshot.windows {
                    println!("{}: {:.1}%", window.label, window.used_percent);
                }
                if let Some(ref credits) = snapshot.credits {
                    if let Some(limit) = credits.limit {
                        println!("Credits: ${:.2} / ${:.2}", credits.used, limit);
                    } else {
                        println!("Credits: ${:.2} (unlimited)", credits.used);
                    }
                }
            }
            Err(e) => {
                if let Some(provider_name) = extract_provider_name(&e) {
                    eprintln!("\n{}: {}", provider_name, e);
                }
            }
        }
    }

    Ok(())
}

fn extract_provider_name(e: &anyhow::Error) -> Option<String> {
    let msg = e.to_string();
    if msg.contains("claude") {
        Some("Claude Code".to_string())
    } else if msg.contains("codex") {
        Some("Codex".to_string())
    } else {
        None
    }
}
