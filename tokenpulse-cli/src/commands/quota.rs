use crate::tui;
use anyhow::Result;
use crossterm::terminal;
use tokenpulse_core::config::{ConfigManager, QuotaDisplayMode};
use tokenpulse_core::{
    quota::{
        fetch_all, AntigravityQuotaFetcher, ClaudeQuotaFetcher, CodexQuotaFetcher,
        CopilotQuotaFetcher, GeminiQuotaFetcher, QuotaCacheStore,
    },
    QuotaFetcher,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const BAR_WIDTH: usize = 20;
const GRID_GAP: usize = 4;
const MIN_CARD_WIDTH: usize = 42;
const MAX_CARD_WIDTH: usize = 56;
const TWO_COLUMN_MIN_WIDTH: usize = MIN_CARD_WIDTH * 2 + GRID_GAP;

fn display_provider_name(provider: &str) -> &'static str {
    match provider {
        "claude" => "CLAUDE CODE",
        "gemini" => "GEMINI CLI",
        "codex" => "CODEX",
        "copilot" => "GITHUB COPILOT",
        "antigravity" => "ANTIGRAVITY",
        _ => "UNKNOWN",
    }
}

fn draw_progress_bar(
    used_percent: f64,
    period_duration_ms: Option<i64>,
    resets_at: Option<chrono::DateTime<chrono::Utc>>,
    display_mode: &QuotaDisplayMode,
) -> String {
    let shown_percent = match display_mode {
        QuotaDisplayMode::Used => used_percent,
        QuotaDisplayMode::Remaining => (100.0 - used_percent).max(0.0),
    };
    let shown_blocks = ((shown_percent / 100.0) * BAR_WIDTH as f64).round() as usize;
    let shown_blocks = shown_blocks.min(BAR_WIDTH);

    // Calculate expected position (vertical line marker)
    let marker_pos = if let (Some(period_ms), Some(reset_time)) = (period_duration_ms, resets_at) {
        let now = chrono::Utc::now();
        let period_start = reset_time - chrono::Duration::milliseconds(period_ms);
        let elapsed_ms = (now - period_start).num_milliseconds();
        if elapsed_ms > 0 && period_ms > 0 {
            let elapsed_fraction = elapsed_ms as f64 / period_ms as f64;
            let expected_percent = match display_mode {
                QuotaDisplayMode::Used => elapsed_fraction * 100.0,
                QuotaDisplayMode::Remaining => (1.0 - elapsed_fraction).max(0.0) * 100.0,
            };
            let expected_blocks = ((expected_percent / 100.0) * BAR_WIDTH as f64).round() as usize;
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
        let filled = i < shown_blocks;
        if marker_pos == Some(i) {
            bar.push('│');
        } else if filled {
            bar.push('█');
        } else {
            bar.push('░');
        }
    }
    bar.push(']');

    match display_mode {
        QuotaDisplayMode::Used => format!("{} {:.0}% used", bar, shown_percent),
        QuotaDisplayMode::Remaining => format!("{} {:.0}% left", bar, shown_percent),
    }
}

fn calculate_pace(
    used_percent: f64,
    period_duration_ms: Option<i64>,
    resets_at: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<(String, String)> {
    let period_ms = period_duration_ms?;
    let reset_time = resets_at?;

    if used_percent >= 100.0 {
        return None;
    }

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
        return Some((
            "behind".to_string(),
            format!("+{:.0}% deficit, runs out in {}", deficit, eta_text),
        ));
    } else {
        return Some((
            "ahead".to_string(),
            format!("{:.0}% under budget", deficit.abs()),
        ));
    }
}

fn format_duration(ms: i64) -> String {
    let seconds = ms / 1000;
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;

    if minutes > 24 * 60 {
        format!("{}d {}h", days, hours % 24)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes % 60)
    } else {
        format!("{}m", minutes)
    }
}

fn pad_line(line: &str, width: usize) -> String {
    let len = UnicodeWidthStr::width(line);
    if len >= width {
        truncate_display_width(line, width)
    } else {
        format!("{}{}", line, " ".repeat(width - len))
    }
}

fn truncate_display_width(line: &str, width: usize) -> String {
    let mut result = String::new();
    let mut used = 0;

    for ch in line.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        result.push(ch);
        used += ch_width;
    }

    result
}

fn terminal_width() -> usize {
    terminal::size()
        .ok()
        .map(|(width, _)| width as usize)
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
        })
        .filter(|v| *v >= MIN_CARD_WIDTH)
        .unwrap_or(100)
}

fn format_window_block(
    window: &tokenpulse_core::RateWindow,
    display_mode: &QuotaDisplayMode,
) -> Vec<String> {
    let mut lines = vec![
        window.label.clone(),
        draw_progress_bar(
            window.used_percent,
            window.period_duration_ms,
            window.resets_at,
            display_mode,
        ),
    ];

    if let Some((status, pace_text)) = calculate_pace(
        window.used_percent,
        window.period_duration_ms,
        window.resets_at,
    ) {
        let indicator = match status.as_str() {
            "ahead" => "🟢",
            "on-track" => "🟡",
            "behind" => "🔴",
            _ => "⚪",
        };
        lines.push(format!("{} {}", indicator, pace_text));
    }

    if let Some(ref resets_at) = window.resets_at {
        let now = chrono::Utc::now();
        let duration = resets_at.signed_duration_since(now);
        if duration.num_seconds() > 0 {
            lines.push(format!(
                "Resets in {}",
                format_duration(duration.num_milliseconds())
            ));
        }
    }

    lines
}

fn format_provider_content(
    snapshot: &tokenpulse_core::QuotaSnapshot,
    display_mode: &QuotaDisplayMode,
) -> Vec<String> {
    let mut lines = vec![display_provider_name(&snapshot.provider).to_string()];

    for window in &snapshot.windows {
        lines.push(String::new());
        for line in format_window_block(window, display_mode) {
            lines.push(line);
        }
    }

    if let Some(ref credits) = snapshot.credits {
        lines.push(String::new());
        lines.push(format!("Credits: ${:.2}", credits.used));
        if let Some(limit) = credits.limit {
            lines.push(format!("Limit: ${:.2}", limit));
        }
    }

    lines
}

fn format_provider_card(
    snapshot: &tokenpulse_core::QuotaSnapshot,
    display_mode: &QuotaDisplayMode,
    width: usize,
) -> Vec<String> {
    let inner_width = width.saturating_sub(2).max(1);
    let content = format_provider_content(snapshot, display_mode);
    let title = pad_line(
        &format!(" {} ", display_provider_name(&snapshot.provider)),
        inner_width,
    );
    let mut lines = vec![format!("┌{}┐", title)];

    for line in content.into_iter().skip(1) {
        lines.push(format!("│{}│", pad_line(&line, inner_width)));
    }

    lines.push(format!("└{}┘", "─".repeat(inner_width)));
    lines
}

fn print_provider_block(
    snapshot: &tokenpulse_core::QuotaSnapshot,
    display_mode: &QuotaDisplayMode,
) {
    let width = terminal_width().clamp(MIN_CARD_WIDTH, MAX_CARD_WIDTH);
    println!();
    for line in format_provider_card(snapshot, display_mode, width) {
        println!("{}", line);
    }
}

fn provider_grid_columns(total_width: usize, count: usize) -> usize {
    if total_width < TWO_COLUMN_MIN_WIDTH {
        return 1;
    }

    for cols in (1..=count.min(2)).rev() {
        let width = (total_width.saturating_sub(GRID_GAP * cols.saturating_sub(1))) / cols;
        if width >= MIN_CARD_WIDTH {
            return cols;
        }
    }
    1
}

fn print_provider_grid(
    snapshots: &[tokenpulse_core::QuotaSnapshot],
    display_mode: &QuotaDisplayMode,
) {
    let total_width = terminal_width();
    let cols = provider_grid_columns(total_width, snapshots.len());
    let col_width = (total_width.saturating_sub(GRID_GAP * cols.saturating_sub(1)) / cols)
        .clamp(MIN_CARD_WIDTH, MAX_CARD_WIDTH);
    let blocks: Vec<Vec<String>> = snapshots
        .iter()
        .map(|snapshot| format_provider_card(snapshot, display_mode, col_width))
        .collect();

    println!();
    for row in blocks.chunks(cols) {
        let row_height = row.iter().map(|block| block.len()).max().unwrap_or(0);

        for idx in 0..row_height {
            let mut rendered = Vec::with_capacity(row.len());
            for block in row {
                rendered.push(
                    block
                        .get(idx)
                        .map(|line| pad_line(line, col_width))
                        .unwrap_or_else(|| " ".repeat(col_width)),
                );
            }
            println!("{}", rendered.join(&" ".repeat(GRID_GAP)));
        }
        println!();
    }
}

pub async fn run(provider: Option<String>, refresh: bool, use_tui: bool) -> Result<()> {
    let config_manager = ConfigManager::new();
    let config = config_manager.load().unwrap_or_default();
    let display_mode = &config.display.quota_display_mode;
    let observed_at = chrono::Utc::now();
    let cache_store = QuotaCacheStore::new();
    let enabled_providers = config
        .providers
        .iter()
        .filter(|(_, p)| p.enabled)
        .map(|(k, _)| k.clone())
        .collect::<Vec<_>>();

    let providers: Vec<Box<dyn QuotaFetcher>> = match provider {
        Some(ref p) => match p.as_str() {
            "claude" => vec![Box::new(ClaudeQuotaFetcher::new())],
            "codex" => vec![Box::new(CodexQuotaFetcher::new())],
            "copilot" => vec![Box::new(CopilotQuotaFetcher::new())],
            "gemini" => vec![Box::new(GeminiQuotaFetcher::new())],
            "antigravity" => vec![Box::new(AntigravityQuotaFetcher::new())],
            _ => {
                eprintln!("Unknown provider: {}", p);
                eprintln!("Supported providers: claude, codex, copilot, gemini, antigravity");
                return Ok(());
            }
        },
        None => {
            let mut list: Vec<Box<dyn QuotaFetcher>> = Vec::new();
            if enabled_providers.contains(&"claude".to_string()) {
                list.push(Box::new(ClaudeQuotaFetcher::new()));
            }
            if enabled_providers.contains(&"codex".to_string()) {
                list.push(Box::new(CodexQuotaFetcher::new()));
            }
            if enabled_providers.contains(&"copilot".to_string()) {
                list.push(Box::new(CopilotQuotaFetcher::new()));
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

    let mut results: Vec<Option<Result<tokenpulse_core::QuotaSnapshot>>> =
        (0..providers.len()).map(|_| None).collect();
    let mut fetch_indices = Vec::new();
    let mut to_fetch = Vec::new();

    for (idx, quota_fetcher) in providers.into_iter().enumerate() {
        let provider_name = quota_fetcher.provider_name().to_string();

        if !refresh {
            if let Some(cached) = cache_store.load_valid(&provider_name, observed_at)? {
                results[idx] = Some(Ok(cached.snapshot));
                continue;
            }
        }

        fetch_indices.push((idx, provider_name));
        to_fetch.push(quota_fetcher);
    }

    let fetched_results = fetch_all(to_fetch).await;
    for ((idx, provider_name), result) in fetch_indices.into_iter().zip(fetched_results) {
        if let Ok(snapshot) = &result {
            cache_store.save(&provider_name, observed_at, snapshot)?;
        }
        results[idx] = Some(result);
    }

    let results: Vec<Result<tokenpulse_core::QuotaSnapshot>> =
        results.into_iter().flatten().collect();

    let success_count = results.iter().filter(|r| r.is_ok()).count();
    if success_count == 0 {
        if provider.is_some() {
            for result in &results {
                if let Err(e) = result {
                    eprintln!("\nError: {}", e);
                }
            }
            return Ok(());
        }

        eprintln!("\nNo providers configured.\n");
        eprintln!("To use tokenpulse, you need to have at least one of these tools installed:");
        eprintln!(" - Claude Code: https://docs.anthropic.com/en/docs/claude-code");
        eprintln!(" - Codex: https://github.com/openai/codex");
        eprintln!(" - GitHub Copilot: https://github.com/features/copilot");
        eprintln!(" - Gemini CLI: https://github.com/google-gemini/gemini-cli");
        eprintln!(" - Antigravity: https://antigravity.com\n");
        eprintln!("After installing, run the tool to login, then try again.");
        return Ok(());
    }

    if use_tui {
        return tui::quota::run(results, display_mode.clone());
    }

    let snapshots: Vec<_> = results
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .cloned()
        .collect();

    if snapshots.len() > 1 && provider.is_none() {
        print_provider_grid(&snapshots, display_mode);
    } else {
        for snapshot in &snapshots {
            print_provider_block(snapshot, display_mode);
        }
    }

    for result in &results {
        if let Err(e) = result {
            eprintln!("\nError: {}", e);
        }
    }

    Ok(())
}
