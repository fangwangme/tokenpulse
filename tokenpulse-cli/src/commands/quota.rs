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

    if json {
        let snapshots: Vec<&QuotaSnapshot> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
        println!("{}", serde_json::to_string_pretty(&snapshots)?);
        return Ok(());
    }

    tui::quota::run(results)?;
    Ok(())
}
