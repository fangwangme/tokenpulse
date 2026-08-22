use crate::provider::{
    local_date_string_from_timestamp, SessionParser, TokenBreakdown, UnifiedMessage,
};
use crate::usage::utils::detect_provider_from_model;

use anyhow::Result;
use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, warn};

/// Bumped from `antigravity-v2` when local `gen_metadata` parsing landed. Both
/// the local and the RPC path stamp rows with it, so the ledger's staleness
/// check sees one version per source, and a bump re-reads every cached
/// conversation instead of only the ones whose files changed.
const PARSER_VERSION: &str = "antigravity-v3";
const MODEL_ALIAS_HISTORY_VERSION: u32 = 1;
const MODEL_ALIAS_HISTORY_FILE_NAME: &str = "model-aliases.json";
const ANTIGRAVITY_LS_SERVICE: &str = "exa.language_server_pb.LanguageServerService";
const ANTIGRAVITY_RPC_BODY_CAP: usize = 64 * 1024 * 1024;

pub struct AntigravitySessionParser {
    rebuild_cache: bool,
    custom_paths: Option<Vec<PathBuf>>,
    skip_sync: bool,
}

impl AntigravitySessionParser {
    pub fn new() -> Self {
        Self {
            rebuild_cache: false,
            custom_paths: None,
            skip_sync: false,
        }
    }

    pub fn with_rebuild_cache(mut self, rebuild_cache: bool) -> Self {
        self.rebuild_cache = rebuild_cache;
        self
    }

    pub fn with_custom_paths(mut self, paths: Vec<PathBuf>) -> Self {
        self.custom_paths = Some(paths);
        self
    }

    pub fn with_skip_sync(mut self, skip_sync: bool) -> Self {
        self.skip_sync = skip_sync;
        self
    }
}

impl Default for AntigravitySessionParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionParser for AntigravitySessionParser {
    fn provider_name(&self) -> &str {
        "antigravity"
    }

    fn session_paths(&self) -> Vec<PathBuf> {
        if let Some(ref paths) = self.custom_paths {
            paths.clone()
        } else {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            vec![home
                .join(".local")
                .join("share")
                .join("tokenpulse")
                .join("antigravity-cache")]
        }
    }

    fn parse_sessions(&self, _since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        let mut all_messages = Vec::new();

        for root in self.session_paths() {
            // Sync first!
            let sync_started_ms = Local::now().timestamp_millis();
            if !self.skip_sync {
                if let Err(e) = sync_antigravity_with_options(
                    &root,
                    AntigravitySyncOptions {
                        rebuild_all_cache: self.rebuild_cache,
                    },
                ) {
                    debug!("Failed to sync Antigravity: {}", e);
                }
            }

            let alias_history = match load_model_alias_history_map(&root) {
                Ok(aliases) => aliases,
                Err(e) => {
                    debug!("Failed to load Antigravity model alias history: {}", e);
                    HashMap::new()
                }
            };
            if let Err(e) = normalize_cached_antigravity_artifacts(&root, &alias_history) {
                debug!("Failed to normalize Antigravity cache: {}", e);
            }

            // Now read from SQLite
            let conn = open_cache_db(&root)?;
            let mut query = String::from(
                "SELECT client, model_id, provider_id, session_id, COALESCE(response_id, id), timestamp,
                        input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
                        pricing_day, parser_version
                 FROM session_usage"
            );

            let mut params = Vec::new();
            if let Some(since_date) = _since {
                let since_ms = since_date
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_utc()
                    .timestamp_millis();
                query.push_str(
                    " WHERE timestamp >= ?1
                       OR (client || ':' || session_id) IN (
                           SELECT client || ':' || session_id FROM sessions WHERE synced_at >= ?2
                       )",
                );
                params.push(since_ms);
                params.push(sync_started_ms);
            }
            query.push_str(" ORDER BY timestamp ASC");

            let mut stmt = conn.prepare(&query)?;

            let map_row = |row: &rusqlite::Row<'_>| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, i64>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                ))
            };

            let rows = stmt.query_map(rusqlite::params_from_iter(params), map_row)?;

            for row in rows {
                let (
                    client,
                    model_id,
                    provider_id,
                    session_id,
                    message_key,
                    timestamp,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    reasoning,
                    pricing_day,
                    parser_version,
                ) = row?;

                let tokens = TokenBreakdown {
                    input,
                    output,
                    cache_read,
                    cache_write,
                    reasoning,
                };

                let msg = UnifiedMessage::new(
                    "antigravity",
                    model_id,
                    provider_id,
                    session_id,
                    message_key,
                    timestamp,
                    tokens,
                )
                .with_client_detail(client)
                .with_pricing_day(pricing_day)
                .with_parser_version(parser_version);

                all_messages.push(msg);
            }
        }

        all_messages.sort_by_key(|m| m.timestamp);
        Ok(all_messages)
    }

    fn parser_version(&self) -> &str {
        PARSER_VERSION
    }
}

#[derive(Debug, Clone)]
pub struct AntigravityConnection {
    pub pid: u32,
    pub port: u16,
    pub csrf_token: Option<String>,
    pub scheme: String,
    pub fingerprint: String,
    pub runtime_kind: AntigravityRuntimeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AntigravityRuntimeKind {
    Desktop,
    Cli,
    Unknown,
}

impl AntigravityRuntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Cli => "cli",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for AntigravityRuntimeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Desktop => write!(f, "Desktop"),
            Self::Cli => write!(f, "CLI"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

#[derive(Debug, Clone)]
struct ProcessCandidate {
    pid: u32,
    ppid: u32,
    declared_port: Option<u16>,
    csrf_token: Option<String>,
    runtime_kind: AntigravityRuntimeKind,
}

#[derive(Debug, Clone)]
struct LocalConversationId {
    session_id: String,
    modified_ms: Option<i64>,
    runtime_kind: AntigravityRuntimeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct AntigravitySessionCacheKey {
    session_id: String,
    runtime_kind: AntigravityRuntimeKind,
}

#[derive(Debug, Clone, Copy, Default)]
struct AntigravitySyncOptions {
    rebuild_all_cache: bool,
}

fn open_cache_db(sessions_dir: &Path) -> Result<rusqlite::Connection> {
    std::fs::create_dir_all(sessions_dir)?;
    let db_path = sessions_dir.join("cache.db");
    let conn = rusqlite::Connection::open(db_path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute("PRAGMA foreign_keys = ON;", [])?;

    let sessions_pk_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE pk > 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if sessions_pk_count > 0 && sessions_pk_count != 2 {
        let _ = conn.execute("DROP TABLE IF EXISTS session_usage;", []);
        let _ = conn.execute("DROP TABLE IF EXISTS sessions;", []);
    }

    conn.execute(
        "CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT NOT NULL,
            trajectory_id TEXT,
            client TEXT NOT NULL,
            title TEXT,
            model_id TEXT NOT NULL,
            status TEXT,
            step_count INTEGER,
            created_time_ms INTEGER,
            last_modified_ms INTEGER,
            last_user_input_time_ms INTEGER,
            project_id TEXT,
            workspace_path TEXT,
            git_root TEXT,
            repository TEXT,
            git_origin_url TEXT,
            branch_name TEXT,
            parent_conversation_id TEXT,
            mendel_experiment_ids TEXT,
            synced_at INTEGER NOT NULL,
            PRIMARY KEY (session_id, client)
        );",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS session_usage (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            client TEXT NOT NULL,
            model_id TEXT NOT NULL,
            provider_id TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            step_index INTEGER NOT NULL,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            cache_read_tokens INTEGER NOT NULL,
            cache_write_tokens INTEGER NOT NULL,
            reasoning_tokens INTEGER NOT NULL,
            response_id TEXT,
            pricing_day TEXT NOT NULL,
            parser_version TEXT NOT NULL,
            FOREIGN KEY(session_id, client) REFERENCES sessions(session_id, client) ON DELETE CASCADE
        );",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_session_usage_session_id ON session_usage(session_id);",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_session_usage_pricing_day ON session_usage(pricing_day);",
        [],
    )?;

    migrate_legacy_usage_rows(&conn)?;

    Ok(conn)
}

/// Rewrites `antigravity-v2` usage rows in place.
///
/// v2 rows all came from the language-server RPC, which stored `outputTokens`
/// — thinking tokens included — in `output_tokens`, double-counting reasoning
/// in cost. Splitting them here fixes those rows and, just as importantly,
/// leaves no row behind at an old `parser_version`: a single one that the local
/// parser can never re-read (an encrypted `.pb` session) would otherwise mark
/// the source stale on every launch, forcing a full ledger re-read forever.
/// `antigravity-v2` is the only version that ever shipped, so this closes the
/// set.
fn migrate_legacy_usage_rows(conn: &rusqlite::Connection) -> Result<()> {
    let migrated = conn.execute(
        "UPDATE session_usage
            SET output_tokens = MAX(output_tokens - reasoning_tokens, 0),
                parser_version = ?1
          WHERE parser_version = 'antigravity-v2';",
        [PARSER_VERSION],
    )?;
    if migrated > 0 {
        info!(
            "Antigravity cache: migrated {} usage rows from antigravity-v2 to {}",
            migrated, PARSER_VERSION
        );
    }
    Ok(())
}

fn count_antigravity_session_cache_rows(conn: &rusqlite::Connection) -> usize {
    conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap_or(0)
}

pub fn sync_antigravity(sessions_dir: &Path) -> Result<()> {
    sync_antigravity_with_options(sessions_dir, AntigravitySyncOptions::default())
}

fn block_on_async<F: std::future::Future>(future: F) -> F::Output {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(future))
    } else {
        tokio::runtime::Runtime::new().unwrap().block_on(future)
    }
}

fn sync_antigravity_with_options(
    sessions_dir: &Path,
    options: AntigravitySyncOptions,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()?;
    block_on_async(sync_antigravity_with_options_async(
        &client,
        sessions_dir,
        options,
    ))
}

async fn sync_antigravity_with_options_async(
    client: &reqwest::Client,
    sessions_dir: &Path,
    options: AntigravitySyncOptions,
) -> Result<()> {
    let sync_start = std::time::Instant::now();
    std::fs::create_dir_all(sessions_dir)?;

    let mut db_conn = open_cache_db(sessions_dir)?;

    if options.rebuild_all_cache {
        db_conn.execute("DELETE FROM session_usage;", [])?;
        db_conn.execute("DELETE FROM sessions;", [])?;
    }

    let mut cached_sessions: HashMap<(String, String), (Option<i64>, Option<i64>)> = HashMap::new();
    if let Ok(mut stmt) = db_conn.prepare(
        r#"
        SELECT client, session_id, last_modified_ms, step_count FROM sessions
        UNION
        SELECT session_usage.client, sessions.session_id, sessions.last_modified_ms, sessions.step_count
        FROM session_usage
        JOIN sessions ON sessions.session_id = session_usage.session_id
        "#,
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        }) {
            for row in rows {
                if let Ok((client, id, last_mod, step_count)) = row {
                    cached_sessions.insert((client, id), (last_mod, step_count));
                }
            }
        }
    }

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    // The local scan runs before the language server is probed, so only the
    // persisted alias history is available; `normalize_cached_antigravity_artifacts`
    // reconciles anything the live server later renames.
    let persisted_model_aliases = load_model_alias_history_map(sessions_dir).unwrap_or_else(|e| {
        debug!("Failed to load Antigravity model alias history: {}", e);
        static_model_aliases()
    });
    if let Err(e) = sync_local_conversations(
        &mut db_conn,
        &home,
        &cached_sessions,
        options.rebuild_all_cache,
        &persisted_model_aliases,
    ) {
        debug!(
            "Failed to sync local Antigravity conversation databases: {}",
            e
        );
    }

    let connections = match detect_antigravity_connections_with_client(client).await {
        Ok(c) => c,
        Err(e) => {
            debug!(
                "Antigravity language server process discovery skipped/failed: {}",
                e
            );
            Vec::new()
        }
    };

    if connections.is_empty() {
        if sessions_dir.exists() {
            if let Err(e) = merge_and_save_model_alias_history(sessions_dir, &HashMap::new()) {
                debug!("Failed to seed Antigravity model alias history: {}", e);
            }
        }
        debug!("No running Antigravity Desktop language servers detected; local CLI conversations synced");
        return Ok(());
    }

    let cached_rows_before = count_antigravity_session_cache_rows(&db_conn);

    let dynamic_model_aliases = fetch_dynamic_model_aliases(client, &connections).await;
    let model_aliases =
        match merge_and_save_model_alias_history(sessions_dir, &dynamic_model_aliases) {
            Ok(aliases) => aliases,
            Err(e) => {
                debug!("Failed to update Antigravity model alias history: {}", e);
                dynamic_model_aliases
            }
        };

    let mut synced_sessions_count = 0;
    if let Ok(mut stmt) = db_conn.prepare(
        r#"
        SELECT client, session_id, last_modified_ms, step_count FROM sessions
        UNION
        SELECT session_usage.client, sessions.session_id, sessions.last_modified_ms, sessions.step_count
        FROM session_usage
        JOIN sessions ON sessions.session_id = session_usage.session_id
        "#,
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        }) {
            for row in rows {
                if let Ok((client, id, last_mod, step_count)) = row {
                    cached_sessions.insert((client, id), (last_mod, step_count));
                }
            }
        }
    }

    let active_kinds: Vec<AntigravityRuntimeKind> =
        connections.iter().map(|c| c.runtime_kind).collect();
    let local_conversation_ids = discover_local_conversation_ids(&active_kinds);
    debug!(
        "Discovered {} local conversation files",
        local_conversation_ids.len()
    );

    let mut unique_summaries: HashMap<AntigravitySessionCacheKey, AntigravitySyncSummary> =
        HashMap::new();
    for connection in &connections {
        if !options.rebuild_all_cache && !local_conversation_ids.is_empty() {
            continue;
        }

        let response =
            match rpc_request(client, connection, "GetAllCascadeTrajectories", &json!({})).await {
                Ok(r) => r,
                Err(e) => {
                    debug!(
                        "Failed to query GetAllCascadeTrajectories from {}: {}",
                        connection.port, e
                    );
                    continue;
                }
            };

        let trajectory_entries = extract_trajectory_entries(&response);

        for (key, item) in trajectory_entries {
            let session_id = if !key.is_empty() {
                key
            } else {
                item.get("cascadeId")
                    .or_else(|| item.get("trajectoryId"))
                    .or_else(|| item.get("id"))
                    .or_else(|| item.get("sessionId"))
                    .and_then(Value::as_str)
                    .map(String::from)
                    .unwrap_or_default()
            };

            if session_id.is_empty() {
                continue;
            }

            upsert_sync_summary(
                &mut unique_summaries,
                AntigravitySessionCacheKey {
                    session_id,
                    runtime_kind: connection.runtime_kind,
                },
                summary_from_trajectory_item(&item, connection.clone()),
            );
        }
    }
    for local in local_conversation_ids {
        let cache_key = AntigravitySessionCacheKey {
            session_id: local.session_id.clone(),
            runtime_kind: local.runtime_kind,
        };
        if let Some(summary) = unique_summaries.get_mut(&cache_key) {
            summary.last_modified_ms = newer_timestamp(summary.last_modified_ms, local.modified_ms);
            prioritize_connections_for_runtime(
                &mut summary.connections,
                &connections,
                local.runtime_kind,
            );
        } else {
            let mut conns = Vec::new();
            prioritize_connections_for_runtime(&mut conns, &connections, local.runtime_kind);
            unique_summaries.insert(
                cache_key,
                AntigravitySyncSummary {
                    last_modified_ms: local.modified_ms,
                    connections: conns,
                    trajectory_id: None,
                    title: None,
                    status: None,
                    step_count: None,
                    created_time_ms: None,
                    last_user_input_time_ms: None,
                    project_id: None,
                    workspace_path: None,
                    git_root: None,
                    repository: None,
                    git_origin_url: None,
                    branch_name: None,
                    parent_conversation_id: None,
                    mendel_experiment_ids: None,
                },
            );
        }
    }

    let mut sessions_to_sync = Vec::new();
    for (cache_key, summary) in unique_summaries {
        let session_id = cache_key.session_id.clone();
        let client_str = client_str_for_runtime_kind(cache_key.runtime_kind).to_string();
        let last_modified_ms = summary.last_modified_ms;

        if !options.rebuild_all_cache {
            if let Some((cached_lm, cached_step)) =
                cached_sessions.get(&(client_str.clone(), session_id.clone()))
            {
                let unchanged = match (cached_lm, last_modified_ms) {
                    (Some(cached), Some(current)) => cached >= &current,
                    _ => match (cached_step, summary.step_count) {
                        (Some(cached), Some(current)) => cached >= &current,
                        _ => false,
                    },
                };
                if unchanged {
                    debug!("Session {} is unchanged, skipping sync", session_id);
                    continue;
                }
            }
        }
        sessions_to_sync.push((cache_key, summary));
    }

    let total_detected_sessions = sessions_to_sync.len();
    if total_detected_sessions == 0 {
        return Ok(());
    }

    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let mut join_set = tokio::task::JoinSet::new();

    for (idx, (cache_key, summary)) in sessions_to_sync.iter().enumerate() {
        let client_clone = client.clone();
        let sem_clone = semaphore.clone();
        let session_id = cache_key.session_id.clone();
        let connections = summary.connections.clone();
        let metadata_timeout = if options.rebuild_all_cache {
            Duration::from_secs(30)
        } else {
            Duration::from_secs(10)
        };

        join_set.spawn(async move {
            let _permit = sem_clone.acquire().await.ok();
            let mut metadata_response = None;
            for conn in &connections {
                match rpc_request_with_timeout(
                    &client_clone,
                    conn,
                    "GetCascadeTrajectoryGeneratorMetadata",
                    &json!({ "cascadeId": session_id }),
                    metadata_timeout,
                )
                .await
                {
                    Ok(r) => {
                        metadata_response = Some(r);
                        break;
                    }
                    Err(e) => {
                        debug!(
                            "Failed to fetch generator metadata for session {} from port {}: {}",
                            session_id, conn.port, e
                        );
                    }
                }
            }
            (idx, metadata_response)
        });
    }

    let mut responses = vec![None; sessions_to_sync.len()];
    let mut failed_metadata_count = 0usize;
    while let Some(res) = join_set.join_next().await {
        match res {
            Ok((idx, metadata_response)) => {
                if metadata_response.is_none() {
                    failed_metadata_count += 1;
                }
                responses[idx] = metadata_response;
            }
            Err(e) => {
                failed_metadata_count += 1;
                debug!("Antigravity metadata sync task failed: {}", e);
            }
        }
    }

    let now_ms = Local::now().timestamp_millis();
    let mut empty_metadata_count = 0usize;
    let mut zero_usage_sessions_count = 0usize;
    for (idx, (cache_key, summary)) in sessions_to_sync.into_iter().enumerate() {
        let Some(metadata_response) = responses[idx].take() else {
            let tx = db_conn.transaction()?;
            upsert_antigravity_session_row(
                &tx,
                &cache_key.session_id,
                client_str_for_runtime_kind(cache_key.runtime_kind),
                &summary,
                "unknown",
                now_ms,
            )?;
            tx.commit()?;
            continue;
        };

        let session_id = cache_key.session_id;
        let client_str = client_str_for_runtime_kind(cache_key.runtime_kind);

        let metadata = metadata_response
            .get("generatorMetadata")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let metadata_was_empty = metadata.is_empty();
        if metadata_was_empty {
            empty_metadata_count += 1;
            debug!(
                "Antigravity session {} ({}) returned empty generatorMetadata",
                session_id, client_str
            );
        }

        let mut primary_model_id = "unknown".to_string();
        for meta in &metadata {
            let chat_model = meta.get("chatModel").unwrap_or(meta);
            let raw_model_id = chat_model
                .get("responseModel")
                .or_else(|| chat_model.get("model"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let mut model_id =
                resolve_antigravity_model_id_with_aliases(raw_model_id, &model_aliases);
            if is_pseudo_raw_model(&model_id) {
                if let Some(display_name) =
                    chat_model.get("modelDisplayName").and_then(Value::as_str)
                {
                    if let Some(normalized) = normalize_display_name_to_id(display_name) {
                        model_id = normalized;
                    }
                }
            }
            if model_id != "unknown" {
                primary_model_id = model_id;
                break;
            }
        }

        // No DELETE before re-inserting: `INSERT OR REPLACE` on the shared
        // `{client}:{session_id}:{responseId}` key already overwrites in place,
        // and deleting first would drop rows the local parser produced whenever
        // the RPC returns a subset of them.
        let tx = db_conn.transaction()?;

        upsert_antigravity_session_row(
            &tx,
            &session_id,
            client_str,
            &summary,
            &primary_model_id,
            now_ms,
        )?;

        let mut inserted_usage_rows = 0usize;
        for (step_idx, meta) in metadata.iter().enumerate() {
            let chat_model = meta.get("chatModel").unwrap_or(meta);
            let raw_model_id = chat_model
                .get("responseModel")
                .or_else(|| chat_model.get("model"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let mut model_id =
                resolve_antigravity_model_id_with_aliases(raw_model_id, &model_aliases);
            if is_pseudo_raw_model(&model_id) {
                if let Some(display_name) =
                    chat_model.get("modelDisplayName").and_then(Value::as_str)
                {
                    if let Some(normalized) = normalize_display_name_to_id(display_name) {
                        model_id = normalized;
                    }
                }
            }

            let created_at = chat_model
                .get("chatStartMetadata")
                .and_then(|v| v.get("createdAt"))
                .and_then(parse_timestamp_value);

            if let Some(retry_infos) = chat_model.get("retryInfos").and_then(Value::as_array) {
                for retry in retry_infos {
                    let usage = retry.get("usage").unwrap_or(retry);
                    let Some(tokens) = normalize_antigravity_tokens(
                        to_safe_i64(usage.get("inputTokens")),
                        to_safe_i64(usage.get("outputTokens")),
                        usage
                            .get("responseOutputTokens")
                            .map(|v| to_safe_i64(Some(v))),
                        to_safe_i64(usage.get("cacheReadTokens")),
                        to_safe_i64(usage.get("cacheWriteTokens")),
                        to_safe_i64(usage.get("thinkingOutputTokens")),
                    ) else {
                        continue;
                    };
                    let AntigravityTokens {
                        input,
                        output,
                        cache_read,
                        cache_write,
                        reasoning,
                    } = tokens;
                    let timestamp = usage
                        .get("createdAt")
                        .or_else(|| usage.get("timestamp"))
                        .and_then(parse_timestamp_value)
                        .or(created_at)
                        .unwrap_or(now_ms);

                    if input == 0
                        && output == 0
                        && cache_read == 0
                        && reasoning == 0
                        && cache_write == 0
                    {
                        continue;
                    }

                    let raw_message_key = usage
                        .get("responseId")
                        .and_then(Value::as_str)
                        .map(String::from)
                        .unwrap_or_else(|| format!("{}:{}:{}", timestamp, model_id, step_idx));
                    let message_key =
                        antigravity_logical_message_key(&session_id, &raw_message_key);
                    let storage_message_id =
                        antigravity_storage_message_id(client_str, &message_key);

                    let provider_id = detect_provider_from_model(&model_id);
                    let date_str = local_date_string_from_timestamp(timestamp);

                    tx.execute(
                        "INSERT OR REPLACE INTO session_usage (
                            id, session_id, client, model_id, provider_id, timestamp, step_index,
                            input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
                            response_id, pricing_day, parser_version
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        rusqlite::params![
                            storage_message_id,
                            &session_id,
                            client_str,
                            model_id,
                            provider_id,
                            timestamp,
                            step_idx as i64,
                            input,
                            output,
                            cache_read,
                            cache_write,
                            reasoning,
                            message_key,
                            date_str,
                            PARSER_VERSION,
                        ],
                    )?;
                    inserted_usage_rows += 1;
                }
            }
        }

        tx.commit()?;
        if metadata_was_empty {
            continue;
        }
        if inserted_usage_rows == 0 {
            zero_usage_sessions_count += 1;
            debug!(
                "Antigravity session {} ({}) produced no usage rows from {} metadata entries",
                session_id,
                client_str,
                metadata.len()
            );
        }
        synced_sessions_count += 1;
    }

    let cached_rows_after = count_antigravity_session_cache_rows(&db_conn);

    info!(
        "Antigravity sync: Synced local Antigravity cache in {} ms. Connections: {}, sessions: (total: {}, synced: {}, metadata_failed: {}, metadata_empty: {}, zero_usage: {}), cache rows: {} -> {}",
        sync_start.elapsed().as_millis(),
        connections.len(),
        total_detected_sessions,
        synced_sessions_count,
        failed_metadata_count,
        empty_metadata_count,
        zero_usage_sessions_count,
        cached_rows_before,
        cached_rows_after
    );

    Ok(())
}

/// Disjoint token columns for one Antigravity generation.
///
/// Antigravity reports `outputTokens` as the *total* output, thinking included,
/// while `calculate_cost` bills `input + output + cache_read + cache_write +
/// reasoning`. Storing the total as `output` therefore charges the thinking
/// tokens twice. Both the local `gen_metadata` parser and the language-server
/// RPC funnel through here so the two can never drift apart again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AntigravityTokens {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
}

/// `total_output` is `outputTokens` (`1.4.3`); `response_output` is
/// `responseOutputTokens` (`1.4.10`) when the source provides it.
///
/// Returns `None` when the two disagree — `thinking + response == total` held
/// on every one of 20,729 real records, so a mismatch means the field map
/// drifted and the record must not be stored. All arithmetic is checked: these
/// values come from third-party blobs and JSON.
fn normalize_antigravity_tokens(
    input: i64,
    total_output: i64,
    response_output: Option<i64>,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
) -> Option<AntigravityTokens> {
    let output = match response_output {
        Some(response_output) => {
            let partition = reasoning.checked_add(response_output);
            if partition != Some(total_output) {
                warn!(
                    "Antigravity usage integrity check failed: thinking {} + response {} != total {}",
                    reasoning, response_output, total_output
                );
                return None;
            }
            response_output
        }
        // Older RPC responses carry only the total; recover the disjoint part.
        None => total_output.saturating_sub(reasoning).max(0),
    };

    Some(AntigravityTokens {
        input,
        output,
        cache_read,
        cache_write,
        reasoning,
    })
}

fn client_str_for_runtime_kind(runtime_kind: AntigravityRuntimeKind) -> &'static str {
    match runtime_kind {
        AntigravityRuntimeKind::Cli => "antigravity-cli",
        AntigravityRuntimeKind::Desktop => "antigravity-desktop",
        AntigravityRuntimeKind::Unknown => "antigravity",
    }
}

fn upsert_antigravity_session_row(
    tx: &rusqlite::Transaction<'_>,
    session_id: &str,
    client: &str,
    summary: &AntigravitySyncSummary,
    model_id: &str,
    synced_at: i64,
) -> Result<()> {
    tx.execute(
        "INSERT INTO sessions (
            session_id, trajectory_id, client, title, model_id, status, step_count,
            created_time_ms, last_modified_ms, last_user_input_time_ms, project_id,
            workspace_path, git_root, repository, git_origin_url, branch_name,
            parent_conversation_id, mendel_experiment_ids, synced_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(session_id, client) DO UPDATE SET
            -- COALESCE, not plain assignment: the local scan can only fill the
            -- fields a conversation database actually holds and leaves the rest
            -- NULL. Overwriting would wipe the richer metadata the language
            -- server supplied, which nothing can re-derive offline.
            trajectory_id = COALESCE(excluded.trajectory_id, sessions.trajectory_id),
            title = COALESCE(excluded.title, sessions.title),
            model_id = CASE
                WHEN excluded.model_id = 'unknown' THEN sessions.model_id
                ELSE excluded.model_id
            END,
            status = COALESCE(excluded.status, sessions.status),
            step_count = COALESCE(excluded.step_count, sessions.step_count),
            created_time_ms = COALESCE(excluded.created_time_ms, sessions.created_time_ms),
            last_modified_ms = COALESCE(excluded.last_modified_ms, sessions.last_modified_ms),
            last_user_input_time_ms =
                COALESCE(excluded.last_user_input_time_ms, sessions.last_user_input_time_ms),
            project_id = COALESCE(excluded.project_id, sessions.project_id),
            workspace_path = COALESCE(excluded.workspace_path, sessions.workspace_path),
            git_root = COALESCE(excluded.git_root, sessions.git_root),
            repository = COALESCE(excluded.repository, sessions.repository),
            git_origin_url = COALESCE(excluded.git_origin_url, sessions.git_origin_url),
            branch_name = COALESCE(excluded.branch_name, sessions.branch_name),
            parent_conversation_id =
                COALESCE(excluded.parent_conversation_id, sessions.parent_conversation_id),
            mendel_experiment_ids =
                COALESCE(excluded.mendel_experiment_ids, sessions.mendel_experiment_ids),
            synced_at = excluded.synced_at",
        rusqlite::params![
            session_id,
            summary.trajectory_id.as_deref(),
            client,
            summary.title.as_deref(),
            model_id,
            summary.status.as_deref(),
            summary.step_count,
            summary.created_time_ms,
            summary.last_modified_ms,
            summary.last_user_input_time_ms,
            summary.project_id.as_deref(),
            summary.workspace_path.as_deref(),
            summary.git_root.as_deref(),
            summary.repository.as_deref(),
            summary.git_origin_url.as_deref(),
            summary.branch_name.as_deref(),
            summary.parent_conversation_id.as_deref(),
            summary.mendel_experiment_ids.as_deref(),
            synced_at,
        ],
    )?;
    Ok(())
}

/// Minimal protobuf wire-format reader for Antigravity's local `gen_metadata`
/// blobs. No schema file exists for them, so fields are addressed by number.
/// Every read is bounds-checked: these are third-party blobs, and a malformed
/// one must skip its record rather than abort the sync.
enum ProtoValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

fn read_proto_field<'a>(buf: &'a [u8], pos: &mut usize) -> Option<(u32, ProtoValue<'a>)> {
    let key = read_varint(buf, pos)?;
    let field_number = u32::try_from(key >> 3).ok()?;
    let take = |pos: &mut usize, len: usize| -> Option<&'a [u8]> {
        let end = pos.checked_add(len)?;
        let slice = buf.get(*pos..end)?;
        *pos = end;
        Some(slice)
    };
    let value = match key & 0x7 {
        0 => ProtoValue::Varint(read_varint(buf, pos)?),
        1 => ProtoValue::Bytes(take(pos, 8)?),
        2 => {
            let len = usize::try_from(read_varint(buf, pos)?).ok()?;
            ProtoValue::Bytes(take(pos, len)?)
        }
        5 => ProtoValue::Bytes(take(pos, 4)?),
        // Deprecated group wire types (3, 4) never appear here; stop rather
        // than guess at a length.
        _ => return None,
    };
    Some((field_number, value))
}

/// Visits every well-formed field in `buf`, stopping at the first malformed
/// one so a truncated tail still yields the fields that preceded it.
fn for_each_proto_field<'a>(buf: &'a [u8], mut visit: impl FnMut(u32, ProtoValue<'a>)) {
    let mut pos = 0usize;
    while pos < buf.len() {
        let Some((field_number, value)) = read_proto_field(buf, &mut pos) else {
            return;
        };
        visit(field_number, value);
    }
}

/// First length-delimited value for `field_number`.
fn proto_message(buf: &[u8], field_number: u32) -> Option<&[u8]> {
    let mut found = None;
    for_each_proto_field(buf, |number, value| {
        if found.is_none() && number == field_number {
            if let ProtoValue::Bytes(bytes) = value {
                found = Some(bytes);
            }
        }
    });
    found
}

/// First varint value for `field_number`. proto3 omits zero-valued scalars, so
/// callers read `None` as 0 rather than as a missing field.
fn proto_varint(buf: &[u8], field_number: u32) -> Option<u64> {
    let mut found = None;
    for_each_proto_field(buf, |number, value| {
        if found.is_none() && number == field_number {
            if let ProtoValue::Varint(varint) = value {
                found = Some(varint);
            }
        }
    });
    found
}

fn proto_string(buf: &[u8], field_number: u32) -> Option<String> {
    let text = std::str::from_utf8(proto_message(buf, field_number)?)
        .ok()?
        .trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn proto_token_count(buf: &[u8], field_number: u32) -> i64 {
    proto_varint(buf, field_number)
        .and_then(|value| i64::try_from(value).ok())
        .unwrap_or(0)
        .max(0)
}

/// One generation recorded in a local conversation's `gen_metadata` table.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AntigravityLocalUsage {
    request_uuid: Option<String>,
    response_id: Option<String>,
    raw_model_id: Option<String>,
    tokens: AntigravityTokens,
}

/// Decodes one `gen_metadata.data` blob.
///
/// Field map, validated field by field against the language server's
/// `GetCascadeTrajectoryGeneratorMetadata` response: outer field 4 is the
/// request UUID and field 1 the payload. Inside the payload, field 4 is the
/// usage struct (`.2` `inputTokens`, `.3` `outputTokens`, `.5`
/// `cacheReadTokens`, `.9` `thinkingOutputTokens`, `.10`
/// `responseOutputTokens`, `.11` `responseId`), field 19 the plaintext model
/// name and field 20 repeated `{1: key, 2: value}` metadata pairs.
///
/// Returns `None` for non-generation steps (no total output) and for blobs
/// failing the `thinking + response == total` integrity check, which would
/// mean the field map has drifted.
fn parse_gen_metadata_usage(blob: &[u8]) -> Option<AntigravityLocalUsage> {
    let payload = proto_message(blob, 1)?;
    let usage = proto_message(payload, 4)?;

    let total_output = proto_token_count(usage, 3);
    if total_output == 0 {
        // Non-generation step, not a parse failure.
        return None;
    }
    // Antigravity reports no cache-write tokens in the local blob.
    let tokens = normalize_antigravity_tokens(
        proto_token_count(usage, 2),
        total_output,
        Some(proto_token_count(usage, 10)),
        proto_token_count(usage, 5),
        0,
        proto_token_count(usage, 9),
    )?;

    // `.19` carries a resolved plaintext name but is sometimes a placeholder
    // like `gemini-default`; the `model_enum` pair resolves through the alias
    // table in that case.
    let mut raw_model_id = proto_string(payload, 19);
    if raw_model_id.as_deref().is_none_or(is_pseudo_raw_model) {
        if let Some(model_enum) = proto_custom_metadata(payload, "model_enum") {
            raw_model_id = Some(model_enum);
        }
    }

    Some(AntigravityLocalUsage {
        request_uuid: proto_string(blob, 4),
        response_id: proto_string(usage, 11),
        raw_model_id,
        tokens,
    })
}

/// Looks `key` up in the payload's repeated field 20 `{1: key, 2: value}` pairs.
fn proto_custom_metadata(payload: &[u8], key: &str) -> Option<String> {
    let mut found = None;
    for_each_proto_field(payload, |number, value| {
        if found.is_none() && number == 20 {
            if let ProtoValue::Bytes(pair) = value {
                if proto_string(pair, 1).as_deref() == Some(key) {
                    found = proto_string(pair, 2);
                }
            }
        }
    });
    found
}

/// Maps each step's request UUID (`steps.metadata` field 12) to its wall-clock
/// time (field 1, a protobuf `Timestamp`), so `gen_metadata` records can be
/// dated through their outer field 4.
fn local_step_timestamps(local_db: &rusqlite::Connection) -> HashMap<String, i64> {
    let mut timestamps = HashMap::new();
    let Ok(mut stmt) = local_db.prepare("SELECT metadata FROM steps") else {
        return timestamps;
    };
    let Ok(mut rows) = stmt.query([]) else {
        return timestamps;
    };
    while let Ok(Some(row)) = rows.next() {
        let Ok(Some(metadata)) = row.get::<_, Option<Vec<u8>>>(0) else {
            continue;
        };
        let (Some(request_uuid), Some(timestamp)) =
            (proto_string(&metadata, 12), proto_message(&metadata, 1))
        else {
            continue;
        };
        let Ok(seconds) = i64::try_from(proto_varint(timestamp, 1).unwrap_or(0)) else {
            continue;
        };
        // A protobuf `Timestamp` keeps nanos in [0, 1e9); anything else is not
        // one, and would otherwise overflow the millisecond conversion.
        let nanos = proto_varint(timestamp, 2).unwrap_or(0);
        if nanos >= 1_000_000_000 {
            continue;
        }
        if let Some(millis) = seconds
            .checked_mul(1000)
            .and_then(|ms| ms.checked_add(nanos as i64 / 1_000_000))
        {
            timestamps.insert(request_uuid, millis);
        }
    }
    timestamps
}

/// Reads every generation record out of a local conversation database, paired
/// with the wall-clock time joined from `steps`.
fn read_local_conversation_usage(
    local_db: &rusqlite::Connection,
) -> Vec<(AntigravityLocalUsage, Option<i64>)> {
    let timestamps = local_step_timestamps(local_db);
    let Ok(mut stmt) = local_db.prepare("SELECT data FROM gen_metadata ORDER BY idx") else {
        return Vec::new();
    };
    let Ok(mut rows) = stmt.query([]) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let Ok(Some(blob)) = row.get::<_, Option<Vec<u8>>>(0) else {
            continue;
        };
        let Some(usage) = parse_gen_metadata_usage(&blob) else {
            continue;
        };
        let timestamp = usage
            .request_uuid
            .as_deref()
            .and_then(|uuid| timestamps.get(uuid).copied());
        records.push((usage, timestamp));
    }
    records
}

/// Conversation directories written by the local Antigravity runtimes, paired
/// with the runtime that owns them. `antigravity-ide` and `antigravity-backup`
/// stay out: both hold only encrypted `.pb` files, and `backup` duplicates
/// `ide` session for session.
fn antigravity_local_conversation_dirs(home: &Path) -> [(PathBuf, AntigravityRuntimeKind); 2] {
    let gemini = home.join(".gemini");
    [
        (
            gemini.join("antigravity-cli").join("conversations"),
            AntigravityRuntimeKind::Cli,
        ),
        (
            gemini.join("antigravity").join("conversations"),
            AntigravityRuntimeKind::Desktop,
        ),
    ]
}

/// Newest mtime across a conversation file and, for SQLite conversations, its
/// write-ahead log sidecars — content can land in the WAL without touching the
/// main file.
fn local_conversation_modified_ms(path: &Path) -> Option<i64> {
    let file_modified_ms = |path: &Path| {
        path.metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(system_time_to_millis)
    };
    let mut modified_ms = file_modified_ms(path);
    if path.extension().and_then(|ext| ext.to_str()) == Some("db") {
        for extra_ext in ["db-wal", "db-shm"] {
            if let Some(extra_ms) = file_modified_ms(&path.with_extension(extra_ext)) {
                modified_ms = Some(match modified_ms {
                    Some(current) => current.max(extra_ms),
                    None => extra_ms,
                });
            }
        }
    }
    modified_ms
}

/// Sessions that already hold usage rows written by the current parser version.
fn sessions_with_current_usage(
    db_conn: &rusqlite::Connection,
) -> std::collections::HashSet<(String, String)> {
    let mut parsed = std::collections::HashSet::new();
    let Ok(mut stmt) = db_conn
        .prepare("SELECT DISTINCT client, session_id FROM session_usage WHERE parser_version = ?1")
    else {
        return parsed;
    };
    let Ok(rows) = stmt.query_map([PARSER_VERSION], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) else {
        return parsed;
    };
    parsed.extend(rows.flatten());
    parsed
}

/// Scans the local conversation SQLite databases of every Antigravity runtime
/// and writes both session metadata and token usage into the cache. Encrypted
/// `.pb` conversations carry no readable usage and are left to the
/// language-server RPC path.
fn sync_local_conversations(
    db_conn: &mut rusqlite::Connection,
    home: &Path,
    cached_sessions: &HashMap<(String, String), (Option<i64>, Option<i64>)>,
    rebuild_all: bool,
    model_aliases: &HashMap<String, ModelAlias>,
) -> Result<()> {
    // A session already listed in `sessions` but carrying no usage row from the
    // current parser — an upgrade from a cache built before local parsing
    // existed, or a `PARSER_VERSION` bump — has to be read even though its file
    // has not changed since the last sync.
    let already_parsed = sessions_with_current_usage(db_conn);

    let now_ms = Local::now().timestamp_millis();
    let tx = db_conn.transaction()?;
    let mut synced_sessions = 0usize;
    let mut inserted_usage_rows = 0usize;

    for (dir, runtime_kind) in antigravity_local_conversation_dirs(home) {
        let client_str = client_str_for_runtime_kind(runtime_kind);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("db") {
                continue;
            }
            let Some(session_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if session_id.len() < 20 {
                continue;
            }

            let modified_ms = local_conversation_modified_ms(&path);
            let cache_key = (client_str.to_string(), session_id.to_string());
            if !rebuild_all && already_parsed.contains(&cache_key) {
                if let Some((cached_lm, _)) = cached_sessions.get(&cache_key) {
                    if let (Some(cached), Some(current)) = (cached_lm, modified_ms) {
                        if *cached >= current {
                            continue;
                        }
                    }
                }
            }

            let (trajectory_id, step_count, usage_records) =
                match rusqlite::Connection::open_with_flags(
                    &path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                ) {
                    Ok(local_db) => {
                        let trajectory_id: Option<String> = local_db
                            .query_row(
                                "SELECT trajectory_id FROM trajectory_meta LIMIT 1",
                                [],
                                |row| row.get(0),
                            )
                            .ok();
                        let step_count: Option<i64> = local_db
                            .query_row("SELECT count(*) FROM steps", [], |row| row.get(0))
                            .ok();
                        (
                            trajectory_id,
                            step_count,
                            read_local_conversation_usage(&local_db),
                        )
                    }
                    Err(e) => {
                        debug!(
                            "Failed to open local Antigravity conversation {}: {}",
                            path.display(),
                            e
                        );
                        (None, None, Vec::new())
                    }
                };

            let resolved: Vec<(String, i64, &AntigravityLocalUsage)> = usage_records
                .iter()
                .map(|(usage, timestamp)| {
                    let model_id = usage
                        .raw_model_id
                        .as_deref()
                        .map(|raw| resolve_antigravity_model_id_with_aliases(raw, model_aliases))
                        .unwrap_or_else(|| "unknown".to_string());
                    (model_id, timestamp.or(modified_ms).unwrap_or(now_ms), usage)
                })
                .collect();

            let primary_model_id = resolved
                .iter()
                .map(|(model_id, _, _)| model_id.as_str())
                .find(|model_id| *model_id != "unknown")
                .unwrap_or("unknown")
                .to_string();

            let summary = AntigravitySyncSummary {
                last_modified_ms: modified_ms,
                connections: Vec::new(),
                trajectory_id,
                title: None,
                status: None,
                step_count,
                created_time_ms: None,
                last_user_input_time_ms: None,
                project_id: None,
                workspace_path: None,
                git_root: None,
                repository: None,
                git_origin_url: None,
                branch_name: None,
                parent_conversation_id: None,
                mendel_experiment_ids: None,
            };

            upsert_antigravity_session_row(
                &tx,
                session_id,
                client_str,
                &summary,
                &primary_model_id,
                now_ms,
            )?;

            for (step_index, (model_id, timestamp, usage)) in resolved.iter().enumerate() {
                let raw_message_key = usage
                    .response_id
                    .clone()
                    .unwrap_or_else(|| format!("{}:{}:{}", timestamp, model_id, step_index));
                let message_key = antigravity_logical_message_key(session_id, &raw_message_key);
                let storage_message_id = antigravity_storage_message_id(client_str, &message_key);

                tx.execute(
                    "INSERT OR REPLACE INTO session_usage (
                        id, session_id, client, model_id, provider_id, timestamp, step_index,
                        input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
                        response_id, pricing_day, parser_version
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        storage_message_id,
                        session_id,
                        client_str,
                        model_id,
                        detect_provider_from_model(model_id),
                        timestamp,
                        step_index as i64,
                        usage.tokens.input,
                        usage.tokens.output,
                        usage.tokens.cache_read,
                        usage.tokens.cache_write,
                        usage.tokens.reasoning,
                        message_key,
                        local_date_string_from_timestamp(*timestamp),
                        PARSER_VERSION,
                    ],
                )?;
                inserted_usage_rows += 1;
            }

            synced_sessions += 1;
        }
    }

    tx.commit()?;
    if synced_sessions > 0 {
        info!(
            "Antigravity local sync: {} conversation databases, {} usage rows",
            synced_sessions, inserted_usage_rows
        );
    }
    Ok(())
}

fn antigravity_logical_message_key(session_id: &str, raw_message_key: &str) -> String {
    format!("{session_id}:{raw_message_key}")
}

fn antigravity_storage_message_id(client: &str, logical_message_key: &str) -> String {
    format!("{client}:{logical_message_key}")
}

fn update_opt<T>(dest: &mut Option<T>, src: Option<T>) {
    if dest.is_none() {
        *dest = src;
    }
}

fn summary_from_trajectory_item(
    item: &Value,
    connection: AntigravityConnection,
) -> AntigravitySyncSummary {
    let last_modified_ms = item
        .get("lastModifiedTime")
        .or_else(|| item.get("lastModified"))
        .or_else(|| item.get("updatedAt"))
        .and_then(parse_timestamp_value);
    let workspace = item.get("workspace");
    let mendel_experiment_ids = item.get("mendelExperimentIds").and_then(|v| {
        if let Some(arr) = v.as_array() {
            let ids: Vec<String> = arr
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
            Some(ids.join(","))
        } else {
            v.as_str().map(String::from)
        }
    });

    AntigravitySyncSummary {
        last_modified_ms,
        connections: vec![connection],
        trajectory_id: item
            .get("trajectoryId")
            .and_then(Value::as_str)
            .map(String::from),
        title: item
            .get("summary")
            .and_then(Value::as_str)
            .map(String::from),
        status: item.get("status").and_then(Value::as_str).map(String::from),
        step_count: item.get("stepCount").and_then(Value::as_i64),
        created_time_ms: item.get("createdTime").and_then(parse_timestamp_value),
        last_user_input_time_ms: item
            .get("lastUserInputTime")
            .and_then(parse_timestamp_value),
        project_id: item
            .get("projectId")
            .and_then(Value::as_str)
            .map(String::from),
        workspace_path: workspace
            .and_then(|w| w.get("workspacePath").or_else(|| w.get("path")))
            .and_then(Value::as_str)
            .map(String::from),
        git_root: workspace
            .and_then(|w| w.get("gitRoot"))
            .and_then(Value::as_str)
            .map(String::from),
        repository: workspace
            .and_then(|w| w.get("repository"))
            .and_then(Value::as_str)
            .map(String::from),
        git_origin_url: workspace
            .and_then(|w| w.get("gitOriginUrl"))
            .and_then(Value::as_str)
            .map(String::from),
        branch_name: workspace
            .and_then(|w| w.get("branchName"))
            .and_then(Value::as_str)
            .map(String::from),
        parent_conversation_id: item
            .get("parentConversationId")
            .and_then(Value::as_str)
            .map(String::from),
        mendel_experiment_ids,
    }
}

fn upsert_sync_summary(
    summaries: &mut HashMap<AntigravitySessionCacheKey, AntigravitySyncSummary>,
    cache_key: AntigravitySessionCacheKey,
    data: AntigravitySyncSummary,
) {
    if let Some(summary) = summaries.get_mut(&cache_key) {
        summary.last_modified_ms = newer_timestamp(summary.last_modified_ms, data.last_modified_ms);
        for conn in data.connections {
            push_unique_connection(&mut summary.connections, conn);
        }
        update_opt(&mut summary.trajectory_id, data.trajectory_id);
        update_opt(&mut summary.title, data.title);
        update_opt(&mut summary.status, data.status);
        update_opt(&mut summary.step_count, data.step_count);
        update_opt(&mut summary.created_time_ms, data.created_time_ms);
        update_opt(
            &mut summary.last_user_input_time_ms,
            data.last_user_input_time_ms,
        );
        update_opt(&mut summary.project_id, data.project_id);
        update_opt(&mut summary.workspace_path, data.workspace_path);
        update_opt(&mut summary.git_root, data.git_root);
        update_opt(&mut summary.repository, data.repository);
        update_opt(&mut summary.git_origin_url, data.git_origin_url);
        update_opt(&mut summary.branch_name, data.branch_name);
        update_opt(
            &mut summary.parent_conversation_id,
            data.parent_conversation_id,
        );
        update_opt(
            &mut summary.mendel_experiment_ids,
            data.mendel_experiment_ids,
        );
    } else {
        summaries.insert(cache_key, data);
    }
}

struct AntigravitySyncSummary {
    last_modified_ms: Option<i64>,
    connections: Vec<AntigravityConnection>,
    trajectory_id: Option<String>,
    title: Option<String>,
    status: Option<String>,
    step_count: Option<i64>,
    created_time_ms: Option<i64>,
    last_user_input_time_ms: Option<i64>,
    project_id: Option<String>,
    workspace_path: Option<String>,
    git_root: Option<String>,
    repository: Option<String>,
    git_origin_url: Option<String>,
    branch_name: Option<String>,
    parent_conversation_id: Option<String>,
    mendel_experiment_ids: Option<String>,
}

fn push_unique_connection(
    connections: &mut Vec<AntigravityConnection>,
    conn: AntigravityConnection,
) {
    if !connections
        .iter()
        .any(|c| c.pid == conn.pid && c.port == conn.port)
    {
        connections.push(conn);
    }
}

fn prioritize_connections_for_runtime(
    connections: &mut Vec<AntigravityConnection>,
    all_connections: &[AntigravityConnection],
    runtime_kind: AntigravityRuntimeKind,
) {
    let mut ordered = Vec::new();
    for conn in all_connections
        .iter()
        .filter(|conn| conn.runtime_kind == runtime_kind)
    {
        push_unique_connection(&mut ordered, conn.clone());
    }
    for conn in connections.drain(..) {
        if conn.runtime_kind == runtime_kind {
            push_unique_connection(&mut ordered, conn);
        }
    }
    *connections = ordered;
}

fn newer_timestamp(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelAliasHistory {
    version: u32,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    aliases: BTreeMap<String, ModelAliasHistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelAliasHistoryEntry {
    #[serde(rename = "rawModelId")]
    raw_model_id: String,
    #[serde(rename = "modelId")]
    model_id: String,
    label: Option<String>,
    source: String,
    #[serde(rename = "firstSeenAt")]
    first_seen_at: String,
    #[serde(rename = "lastSeenAt")]
    last_seen_at: String,
}

#[derive(Debug, Clone)]
struct ModelAlias {
    raw_model_id: String,
    model_id: String,
    label: Option<String>,
    source: String,
}

struct StaticModelAlias {
    raw_model_id: &'static str,
    model_id: &'static str,
    label: Option<&'static str>,
    source: &'static str,
}

const STATIC_MODEL_ALIASES: &[StaticModelAlias] = &[
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M37",
        model_id: "gemini-3.1-pro-preview-high",
        label: Some("Gemini 3.1 Pro (High)"),
        source: "user-initial-mapping;tokscale",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M36",
        model_id: "gemini-3.1-pro-preview-low",
        label: Some("Gemini 3.1 Pro (Low)"),
        source: "user-initial-mapping;tokscale",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M18",
        model_id: "gemini-3-flash-preview",
        label: Some("Gemini 3 Flash"),
        source: "user-initial-mapping",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M8",
        model_id: "gemini-3-pro-preview-high",
        label: Some("Gemini 3 Pro (High)"),
        source: "user-initial-mapping",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M7",
        model_id: "gemini-3-pro-preview-low",
        label: Some("Gemini 3 Pro (Low)"),
        source: "user-initial-mapping",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M9",
        model_id: "gemini-3-pro-preview-image",
        label: Some("Gemini 3 Pro (Image)"),
        source: "user-initial-mapping",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M26",
        model_id: "claude-opus-4-6-thinking",
        label: Some("Claude Opus 4.6 (Thinking)"),
        source: "user-initial-mapping;openusage",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M35",
        model_id: "claude-sonnet-4-6-thinking",
        label: Some("Claude Sonnet 4.6 (Thinking)"),
        source: "user-initial-mapping;antigravity-mobility-cli",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M12",
        model_id: "claude-opus-4-5-thinking",
        label: Some("Claude Opus 4.5 (Thinking)"),
        source: "user-initial-mapping",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_OPENAI_GPT_OSS_120B_MEDIUM",
        model_id: "gpt-oss-120b-medium",
        label: Some("GPT-OSS 120B (Medium)"),
        source: "user-initial-mapping;tokscale",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_CLAUDE_4_5_SONNET",
        model_id: "claude-sonnet-4-5",
        label: Some("Claude Sonnet 4.5"),
        source: "user-initial-mapping",
    },
    // Additional placeholders from dynamic JSON:
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M132",
        model_id: "gemini-3.5-flash-high",
        label: Some("Gemini 3.5 Flash (High)"),
        source: "captured-antigravity-get-user-status",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M16",
        model_id: "gemini-3.1-pro-preview-high",
        label: Some("Gemini 3.1 Pro (High)"),
        source: "captured-antigravity-get-user-status",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M187",
        model_id: "gemini-3.5-flash-low",
        label: Some("Gemini 3.5 Flash (Low)"),
        source: "captured-antigravity-get-user-status",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M20",
        model_id: "gemini-3.5-flash-medium",
        label: Some("Gemini 3.5 Flash (Medium)"),
        source: "captured-antigravity-get-user-status",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M47",
        model_id: "gemini-3-flash-preview",
        label: Some("Gemini 3 Flash"),
        source: "tokscale;antigravity-mobility-cli",
    },
];

fn static_model_aliases() -> HashMap<String, ModelAlias> {
    let mut aliases = HashMap::new();
    for alias in STATIC_MODEL_ALIASES {
        aliases.insert(
            normalize_alias_key(alias.raw_model_id),
            ModelAlias {
                raw_model_id: alias.raw_model_id.to_string(),
                model_id: alias.model_id.to_string(),
                label: alias.label.map(str::to_string),
                source: alias.source.to_string(),
            },
        );
    }
    aliases
}

async fn fetch_dynamic_model_aliases(
    client: &reqwest::Client,
    connections: &[AntigravityConnection],
) -> HashMap<String, ModelAlias> {
    let mut aliases = HashMap::new();

    for connection in connections {
        let response = match rpc_request(
            client,
            connection,
            "GetUserStatus",
            &json!({
                "metadata": {
                    "ideName": "antigravity",
                    "extensionName": "antigravity",
                    "ideVersion": "unknown",
                    "locale": "en",
                }
            }),
        )
        .await
        {
            Ok(response) => response,
            Err(e) => {
                debug!(
                    "Failed to fetch Antigravity model labels from port {}: {}",
                    connection.port, e
                );
                continue;
            }
        };

        collect_model_aliases_from_value(&response, &mut aliases);
    }

    aliases
}

fn collect_model_aliases_from_value(value: &Value, aliases: &mut HashMap<String, ModelAlias>) {
    if let Some(configs) = value.get("clientModelConfigs").and_then(Value::as_array) {
        collect_model_aliases_from_configs(configs, aliases);
    }

    if let Some(configs) = value
        .get("cascadeModelConfigData")
        .and_then(|data| data.get("clientModelConfigs"))
        .and_then(Value::as_array)
    {
        collect_model_aliases_from_configs(configs, aliases);
    }

    if let Some(configs) = value
        .get("userStatus")
        .and_then(|status| status.get("cascadeModelConfigData"))
        .and_then(|data| data.get("clientModelConfigs"))
        .and_then(Value::as_array)
    {
        collect_model_aliases_from_configs(configs, aliases);
    }
}

fn collect_model_aliases_from_configs(
    configs: &[Value],
    aliases: &mut HashMap<String, ModelAlias>,
) {
    for config in configs {
        let Some(raw_model) = config
            .get("modelOrAlias")
            .and_then(|model| model.get("model"))
            .and_then(Value::as_str)
            .filter(|model| !model.trim().is_empty())
            .filter(|model| model.to_ascii_lowercase().starts_with("model"))
        else {
            continue;
        };
        let Some(label) = config
            .get("label")
            .and_then(Value::as_str)
            .filter(|label| !label.trim().is_empty())
        else {
            continue;
        };
        let Some(model_id) = antigravity_label_to_model_id(label) else {
            continue;
        };
        aliases.insert(
            normalize_alias_key(raw_model),
            ModelAlias {
                raw_model_id: raw_model.to_string(),
                model_id,
                label: Some(label.to_string()),
                source: "antigravity-get-user-status".to_string(),
            },
        );
    }
}

fn model_alias_history_path(sessions_dir: &Path) -> Option<PathBuf> {
    let dir = if sessions_dir
        .file_name()
        .map_or(false, |name| name == "sessions")
    {
        sessions_dir.parent()?
    } else {
        sessions_dir
    };
    Some(dir.join(MODEL_ALIAS_HISTORY_FILE_NAME))
}

fn load_model_alias_history_map(sessions_dir: &Path) -> Result<HashMap<String, ModelAlias>> {
    let Some(path) = model_alias_history_path(sessions_dir) else {
        return Ok(static_model_aliases());
    };
    if !path.exists() {
        return Ok(static_model_aliases());
    }

    let content = std::fs::read_to_string(&path)?;
    let history: ModelAliasHistory = serde_json::from_str(&content)?;
    let mut aliases = static_model_aliases();
    aliases.extend(
        history
            .aliases
            .into_iter()
            .filter(|(key, entry)| {
                key.to_ascii_lowercase().starts_with("model")
                    && entry.raw_model_id.to_ascii_lowercase().starts_with("model")
            })
            .map(|(key, entry)| {
                (
                    key,
                    ModelAlias {
                        raw_model_id: entry.raw_model_id,
                        model_id: entry.model_id,
                        label: entry.label,
                        source: entry.source,
                    },
                )
            }),
    );
    Ok(aliases)
}

fn merge_and_save_model_alias_history(
    sessions_dir: &Path,
    dynamic_aliases: &HashMap<String, ModelAlias>,
) -> Result<HashMap<String, ModelAlias>> {
    let now = chrono::Utc::now().to_rfc3339();
    let Some(path) = model_alias_history_path(sessions_dir) else {
        return Ok(dynamic_aliases.clone());
    };

    let mut history = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let mut loaded = serde_json::from_str::<ModelAliasHistory>(&content).unwrap_or_else(|_| {
            ModelAliasHistory {
                version: MODEL_ALIAS_HISTORY_VERSION,
                updated_at: now.clone(),
                aliases: BTreeMap::new(),
            }
        });
        loaded.aliases.retain(|key, entry| {
            key.to_ascii_lowercase().starts_with("model")
                && entry.raw_model_id.to_ascii_lowercase().starts_with("model")
        });
        loaded
    } else {
        ModelAliasHistory {
            version: MODEL_ALIAS_HISTORY_VERSION,
            updated_at: now.clone(),
            aliases: BTreeMap::new(),
        }
    };

    history.version = MODEL_ALIAS_HISTORY_VERSION;
    history.updated_at = now.clone();

    let mut merged_aliases = static_model_aliases();
    for (key, entry) in &history.aliases {
        merged_aliases.insert(
            key.clone(),
            ModelAlias {
                raw_model_id: entry.raw_model_id.clone(),
                model_id: entry.model_id.clone(),
                label: entry.label.clone(),
                source: entry.source.clone(),
            },
        );
    }
    for (key, alias) in dynamic_aliases {
        merged_aliases.insert(key.clone(), alias.clone());
    }

    for (key, alias) in &merged_aliases {
        if !alias.raw_model_id.to_ascii_lowercase().starts_with("model") {
            continue;
        }
        let entry = history
            .aliases
            .entry(key.clone())
            .or_insert_with(|| ModelAliasHistoryEntry {
                raw_model_id: alias.raw_model_id.clone(),
                model_id: alias.model_id.clone(),
                label: alias.label.clone(),
                source: alias.source.clone(),
                first_seen_at: now.clone(),
                last_seen_at: now.clone(),
            });
        entry.raw_model_id = alias.raw_model_id.clone();
        entry.model_id = alias.model_id.clone();
        entry.label = alias.label.clone();
        entry.source = alias.source.clone();
        entry.last_seen_at = now.clone();
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&history)?),
    )?;

    Ok(merged_aliases)
}

fn normalized_paths() -> &'static std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>> {
    static PATHS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    > = std::sync::OnceLock::new();
    PATHS.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

fn normalize_cached_antigravity_artifacts(
    sessions_dir: &Path,
    model_aliases: &HashMap<String, ModelAlias>,
) -> Result<()> {
    let canonical = sessions_dir
        .canonicalize()
        .unwrap_or_else(|_| sessions_dir.to_path_buf());
    {
        let paths = normalized_paths()
            .lock()
            .map_err(|_| anyhow::anyhow!("Normalized paths mutex poisoned"))?;
        if paths.contains(&canonical) {
            return Ok(());
        }
    }

    let mut conn = open_cache_db(sessions_dir)?;

    let mut all_aliases = static_model_aliases();
    for (k, v) in model_aliases {
        all_aliases.insert(k.clone(), v.clone());
    }

    let db_models = {
        let mut db_models = std::collections::HashSet::new();
        if let Ok(mut stmt) = conn.prepare("SELECT DISTINCT model_id FROM sessions") {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let Ok(m) = row.get::<_, String>(0) {
                        db_models.insert(m.to_lowercase());
                    }
                }
            }
        }
        if let Ok(mut stmt) = conn.prepare("SELECT DISTINCT model_id FROM session_usage") {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let Ok(m) = row.get::<_, String>(0) {
                        db_models.insert(m.to_lowercase());
                    }
                }
            }
        }
        db_models
    };

    let tx = conn.transaction()?;

    for (alias_key, alias) in &all_aliases {
        let key_lower = alias_key.to_lowercase();
        let raw_lower = alias.raw_model_id.to_lowercase();
        if db_models.contains(&key_lower) || db_models.contains(&raw_lower) {
            let target_model_id =
                if let Some(normalized) = format_normalization_fallback(&alias.model_id) {
                    normalized
                } else {
                    alias.model_id.clone()
                };
            tx.execute(
                "UPDATE sessions SET model_id = ? WHERE model_id = ? OR LOWER(model_id) = ?;",
                rusqlite::params![&target_model_id, &alias.raw_model_id, alias_key],
            )?;
            tx.execute(
                "UPDATE session_usage SET model_id = ? WHERE model_id = ? OR LOWER(model_id) = ?;",
                rusqlite::params![&target_model_id, &alias.raw_model_id, alias_key],
            )?;
        }
    }

    for db_model in &db_models {
        if let Some(normalized) = format_normalization_fallback(db_model) {
            tx.execute(
                "UPDATE sessions SET model_id = ? WHERE model_id = ? OR LOWER(model_id) = ?;",
                rusqlite::params![&normalized, db_model, db_model.to_lowercase()],
            )?;
            tx.execute(
                "UPDATE session_usage SET model_id = ? WHERE model_id = ? OR LOWER(model_id) = ?;",
                rusqlite::params![&normalized, db_model, db_model.to_lowercase()],
            )?;
        }
    }

    tx.commit()?;

    {
        let mut paths = normalized_paths()
            .lock()
            .map_err(|_| anyhow::anyhow!("Normalized paths mutex poisoned"))?;
        paths.insert(canonical);
    }

    Ok(())
}

fn is_pseudo_raw_model(model: &str) -> bool {
    crate::model_id::is_pseudo(model)
}

fn normalize_display_name_to_id(display_name: &str) -> Option<String> {
    let trimmed = display_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_lowercase();
    if !lower.starts_with("gemini ")
        && !lower.starts_with("claude ")
        && !lower.starts_with("gpt-oss ")
    {
        return None;
    }

    let replaced_parens = lower.replace('(', " ").replace(')', " ");
    let replaced_spaces = replaced_parens.replace(' ', "-");

    let mut normalized = String::new();
    let mut last_was_dash = false;
    for c in replaced_spaces.chars() {
        if c == '-' {
            if !last_was_dash {
                normalized.push('-');
                last_was_dash = true;
            }
        } else {
            normalized.push(c);
            last_was_dash = false;
        }
    }

    let trimmed_normalized = normalized.trim_matches('-');
    if trimmed_normalized.is_empty() {
        None
    } else {
        Some(trimmed_normalized.to_string())
    }
}

#[cfg(test)]
fn resolve_antigravity_model_id(model_id: &str) -> String {
    resolve_antigravity_model_id_with_aliases(model_id, &HashMap::new())
}

fn resolve_antigravity_model_id_with_aliases(
    model_id: &str,
    dynamic_aliases: &HashMap<String, ModelAlias>,
) -> String {
    let resolved = if let Some(alias) = resolve_direct(model_id, dynamic_aliases) {
        alias
    } else if let Some(normalized) = format_normalization_fallback(model_id) {
        if let Some(alias) = resolve_direct(&normalized, dynamic_aliases) {
            alias
        } else {
            normalized
        }
    } else {
        model_id.to_string()
    };

    if let Some(normalized_resolved) = format_normalization_fallback(&resolved) {
        normalized_resolved
    } else {
        resolved
    }
}

fn resolve_direct(model_id: &str, dynamic_aliases: &HashMap<String, ModelAlias>) -> Option<String> {
    alias_key_candidates(model_id)
        .iter()
        .find_map(|key| dynamic_aliases.get(key).map(|alias| alias.model_id.clone()))
        .or_else(|| antigravity_model_alias(model_id).map(|s| s.to_string()))
}

fn format_normalization_fallback(model_id: &str) -> Option<String> {
    let mut normalized = model_id.trim().to_ascii_lowercase();

    // 0. Handle raw gemini-3-flash-a mapping
    let mut flash_a_normalized = false;
    if normalized == "gemini-3-flash-a"
        || normalized == "antigravity-gemini-3-flash-a"
        || normalized == "gemini-3-flash-preview-a"
    {
        normalized = "gemini-3.5-flash".to_string();
        flash_a_normalized = true;
    }

    // 1. Strip antigravity- or anti-gravity- prefix
    let mut prefix_stripped = false;
    if !flash_a_normalized {
        for prefix in ["antigravity-", "anti-gravity-"] {
            if let Some(stripped) = normalized.strip_prefix(prefix) {
                normalized = stripped.to_string();
                prefix_stripped = true;
                break;
            }
        }
    }

    // 2. Replace dots, underscores, and spaces with hyphens
    let mut chars_replaced = false;
    if !flash_a_normalized
        && (normalized.contains('.') || normalized.contains('_') || normalized.contains(' '))
    {
        normalized = normalized.replace(['.', '_', ' '], "-");
        chars_replaced = true;
    }

    // 3. Collapse repeated hyphens
    if !flash_a_normalized && normalized.contains("--") {
        let mut out = String::with_capacity(normalized.len());
        let mut last_was_hyphen = false;
        for ch in normalized.chars() {
            if ch == '-' {
                if !last_was_hyphen {
                    out.push(ch);
                }
                last_was_hyphen = true;
            } else {
                out.push(ch);
                last_was_hyphen = false;
            }
        }
        normalized = out;
        chars_replaced = true;
    }

    // 4. Strip a trailing `-tiered` routing suffix, e.g.
    // gemini-3.6-flash-tiered -> gemini-3-6-flash. Antigravity uses it for
    // sub-agent routing, so it must not become a separate model family.
    let mut tiered_stripped = false;
    if !flash_a_normalized && normalized.ends_with("-tiered") {
        normalized.truncate(normalized.len() - "-tiered".len());
        tiered_stripped = true;
    }

    // 5. Version formatting rule (converting hyphens back to dots for Gemini versions):
    // e.g. gemini-3-0 -> gemini-3, gemini-3-1 -> gemini-3.1, gemini-3-5 -> gemini-3.5
    let mut version_formatted = false;
    if !flash_a_normalized {
        if normalized.contains("gemini-3-0-pro") {
            normalized = normalized.replace("gemini-3-0-pro", "gemini-3-pro");
            version_formatted = true;
        } else if normalized.contains("gemini-3-1-pro") {
            normalized = normalized.replace("gemini-3-1-pro", "gemini-3.1-pro");
            version_formatted = true;
        } else if normalized.contains("gemini-3-0-flash") {
            normalized = normalized.replace("gemini-3-0-flash", "gemini-3-flash");
            version_formatted = true;
        } else if normalized.contains("gemini-3-5-flash") {
            normalized = normalized.replace("gemini-3-5-flash", "gemini-3.5-flash");
            version_formatted = true;
        }
    }

    // 6. Preview ruleset logic:
    // Models in this list must have "-preview".
    // Models not in this list must NOT have "-preview".
    let mut preview_processed = false;
    if !flash_a_normalized && normalized.contains("gemini") {
        const GEMINI_PREVIEW_MODELS: &[&str] =
            &["gemini-3-pro", "gemini-3.1-pro", "gemini-3-flash"];

        let belongs_to_preview = GEMINI_PREVIEW_MODELS
            .iter()
            .any(|&m| normalized.contains(m));

        if belongs_to_preview {
            if !normalized.contains("preview") {
                for base in GEMINI_PREVIEW_MODELS {
                    if normalized.contains(base) {
                        let target = format!("{}-preview", base);
                        normalized = normalized.replace(base, &target);
                        preview_processed = true;
                        break;
                    }
                }
            }
        } else {
            // Must NOT have preview
            if normalized.contains("-preview") {
                normalized = normalized.replace("-preview", "");
                preview_processed = true;
            }
        }
    }

    if prefix_stripped
        || chars_replaced
        || tiered_stripped
        || version_formatted
        || preview_processed
        || flash_a_normalized
    {
        Some(normalized)
    } else {
        None
    }
}

fn normalize_alias_key(model_id: &str) -> String {
    model_id.trim().to_ascii_lowercase()
}

fn legacy_normalize_alias_key(model_id: &str) -> String {
    normalize_alias_key(model_id).replace('-', "_")
}

fn alias_key_candidates(model_id: &str) -> Vec<String> {
    let key = normalize_alias_key(model_id);
    let legacy_key = legacy_normalize_alias_key(model_id);
    if legacy_key == key {
        vec![key]
    } else {
        vec![key, legacy_key]
    }
}

fn antigravity_label_to_model_id(label: &str) -> Option<String> {
    let cleaned = label.trim();
    if cleaned.is_empty() {
        return None;
    }

    let lower = cleaned.to_ascii_lowercase();
    if !(lower.starts_with("claude ")
        || lower.starts_with("gemini ")
        || lower.starts_with("gpt-oss "))
    {
        return None;
    }

    let normalized = lower.replace('(', " ").replace(')', " ");
    let tokens = normalized
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(|token| token.replace('.', "-"))
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    let model_id = tokens.join("-");
    if let Some(normalized) = format_normalization_fallback(&model_id) {
        Some(normalized)
    } else {
        Some(model_id)
    }
}

fn antigravity_model_alias(model_id: &str) -> Option<&'static str> {
    let keys = alias_key_candidates(model_id);
    STATIC_MODEL_ALIASES
        .iter()
        .find(|alias| {
            let alias_keys = alias_key_candidates(alias.raw_model_id);
            keys.iter().any(|key| alias_keys.contains(key))
        })
        .map(|alias| alias.model_id)
}

/// Extracts trajectory entries from the RPC response.
/// Supports these formats:
/// 1. `{ "trajectorySummaries": [ ... ] }` — array of objects
/// 2. `{ "trajectorySummaries": { "id1": {...}, "id2": {...} } }` — object map (keys = cascade IDs)
/// 3. `{ "cascadeId1": { ... }, "cascadeId2": { ... } }` — flat object map at root level
fn extract_trajectory_entries(response: &Value) -> Vec<(String, Value)> {
    let known_keys = ["trajectorySummaries", "cascadeTrajectories"];

    // Try known container keys first
    for key in &known_keys {
        if let Some(container) = response.get(*key) {
            // Container is an array
            if let Some(arr) = container.as_array() {
                return arr
                    .iter()
                    .map(|item| (String::new(), item.clone()))
                    .collect();
            }
            // Container is an object map (keys = cascade IDs)
            if let Some(obj) = container.as_object() {
                return obj
                    .iter()
                    .filter(|(_, v)| v.is_object())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
            }
        }
    }

    // Fall back to flat object map at root level
    if let Some(obj) = response.as_object() {
        let entries: Vec<_> = obj
            .iter()
            .filter(|(k, v)| v.is_object() && !known_keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !entries.is_empty() {
            return entries;
        }
    }

    Vec::new()
}

#[cfg(test)]
fn sanitize_session_id(session_id: &str) -> String {
    let sanitized: String = session_id
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();

    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
fn session_artifact_file_stem(session_id: &str) -> String {
    let sanitized = sanitize_session_id(session_id);
    let hash = stable_fnv1a_64(session_id);
    format!("{}-{:016x}", sanitized, hash)
}

#[cfg(test)]
fn stable_fnv1a_64(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn to_safe_i64(value: Option<&Value>) -> i64 {
    value
        .and_then(|inner| {
            inner
                .as_i64()
                .or_else(|| inner.as_u64().and_then(|number| i64::try_from(number).ok()))
                .or_else(|| inner.as_str().and_then(|text| text.parse::<i64>().ok()))
        })
        .unwrap_or(0)
        .max(0)
}

fn parse_timestamp_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| {
            value.as_str().and_then(|text| {
                text.parse::<i64>().ok().or_else(|| {
                    chrono::DateTime::parse_from_rfc3339(text)
                        .ok()
                        .map(|datetime| datetime.timestamp_millis())
                })
            })
        })
        .filter(|timestamp| *timestamp > 0)
}

pub async fn detect_antigravity_connections() -> Result<Vec<AntigravityConnection>> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()?;
    detect_antigravity_connections_with_client(&client).await
}

pub async fn detect_antigravity_connections_with_client(
    client: &reqwest::Client,
) -> Result<Vec<AntigravityConnection>> {
    let candidates = detect_process_candidates()?;
    let mut connections = Vec::new();

    for candidate in candidates {
        let ports = candidate_probe_ports(&candidate, find_listening_ports(candidate.pid)?);
        for port in ports {
            if let Some(scheme) =
                probe_heartbeat(client, port, candidate.csrf_token.as_deref()).await
            {
                connections.push(AntigravityConnection {
                    pid: candidate.pid,
                    port,
                    csrf_token: candidate.csrf_token.clone(),
                    scheme: scheme.to_string(),
                    fingerprint: format!("pid:{}:{}:{}", candidate.pid, scheme, port),
                    runtime_kind: candidate.runtime_kind,
                });
                break;
            }
        }
    }

    connections.sort_by(|left, right| {
        right
            .pid
            .cmp(&left.pid)
            .then_with(|| left.port.cmp(&right.port))
    });
    connections.dedup_by(|left, right| left.pid == right.pid && left.port == right.port);

    Ok(connections)
}

fn candidate_probe_ports(candidate: &ProcessCandidate, mut ports: Vec<u16>) -> Vec<u16> {
    if let Some(declared_port) = candidate.declared_port {
        if !ports.contains(&declared_port) {
            ports.push(declared_port);
        }
    }

    ports.sort_unstable();
    ports.dedup();
    ports
}

fn detect_process_candidates() -> Result<Vec<ProcessCandidate>> {
    let output = run_command("ps", &["-ww", "-eo", "pid,ppid,args"])?;
    let mut candidates = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let Ok(pid) = parts[0].parse::<u32>() else {
            continue;
        };
        let Ok(ppid) = parts[1].parse::<u32>() else {
            continue;
        };
        let command = parts[2..].join(" ");
        if !is_antigravity_process(&command) {
            continue;
        }

        let exe_path = process_executable_path(pid);
        let exe_ok = exe_path
            .as_ref()
            .map(|path| {
                let lower = path.to_string_lossy().to_lowercase();
                lower.contains("antigravity")
                    || lower.contains("language_server")
                    || path_basename_is(path, "agy")
            })
            .unwrap_or(true);
        if !exe_ok {
            continue;
        }

        let csrf_token = extract_csrf_token(&command);
        let declared_port = extract_declared_port(&command);
        let runtime_kind = infer_antigravity_runtime_kind(&command, exe_path.as_deref());

        candidates.push(ProcessCandidate {
            pid,
            ppid,
            declared_port,
            csrf_token,
            runtime_kind,
        });
    }

    candidates.sort_by(|left, right| {
        right
            .pid
            .cmp(&left.pid)
            .then_with(|| right.ppid.cmp(&left.ppid))
            .then_with(|| right.declared_port.cmp(&left.declared_port))
    });
    candidates.dedup_by(|left, right| left.pid == right.pid);

    Ok(candidates)
}

fn is_antigravity_process(command: &str) -> bool {
    let lower = command.to_lowercase();
    let first_token = command.split_whitespace().next().unwrap_or("");
    (lower.contains("language_server")
        && (lower.contains("antigravity")
            || lower.contains("--app_data_dir") && lower.contains("antigravity")))
        || lower.contains("/antigravity/")
        || lower.contains("\\antigravity\\")
        || lower.contains("/antigravity-cli/")
        || lower.contains("\\antigravity-cli\\")
        || path_basename_is(Path::new(first_token), "agy")
}

fn infer_antigravity_runtime_kind(
    command: &str,
    exe_path: Option<&Path>,
) -> AntigravityRuntimeKind {
    let lower = command.to_lowercase();
    let exe_lower = exe_path
        .map(|path| path.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let first_token = command.split_whitespace().next().unwrap_or("");

    if lower.contains("antigravity-cli")
        || exe_lower.contains("antigravity-cli")
        || path_basename_is(Path::new(first_token), "agy")
        || exe_path.map_or(false, |path| path_basename_is(path, "agy"))
        || extract_flag_value(command, "--app_data_dir").as_deref() == Some("antigravity-cli")
    {
        AntigravityRuntimeKind::Cli
    } else if lower.contains("/antigravity/")
        || lower.contains("\\antigravity\\")
        || lower.contains("antigravity.app")
        || exe_lower.contains("antigravity.app")
        || extract_flag_value(command, "--app_data_dir").as_deref() == Some("antigravity")
    {
        AntigravityRuntimeKind::Desktop
    } else {
        AntigravityRuntimeKind::Unknown
    }
}

fn path_basename_is(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

fn discover_local_conversation_ids(
    active_kinds: &[AntigravityRuntimeKind],
) -> Vec<LocalConversationId> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    discover_local_conversation_ids_from_home(&home, active_kinds)
}

fn discover_local_conversation_ids_from_home(
    home: &Path,
    active_kinds: &[AntigravityRuntimeKind],
) -> Vec<LocalConversationId> {
    let mut session_ids = Vec::new();
    for (dir, runtime_kind) in antigravity_local_conversation_dirs(home) {
        if !active_kinds.is_empty() && !active_kinds.contains(&runtime_kind) {
            continue;
        }
        if !dir.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext == "pb" || ext == "db" {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                if stem.len() >= 20 {
                                    let modified_ms = local_conversation_modified_ms(&path);
                                    session_ids.push(LocalConversationId {
                                        session_id: stem.to_string(),
                                        modified_ms,
                                        runtime_kind,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    session_ids.sort_by(|left, right| {
        left.session_id
            .cmp(&right.session_id)
            .then_with(|| left.modified_ms.cmp(&right.modified_ms))
    });
    session_ids.dedup_by(|left, right| {
        left.session_id == right.session_id && left.runtime_kind == right.runtime_kind
    });
    session_ids
}

fn system_time_to_millis(time: std::time::SystemTime) -> Option<i64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

#[cfg(target_os = "macos")]
fn process_executable_path(pid: u32) -> Option<PathBuf> {
    let pid_str = pid.to_string();
    let output = run_command("lsof", &["-p", &pid_str, "-Fn"]).ok()?;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix('n') {
            if rest.contains(".app/Contents/MacOS/") {
                return Some(PathBuf::from(rest));
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn process_executable_path(_pid: u32) -> Option<PathBuf> {
    None
}

fn extract_csrf_token(command: &str) -> Option<String> {
    let token = extract_flag_value(command, "--csrf_token")?;
    if token.len() >= 32 && token.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-') {
        Some(token)
    } else {
        None
    }
}

fn extract_declared_port(command: &str) -> Option<u16> {
    extract_flag_value(command, "--extension_server_port")?
        .parse::<u16>()
        .ok()
}

fn extract_flag_value(command: &str, flag: &str) -> Option<String> {
    let compact = format!("{}=", flag);
    for token in command.split_whitespace() {
        if let Some(value) = token.strip_prefix(&compact) {
            return Some(unquote_flag_value(value));
        }
    }

    let mut tokens = command.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == flag {
            return tokens.next().map(unquote_flag_value);
        }
    }

    None
}

fn unquote_flag_value(value: &str) -> String {
    value
        .trim()
        .trim_matches(|ch| ch == '\'' || ch == '"')
        .to_string()
}

fn find_listening_ports(pid: u32) -> Result<Vec<u16>> {
    let pid_str = pid.to_string();
    let mut ports = run_port_query("lsof", &["-Pan", "-p", &pid_str, "-iTCP", "-sTCP:LISTEN"])?;

    if ports.is_empty() {
        ports = run_port_query("lsof", &["-Pan", "-p", &pid_str, "-i"])?;
    }

    ports.sort_unstable();
    ports.dedup();
    Ok(ports)
}

fn run_port_query(program: &str, args: &[&str]) -> Result<Vec<u16>> {
    match run_command(program, args) {
        Ok(output) => Ok(parse_ports(&output)),
        Err(_) => Ok(Vec::new()),
    }
}

fn parse_ports(output: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in output.lines() {
        if let Some(port) = parse_port_from_line(line) {
            ports.push(port);
        }
    }
    ports
}

fn parse_port_from_line(line: &str) -> Option<u16> {
    for token in line.split_whitespace() {
        if let Some(port) = token
            .strip_prefix("127.0.0.1:")
            .or_else(|| token.strip_prefix("localhost:"))
            .or_else(|| token.strip_prefix("*:"))
            .or_else(|| token.strip_prefix("::1:"))
        {
            let cleaned = port.trim_end_matches("(LISTEN)").trim_end_matches(',');
            if let Ok(parsed) = cleaned.parse::<u16>() {
                return Some(parsed);
            }
        }
    }

    if let Some(idx) = line.rfind(':') {
        let rest = line[idx + 1..].trim();
        let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return digits.parse::<u16>().ok();
        }
    }

    None
}

async fn probe_heartbeat(
    client: &reqwest::Client,
    port: u16,
    csrf_token: Option<&str>,
) -> Option<&'static str> {
    // Only probe https (pure h2) per user decision
    for scheme in ["https"] {
        if probe_heartbeat_with_scheme(client, scheme, port, csrf_token).await {
            return Some(scheme);
        }
    }
    None
}

async fn probe_heartbeat_with_scheme(
    client: &reqwest::Client,
    scheme: &'static str,
    port: u16,
    csrf_token: Option<&str>,
) -> bool {
    let body = json!({ "uuid": "00000000-0000-0000-0000-000000000000" });
    let Ok((status, text)) = language_server_request_text(
        client,
        scheme,
        port,
        csrf_token,
        "Heartbeat",
        &body,
        Duration::from_secs(1),
        ANTIGRAVITY_RPC_BODY_CAP,
    )
    .await
    else {
        return false;
    };

    status == 200
        && heartbeat_response_looks_well_formed(&text)
        && probe_endpoint_identity(client, scheme, port, csrf_token).await
}

fn heartbeat_response_looks_well_formed(body: &str) -> bool {
    let trimmed = body.trim_start();
    let json_start = trimmed.find(['{', '[']).map(|idx| &trimmed[idx..]);
    let Some(slice) = json_start else {
        return false;
    };
    serde_json::from_str::<Value>(slice).is_ok()
}

async fn probe_endpoint_identity(
    client: &reqwest::Client,
    scheme: &'static str,
    port: u16,
    csrf_token: Option<&str>,
) -> bool {
    for method in [
        "GetCascadeTrajectoryGeneratorMetadata",
        "GetAllCascadeTrajectories",
    ] {
        if let Some(body) = identity_probe_request(client, scheme, port, csrf_token, method).await {
            if response_contains_antigravity_marker(method, &body) {
                return true;
            }
        }
    }
    false
}

async fn identity_probe_request(
    client: &reqwest::Client,
    scheme: &'static str,
    port: u16,
    csrf_token: Option<&str>,
    method: &str,
) -> Option<String> {
    let (status, text) = language_server_request_text(
        client,
        scheme,
        port,
        csrf_token,
        method,
        &json!({}),
        Duration::from_secs(1),
        ANTIGRAVITY_RPC_BODY_CAP,
    )
    .await
    .ok()?;

    (status == 200 || (status == 500 && text.contains("trajectory not found"))).then_some(text)
}

fn response_contains_antigravity_marker(method: &str, body: &str) -> bool {
    let trimmed = body.trim_start();
    let json_start = trimmed.find(['{', '[']);
    let Some(idx) = json_start else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&trimmed[idx..]) else {
        return prefix_contains_antigravity_marker(&trimmed[idx..]);
    };
    if method == "GetAllCascadeTrajectories" {
        if value.is_array() && value.as_array().map_or(false, |a| a.is_empty()) {
            return true;
        }
        if value.is_object() && value.as_object().map_or(false, |m| m.is_empty()) {
            return true;
        }
    }
    contains_antigravity_marker(&value)
}

fn prefix_contains_antigravity_marker(body: &str) -> bool {
    let trimmed = body.trim_start();
    if !trimmed.starts_with(['{', '[']) {
        return false;
    }

    [
        "\"cascadeId\"",
        "\"cascadeTrajectories\"",
        "\"trajectorySummaries\"",
        "\"generatorMetadata\"",
        "\"serverInfo\"",
        "\"serverCapabilities\"",
    ]
    .iter()
    .any(|marker| {
        trimmed
            .split(marker)
            .skip(1)
            .any(|suffix| suffix.trim_start().starts_with(':'))
    })
}

fn contains_antigravity_marker(value: &Value) -> bool {
    const MARKERS: &[&str] = &[
        "cascadeId",
        "cascadeTrajectories",
        "trajectorySummaries",
        "generatorMetadata",
        "serverInfo",
        "serverCapabilities",
        "trajectoryId",
        "trajectoryMetadata",
        "trajectoryType",
    ];
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                if MARKERS.iter().any(|m| m.eq_ignore_ascii_case(key)) {
                    return true;
                }
                if contains_antigravity_marker(val) {
                    return true;
                }
            }
            false
        }
        Value::Array(items) => items.iter().any(contains_antigravity_marker),
        Value::String(s) => {
            let lower = s.to_lowercase();
            lower.contains("trajectory") || lower.contains("generatormetadata")
        }
        _ => false,
    }
}

fn run_command(program: &str, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new(program).args(args).output()?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

async fn language_server_request_text(
    client: &reqwest::Client,
    scheme: &str,
    port: u16,
    csrf_token: Option<&str>,
    method: &str,
    body: &Value,
    timeout: Duration,
    max_body_bytes: usize,
) -> Result<(u16, String)> {
    let url = format!(
        "{}://127.0.0.1:{}/{}/{}",
        scheme, port, ANTIGRAVITY_LS_SERVICE, method
    );
    let body_text = serde_json::to_string(body)?;
    let mut request = client
        .post(url)
        .timeout(timeout)
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .body(body_text);
    if let Some(token) = csrf_token.filter(|t| !t.trim().is_empty()) {
        request = request.header("X-Codeium-Csrf-Token", token);
    }
    let response = request.send().await?;
    let status_code = response.status().as_u16();

    if let Some(length) = response.content_length() {
        if length > max_body_bytes as u64 {
            anyhow::bail!("RPC body of {length} bytes exceeds {max_body_bytes} cap");
        }
    }

    let bytes = response.bytes().await?;
    if bytes.len() > max_body_bytes {
        anyhow::bail!(
            "RPC body of {} bytes exceeds {} cap",
            bytes.len(),
            max_body_bytes
        );
    }
    Ok((status_code, String::from_utf8(bytes.to_vec())?))
}

async fn rpc_request(
    client: &reqwest::Client,
    connection: &AntigravityConnection,
    method: &str,
    body: &Value,
) -> Result<Value> {
    let timeout = match method {
        "GetCascadeTrajectoryGeneratorMetadata" => Duration::from_secs(10),
        "GetAllCascadeTrajectories" => Duration::from_secs(10),
        _ => Duration::from_secs(3),
    };
    rpc_request_with_timeout(client, connection, method, body, timeout).await
}

async fn rpc_request_with_timeout(
    client: &reqwest::Client,
    connection: &AntigravityConnection,
    method: &str,
    body: &Value,
    timeout: Duration,
) -> Result<Value> {
    let (status_code, response_body) = language_server_request_text(
        client,
        &connection.scheme,
        connection.port,
        connection.csrf_token.as_deref(),
        method,
        body,
        timeout,
        ANTIGRAVITY_RPC_BODY_CAP,
    )
    .await?;

    if status_code != 200 {
        return Err(anyhow::anyhow!(
            "RPC {} failed with status {}: {}",
            method,
            status_code,
            response_body
        ));
    }

    let trimmed = response_body.trim_start();
    let json_start = trimmed.find(['{', '[']).unwrap_or(0);
    Ok(serde_json::from_str(&trimmed[json_start..])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_file_antigravity() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let conn = open_cache_db(&sessions_dir).unwrap();
        conn.execute(
            "INSERT INTO sessions (session_id, trajectory_id, client, title, model_id, status, step_count, created_time_ms, last_modified_ms, last_user_input_time_ms, synced_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params!["sess-123", "traj-123", "antigravity-cli", "title", "gemini-3-pro-preview", "status", 1_i64, 1672531200000_i64, 1672531200000_i64, 1672531200000_i64, 1672531200000_i64],
        ).unwrap();

        conn.execute(
            "INSERT INTO session_usage (id, session_id, client, model_id, provider_id, timestamp, step_index, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, response_id, pricing_day, parser_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                "resp-456", "sess-123", "antigravity-cli", "gemini-3-pro-preview", "google", 1672531201000_i64, 0_i64,
                150_i64, 50_i64, 20_i64, 0_i64, 10_i64, "resp-456", "2023-01-01", PARSER_VERSION
            ],
        ).unwrap();

        let parser = AntigravitySessionParser::new()
            .with_custom_paths(vec![sessions_dir])
            .with_skip_sync(true);
        let messages = parser.parse_sessions(None).unwrap();

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.client, "antigravity");
        assert_eq!(msg.client_detail.as_deref(), Some("antigravity-cli"));
        assert_eq!(msg.session_id, "sess-123");
        assert_eq!(msg.model_id, "gemini-3-pro-preview");
        assert_eq!(msg.provider_id, "google");
        assert_eq!(msg.message_key, "resp-456");
        assert_eq!(msg.timestamp, 1672531201000);
        assert_eq!(msg.tokens.input, 150);
        assert_eq!(msg.tokens.output, 50);
        assert_eq!(msg.tokens.cache_read, 20);
        assert_eq!(msg.tokens.cache_write, 0);
        assert_eq!(msg.tokens.reasoning, 10);
    }

    #[test]
    fn test_parse_includes_recently_synced_historical_session() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let conn = open_cache_db(&sessions_dir).unwrap();
        let recently_synced_ms = Local::now().timestamp_millis() + 60_000;
        conn.execute(
            "INSERT INTO sessions (session_id, trajectory_id, client, title, model_id, status, step_count, created_time_ms, last_modified_ms, last_user_input_time_ms, synced_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params!["sess-historical", "traj-historical", "antigravity-desktop", "title", "gemini-3.5-flash", "status", 1_i64, 1672531200000_i64, 1672531200000_i64, 1672531200000_i64, recently_synced_ms],
        ).unwrap();

        conn.execute(
            "INSERT INTO session_usage (id, session_id, client, model_id, provider_id, timestamp, step_index, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, response_id, pricing_day, parser_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                "resp-historical", "sess-historical", "antigravity-desktop", "gemini-3.5-flash", "google", 1672531201000_i64, 0_i64,
                100_i64, 20_i64, 5_i64, 0_i64, 0_i64, "resp-historical", "2023-01-01", PARSER_VERSION
            ],
        ).unwrap();

        let parser = AntigravitySessionParser::new()
            .with_custom_paths(vec![sessions_dir])
            .with_skip_sync(true);
        let since = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let messages = parser.parse_sessions(Some(since)).unwrap();

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].session_id, "sess-historical");
        assert_eq!(messages[0].timestamp, 1672531201000);
    }

    #[test]
    fn test_parse_file_antigravity_deduplication() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let conn = open_cache_db(&sessions_dir).unwrap();
        conn.execute(
            "INSERT INTO sessions (session_id, trajectory_id, client, title, model_id, status, step_count, created_time_ms, last_modified_ms, last_user_input_time_ms, synced_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params!["sess-123", "traj-123", "antigravity-cli", "title", "gemini-3-pro-preview", "status", 1_i64, 1672531200000_i64, 1672531200000_i64, 1672531200000_i64, 1672531200000_i64],
        ).unwrap();

        conn.execute(
            "INSERT INTO session_usage (id, session_id, client, model_id, provider_id, timestamp, step_index, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, response_id, pricing_day, parser_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                "resp-456", "sess-123", "antigravity-cli", "gemini-3-pro-preview", "google", 1672531201000_i64, 0_i64,
                150_i64, 50_i64, 20_i64, 0_i64, 10_i64, "resp-456", "2023-01-01", PARSER_VERSION
            ],
        ).unwrap();

        conn.execute(
            "INSERT OR REPLACE INTO session_usage (id, session_id, client, model_id, provider_id, timestamp, step_index, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, response_id, pricing_day, parser_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                "resp-456", "sess-123", "antigravity-cli", "gemini-3-pro-preview", "google", 1672531202000_i64, 0_i64,
                200_i64, 60_i64, 30_i64, 0_i64, 15_i64, "resp-456", "2023-01-01", PARSER_VERSION
            ],
        ).unwrap();

        let parser = AntigravitySessionParser::new()
            .with_custom_paths(vec![sessions_dir])
            .with_skip_sync(true);
        let messages = parser.parse_sessions(None).unwrap();

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.session_id, "sess-123");
        assert_eq!(msg.message_key, "resp-456");
        assert_eq!(msg.tokens.input, 200);
        assert_eq!(msg.tokens.output, 60);
    }

    #[test]
    fn test_parse_file_keeps_cli_and_desktop_copies_for_logical_dedup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let conn = open_cache_db(&sessions_dir).unwrap();
        for client in ["antigravity-cli", "antigravity-desktop"] {
            conn.execute(
                "INSERT INTO sessions (session_id, trajectory_id, client, title, model_id, status, step_count, created_time_ms, last_modified_ms, last_user_input_time_ms, synced_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params!["sess-dup", "traj-dup", client, "title", "gemini-3-pro-preview", "status", 1_i64, 1672531200000_i64, 1672531200000_i64, 1672531200000_i64, 1672531200000_i64],
            ).unwrap();
        }

        let logical_key = antigravity_logical_message_key("sess-dup", "resp-dup");
        for client in ["antigravity-cli", "antigravity-desktop"] {
            let storage_id = antigravity_storage_message_id(client, &logical_key);
            conn.execute(
                "INSERT INTO session_usage (id, session_id, client, model_id, provider_id, timestamp, step_index, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, response_id, pricing_day, parser_version)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    storage_id, "sess-dup", client, "gemini-3-pro-preview", "google", 1672531201000_i64, 0_i64,
                    150_i64, 50_i64, 20_i64, 0_i64, 10_i64, &logical_key, "2023-01-01", PARSER_VERSION
                ],
            ).unwrap();
        }

        let parser = AntigravitySessionParser::new()
            .with_custom_paths(vec![sessions_dir])
            .with_skip_sync(true);
        let messages = parser.parse_sessions(None).unwrap();

        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|msg| msg.client == "antigravity"));
        assert!(messages
            .iter()
            .all(|msg| msg.session_id == "sess-dup" && msg.message_key == logical_key));
        let details = messages
            .iter()
            .filter_map(|msg| msg.client_detail.as_deref())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(details.contains("antigravity-cli"));
        assert!(details.contains("antigravity-desktop"));
    }

    #[test]
    fn test_parse_file_resolves_antigravity_model_aliases() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let conn = open_cache_db(&sessions_dir).unwrap();

        conn.execute(
            "INSERT INTO sessions (session_id, client, model_id, synced_at)
             VALUES (?, ?, ?, ?)",
            rusqlite::params![
                "sess-1",
                "antigravity-cli",
                "MODEL_PLACEHOLDER_M26",
                1672531200000_i64
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_usage (id, session_id, client, model_id, provider_id, timestamp, step_index, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, pricing_day, parser_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params!["msg-1", "sess-1", "antigravity-cli", "MODEL_PLACEHOLDER_M26", "anthropic", 1672531201000_i64, 0_i64, 150_i64, 50_i64, 0_i64, 0_i64, 0_i64, "2023-01-01", PARSER_VERSION],
        ).unwrap();

        conn.execute(
            "INSERT INTO sessions (session_id, client, model_id, synced_at)
             VALUES (?, ?, ?, ?)",
            rusqlite::params![
                "sess-2",
                "antigravity-cli",
                "MODEL_PLACEHOLDER_M20",
                1672531200000_i64
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_usage (id, session_id, client, model_id, provider_id, timestamp, step_index, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, pricing_day, parser_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params!["msg-2", "sess-2", "antigravity-cli", "MODEL_PLACEHOLDER_M20", "google", 1672531202000_i64, 0_i64, 200_i64, 60_i64, 0_i64, 0_i64, 0_i64, "2023-01-01", PARSER_VERSION],
        ).unwrap();

        conn.execute(
            "INSERT INTO sessions (session_id, client, model_id, synced_at)
             VALUES (?, ?, ?, ?)",
            rusqlite::params![
                "sess-3",
                "antigravity-cli",
                "claude-opus-4.6-thinking",
                1672531200000_i64
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_usage (id, session_id, client, model_id, provider_id, timestamp, step_index, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, pricing_day, parser_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params!["msg-3", "sess-3", "antigravity-cli", "claude-opus-4.6-thinking", "anthropic", 1672531203000_i64, 0_i64, 300_i64, 70_i64, 0_i64, 0_i64, 0_i64, "2023-01-01", PARSER_VERSION],
        ).unwrap();

        normalize_cached_antigravity_artifacts(&sessions_dir, &HashMap::new()).unwrap();

        let parser = AntigravitySessionParser::new()
            .with_custom_paths(vec![sessions_dir])
            .with_skip_sync(true);
        let messages = parser.parse_sessions(None).unwrap();

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].model_id, "claude-opus-4-6-thinking");
        assert_eq!(messages[0].provider_id, "anthropic");
        assert_eq!(messages[1].model_id, "gemini-3.5-flash-medium");
        assert_eq!(messages[1].provider_id, "google");
        assert_eq!(messages[2].model_id, "claude-opus-4-6-thinking");
        assert_eq!(messages[2].provider_id, "anthropic");
    }

    #[test]
    fn test_normalize_cached_artifact_rewrites_known_model_aliases() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let conn = open_cache_db(&sessions_dir).unwrap();
        conn.execute(
            "INSERT INTO sessions (session_id, client, model_id, synced_at)
             VALUES (?, ?, ?, ?)",
            rusqlite::params![
                "sess-1",
                "antigravity-cli",
                "MODEL_PLACEHOLDER_M37",
                1672531200000_i64
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_usage (id, session_id, client, model_id, provider_id, timestamp, step_index, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, pricing_day, parser_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params!["msg-1", "sess-1", "antigravity-cli", "MODEL_PLACEHOLDER_M20", "google", 1672531201000_i64, 0_i64, 1_i64, 2_i64, 0_i64, 0_i64, 0_i64, "2023-01-01", PARSER_VERSION],
        ).unwrap();

        normalize_cached_antigravity_artifacts(&sessions_dir, &HashMap::new()).unwrap();

        let mut stmt = conn
            .prepare("SELECT model_id FROM sessions WHERE session_id = 'sess-1'")
            .unwrap();
        let session_model: String = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(session_model, "gemini-3.1-pro-preview-high");

        let mut stmt2 = conn
            .prepare("SELECT model_id FROM session_usage WHERE id = 'msg-1'")
            .unwrap();
        let usage_model: String = stmt2.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(usage_model, "gemini-3.5-flash-medium");
    }

    #[test]
    fn test_dynamic_model_aliases_are_saved_and_reused() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let mut dynamic_aliases = HashMap::new();
        dynamic_aliases.insert(
            normalize_alias_key("MODEL_PLACEHOLDER_M132"),
            ModelAlias {
                raw_model_id: "MODEL_PLACEHOLDER_M132".to_string(),
                model_id: "gemini-3.5-flash-high".to_string(),
                label: Some("Gemini 3.5 Flash (High)".to_string()),
                source: "antigravity-get-user-status".to_string(),
            },
        );

        let saved = merge_and_save_model_alias_history(&sessions_dir, &dynamic_aliases).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M132", &saved),
            "gemini-3.5-flash-high"
        );

        let loaded = load_model_alias_history_map(&sessions_dir).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M132", &loaded),
            "gemini-3.5-flash-high"
        );

        let history_path = model_alias_history_path(&sessions_dir).unwrap();
        let content = std::fs::read_to_string(history_path).unwrap();
        assert!(content.contains("MODEL_PLACEHOLDER_M132"));
        assert!(content.contains("Gemini 3.5 Flash (High)"));
        assert!(content.contains("firstSeenAt"));
        assert!(content.contains("lastSeenAt"));
    }

    #[test]
    fn test_model_alias_history_is_seeded_from_static_sources() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let saved = merge_and_save_model_alias_history(&sessions_dir, &HashMap::new()).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M26", &saved),
            "claude-opus-4-6-thinking"
        );
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M37", &saved),
            "gemini-3.1-pro-preview-high"
        );

        let history_path = model_alias_history_path(&sessions_dir).unwrap();
        let content = std::fs::read_to_string(history_path).unwrap();
        assert!(content.contains("MODEL_PLACEHOLDER_M26"));
        assert!(content.contains("user-initial-mapping;tokscale"));
    }

    #[test]
    fn test_dynamic_model_aliases_override_static_seed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let mut dynamic_aliases = HashMap::new();
        dynamic_aliases.insert(
            normalize_alias_key("MODEL_PLACEHOLDER_M37"),
            ModelAlias {
                raw_model_id: "MODEL_PLACEHOLDER_M37".to_string(),
                model_id: "gemini-3.5-flash".to_string(),
                label: Some("Gemini 3.5 Flash".to_string()),
                source: "antigravity-get-user-status".to_string(),
            },
        );

        let saved = merge_and_save_model_alias_history(&sessions_dir, &dynamic_aliases).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M37", &saved),
            "gemini-3.5-flash"
        );

        let loaded = load_model_alias_history_map(&sessions_dir).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M37", &loaded),
            "gemini-3.5-flash"
        );
    }

    #[test]
    fn test_normalize_alias_key_preserves_separator_distinctions() {
        let mut aliases = HashMap::new();
        aliases.insert(
            normalize_alias_key("gemini-3-pro"),
            ModelAlias {
                raw_model_id: "gemini-3-pro".to_string(),
                model_id: "gemini-3-pro-preview".to_string(),
                label: None,
                source: "test".to_string(),
            },
        );
        aliases.insert(
            normalize_alias_key("gemini_3_pro"),
            ModelAlias {
                raw_model_id: "gemini_3_pro".to_string(),
                model_id: "underscore-model".to_string(),
                label: None,
                source: "test".to_string(),
            },
        );

        assert_eq!(
            resolve_antigravity_model_id_with_aliases("gemini-3-pro", &aliases),
            "gemini-3-pro-preview"
        );
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("gemini_3_pro", &aliases),
            "underscore-model"
        );
    }

    #[test]
    fn test_antigravity_gemini_3_pro_aliases_keep_display_and_pricing_ids_aligned() {
        assert_eq!(
            resolve_antigravity_model_id("antigravity-gemini-3-pro"),
            "gemini-3-pro-preview"
        );
        assert_eq!(
            resolve_antigravity_model_id("antigravity-gemini-3-pro-high"),
            "gemini-3-pro-preview-high"
        );
        assert_eq!(
            resolve_antigravity_model_id("antigravity-gemini-3-pro-low"),
            "gemini-3-pro-preview-low"
        );
        assert_eq!(
            resolve_antigravity_model_id("gemini-3.0-pro-high"),
            "gemini-3-pro-preview-high"
        );
        assert_eq!(
            resolve_antigravity_model_id("gemini-3.1-pro-high"),
            "gemini-3.1-pro-preview-high"
        );
        assert_eq!(
            resolve_antigravity_model_id("gemini-3-1-pro-preview-low"),
            "gemini-3.1-pro-preview-low"
        );
        assert_eq!(
            resolve_antigravity_model_id("gemini-3-pro-image"),
            "gemini-3-pro-preview-image"
        );
    }

    #[test]
    fn test_antigravity_tiered_response_model_resolves_to_base_model() {
        // Observed sub-agent generator metadata: responseModel is the concrete
        // model, with `-tiered` describing routing rather than a model family.
        assert_eq!(
            resolve_antigravity_model_id("gemini-3.6-flash-tiered"),
            "gemini-3-6-flash"
        );
        assert_eq!(
            resolve_antigravity_model_id("gemini-3-6-flash-tiered"),
            "gemini-3-6-flash"
        );
        assert_eq!(
            resolve_antigravity_model_id("antigravity-gemini-3.6-flash-tiered"),
            "gemini-3-6-flash"
        );
        assert_eq!(
            resolve_antigravity_model_id("gemini-3.5-flash-tiered"),
            "gemini-3.5-flash"
        );

        // Only a final `-tiered` segment is a routing suffix.
        assert_eq!(
            resolve_antigravity_model_id("gemini-tiered-flash"),
            "gemini-tiered-flash"
        );
    }

    #[test]
    fn test_antigravity_label_to_model_id_handles_future_label_shapes() {
        assert_eq!(
            antigravity_label_to_model_id("Claude Opus 4.5 (Thinking)").as_deref(),
            Some("claude-opus-4-5-thinking")
        );
        assert_eq!(
            antigravity_label_to_model_id("Claude Sonnet 4.5 (Thinking)").as_deref(),
            Some("claude-sonnet-4-5-thinking")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3.0 Pro Preview (High)").as_deref(),
            Some("gemini-3-pro-preview-high")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3.0 Pro (High)").as_deref(),
            Some("gemini-3-pro-preview-high")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3.1 Pro (High)").as_deref(),
            Some("gemini-3.1-pro-preview-high")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3.1 Pro Preview (Low)").as_deref(),
            Some("gemini-3.1-pro-preview-low")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3 Pro Preview (Low)").as_deref(),
            Some("gemini-3-pro-preview-low")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3 Pro (Image)").as_deref(),
            Some("gemini-3-pro-preview-image")
        );
        assert_eq!(antigravity_label_to_model_id("Unknown Beta"), None);
    }

    #[test]
    fn test_extract_flag_value_matches_exact_flags_and_strips_quotes() {
        let command = "language_server --other_csrf_token=00000000000000000000000000000000 --csrf_token=\"abcdefabcdefabcdefabcdefabcdefab\" --extension_server_port '54321'";

        assert_eq!(
            extract_flag_value(command, "--csrf_token").as_deref(),
            Some("abcdefabcdefabcdefabcdefabcdefab")
        );
        assert_eq!(
            extract_flag_value(command, "--extension_server_port").as_deref(),
            Some("54321")
        );
        assert_eq!(extract_flag_value(command, "--csrf").as_deref(), None);
    }

    #[test]
    fn test_detects_antigravity_cli_process_shape() {
        assert!(is_antigravity_process("agy"));
        assert!(is_antigravity_process("/Users/test/.local/bin/agy"));
        assert_eq!(
            infer_antigravity_runtime_kind("agy", None),
            AntigravityRuntimeKind::Cli
        );
        assert_eq!(
            infer_antigravity_runtime_kind("language_server --app_data_dir antigravity-cli", None),
            AntigravityRuntimeKind::Cli
        );
        assert_eq!(
            infer_antigravity_runtime_kind("language_server --app_data_dir antigravity", None),
            AntigravityRuntimeKind::Desktop
        );
    }

    #[test]
    fn test_session_artifact_file_stem_uses_stable_hash() {
        assert_eq!(
            session_artifact_file_stem("session/with spaces"),
            "session-with-spaces-ffc7e153e201ef5f"
        );
    }

    #[test]
    fn test_discover_local_conversation_ids() {
        let temp_dir = tempfile::tempdir().unwrap();
        let gemini_dir = temp_dir.path().join(".gemini");
        let anti_dir = gemini_dir.join("antigravity").join("conversations");
        let cli_dir = gemini_dir.join("antigravity-cli").join("conversations");

        std::fs::create_dir_all(&anti_dir).unwrap();
        std::fs::create_dir_all(&cli_dir).unwrap();

        let uuid_1 = "12345678901234567890.pb";
        let uuid_2 = "abcdefabcdefabcdefabcdef.db";
        let non_uuid = "short.pb";
        let txt_file = "12345678901234567890.txt";

        std::fs::write(anti_dir.join(uuid_1), "mock").unwrap();

        let db_file_path = cli_dir.join(uuid_2);
        std::fs::write(&db_file_path, "mock").unwrap();

        // Wait briefly to ensure file modification times are different
        std::thread::sleep(std::time::Duration::from_millis(50));

        let wal_file_path = cli_dir.join("abcdefabcdefabcdefabcdef.db-wal");
        std::fs::write(&wal_file_path, "mock").unwrap();

        std::fs::write(anti_dir.join(non_uuid), "mock").unwrap();
        std::fs::write(cli_dir.join(txt_file), "mock").unwrap();

        let discovered = discover_local_conversation_ids_from_home(
            temp_dir.path(),
            &[AntigravityRuntimeKind::Desktop, AntigravityRuntimeKind::Cli],
        );

        assert_eq!(discovered.len(), 2);
        let pb_entry = discovered
            .iter()
            .find(|entry| entry.session_id == "12345678901234567890")
            .expect("Desktop .pb conversation should be discovered");
        assert_eq!(pb_entry.runtime_kind, AntigravityRuntimeKind::Desktop);
        assert!(!discovered.iter().any(|entry| entry.session_id == "short"));

        let discovered_cli_only = discover_local_conversation_ids_from_home(
            temp_dir.path(),
            &[AntigravityRuntimeKind::Cli],
        );
        assert!(discovered_cli_only
            .iter()
            .all(|entry| entry.runtime_kind == AntigravityRuntimeKind::Cli));

        let db_entry = discovered
            .iter()
            .find(|entry| entry.session_id == "abcdefabcdefabcdefabcdef")
            .unwrap();
        assert_eq!(db_entry.runtime_kind, AntigravityRuntimeKind::Cli);
        let db_entry_mtime_ms = db_entry.modified_ms.unwrap();
        let wal_mtime = wal_file_path.metadata().unwrap().modified().unwrap();
        let wal_mtime_ms = system_time_to_millis(wal_mtime).unwrap();
        assert_eq!(db_entry_mtime_ms, wal_mtime_ms);
    }

    #[test]
    fn test_normalize_display_name_to_id() {
        assert_eq!(
            normalize_display_name_to_id("Gemini (3.5 Flash)"),
            Some("gemini-3.5-flash".to_string())
        );
        assert_eq!(
            normalize_display_name_to_id("Claude (3.5 Sonnet)"),
            Some("claude-3.5-sonnet".to_string())
        );
        assert_eq!(
            normalize_display_name_to_id("gpt-oss (some model)"),
            Some("gpt-oss-some-model".to_string())
        );
        assert_eq!(normalize_display_name_to_id("gpt-4o"), None);
        assert_eq!(normalize_display_name_to_id("   "), None);
    }

    #[test]
    fn test_is_pseudo_raw_model() {
        assert!(is_pseudo_raw_model(""));
        assert!(is_pseudo_raw_model("unknown"));
        assert!(is_pseudo_raw_model("UNKNOWN"));
        assert!(is_pseudo_raw_model("auto-review"));
        assert!(is_pseudo_raw_model("gemini-default"));
        assert!(!is_pseudo_raw_model("gemini-1.5-pro"));
    }

    fn proto_varint_bytes(value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut value = value;
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn proto_varint_field(field_number: u32, value: u64) -> Vec<u8> {
        let mut out = proto_varint_bytes(u64::from(field_number) << 3);
        out.extend(proto_varint_bytes(value));
        out
    }

    fn proto_bytes_field(field_number: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = proto_varint_bytes((u64::from(field_number) << 3) | 2);
        out.extend(proto_varint_bytes(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    /// Builds a `gen_metadata.data` blob in the shape Antigravity writes.
    struct GenMetadataFixture {
        request_uuid: String,
        input_tokens: u64,
        total_output_tokens: u64,
        cache_read_tokens: u64,
        thinking_output_tokens: u64,
        response_output_tokens: u64,
        response_id: String,
        response_model: Option<String>,
        model_enum: Option<String>,
    }

    impl Default for GenMetadataFixture {
        fn default() -> Self {
            Self {
                request_uuid: "a04cc152-933d-413f-bf77-e5e046d8005c".to_string(),
                input_tokens: 5911,
                total_output_tokens: 669,
                cache_read_tokens: 8138,
                thinking_output_tokens: 614,
                response_output_tokens: 55,
                response_id: "bGZmapHNJ8XVz7IPxtDuoAo".to_string(),
                response_model: Some("gemini-3.7-flash".to_string()),
                model_enum: None,
            }
        }
    }

    impl GenMetadataFixture {
        fn encode(&self) -> Vec<u8> {
            let mut usage = Vec::new();
            usage.extend(proto_varint_field(1, 1196));
            usage.extend(proto_varint_field(2, self.input_tokens));
            usage.extend(proto_varint_field(3, self.total_output_tokens));
            usage.extend(proto_varint_field(5, self.cache_read_tokens));
            usage.extend(proto_varint_field(6, 24));
            usage.extend(proto_varint_field(9, self.thinking_output_tokens));
            usage.extend(proto_varint_field(10, self.response_output_tokens));
            usage.extend(proto_bytes_field(11, self.response_id.as_bytes()));

            let mut payload = Vec::new();
            payload.extend(proto_varint_field(3, 1196));
            payload.extend(proto_bytes_field(4, &usage));
            if let Some(response_model) = &self.response_model {
                payload.extend(proto_bytes_field(19, response_model.as_bytes()));
            }
            let mut trajectory_pair = Vec::new();
            trajectory_pair.extend(proto_bytes_field(1, b"trajectory_id"));
            trajectory_pair.extend(proto_bytes_field(2, b"traj-123"));
            payload.extend(proto_bytes_field(20, &trajectory_pair));
            if let Some(model_enum) = &self.model_enum {
                let mut pair = Vec::new();
                pair.extend(proto_bytes_field(1, b"model_enum"));
                pair.extend(proto_bytes_field(2, model_enum.as_bytes()));
                payload.extend(proto_bytes_field(20, &pair));
            }

            let mut blob = Vec::new();
            blob.extend(proto_bytes_field(4, self.request_uuid.as_bytes()));
            blob.extend(proto_bytes_field(8, b"ignored"));
            blob.extend(proto_bytes_field(1, &payload));
            blob
        }
    }

    /// Builds a `steps.metadata` blob carrying the wall clock and request UUID.
    fn encode_step_metadata(request_uuid: &str, seconds: u64, nanos: u64) -> Vec<u8> {
        let mut timestamp = Vec::new();
        timestamp.extend(proto_varint_field(1, seconds));
        timestamp.extend(proto_varint_field(2, nanos));
        let mut metadata = Vec::new();
        metadata.extend(proto_bytes_field(1, &timestamp));
        metadata.extend(proto_varint_field(11, 1196));
        metadata.extend(proto_bytes_field(12, request_uuid.as_bytes()));
        metadata
    }

    fn write_conversation_db(path: &Path, gen_metadata: &[Vec<u8>], steps: &[(String, u64)]) {
        let local_db = rusqlite::Connection::open(path).unwrap();
        local_db
            .execute_batch(
                "CREATE TABLE trajectory_meta (trajectory_id text, cascade_id text, trajectory_type integer, source integer, PRIMARY KEY (trajectory_id));
                 CREATE TABLE steps (idx integer PRIMARY KEY, step_type integer, status integer, metadata blob);
                 CREATE TABLE gen_metadata (idx integer PRIMARY KEY, data blob, size integer);
                 INSERT INTO trajectory_meta VALUES ('traj-123', 'cascade-123', 1, 1);",
            )
            .unwrap();
        for (idx, (request_uuid, seconds)) in steps.iter().enumerate() {
            local_db
                .execute(
                    "INSERT INTO steps VALUES (?, 1, 1, ?)",
                    rusqlite::params![
                        idx as i64,
                        encode_step_metadata(request_uuid, *seconds, 500_000_000)
                    ],
                )
                .unwrap();
        }
        for (idx, blob) in gen_metadata.iter().enumerate() {
            local_db
                .execute(
                    "INSERT INTO gen_metadata VALUES (?, ?, ?)",
                    rusqlite::params![idx as i64, blob, blob.len() as i64],
                )
                .unwrap();
        }
    }

    #[test]
    fn test_parse_gen_metadata_usage_maps_validated_fields() {
        let fixture = GenMetadataFixture::default();
        let usage = parse_gen_metadata_usage(&fixture.encode()).expect("blob should parse");

        assert_eq!(usage.tokens.input, 5911);
        assert_eq!(usage.tokens.cache_read, 8138);
        assert_eq!(usage.tokens.reasoning, 614);
        // `responseOutputTokens` (1.4.10), never `outputTokens` (1.4.3 = 669),
        // which already contains the thinking tokens.
        assert_eq!(usage.tokens.output, 55);
        assert_ne!(usage.tokens.output, fixture.total_output_tokens as i64);
        assert_eq!(
            usage.response_id.as_deref(),
            Some("bGZmapHNJ8XVz7IPxtDuoAo")
        );
        assert_eq!(usage.raw_model_id.as_deref(), Some("gemini-3.7-flash"));
        assert_eq!(
            usage.request_uuid.as_deref(),
            Some("a04cc152-933d-413f-bf77-e5e046d8005c")
        );
    }

    #[test]
    fn test_parse_gen_metadata_usage_rejects_broken_token_partition() {
        // `thinkingOutputTokens + responseOutputTokens` must equal
        // `outputTokens`; a blob that violates it means the field map drifted.
        let broken = GenMetadataFixture {
            total_output_tokens: 700,
            ..GenMetadataFixture::default()
        };
        assert_eq!(parse_gen_metadata_usage(&broken.encode()), None);

        // Non-generation steps carry no output at all and are skipped.
        let non_generation = GenMetadataFixture {
            total_output_tokens: 0,
            thinking_output_tokens: 0,
            response_output_tokens: 0,
            ..GenMetadataFixture::default()
        };
        assert_eq!(parse_gen_metadata_usage(&non_generation.encode()), None);
    }

    #[test]
    fn test_parse_gen_metadata_usage_falls_back_to_model_enum() {
        let placeholder = GenMetadataFixture {
            response_model: Some("gemini-default".to_string()),
            model_enum: Some("MODEL_PLACEHOLDER_M20".to_string()),
            ..GenMetadataFixture::default()
        };
        let usage = parse_gen_metadata_usage(&placeholder.encode()).unwrap();
        assert_eq!(usage.raw_model_id.as_deref(), Some("MODEL_PLACEHOLDER_M20"));

        let missing = GenMetadataFixture {
            response_model: None,
            model_enum: Some("MODEL_PLACEHOLDER_M132".to_string()),
            ..GenMetadataFixture::default()
        };
        let usage = parse_gen_metadata_usage(&missing.encode()).unwrap();
        assert_eq!(
            usage.raw_model_id.as_deref(),
            Some("MODEL_PLACEHOLDER_M132")
        );
    }

    #[test]
    fn test_parse_gen_metadata_usage_survives_truncated_blob() {
        let blob = GenMetadataFixture::default().encode();
        for len in 0..blob.len() {
            // Must never panic; a partial blob either parses or is skipped.
            let _ = parse_gen_metadata_usage(&blob[..len]);
        }
    }

    #[test]
    fn test_sync_local_conversations_reparses_sessions_without_local_usage() {
        // Upgrade case: the cache already knows the session (older releases
        // only stored metadata), so the mtime check alone would skip it forever
        // and its tokens would never arrive.
        let temp_dir = tempfile::tempdir().unwrap();
        let home = temp_dir.path();
        let cli_conv_dir = home
            .join(".gemini")
            .join("antigravity-cli")
            .join("conversations");
        std::fs::create_dir_all(&cli_conv_dir).unwrap();

        let session_id = "cli-session-12345678901234567890";
        let request_uuid = "11111111-1111-4111-8111-111111111111";
        write_conversation_db(
            &cli_conv_dir.join(format!("{}.db", session_id)),
            &[GenMetadataFixture {
                request_uuid: request_uuid.to_string(),
                ..GenMetadataFixture::default()
            }
            .encode()],
            &[(request_uuid.to_string(), 1_767_225_600)],
        );

        let cache_dir = temp_dir.path().join("cache");
        let mut db_conn = open_cache_db(&cache_dir).unwrap();
        let aliases = static_model_aliases();
        let mut cached_sessions = HashMap::new();
        cached_sessions.insert(
            ("antigravity-cli".to_string(), session_id.to_string()),
            (Some(i64::MAX), Some(1_i64)),
        );

        sync_local_conversations(&mut db_conn, home, &cached_sessions, false, &aliases).unwrap();
        let rows: i64 = db_conn
            .query_row("SELECT count(*) FROM session_usage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);

        // Once parsed, the unchanged file is skipped again: a sentinel written
        // over the row survives the next sync.
        db_conn
            .execute("UPDATE session_usage SET input_tokens = -1", [])
            .unwrap();
        sync_local_conversations(&mut db_conn, home, &cached_sessions, false, &aliases).unwrap();
        let (rows_after, input_after): (i64, i64) = db_conn
            .query_row(
                "SELECT count(*), min(input_tokens) FROM session_usage",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(rows_after, 1);
        assert_eq!(input_after, -1);
    }

    #[test]
    fn test_normalize_antigravity_tokens_never_double_counts_reasoning() {
        // The shape both sources report: total 669 = thinking 614 + response 55.
        let tokens = normalize_antigravity_tokens(5911, 669, Some(55), 8138, 0, 614).unwrap();
        assert_eq!(tokens.output, 55);
        assert_eq!(tokens.reasoning, 614);
        // Billing sums output and reasoning, so together they must equal the
        // reported total exactly once.
        assert_eq!(tokens.output + tokens.reasoning, 669);

        // A source that reports only the total still yields the disjoint split.
        let legacy = normalize_antigravity_tokens(5911, 669, None, 8138, 0, 614).unwrap();
        assert_eq!(legacy.output, 55);
        assert_eq!(legacy.reasoning, 614);

        // Disagreement means the field map drifted: reject, do not guess.
        assert_eq!(
            normalize_antigravity_tokens(1, 700, Some(55), 0, 0, 614),
            None
        );

        // Values large enough to overflow an i64 sum must not panic.
        assert_eq!(
            normalize_antigravity_tokens(0, 1, Some(i64::MAX), 0, 0, i64::MAX),
            None
        );
    }

    #[test]
    fn test_rpc_and_local_paths_agree_on_the_same_usage_record() {
        // Same generation seen through both sources must produce identical
        // columns; this is the drift guard the two paths previously lacked.
        let local = parse_gen_metadata_usage(&GenMetadataFixture::default().encode()).unwrap();
        let rpc_usage = serde_json::json!({
            "inputTokens": 5911,
            "outputTokens": 669,
            "responseOutputTokens": 55,
            "cacheReadTokens": 8138,
            "thinkingOutputTokens": 614,
        });
        let rpc = normalize_antigravity_tokens(
            to_safe_i64(rpc_usage.get("inputTokens")),
            to_safe_i64(rpc_usage.get("outputTokens")),
            rpc_usage
                .get("responseOutputTokens")
                .map(|v| to_safe_i64(Some(v))),
            to_safe_i64(rpc_usage.get("cacheReadTokens")),
            to_safe_i64(rpc_usage.get("cacheWriteTokens")),
            to_safe_i64(rpc_usage.get("thinkingOutputTokens")),
        )
        .unwrap();
        assert_eq!(local.tokens, rpc);
    }

    #[test]
    fn test_local_step_timestamps_rejects_out_of_range_nanos() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("conversation.db");
        let local_db = rusqlite::Connection::open(&path).unwrap();
        local_db
            .execute_batch("CREATE TABLE steps (idx integer PRIMARY KEY, metadata blob);")
            .unwrap();

        // nanos outside [0, 1e9) is not a protobuf Timestamp, and the naive
        // millisecond conversion would overflow on it.
        let mut overflowing = Vec::new();
        overflowing.extend(proto_varint_field(1, (i64::MAX / 1000) as u64));
        overflowing.extend(proto_varint_field(2, i64::MAX as u64));
        let mut bad = Vec::new();
        bad.extend(proto_bytes_field(1, &overflowing));
        bad.extend(proto_bytes_field(12, b"bad-uuid-0000-0000-000000000000"));
        local_db
            .execute("INSERT INTO steps VALUES (0, ?)", rusqlite::params![bad])
            .unwrap();

        let good = encode_step_metadata(
            "good-uuid-0000-0000-000000000000",
            1_767_225_600,
            500_000_000,
        );
        local_db
            .execute("INSERT INTO steps VALUES (1, ?)", rusqlite::params![good])
            .unwrap();

        let timestamps = local_step_timestamps(&local_db);
        assert_eq!(timestamps.len(), 1);
        assert_eq!(
            timestamps.get("good-uuid-0000-0000-000000000000"),
            Some(&1_767_225_600_500)
        );
    }

    #[test]
    fn test_local_sync_preserves_language_server_session_metadata() {
        // The local scan knows nothing about titles or workspaces; a
        // `PARSER_VERSION` bump rescans every session, so a plain overwrite
        // would wipe metadata only a live language server can supply.
        let temp_dir = tempfile::tempdir().unwrap();
        let home = temp_dir.path();
        let cli_conv_dir = home
            .join(".gemini")
            .join("antigravity-cli")
            .join("conversations");
        std::fs::create_dir_all(&cli_conv_dir).unwrap();

        let session_id = "cli-session-12345678901234567890";
        let request_uuid = "11111111-1111-4111-8111-111111111111";
        write_conversation_db(
            &cli_conv_dir.join(format!("{}.db", session_id)),
            &[GenMetadataFixture {
                request_uuid: request_uuid.to_string(),
                ..GenMetadataFixture::default()
            }
            .encode()],
            &[(request_uuid.to_string(), 1_767_225_600)],
        );

        let cache_dir = temp_dir.path().join("cache");
        let mut db_conn = open_cache_db(&cache_dir).unwrap();
        db_conn
            .execute(
                "INSERT INTO sessions (session_id, client, title, status, workspace_path, git_root,
                                       branch_name, model_id, step_count, last_modified_ms, synced_at)
                 VALUES (?, 'antigravity-cli', 'Real title', 'DONE', '/home/me/proj', '/home/me/proj',
                         'main', 'gemini-3-7-flash', 9, 1, 1)",
                [session_id],
            )
            .unwrap();

        sync_local_conversations(
            &mut db_conn,
            home,
            &HashMap::new(),
            false,
            &static_model_aliases(),
        )
        .unwrap();

        let (title, status, workspace, git_root, branch): (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = db_conn
            .query_row(
                "SELECT title, status, workspace_path, git_root, branch_name
                 FROM sessions WHERE session_id = ?",
                [session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(title.as_deref(), Some("Real title"));
        assert_eq!(status.as_deref(), Some("DONE"));
        assert_eq!(workspace.as_deref(), Some("/home/me/proj"));
        assert_eq!(git_root.as_deref(), Some("/home/me/proj"));
        assert_eq!(branch.as_deref(), Some("main"));

        // The fields the local scan does own are still updated.
        let (trajectory_id, usage_rows): (Option<String>, i64) = db_conn
            .query_row(
                "SELECT (SELECT trajectory_id FROM sessions WHERE session_id = ?1),
                        (SELECT count(*) FROM session_usage WHERE session_id = ?1)",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(trajectory_id.as_deref(), Some("traj-123"));
        assert_eq!(usage_rows, 1);
    }

    #[test]
    fn test_legacy_v2_usage_rows_are_migrated_on_cache_open() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir.path().join("cache");
        let conn = open_cache_db(&cache_dir).unwrap();
        conn.execute(
            "INSERT INTO sessions (session_id, client, model_id, synced_at)
             VALUES ('sess-legacy', 'antigravity-cli', 'gemini-3-7-flash', 1)",
            [],
        )
        .unwrap();
        // A v2 row as the RPC wrote it: output_tokens is the total, thinking
        // included.
        conn.execute(
            "INSERT INTO session_usage (id, session_id, client, model_id, provider_id, timestamp,
                                        step_index, input_tokens, output_tokens, cache_read_tokens,
                                        cache_write_tokens, reasoning_tokens, response_id,
                                        pricing_day, parser_version)
             VALUES ('legacy', 'sess-legacy', 'antigravity-cli', 'gemini-3-7-flash', 'google',
                     1672531201000, 0, 5911, 669, 8138, 0, 614, 'resp', '2023-01-01',
                     'antigravity-v2')",
            [],
        )
        .unwrap();
        drop(conn);

        let conn = open_cache_db(&cache_dir).unwrap();
        let (output, reasoning, version): (i64, i64, String) = conn
            .query_row(
                "SELECT output_tokens, reasoning_tokens, parser_version FROM session_usage",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(output, 55);
        assert_eq!(reasoning, 614);
        // No row may linger at an old version: one that the local parser can
        // never re-read would mark the source stale on every launch.
        assert_eq!(version, PARSER_VERSION);

        // Idempotent — reopening must not subtract twice.
        drop(conn);
        let conn = open_cache_db(&cache_dir).unwrap();
        let output_again: i64 = conn
            .query_row("SELECT output_tokens FROM session_usage", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(output_again, 55);
    }

    #[test]
    fn test_sync_local_conversations_writes_usage_for_both_runtimes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let home = temp_dir.path();
        let cli_conv_dir = home
            .join(".gemini")
            .join("antigravity-cli")
            .join("conversations");
        let desktop_conv_dir = home
            .join(".gemini")
            .join("antigravity")
            .join("conversations");
        std::fs::create_dir_all(&cli_conv_dir).unwrap();
        std::fs::create_dir_all(&desktop_conv_dir).unwrap();

        let cli_session_id = "cli-session-12345678901234567890";
        let desktop_session_id = "desktop-session-12345678901234567890";
        let cli_uuid = "11111111-1111-4111-8111-111111111111";
        let desktop_uuid = "22222222-2222-4222-8222-222222222222";

        write_conversation_db(
            &cli_conv_dir.join(format!("{}.db", cli_session_id)),
            &[
                GenMetadataFixture {
                    request_uuid: cli_uuid.to_string(),
                    response_id: "resp-cli-1".to_string(),
                    ..GenMetadataFixture::default()
                }
                .encode(),
                // Non-generation step: skipped, so it must not add a row.
                GenMetadataFixture {
                    request_uuid: cli_uuid.to_string(),
                    response_id: "resp-cli-2".to_string(),
                    total_output_tokens: 0,
                    thinking_output_tokens: 0,
                    response_output_tokens: 0,
                    ..GenMetadataFixture::default()
                }
                .encode(),
            ],
            &[(cli_uuid.to_string(), 1_767_225_600)],
        );
        write_conversation_db(
            &desktop_conv_dir.join(format!("{}.db", desktop_session_id)),
            &[GenMetadataFixture {
                request_uuid: desktop_uuid.to_string(),
                response_id: "resp-desktop-1".to_string(),
                input_tokens: 100,
                cache_read_tokens: 200,
                total_output_tokens: 30,
                thinking_output_tokens: 20,
                response_output_tokens: 10,
                ..GenMetadataFixture::default()
            }
            .encode()],
            &[(desktop_uuid.to_string(), 1_767_225_600)],
        );

        let cache_dir = temp_dir.path().join("cache");
        let mut db_conn = open_cache_db(&cache_dir).unwrap();
        let aliases = static_model_aliases();

        sync_local_conversations(&mut db_conn, home, &HashMap::new(), false, &aliases).unwrap();

        let (traj_id, step_cnt): (Option<String>, Option<i64>) = db_conn
            .query_row(
                "SELECT trajectory_id, step_count FROM sessions WHERE session_id = ?1 AND client = 'antigravity-cli'",
                [cli_session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(traj_id, Some("traj-123".to_string()));
        assert_eq!(step_cnt, Some(1));

        let clients: Vec<(String, i64)> = db_conn
            .prepare("SELECT client, count(*) FROM session_usage GROUP BY client ORDER BY client")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            clients,
            vec![
                ("antigravity-cli".to_string(), 1),
                ("antigravity-desktop".to_string(), 1),
            ]
        );

        let (model_id, input, output, cache_read, cache_write, reasoning, timestamp, parser_version): (
            String,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            String,
        ) = db_conn
            .query_row(
                "SELECT model_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                        reasoning_tokens, timestamp, parser_version
                 FROM session_usage WHERE client = 'antigravity-desktop'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .unwrap();
        // `resolve_antigravity_model_id_with_aliases` normalizes the dotted
        // version the same way the RPC path does.
        assert_eq!(model_id, "gemini-3-7-flash");
        assert_eq!(
            (input, output, cache_read, cache_write, reasoning),
            (100, 10, 200, 0, 20)
        );
        // Wall clock joined from `steps.metadata`, not the file mtime.
        assert_eq!(timestamp, 1_767_225_600_500);
        assert_eq!(parser_version, PARSER_VERSION);

        // A second sync of unchanged files must not duplicate rows: the
        // `{client}:{session_id}:{responseId}` key replaces in place.
        let rows_after_first: i64 = db_conn
            .query_row("SELECT count(*) FROM session_usage", [], |row| row.get(0))
            .unwrap();
        sync_local_conversations(&mut db_conn, home, &HashMap::new(), true, &aliases).unwrap();
        let rows_after_second: i64 = db_conn
            .query_row("SELECT count(*) FROM session_usage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows_after_first, 2);
        assert_eq!(rows_after_first, rows_after_second);
    }
}
