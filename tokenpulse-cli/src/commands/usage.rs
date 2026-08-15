use crate::tui;
use anyhow::{anyhow, Result};
use chrono::{DateTime, Local, NaiveDate, SecondsFormat, Utc};
use rayon::prelude::*;
use std::{
    collections::{BTreeSet, HashSet},
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};
use tokenpulse_core::{
    config::{Config, ConfigManager, QuotaDisplayMode},
    usage::{
        build_usage_summary_from_daily, AntigravitySessionParser, ClaudeSessionParser,
        CodexSessionParser, CopilotSessionParser, DateRange, GeminiSessionParser,
        OpenCodeSessionParser, PiSessionParser, UsageStore,
    },
    IncrementalIngestMode, QuotaSnapshot, SessionParser, UnifiedMessage,
};

const SUPPORTED_USAGE_PROVIDERS: &[&str] = &[
    "claude",
    "codex",
    "copilot",
    "opencode",
    "gemini",
    "pi",
    "antigravity",
];

pub async fn run(
    since: Option<String>,
    refresh_days: Option<String>,
    refresh_pricing: bool,
    rebuild_all: bool,
    use_tui: bool,
    json: bool,
    csv: Option<String>,
    log: bool,
) -> Result<()> {
    let mut perf = UsagePerfLog::new(log);
    let provider_names = parse_provider_names(None);
    let config_manager = ConfigManager::new();
    let config = config_manager.load().unwrap_or_default();

    let requested_since = since
        .map(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d"))
        .transpose()?;
    let refresh_range = refresh_days.as_deref().map(parse_date_range).transpose()?;

    if !use_tui
        && provider_names.contains(&"antigravity".to_string())
        && config.display.scan_antigravity
    {
        let conns_res = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(tokenpulse_core::usage::detect_antigravity_connections())
        });
        if let Ok(conns) = conns_res {
            if conns.is_empty() {
                eprintln!("Warning: No running Antigravity language servers detected. New sessions will not be synced.");
            }
        }
    }
    let parsers = build_parsers(&provider_names, rebuild_all);
    let store = UsageStore::new();
    let mut stale_sources = HashSet::new();
    perf.log(
        "start",
        format!(
            "providers={} since={:?} refresh_range={:?} refresh_pricing={} rebuild_all={} use_tui={} json={} csv={:?} db={}",
            provider_names.join(","),
            requested_since,
            refresh_range,
            refresh_pricing,
            rebuild_all,
            use_tui,
            json,
            csv,
            store.path().display()
        ),
    );

    if !rebuild_all && refresh_range.is_none() {
        perf.log(
            "stale_check_start",
            format!("providers={}", provider_names.join(",")),
        );
        let stale_check_started = Instant::now();
        let providers_and_versions: Vec<(&str, &str)> = parsers
            .iter()
            .map(|parser| (parser.provider_name(), parser.parser_version()))
            .collect();
        stale_sources = store.check_stale_parser_versions(&providers_and_versions)?;
        perf.log_duration(
            "stale_check_complete",
            stale_check_started.elapsed(),
            format!("stale_count={}", stale_sources.len()),
        );
    }

    if rebuild_all {
        let started = Instant::now();
        store.clear_sources(&provider_names, refresh_pricing)?;
        perf.log_duration(
            "clear_sources",
            started.elapsed(),
            format!("providers={}", provider_names.join(",")),
        );
    } else if let Some(range) = refresh_range {
        let started = Instant::now();
        store.delete_sources_in_date_range(range, &provider_names, refresh_pricing)?;
        perf.log_duration(
            "delete_sources_in_date_range",
            started.elapsed(),
            format!(
                "providers={} start={} end={}",
                provider_names.join(","),
                range.start,
                range.end
            ),
        );
    }

    let mut found_any_source = false;

    // Resolve each provider's incremental window first (sequential store reads).
    let mut effective_sinces: Vec<Option<NaiveDate>> = Vec::with_capacity(parsers.len());
    for parser in &parsers {
        let since_started = Instant::now();
        let effective_since = if rebuild_all
            || refresh_range.is_some()
            || stale_sources.contains(parser.provider_name())
        {
            None
        } else {
            store.default_since(parser.provider_name(), requested_since)?
        };
        perf.log_duration(
            "default_since",
            since_started.elapsed(),
            format!(
                "provider={} effective_since={:?}",
                parser.provider_name(),
                effective_since
            ),
        );
        effective_sinces.push(effective_since);
    }

    // Parse every provider concurrently — parsing is independent and the
    // heaviest step (Antigravity also syncs over the local network). Ingestion
    // below stays sequential so SQLite writes remain serialized and ordered.
    let parse_outcomes: Vec<(Duration, Result<Vec<UnifiedMessage>>)> = parsers
        .par_iter()
        .zip(effective_sinces.par_iter())
        .map(|(parser, since)| {
            let started = Instant::now();
            let result = parser.parse_sessions(*since);
            (started.elapsed(), result)
        })
        .collect();

    for ((parser, &effective_since), (parse_elapsed, parse_result)) in parsers
        .iter()
        .zip(effective_sinces.iter())
        .zip(parse_outcomes)
    {
        let provider_started = Instant::now();
        perf.log(
            "provider_start",
            format!("provider={}", parser.provider_name()),
        );

        match parse_result {
            Ok(messages) => {
                let parsed_count = messages.len();
                let parsed_sessions = message_session_count(&messages);
                let scoped = match refresh_range {
                    Some(range) => filter_messages_to_range(messages, range),
                    None => messages,
                };
                perf.log_duration(
                    "parse_sessions",
                    parse_elapsed,
                    format!(
                        "provider={} messages={} sessions={} scoped_messages={} effective_since={:?} mode={:?}",
                        parser.provider_name(),
                        parsed_count,
                        parsed_sessions,
                        scoped.len(),
                        effective_since,
                        parser.incremental_ingest_mode()
                    ),
                );

                if stale_sources.contains(parser.provider_name()) {
                    if !scoped.is_empty() {
                        found_any_source = true;
                        let started = Instant::now();
                        store.replace_source_messages(
                            parser.provider_name(),
                            &scoped,
                            refresh_pricing,
                        )?;
                        perf.log_duration(
                            "ingest_replace_source",
                            started.elapsed(),
                            format!(
                                "provider={} messages={} sessions={}",
                                parser.provider_name(),
                                scoped.len(),
                                message_session_count(&scoped)
                            ),
                        );
                    }
                } else if !scoped.is_empty() {
                    found_any_source = true;
                    if !rebuild_all
                        && refresh_range.is_none()
                        && parser.incremental_ingest_mode()
                            == IncrementalIngestMode::ReplaceChangedSessions
                    {
                        let started = Instant::now();
                        store.replace_sessions_messages(&scoped, refresh_pricing)?;
                        perf.log_duration(
                            "ingest_replace_sessions",
                            started.elapsed(),
                            format!(
                                "provider={} messages={} sessions={}",
                                parser.provider_name(),
                                scoped.len(),
                                message_session_count(&scoped)
                            ),
                        );
                    } else {
                        let started = Instant::now();
                        store.ingest_messages(&scoped, refresh_pricing)?;
                        perf.log_duration(
                            "ingest_upsert_messages",
                            started.elapsed(),
                            format!(
                                "provider={} messages={} sessions={}",
                                parser.provider_name(),
                                scoped.len(),
                                message_session_count(&scoped)
                            ),
                        );
                    }
                }
            }
            Err(error) => {
                perf.log_duration(
                    "parse_sessions_error",
                    parse_elapsed,
                    format!("provider={} error={}", parser.provider_name(), error),
                );
                eprintln!(
                    "Warning: Failed to parse {} usage: {}",
                    parser.provider_name(),
                    error
                );
            }
        }
        perf.log_duration(
            "provider_complete",
            provider_started.elapsed(),
            format!("provider={}", parser.provider_name()),
        );
    }

    let output_since = output_since_hint(requested_since, refresh_range);
    let started = Instant::now();
    let repaired = store.repair_zero_costs(output_since, &provider_names)?;
    perf.log_duration(
        "repair_zero_costs",
        started.elapsed(),
        format!("repaired={} output_since={:?}", repaired, output_since),
    );

    let started = Instant::now();
    let (message_count, session_count) =
        store.load_summary_counts(output_since, &provider_names)?;
    perf.log_duration(
        "load_summary_counts",
        started.elapsed(),
        format!(
            "messages={} sessions={} output_since={:?}",
            message_count, session_count, output_since
        ),
    );

    if message_count == 0 {
        perf.log("no_data", "message_count=0");
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

        if let Some(csv_type) = csv {
            match csv_type.as_str() {
                "models" => println!("model,provider,source,tokens,cost_usd,messages,sessions,percent"),
                _ => println!("date,source,total_tokens,cost_usd,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,messages,sessions"),
            }
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
            eprintln!(" - Antigravity: ~/.local/share/tokenpulse/antigravity-cache/sessions/");
            eprintln!("\nIf Gemini totals look stale after this fix, run: tokenpulse usage -p gemini --rebuild-all");
        }
        return Ok(());
    }

    let started = Instant::now();
    let dashboard_days = store.load_dashboard_days(output_since, &provider_names)?;
    perf.log_duration(
        "load_dashboard_days",
        started.elapsed(),
        format!("days={}", dashboard_days.len()),
    );
    let started = Instant::now();
    let provider_summaries = store.load_provider_summaries(output_since, &provider_names)?;
    perf.log_duration(
        "load_provider_summaries",
        started.elapsed(),
        format!("providers={}", provider_summaries.len()),
    );
    let started = Instant::now();
    let model_summaries = store.load_model_summaries(output_since, &provider_names)?;
    perf.log_duration(
        "load_model_summaries",
        started.elapsed(),
        format!("models={}", model_summaries.len()),
    );
    let started = Instant::now();
    let summary = build_usage_summary_from_daily(
        dashboard_days,
        provider_summaries,
        model_summaries,
        message_count,
        session_count,
    );
    perf.log_duration(
        "build_summary",
        started.elapsed(),
        format!(
            "messages={} sessions={} active_days={}",
            summary.message_count, summary.session_count, summary.active_days
        ),
    );

    if json {
        perf.log(
            "output_json",
            format!("total_elapsed_ms={}", perf.elapsed_ms()),
        );
        let config_manager = ConfigManager::new();
        let config = config_manager.load().unwrap_or_default();
        let quota_snapshots = collect_quota_snapshots(&config).await;

        print_json_unified(&summary, quota_snapshots)?;
    } else if let Some(csv_type) = csv {
        let started = Instant::now();
        let daily_breakdown = store.load_daily_rows(output_since, &provider_names)?;
        perf.log_duration(
            "load_daily_rows",
            started.elapsed(),
            format!("rows={} output=csv", daily_breakdown.len()),
        );
        perf.log(
            "output_csv",
            format!("kind={csv_type} total_elapsed_ms={}", perf.elapsed_ms()),
        );
        match csv_type.as_str() {
            "models" => print_models_csv(&summary),
            _ => print_daily_csv(&daily_breakdown),
        }
    } else if use_tui {
        let started = Instant::now();
        let daily_breakdown = store.load_daily_rows(output_since, &provider_names)?;
        perf.log_duration(
            "load_daily_rows",
            started.elapsed(),
            format!("rows={} output=tui", daily_breakdown.len()),
        );

        let cache_store = tokenpulse_core::quota::QuotaCacheStore::new();
        let mut quota_snapshots = Vec::new();
        let now = chrono::Utc::now();
        for info in crate::commands::quota::quota_provider_info_list() {
            if let Ok(Some(cached)) = cache_store.load_valid(info.id, now) {
                quota_snapshots.push(cached.snapshot);
            }
        }

        let reload_fn = build_reload_fn(output_since, perf.path().cloned());
        perf.log(
            "tui_start",
            format!("total_elapsed_ms={}", perf.elapsed_ms()),
        );
        let result = tui::usage::run(summary, daily_breakdown, quota_snapshots, reload_fn);
        perf.log(
            "tui_exit",
            format!("total_elapsed_ms={}", perf.elapsed_ms()),
        );
        return result;
    } else {
        perf.log(
            "output_text",
            format!("total_elapsed_ms={}", perf.elapsed_ms()),
        );
        print_summary(&summary);

        let config_manager = ConfigManager::new();
        let config = config_manager.load().unwrap_or_default();
        let quota_snapshots = collect_quota_snapshots(&config).await;

        print_quota_summary(&quota_snapshots, &config.display.quota_display_mode);
    }

    Ok(())
}

/// Collect quota snapshots for the non-TUI outputs.
///
/// `display.refresh_quota` gates the live fetch here the same way it gates the
/// TUI's startup, auto-refresh, and manual refresh. When it is off, only
/// unexpired cached snapshots are shown.
async fn collect_quota_snapshots(config: &Config) -> Vec<QuotaSnapshot> {
    let cache_store = tokenpulse_core::quota::QuotaCacheStore::new();
    let observed_at = Utc::now();
    let mut snapshots = Vec::new();

    let fetchers = crate::commands::quota::build_quota_fetchers(&quota_providers_to_fetch(config));
    for snapshot in tokenpulse_core::quota::fetch_all(fetchers)
        .await
        .into_iter()
        .flatten()
    {
        let _ = cache_store.save(&snapshot.provider, observed_at, &snapshot);
        snapshots.push(snapshot);
    }

    for provider in enabled_quota_providers(config) {
        if !snapshots.iter().any(|s| s.provider == provider) {
            if let Ok(Some(cached)) = cache_store.load_valid(&provider, observed_at) {
                snapshots.push(cached.snapshot);
            }
        }
    }

    snapshots
}

/// Providers whose quota may be fetched live. Empty when `refresh_quota` is
/// off, so no fetcher is built and no quota API is contacted.
fn quota_providers_to_fetch(config: &Config) -> Vec<String> {
    if !config.display.refresh_quota {
        return Vec::new();
    }
    enabled_quota_providers(config)
}

fn enabled_quota_providers(config: &Config) -> Vec<String> {
    config
        .providers
        .iter()
        .filter(|(_, provider)| provider.enabled)
        .map(|(id, _)| id.clone())
        .collect()
}

fn output_since_hint(
    requested_since: Option<NaiveDate>,
    refresh_range: Option<DateRange>,
) -> Option<NaiveDate> {
    requested_since.or(refresh_range.map(|range| range.start))
}

fn build_reload_fn(
    output_since: Option<NaiveDate>,
    log_path: Option<PathBuf>,
) -> impl FnMut() -> Result<(
    tokenpulse_core::usage::UsageSummary,
    Vec<tokenpulse_core::usage::DailyUsageRow>,
)> {
    move || {
        let mut perf = UsagePerfLog::from_path(log_path.clone());
        let current_provider_names = parse_provider_names(None);
        perf.log(
            "reload_start",
            format!(
                "providers={} output_since={:?}",
                current_provider_names.join(","),
                output_since
            ),
        );
        let _ = tokenpulse_core::pricing::PricingCache::clear_memory_cache();
        let store = UsageStore::new();
        let parsers = build_parsers(&current_provider_names, false);

        // Resolve incremental windows (sequential reads), parse concurrently,
        // then ingest sequentially so SQLite writes stay serialized.
        let mut reload_sinces: Vec<Option<NaiveDate>> = Vec::with_capacity(parsers.len());
        for parser in &parsers {
            let started = Instant::now();
            let since = store.default_since(parser.provider_name(), output_since)?;
            perf.log_duration(
                "reload_default_since",
                started.elapsed(),
                format!("provider={} since={:?}", parser.provider_name(), since),
            );
            reload_sinces.push(since);
        }

        let reload_outcomes: Vec<(Duration, Result<Vec<UnifiedMessage>>)> = parsers
            .par_iter()
            .zip(reload_sinces.par_iter())
            .map(|(parser, since)| {
                let started = Instant::now();
                let result = parser.parse_sessions(*since);
                (started.elapsed(), result)
            })
            .collect();

        for (parser, (parse_elapsed, parse_result)) in parsers.iter().zip(reload_outcomes) {
            let provider_started = Instant::now();
            perf.log(
                "reload_provider_start",
                format!("provider={}", parser.provider_name()),
            );

            match parse_result {
                Ok(messages) => {
                    perf.log_duration(
                        "reload_parse_sessions",
                        parse_elapsed,
                        format!(
                            "provider={} messages={} sessions={}",
                            parser.provider_name(),
                            messages.len(),
                            message_session_count(&messages)
                        ),
                    );
                    if !messages.is_empty() {
                        if parser.incremental_ingest_mode()
                            == IncrementalIngestMode::ReplaceChangedSessions
                        {
                            let started = Instant::now();
                            store.replace_sessions_messages(&messages, false)?;
                            perf.log_duration(
                                "reload_ingest_replace_sessions",
                                started.elapsed(),
                                format!(
                                    "provider={} messages={}",
                                    parser.provider_name(),
                                    messages.len()
                                ),
                            );
                        } else {
                            let started = Instant::now();
                            store.ingest_messages(&messages, false)?;
                            perf.log_duration(
                                "reload_ingest_upsert_messages",
                                started.elapsed(),
                                format!(
                                    "provider={} messages={}",
                                    parser.provider_name(),
                                    messages.len()
                                ),
                            );
                        }
                    }
                }
                Err(error) => {
                    perf.log_duration(
                        "reload_parse_sessions_error",
                        parse_elapsed,
                        format!("provider={} error={}", parser.provider_name(), error),
                    );
                } // tolerate per-provider errors during reload
            }
            perf.log_duration(
                "reload_provider_complete",
                provider_started.elapsed(),
                format!("provider={}", parser.provider_name()),
            );
        }

        let started = Instant::now();
        let repaired = store.repair_zero_costs(output_since, &current_provider_names)?;
        perf.log_duration(
            "reload_repair_zero_costs",
            started.elapsed(),
            format!("repaired={repaired}"),
        );

        let started = Instant::now();
        let (message_count, session_count) =
            store.load_summary_counts(output_since, &current_provider_names)?;
        perf.log_duration(
            "reload_load_summary_counts",
            started.elapsed(),
            format!("messages={message_count} sessions={session_count}"),
        );

        let started = Instant::now();
        let dashboard_days = store.load_dashboard_days(output_since, &current_provider_names)?;
        perf.log_duration(
            "reload_load_dashboard_days",
            started.elapsed(),
            format!("days={}", dashboard_days.len()),
        );
        let started = Instant::now();
        let provider_summaries =
            store.load_provider_summaries(output_since, &current_provider_names)?;
        perf.log_duration(
            "reload_load_provider_summaries",
            started.elapsed(),
            format!("providers={}", provider_summaries.len()),
        );
        let started = Instant::now();
        let model_summaries = store.load_model_summaries(output_since, &current_provider_names)?;
        perf.log_duration(
            "reload_load_model_summaries",
            started.elapsed(),
            format!("models={}", model_summaries.len()),
        );
        let summary = build_usage_summary_from_daily(
            dashboard_days,
            provider_summaries,
            model_summaries,
            message_count,
            session_count,
        );

        let started = Instant::now();
        let daily_rows = store.load_daily_rows(output_since, &current_provider_names)?;
        perf.log_duration(
            "reload_load_daily_rows",
            started.elapsed(),
            format!("rows={}", daily_rows.len()),
        );
        perf.log(
            "reload_complete",
            format!("total_elapsed_ms={}", perf.elapsed_ms()),
        );
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

fn build_parsers(provider_names: &[String], rebuild_all: bool) -> Vec<Box<dyn SessionParser>> {
    let config_manager = ConfigManager::new();
    let config = config_manager.load().unwrap_or_default();

    provider_names
        .iter()
        .filter_map(|provider| match provider.as_str() {
            "claude" => Some(Box::new(ClaudeSessionParser::new()) as Box<dyn SessionParser>),
            "codex" => Some(Box::new(CodexSessionParser::new()) as Box<dyn SessionParser>),
            "copilot" => Some(Box::new(CopilotSessionParser::new()) as Box<dyn SessionParser>),
            "opencode" => Some(Box::new(OpenCodeSessionParser::new()) as Box<dyn SessionParser>),
            "gemini" => Some(Box::new(GeminiSessionParser::new()) as Box<dyn SessionParser>),
            "pi" => Some(Box::new(PiSessionParser::new()) as Box<dyn SessionParser>),
            "antigravity" => {
                if config.display.scan_antigravity {
                    Some(
                        Box::new(AntigravitySessionParser::new().with_rebuild_cache(rebuild_all))
                            as Box<dyn SessionParser>,
                    )
                } else {
                    None
                }
            }
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

fn message_session_count(messages: &[UnifiedMessage]) -> usize {
    messages
        .iter()
        .map(|message| (message.client.as_str(), message.session_id.as_str()))
        .collect::<BTreeSet<_>>()
        .len()
}

struct UsagePerfLog {
    file: Option<File>,
    started: Instant,
    run_id: String,
    path: Option<PathBuf>,
}

impl UsagePerfLog {
    fn new(enabled: bool) -> Self {
        let started = Instant::now();
        let run_id = Utc::now()
            .to_rfc3339_opts(SecondsFormat::Millis, true)
            .replace(':', "-");
        let path = enabled.then(usage_perf_log_path);
        Self::from_parts(started, run_id, path)
    }

    fn from_path(path: Option<PathBuf>) -> Self {
        let started = Instant::now();
        let run_id = Utc::now()
            .to_rfc3339_opts(SecondsFormat::Millis, true)
            .replace(':', "-");
        Self::from_parts(started, run_id, path)
    }

    fn from_parts(started: Instant, run_id: String, path: Option<PathBuf>) -> Self {
        let file = path
            .as_ref()
            .and_then(|path| open_usage_perf_log(path).ok());
        let mut log = Self {
            file,
            started,
            run_id,
            path,
        };
        if let Some(path) = log.path.as_ref() {
            let detail = format!("path={}", path.display());
            log.log("log_open", detail);
        }
        log
    }

    fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    fn elapsed_ms(&self) -> u128 {
        self.started.elapsed().as_millis()
    }

    fn log(&mut self, event: &str, detail: impl AsRef<str>) {
        let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let elapsed_ms = self.elapsed_ms();
        let run_id = self.run_id.clone();
        let detail = detail.as_ref().replace('\n', " ");
        let Some(file) = &mut self.file else {
            return;
        };
        let _ = writeln!(
            file,
            "{} run_id={} elapsed_ms={} event={} {}",
            timestamp, run_id, elapsed_ms, event, detail
        );
    }

    fn log_duration(&mut self, event: &str, duration: Duration, detail: impl AsRef<str>) {
        self.log(
            event,
            format!("duration_ms={} {}", duration.as_millis(), detail.as_ref()),
        );
    }
}

fn open_usage_perf_log(path: &PathBuf) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn usage_perf_log_path() -> PathBuf {
    // `dirs` rather than $HOME: the variable is routinely unset on Windows and
    // in slim containers, which would drop this under the working directory.
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let filename = format!("usage-{}.log", Utc::now().format("%Y-%m-%d"));
    home.join(".local")
        .join("share")
        .join("tokenpulse")
        .join("log")
        .join(filename)
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
    for model in &summary.by_model {
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
    for day in summary.daily.iter().rev().take(365).rev() {
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

fn print_json_unified(
    summary: &tokenpulse_core::usage::UsageSummary,
    quota: Vec<tokenpulse_core::QuotaSnapshot>,
) -> Result<()> {
    let output = serde_json::json!({
        "usage": summary,
        "quota": quota,
    });
    serde_json::to_writer_pretty(std::io::stdout(), &output)?;
    println!();
    Ok(())
}

fn print_quota_summary(
    snapshots: &[tokenpulse_core::QuotaSnapshot],
    display_mode: &QuotaDisplayMode,
) {
    if snapshots.is_empty() {
        return;
    }
    println!("\n=== Quota Status ===");
    for snapshot in snapshots {
        let plan_str = snapshot.plan.as_deref().unwrap_or("None");
        let account_str = snapshot.account.as_deref().unwrap_or("None");
        println!("\nProvider: {}", snapshot.provider.to_uppercase());
        println!("  Plan: {}", plan_str);
        println!("  Account: {}", account_str);

        for window in &snapshot.windows {
            let used = window.used_percent;
            let remaining = (100.0 - used).max(0.0);
            let display_percent = match display_mode {
                QuotaDisplayMode::Used => used,
                QuotaDisplayMode::Remaining => remaining,
            };
            let mode_label = match display_mode {
                QuotaDisplayMode::Used => "used",
                QuotaDisplayMode::Remaining => "remaining",
            };
            print!(
                "  - {}: {:.2}% {}",
                window.label, display_percent, mode_label
            );
            if let Some(ref resets_at) = window.resets_at {
                let now = chrono::Utc::now();
                let duration = resets_at.signed_duration_since(now);
                if duration.num_seconds() > 0 {
                    print!(" (resets in {})", format_reset_countdown(duration));
                }
            }
            println!();
        }
        if let Some(ref credits) = snapshot.credits {
            print!("  Credits: ${:.2} {}", credits.used, credits.currency);
            if let Some(limit) = credits.limit {
                print!(" / ${:.2}", limit);
            }
            println!();
        }
        print_rate_limit_reset_credits(snapshot);
    }
}

fn print_rate_limit_reset_credits(snapshot: &tokenpulse_core::QuotaSnapshot) {
    if snapshot.rate_limit_reset_credits.is_empty() {
        return;
    }

    println!(
        "  Banked resets: {} available",
        snapshot.rate_limit_reset_credits.len()
    );
    println!("    {:<13} {}", "Banked reset", "Expiration time");

    let mut credits: Vec<_> = snapshot.rate_limit_reset_credits.iter().collect();
    credits.sort_by_key(|credit| (credit.expires_at.is_none(), credit.expires_at));

    for (index, credit) in credits.iter().enumerate() {
        let expiration = credit
            .expires_at
            .as_ref()
            .map(format_local_timestamp)
            .unwrap_or_else(|| "unknown".to_string());
        println!("    #{:<12} {}", index + 1, expiration);
    }
}

fn format_reset_countdown(diff: chrono::Duration) -> String {
    let total_minutes = diff.num_minutes().max(0);
    let hours = total_minutes / 60;
    let mins = total_minutes % 60;
    format!("{}h {}m", hours, mins)
}

fn format_local_timestamp(time: &DateTime<Utc>) -> String {
    time.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M %Z")
        .to_string()
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

fn print_daily_csv(rows: &[tokenpulse_core::usage::DailyUsageRow]) {
    println!("date,source,total_tokens,cost_usd,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,messages,sessions");
    for row in rows {
        println!(
            "{},{},{},{:.6},{},{},{},{},{},{}",
            row.date,
            row.source,
            row.total_tokens,
            row.cost_usd,
            row.input_tokens,
            row.output_tokens,
            row.cache_read_tokens,
            row.cache_write_tokens,
            row.message_count,
            row.session_count,
        );
    }
}

fn print_models_csv(summary: &tokenpulse_core::usage::UsageSummary) {
    println!("model,provider,source,tokens,cost_usd,messages,sessions,percent");
    for model in &summary.by_model {
        println!(
            "{},{},{},{},{:.6},{},{},{:.2}",
            model.model,
            model.provider,
            model.source,
            model.tokens,
            model.cost,
            model.message_count,
            model.session_count,
            model.percent,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{enabled_quota_providers, parse_provider_names, quota_providers_to_fetch};
    use chrono::NaiveDate;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokenpulse_core::{
        config::{Config, DisplayConfig, ProviderConfig},
        provider::{SessionParser, TokenBreakdown, UnifiedMessage},
        usage::UsageStore,
    };

    struct StubParser {
        provider_name: String,
        parser_version: String,
        messages: Vec<UnifiedMessage>,
    }

    impl SessionParser for StubParser {
        fn provider_name(&self) -> &str {
            &self.provider_name
        }

        fn session_paths(&self) -> Vec<std::path::PathBuf> {
            Vec::new()
        }

        fn parse_sessions(&self, _since: Option<NaiveDate>) -> anyhow::Result<Vec<UnifiedMessage>> {
            Ok(self.messages.clone())
        }

        fn parser_version(&self) -> &str {
            &self.parser_version
        }
    }

    fn sample_message(source: &str, parser_version: &str, date: &str, key: &str) -> UnifiedMessage {
        let timestamp = NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis();

        UnifiedMessage::new(
            source,
            "gemini-2.5-pro",
            "google",
            "session-1",
            key,
            timestamp,
            TokenBreakdown {
                input: 100,
                output: 50,
                cache_read: 10,
                cache_write: 0,
                reasoning: 0,
            },
        )
        .with_cost(1.0)
        .with_parser_version(parser_version)
    }

    fn ingest_parsed_messages(
        store: &UsageStore,
        parser: &dyn SessionParser,
        refresh_pricing: bool,
        stale_sources: &std::collections::HashSet<String>,
    ) {
        let messages = parser.parse_sessions(None).unwrap();
        if stale_sources.contains(parser.provider_name()) {
            if !messages.is_empty() {
                store
                    .replace_source_messages(parser.provider_name(), &messages, refresh_pricing)
                    .unwrap();
            }
        } else if !messages.is_empty() {
            store.ingest_messages(&messages, refresh_pricing).unwrap();
        }
    }

    #[test]
    fn parse_provider_names_preserves_requested_subset_order() {
        assert_eq!(
            parse_provider_names(Some("gemini,codex")),
            vec!["gemini".to_string(), "codex".to_string()]
        );
    }

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
                "antigravity".to_string(),
            ]
        );
    }

    #[test]
    fn stale_parser_rebuild_keeps_existing_rows_when_parser_returns_no_messages() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("tokenpulse-usage-test-{unique}.sqlite3"));
        let _ = fs::remove_file(&path);
        let store = UsageStore::with_path(path.clone());
        store
            .ingest_messages(
                &[sample_message("gemini", "gemini-v2", "2024-03-10", "old")],
                false,
            )
            .unwrap();

        let parser = StubParser {
            provider_name: "gemini".to_string(),
            parser_version: "gemini-v3".to_string(),
            messages: Vec::new(),
        };
        let stale_sources = ["gemini".to_string()].into_iter().collect();

        ingest_parsed_messages(&store, &parser, false, &stale_sources);

        let remaining = store.load_messages(None, &["gemini".to_string()]).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].message_key, "old");
        assert_eq!(remaining[0].parser_version, "gemini-v2");

        let _ = fs::remove_file(path);
    }

    fn config_with_providers(refresh_quota: bool, providers: &[(&str, bool)]) -> Config {
        Config {
            providers: providers
                .iter()
                .map(|(id, enabled)| {
                    (
                        id.to_string(),
                        ProviderConfig {
                            enabled: *enabled,
                            path: None,
                        },
                    )
                })
                .collect(),
            display: DisplayConfig {
                refresh_quota,
                ..DisplayConfig::default()
            },
            ..Config::default()
        }
    }

    #[test]
    fn non_tui_quota_fetch_respects_refresh_quota_setting() {
        let providers = [("claude", true), ("codex", true), ("copilot", false)];

        let mut enabled = enabled_quota_providers(&config_with_providers(true, &providers));
        enabled.sort();
        assert_eq!(enabled, vec!["claude".to_string(), "codex".to_string()]);

        let mut to_fetch = quota_providers_to_fetch(&config_with_providers(true, &providers));
        to_fetch.sort();
        assert_eq!(to_fetch, enabled);

        // refresh_quota = false must build no fetchers, so no quota API is
        // contacted from the --json or plain-text output paths.
        assert!(quota_providers_to_fetch(&config_with_providers(false, &providers)).is_empty());

        // Cached snapshots are still shown for enabled providers.
        let mut cached = enabled_quota_providers(&config_with_providers(false, &providers));
        cached.sort();
        assert_eq!(cached, enabled);
    }
}
