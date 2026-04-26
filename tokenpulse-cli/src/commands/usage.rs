use crate::tui;
use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use tokenpulse_core::{
    usage::{
        build_usage_summary_from_daily, ClaudeSessionParser, CodexSessionParser,
        CopilotSessionParser, DateRange, GeminiSessionParser, OpenCodeSessionParser,
        PiSessionParser, UsageStore,
    },
    SessionParser, UnifiedMessage,
};

const SUPPORTED_USAGE_PROVIDERS: &[&str] =
    &["claude", "codex", "copilot", "opencode", "gemini", "pi"];

pub async fn run(
    since: Option<String>,
    provider: Option<String>,
    refresh_days: Option<String>,
    refresh_pricing: bool,
    rebuild_all: bool,
    use_tui: bool,
    json: bool,
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

    store.repair_zero_costs(
        output_since_hint(requested_since, refresh_range),
        &provider_names,
    )?;

    let output_since = output_since_hint(requested_since, refresh_range);
    let (message_count, session_count) =
        store.load_summary_counts(output_since, &provider_names)?;

    if message_count == 0 {
        if json {
            print_json_summary(&build_usage_summary_from_daily(
                Vec::new(),
                Vec::new(),
                Vec::new(),
                0,
                0,
            ))?;
            return Ok(());
        }

        eprintln!("\nNo usage data found in the local ledger.\n");
        if !found_any_source {
            eprintln!("Checked providers:");
            eprintln!(" - Claude Code: ~/.claude/projects/ or ~/.claude/transcripts/");
            eprintln!(" - Codex: ~/.codex/sessions/");
            eprintln!(" - Copilot: ~/.local/share/github-copilot/events.jsonl");
            eprintln!(" - OpenCode: ~/.local/share/opencode/");
            eprintln!(" - Gemini CLI: ~/.gemini/tmp/");
            eprintln!(" - PI: ~/.pi/agent/sessions/");
        }
        return Ok(());
    }

    let summary = build_usage_summary_from_daily(
        store.load_dashboard_days(output_since, &provider_names)?,
        store.load_provider_summaries(output_since, &provider_names)?,
        store.load_model_summaries(output_since, &provider_names)?,
        message_count,
        session_count,
    );

    if json {
        print_json_summary(&summary)?;
    } else if use_tui {
        let daily_breakdown = store.load_daily_rows(output_since, &provider_names)?;
        let reload_fn = build_reload_fn(provider_names, output_since);
        return tui::usage::run(summary, daily_breakdown, reload_fn);
    } else {
        print_summary(&summary);
    }

    Ok(())
}

fn output_since_hint(
    requested_since: Option<NaiveDate>,
    refresh_range: Option<DateRange>,
) -> Option<NaiveDate> {
    requested_since.or(refresh_range.map(|range| range.start))
}

fn build_reload_fn(
    provider_names: Vec<String>,
    output_since: Option<NaiveDate>,
) -> impl FnMut() -> Result<(
    tokenpulse_core::usage::UsageSummary,
    Vec<tokenpulse_core::usage::DailyUsageRow>,
)> {
    move || {
        let store = UsageStore::new();
        let parsers = build_parsers(&provider_names);

        for parser in &parsers {
            let since = store.default_since(parser.provider_name(), output_since)?;
            match parser.parse_sessions(since) {
                Ok(messages) => {
                    if !messages.is_empty() {
                        store.ingest_messages(&messages, false)?;
                    }
                }
                Err(_) => {} // tolerate per-provider errors during reload
            }
        }

        store.repair_zero_costs(output_since, &provider_names)?;

        let (message_count, session_count) =
            store.load_summary_counts(output_since, &provider_names)?;

        let summary = build_usage_summary_from_daily(
            store.load_dashboard_days(output_since, &provider_names)?,
            store.load_provider_summaries(output_since, &provider_names)?,
            store.load_model_summaries(output_since, &provider_names)?,
            message_count,
            session_count,
        );

        let daily_rows = store.load_daily_rows(output_since, &provider_names)?;
        Ok((summary, daily_rows))
    }
}

fn parse_provider_names(provider: Option<&str>) -> Vec<String> {
    match provider {
        Some(value) => value
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
        None => SUPPORTED_USAGE_PROVIDERS
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
    }
}

fn build_parsers(provider_names: &[String]) -> Vec<Box<dyn SessionParser>> {
    provider_names
        .iter()
        .filter_map(|provider| match provider.as_str() {
            "claude" => Some(Box::new(ClaudeSessionParser::new()) as Box<dyn SessionParser>),
            "codex" => Some(Box::new(CodexSessionParser::new()) as Box<dyn SessionParser>),
            "copilot" => Some(Box::new(CopilotSessionParser::new()) as Box<dyn SessionParser>),
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
    println!("Total tokens: {}", format_int(summary.total_tokens));
    println!("Messages: {}", format_int(summary.message_count));
    println!("Sessions: {}", format_int(summary.session_count));
    println!("Active days: {}", format_int(summary.active_days));
    println!("Avg daily cost: ${:.2}", summary.avg_daily_cost);
    println!(
        "Avg daily tokens: {}",
        format_int(summary.avg_daily_tokens.round() as i64)
    );

    println!("\n=== By Provider ===");
    for provider in &summary.by_provider {
        println!(
            "{}: {} tokens | ${:.2} | {} messages | {} sessions",
            provider.provider.to_uppercase(),
            format_int(provider.tokens),
            provider.cost,
            format_int(provider.message_count),
            format_int(provider.session_count)
        );
    }

    println!("\n=== By Model ===");
    for model in summary.by_model.iter().take(10) {
        println!(
            "{} [{}]: {} tokens | ${:.2} | {} messages",
            model.model,
            model.source,
            format_int(model.tokens),
            model.cost,
            format_int(model.message_count)
        );
    }

    println!("\n=== Recent Daily Totals ===");
    for day in summary.daily.iter().rev().take(14).rev() {
        println!(
            "{}: {} tokens | ${:.2} | {} messages | {} sessions",
            day.date,
            format_int(day.total_tokens),
            day.total_cost_usd,
            format_int(day.message_count),
            format_int(day.session_count)
        );
    }

    println!("\n=== Weekly Totals ===");
    for week in summary.weekly.iter().rev().take(8).rev() {
        println!(
            "{}: {} tokens | ${:.2} | {} messages | {} active days",
            week.label,
            format_int(week.total_tokens),
            week.total_cost_usd,
            format_int(week.message_count),
            format_int(week.active_days)
        );
    }

    println!("\n=== Monthly Totals ===");
    for month in summary.monthly.iter().rev().take(6).rev() {
        println!(
            "{}: {} tokens | ${:.2} | {} messages | {} active days",
            month.label,
            format_int(month.total_tokens),
            month.total_cost_usd,
            format_int(month.message_count),
            format_int(month.active_days)
        );
    }

    println!(
        "\nLoaded {} ledger messages from {} provider(s).",
        format_int(summary.message_count),
        format_int(summary.by_provider.len())
    );
}

fn print_json_summary(summary: &tokenpulse_core::usage::UsageSummary) -> Result<()> {
    serde_json::to_writer_pretty(std::io::stdout(), summary)?;
    println!();
    Ok(())
}

fn format_int<T: ToString>(value: T) -> String {
    let raw = value.to_string();
    let digits = raw.strip_prefix('-').unwrap_or(&raw);
    let mut formatted_rev = String::with_capacity(raw.len() + raw.len() / 3);

    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            formatted_rev.push(',');
        }
        formatted_rev.push(ch);
    }

    let formatted: String = formatted_rev.chars().rev().collect();

    if raw.starts_with('-') {
        format!("-{}", formatted)
    } else {
        formatted
    }
}

#[cfg(test)]
mod tests {
    use super::parse_provider_names;

    #[test]
    fn parse_provider_names_defaults_to_all_supported_usage_sources() {
        assert_eq!(
            parse_provider_names(None),
            vec![
                "claude".to_string(),
                "codex".to_string(),
                "copilot".to_string(),
                "opencode".to_string(),
                "gemini".to_string(),
                "pi".to_string(),
            ]
        );
    }
}
