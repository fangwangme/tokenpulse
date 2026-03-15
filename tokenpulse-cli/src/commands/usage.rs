use anyhow::Result;
use chrono::NaiveDate;
use tokenpulse_core::{SessionParser, UnifiedMessage, usage::{ClaudeSessionParser, CodexSessionParser, OpenCodeSessionParser, PiSessionParser, compute_usage_summary}};

pub async fn run(since: Option<String>, provider: Option<String>, json: bool) -> Result<()> {
    let since_date = since
        .map(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d"))
        .transpose()?;

    let parsers: Vec<Box<dyn SessionParser>> = match provider {
        Some(p) => {
            let providers: Vec<&str> = p.split(',').map(|s| s.trim()).collect();
            providers
                .iter()
                .filter_map(|&p| match p {
                    "claude" => Some(Box::new(ClaudeSessionParser::new()) as Box<dyn SessionParser>),
                    "codex" => Some(Box::new(CodexSessionParser::new()) as Box<dyn SessionParser>),
                    "opencode" => Some(Box::new(OpenCodeSessionParser::new()) as Box<dyn SessionParser>),
                    "pi" => Some(Box::new(PiSessionParser::new()) as Box<dyn SessionParser>),
                    _ => None,
                })
                .collect()
        }
        None => vec![
            Box::new(ClaudeSessionParser::new()),
            Box::new(CodexSessionParser::new()),
            Box::new(OpenCodeSessionParser::new()),
            Box::new(PiSessionParser::new()),
        ],
    };

    let mut all_messages: Vec<UnifiedMessage> = Vec::new();
    let mut found_any = false;
    
    for parser in &parsers {
        match parser.parse_sessions(since_date) {
            Ok(msgs) => {
                if !msgs.is_empty() {
                    found_any = true;
                    all_messages.extend(msgs);
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to parse {}: {}", parser.provider_name(), e);
            }
        }
    }

    all_messages.sort_by_key(|m| m.timestamp);

    if !found_any {
        eprintln!("\nNo usage data found.\n");
        eprintln!("To see usage statistics, you need to have used one of these tools:");
        eprintln!("  - Claude Code: ~/.claude/projects/");
        eprintln!("  - Codex: ~/.codex/sessions/");
        eprintln!("  - OpenCode: ~/.local/share/opencode/");
        eprintln!("  - PI: ~/.pi/agent/sessions/\n");
        eprintln!("Run any of these tools first, then try again.");
        return Ok(());
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&all_messages)?);
        return Ok(());
    }

    // Print simple summary
    let summary = compute_usage_summary(&all_messages);
    
    println!("\n=== Usage Summary ===");
    println!("Total cost: ${:.2}", summary.total_cost);
    println!("Total tokens: {}", summary.total_tokens);
    println!("Active days: {}", summary.active_days);
    println!("Avg daily cost: ${:.2}", summary.avg_daily_cost);
    
    println!("\n=== By Provider ===");
    for prov in &summary.by_provider {
        println!("{}: ${:.2} ({:.1}%)", prov.provider.to_uppercase(), prov.cost, prov.percent);
    }
    
    println!("\n=== By Model ===");
    for model in &summary.by_model.iter().take(10).collect::<Vec<_>>() {
        println!("{}: ${:.2} ({:.1}%)", model.model, model.cost, model.percent);
    }

    Ok(())
}
