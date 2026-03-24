use crate::tui;
use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use tokenpulse_core::{
    usage::{
        build_usage_summary_from_daily, ClaudeSessionParser, CodexSessionParser, DateRange,
        GeminiSessionParser, OpenCodeSessionParser, PiSessionParser, UsageStore,
    },
    SessionParser, UnifiedMessage,
};

pub async fn run(
    since: Option<String>,
    provider: Option<String>,
    refresh_days: Option<String>,
    refresh_pricing: bool,
    rebuild_all: bool,
    tui_enabled: bool,
) -> Result<()> {
    let requested_since = since
        .map(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d"))
        .transpose()?;
    let refresh_range = refresh_days.as_deref().map(parse_date_range).transpose()?;

    let provider_names = parse_provider_names(provider.as_deref());
    let parsers = build_parsers(&provider_names);
    let store = UsageStore::new();

    if rebuild_all {
        store.clear_sources(&provider_names, refresh_pricing)?;
    } else if let Some(range) = refresh_range {
        store.delete_sources_in_date_range(range, &provider_names, refresh_pricing)?;
    }

    let mut found_any_source = false;

    for parser in &parsers {
        let effective_since = if rebuild_all || refresh_range.is_some() {
            None
        } else {
            store.default_since(parser.provider_name(), requested_since)?
        };

        match parser.parse_sessions(effective_since) {
            Ok(messages) => {
                let scoped = match refresh_range {
                    Some(range) => filter_messages_to_range(messages, range),
                    None => messages,
                };

                if !scoped.is_empty() {
                    found_any_source = true;
                    store.ingest_messages(&scoped, refresh_pricing)?;
                }
            }
            Err(error) => {
                eprintln!(
                    "Warning: Failed to parse {} usage: {}",
                    parser.provider_name(),
                    error
                );
            }
        }
    }

    let output_since = requested_since.or(refresh_range.map(|range| range.start));
    let daily = store.load_dashboard_days(output_since, &provider_names)?;

    if daily.is_empty() {
        eprintln!("\nNo usage data found in the local ledger.\n");
        if !found_any_source {
            eprintln!("Checked providers:");
            eprintln!(" - Claude Code: ~/.claude/projects/ or ~/.claude/transcripts/");
            eprintln!(" - Codex: ~/.codex/sessions/");
            eprintln!(" - OpenCode: ~/.local/share/opencode/");
            eprintln!(" - Gemini CLI: ~/.gemini/tmp/");
            eprintln!(" - PI: ~/.pi/agent/sessions/");
        }
        return Ok(());
    }

    let provider_summary = store.load_provider_summaries(output_since, &provider_names)?;
    let model_summary = store.load_model_summaries(output_since, &provider_names)?;
    let daily_breakdown = store.load_daily_rows(output_since, &provider_names)?;
    let message_count: usize = provider_summary.iter().map(|row| row.message_count).sum();
    let session_count: usize = provider_summary.iter().map(|row| row.session_count).sum();
    let summary = build_usage_summary_from_daily(
        daily,
        provider_summary,
        model_summary,
        message_count,
        session_count,
    );

    if tui_enabled {
        return tui::usage::run(summary, daily_breakdown);
    }

    print_summary(&summary);
    Ok(())
}

fn parse_provider_names(provider: Option<&str>) -> Vec<String> {
    match provider {
        Some(value) => value
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        None => vec![
            "claude".to_string(),
            "codex".to_string(),
            "opencode".to_string(),
            "gemini".to_string(),
        ],
    }
}

fn build_parsers(provider_names: &[String]) -> Vec<Box<dyn SessionParser>> {
    provider_names
        .iter()
        .filter_map(|provider| match provider.as_str() {
            "claude" => Some(Box::new(ClaudeSessionParser::new()) as Box<dyn SessionParser>),
            "codex" => Some(Box::new(CodexSessionParser::new()) as Box<dyn SessionParser>),
            "opencode" => Some(Box::new(OpenCodeSessionParser::new()) as Box<dyn SessionParser>),
            "gemini" => Some(Box::new(GeminiSessionParser::new()) as Box<dyn SessionParser>),
            "pi" => Some(Box::new(PiSessionParser::new()) as Box<dyn SessionParser>),
            _ => None,
        })
        .collect()
}

fn parse_date_range(value: &str) -> Result<DateRange> {
    let (start, end) = value
        .split_once(':')
        .ok_or_else(|| anyhow!("Expected --refresh-days in YYYY-MM-DD:YYYY-MM-DD format"))?;
    let start = NaiveDate::parse_from_str(start, "%Y-%m-%d")?;
    let end = NaiveDate::parse_from_str(end, "%Y-%m-%d")?;
    if end < start {
        anyhow::bail!("refresh-days end must not be earlier than start");
    }
    Ok(DateRange { start, end })
}

fn filter_messages_to_range(
    messages: Vec<UnifiedMessage>,
    range: DateRange,
) -> Vec<UnifiedMessage> {
    messages
        .into_iter()
        .filter(|message| {
            NaiveDate::parse_from_str(&message.date, "%Y-%m-%d")
                .map(|date| range.contains(date))
                .unwrap_or(false)
        })
        .collect()
}

fn print_summary(summary: &tokenpulse_core::usage::UsageSummary) {
    println!("\n=== Usage Summary ===");
    println!("Total cost: ${:.2}", summary.total_cost);
    println!("Total tokens: {}", summary.total_tokens);
    println!("Messages: {}", summary.message_count);
    println!("Sessions: {}", summary.session_count);
    println!("Active days: {}", summary.active_days);
    println!("Avg daily cost: ${:.2}", summary.avg_daily_cost);
    println!("Avg daily tokens: {:.0}", summary.avg_daily_tokens);

    println!("\n=== By Provider ===");
    for provider in &summary.by_provider {
        println!(
            "{}: {} tokens | ${:.2} | {} messages | {} sessions",
            provider.provider.to_uppercase(),
            provider.tokens,
            provider.cost,
            provider.message_count,
            provider.session_count
        );
    }

    println!("\n=== By Model ===");
    for model in summary.by_model.iter().take(10) {
        println!(
            "{} [{}]: {} tokens | ${:.2} | {} messages",
            model.model, model.source, model.tokens, model.cost, model.message_count
        );
    }

    println!("\n=== Recent Daily Totals ===");
    for day in summary.daily.iter().rev().take(14).rev() {
        println!(
            "{}: {} tokens | ${:.2} | {} messages | {} sessions",
            day.date, day.total_tokens, day.total_cost_usd, day.message_count, day.session_count
        );
    }

    println!("\n=== Weekly Totals ===");
    for week in summary.weekly.iter().rev().take(8).rev() {
        println!(
            "{}: {} tokens | ${:.2} | {} messages | {} active days",
            week.label,
            week.total_tokens,
            week.total_cost_usd,
            week.message_count,
            week.active_days
        );
    }

    println!("\n=== Monthly Totals ===");
    for month in summary.monthly.iter().rev().take(6).rev() {
        println!(
            "{}: {} tokens | ${:.2} | {} messages | {} active days",
            month.label,
            month.total_tokens,
            month.total_cost_usd,
            month.message_count,
            month.active_days
        );
    }

    println!(
        "\nLoaded {} ledger messages from {} provider(s).",
        summary.message_count,
        summary.by_provider.len()
    );
}
