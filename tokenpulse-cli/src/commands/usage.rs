use anyhow::Result;
use chrono::NaiveDate;
use tokenpulse_core::{SessionParser, UnifiedMessage, usage::{ClaudeSessionParser, CodexSessionParser, OpenCodeSessionParser, PiSessionParser, compute_usage_summary}};
use crate::tui;

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
    for parser in parsers {
        match parser.parse_sessions(since_date) {
            Ok(msgs) => all_messages.extend(msgs),
            Err(e) => eprintln!("Warning: Failed to parse {}: {}", parser.provider_name(), e),
        }
    }

    all_messages.sort_by_key(|m| m.timestamp);

    if json {
        println!("{}", serde_json::to_string_pretty(&all_messages)?);
        return Ok(());
    }

    let summary = compute_usage_summary(&all_messages);
    tui::usage::run(all_messages, summary)?;
    Ok(())
}
