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

const PARSER_VERSION: &str = "antigravity-v2";
const MODEL_ALIAS_HISTORY_VERSION: u32 = 1;
const MODEL_ALIAS_HISTORY_FILE_NAME: &str = "model-aliases.json";
const ANTIGRAVITY_LS_SERVICE: &str = "exa.language_server_pb.LanguageServerService";
const ANTIGRAVITY_RPC_BODY_CAP: usize = 16 * 1024 * 1024;

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
            let query = "SELECT client, model_id, provider_id, session_id, COALESCE(response_id, id), timestamp,
                                input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
                                pricing_day, parser_version
                         FROM session_usage
                         ORDER BY timestamp ASC";
            let mut stmt = conn.prepare(query)?;

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

            let rows = stmt.query_map([], map_row)?;

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

    Ok(conn)
}

fn count_antigravity_session_cache_rows(conn: &rusqlite::Connection) -> usize {
    conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
        .unwrap_or(0)
}

pub fn sync_antigravity(sessions_dir: &Path) -> Result<()> {
    sync_antigravity_with_options(sessions_dir, AntigravitySyncOptions::default())
}

pub fn sync_active_antigravity_aliases() -> Result<()> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let cache_dir = home
        .join(".local")
        .join("share")
        .join("tokenpulse")
        .join("antigravity-cache");
    let connections = detect_antigravity_connections()?;
    if !connections.is_empty() {
        let dynamic_model_aliases = fetch_dynamic_model_aliases(&connections);
        merge_and_save_model_alias_history(&cache_dir, &dynamic_model_aliases)?;
    }
    Ok(())
}

fn sync_antigravity_with_options(
    sessions_dir: &Path,
    options: AntigravitySyncOptions,
) -> Result<()> {
    let sync_start = std::time::Instant::now();
    std::fs::create_dir_all(sessions_dir)?;

    let connections = match detect_antigravity_connections() {
        Ok(c) => c,
        Err(e) => {
            warn!(
                "Antigravity language server process discovery failed: {}",
                e
            );
            return Ok(());
        }
    };

    if connections.is_empty() {
        if sessions_dir.exists() {
            if let Err(e) = merge_and_save_model_alias_history(sessions_dir, &HashMap::new()) {
                debug!("Failed to seed Antigravity model alias history: {}", e);
            }
        }
        warn!("No running Antigravity language servers detected; skipping sync and reading cache");
        return Ok(());
    }

    let mut db_conn = open_cache_db(sessions_dir)?;

    if options.rebuild_all_cache {
        db_conn.execute("DELETE FROM session_usage;", [])?;
        db_conn.execute("DELETE FROM sessions;", [])?;
    }

    let cached_rows_before = count_antigravity_session_cache_rows(&db_conn);

    let dynamic_model_aliases = fetch_dynamic_model_aliases(&connections);
    let model_aliases =
        match merge_and_save_model_alias_history(sessions_dir, &dynamic_model_aliases) {
            Ok(aliases) => aliases,
            Err(e) => {
                debug!("Failed to update Antigravity model alias history: {}", e);
                dynamic_model_aliases
            }
        };

    let mut synced_sessions_count = 0;

    let mut cached_sessions: HashMap<(String, String), i64> = HashMap::new();
    if let Ok(mut stmt) = db_conn.prepare(
        r#"
        SELECT client, session_id, last_modified_ms FROM sessions
        UNION
        SELECT session_usage.client, sessions.session_id, sessions.last_modified_ms
        FROM session_usage
        JOIN sessions ON sessions.session_id = session_usage.session_id
        "#,
    ) {
        if let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        }) {
            for row in rows {
                if let Ok((client, id, Some(last_mod))) = row {
                    cached_sessions.insert((client, id), last_mod);
                }
            }
        }
    }

    let mut unique_summaries: HashMap<AntigravitySessionCacheKey, AntigravitySyncSummary> =
        HashMap::new();
    for connection in &connections {
        let response = match rpc_request(connection, "GetAllCascadeTrajectories", &json!({})) {
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

            let last_modified_ms = item
                .get("lastModifiedTime")
                .or_else(|| item.get("lastModified"))
                .or_else(|| item.get("updatedAt"))
                .and_then(parse_timestamp_value);

            let project_id = item
                .get("projectId")
                .and_then(Value::as_str)
                .map(String::from);
            let trajectory_id = item
                .get("trajectoryId")
                .and_then(Value::as_str)
                .map(String::from);
            let title = item
                .get("summary")
                .and_then(Value::as_str)
                .map(String::from);
            let status = item.get("status").and_then(Value::as_str).map(String::from);
            let step_count = item.get("stepCount").and_then(Value::as_i64);
            let created_time_ms = item.get("createdTime").and_then(parse_timestamp_value);
            let last_user_input_time_ms = item
                .get("lastUserInputTime")
                .and_then(parse_timestamp_value);
            let parent_conversation_id = item
                .get("parentConversationId")
                .and_then(Value::as_str)
                .map(String::from);

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

            let workspace = item.get("workspace");
            let workspace_path = workspace
                .and_then(|w| w.get("workspacePath").or_else(|| w.get("path")))
                .and_then(Value::as_str)
                .map(String::from);
            let git_root = workspace
                .and_then(|w| w.get("gitRoot"))
                .and_then(Value::as_str)
                .map(String::from);
            let repository = workspace
                .and_then(|w| w.get("repository"))
                .and_then(Value::as_str)
                .map(String::from);
            let git_origin_url = workspace
                .and_then(|w| w.get("gitOriginUrl"))
                .and_then(Value::as_str)
                .map(String::from);
            let branch_name = workspace
                .and_then(|w| w.get("branchName"))
                .and_then(Value::as_str)
                .map(String::from);

            let summary_data = AntigravitySyncSummary {
                last_modified_ms,
                connections: vec![connection.clone()],
                trajectory_id,
                title,
                status,
                step_count,
                created_time_ms,
                last_user_input_time_ms,
                project_id,
                workspace_path,
                git_root,
                repository,
                git_origin_url,
                branch_name,
                parent_conversation_id,
                mendel_experiment_ids,
            };

            upsert_sync_summary(
                &mut unique_summaries,
                AntigravitySessionCacheKey {
                    session_id,
                    runtime_kind: connection.runtime_kind,
                },
                summary_data,
            );
        }
    }

    let active_kinds: Vec<AntigravityRuntimeKind> =
        connections.iter().map(|c| c.runtime_kind).collect();
    let local_conversation_ids = discover_local_conversation_ids(&active_kinds);
    debug!(
        "Discovered {} local conversation files",
        local_conversation_ids.len()
    );
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

    let total_detected_sessions = unique_summaries.len();
    let now_ms = Local::now().timestamp_millis();

    for (cache_key, summary) in unique_summaries {
        let session_id = cache_key.session_id;
        let client_str = client_str_for_runtime_kind(cache_key.runtime_kind);
        let last_modified_ms = summary.last_modified_ms;
        let lm = last_modified_ms.unwrap_or(now_ms);

        if !options.rebuild_all_cache {
            if let Some(cached_lm) =
                cached_sessions.get(&(client_str.to_string(), session_id.clone()))
            {
                if *cached_lm >= lm {
                    debug!("Session {} is unchanged, skipping sync", session_id);
                    continue;
                }
            }
        }

        let mut metadata_response = None;
        for conn in &summary.connections {
            debug!(
                "Syncing Antigravity session {} (modified: {:?}) from {} port {}",
                session_id,
                last_modified_ms,
                conn.runtime_kind.as_str(),
                conn.port
            );
            match rpc_request(
                conn,
                "GetCascadeTrajectoryGeneratorMetadata",
                &json!({ "cascadeId": session_id }),
            ) {
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

        let Some(metadata_response) = metadata_response else {
            continue;
        };

        let metadata = metadata_response
            .get("generatorMetadata")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        if metadata.is_empty() {
            continue;
        }

        let mut primary_model_id = "unknown".to_string();
        for meta in &metadata {
            let chat_model = meta.get("chatModel").unwrap_or(meta);
            let raw_model_id = chat_model
                .get("responseModel")
                .or_else(|| chat_model.get("model"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let model_id = resolve_antigravity_model_id_with_aliases(raw_model_id, &model_aliases);
            if model_id != "unknown" {
                primary_model_id = model_id;
                break;
            }
        }

        let tx = db_conn.transaction()?;
        tx.execute(
            "DELETE FROM session_usage WHERE client = ? AND session_id = ?;",
            rusqlite::params![client_str, &session_id],
        )?;

        tx.execute(
            "INSERT INTO sessions (
                session_id, trajectory_id, client, title, model_id, status, step_count,
                created_time_ms, last_modified_ms, last_user_input_time_ms, project_id,
                workspace_path, git_root, repository, git_origin_url, branch_name,
                parent_conversation_id, mendel_experiment_ids, synced_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(session_id, client) DO UPDATE SET
                trajectory_id = excluded.trajectory_id,
                client = excluded.client,
                title = excluded.title,
                model_id = excluded.model_id,
                status = excluded.status,
                step_count = excluded.step_count,
                created_time_ms = excluded.created_time_ms,
                last_modified_ms = excluded.last_modified_ms,
                last_user_input_time_ms = excluded.last_user_input_time_ms,
                project_id = excluded.project_id,
                workspace_path = excluded.workspace_path,
                git_root = excluded.git_root,
                repository = excluded.repository,
                git_origin_url = excluded.git_origin_url,
                branch_name = excluded.branch_name,
                parent_conversation_id = excluded.parent_conversation_id,
                mendel_experiment_ids = excluded.mendel_experiment_ids,
                synced_at = excluded.synced_at",
            rusqlite::params![
                &session_id,
                summary.trajectory_id,
                client_str,
                summary.title,
                primary_model_id,
                summary.status,
                summary.step_count,
                summary.created_time_ms,
                lm,
                summary.last_user_input_time_ms,
                summary.project_id,
                summary.workspace_path,
                summary.git_root,
                summary.repository,
                summary.git_origin_url,
                summary.branch_name,
                summary.parent_conversation_id,
                summary.mendel_experiment_ids,
                now_ms,
            ],
        )?;

        for (step_idx, meta) in metadata.iter().enumerate() {
            let chat_model = meta.get("chatModel").unwrap_or(meta);
            let raw_model_id = chat_model
                .get("responseModel")
                .or_else(|| chat_model.get("model"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let model_id = resolve_antigravity_model_id_with_aliases(raw_model_id, &model_aliases);

            let created_at = chat_model
                .get("chatStartMetadata")
                .and_then(|v| v.get("createdAt"))
                .and_then(parse_timestamp_value);

            if let Some(retry_infos) = chat_model.get("retryInfos").and_then(Value::as_array) {
                for retry in retry_infos {
                    let usage = retry.get("usage").unwrap_or(retry);
                    let input = to_safe_i64(usage.get("inputTokens"));
                    let output = to_safe_i64(usage.get("outputTokens"));
                    let cache_read = to_safe_i64(usage.get("cacheReadTokens"));
                    let cache_write = to_safe_i64(usage.get("cacheWriteTokens"));
                    let reasoning = to_safe_i64(usage.get("thinkingOutputTokens"));
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
                }
            }
        }

        tx.commit()?;
        synced_sessions_count += 1;
    }

    let cached_rows_after = count_antigravity_session_cache_rows(&db_conn);

    info!(
        "Antigravity sync: Synced local Antigravity cache in {} ms. Connections: {}, sessions: (total: {}, synced: {}), cache rows: {} -> {}",
        sync_start.elapsed().as_millis(),
        connections.len(),
        total_detected_sessions,
        synced_sessions_count,
        cached_rows_before,
        cached_rows_after
    );

    Ok(())
}

fn client_str_for_runtime_kind(runtime_kind: AntigravityRuntimeKind) -> &'static str {
    match runtime_kind {
        AntigravityRuntimeKind::Cli => "antigravity-cli",
        AntigravityRuntimeKind::Desktop => "antigravity-desktop",
        AntigravityRuntimeKind::Unknown => "antigravity",
    }
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
    if runtime_kind != AntigravityRuntimeKind::Unknown {
        for conn in all_connections
            .iter()
            .filter(|conn| conn.runtime_kind == runtime_kind)
        {
            push_unique_connection(&mut ordered, conn.clone());
        }
    }
    for conn in connections.drain(..) {
        push_unique_connection(&mut ordered, conn);
    }
    for conn in all_connections {
        push_unique_connection(&mut ordered, conn.clone());
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
        model_id: "gemini-3.1-pro-preview",
        label: Some("Gemini 3.1 Pro (High)"),
        source: "user-initial-mapping;tokscale",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M36",
        model_id: "gemini-3.1-pro-preview",
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
        model_id: "gemini-3-pro-preview",
        label: Some("Gemini 3 Pro (High)"),
        source: "user-initial-mapping",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M7",
        model_id: "gemini-3-pro-preview",
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
        model_id: "gemini-3.5-flash",
        label: Some("Gemini 3.5 Flash (High)"),
        source: "captured-antigravity-get-user-status",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M16",
        model_id: "gemini-3.1-pro-preview",
        label: Some("Gemini 3.1 Pro (High)"),
        source: "captured-antigravity-get-user-status",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M187",
        model_id: "gemini-3.5-flash",
        label: Some("Gemini 3.5 Flash (Low)"),
        source: "captured-antigravity-get-user-status",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M20",
        model_id: "gemini-3.5-flash",
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

fn fetch_dynamic_model_aliases(
    connections: &[AntigravityConnection],
) -> HashMap<String, ModelAlias> {
    let mut aliases = HashMap::new();

    for connection in connections {
        let response = match rpc_request(
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
        ) {
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
    let mut aliases = static_model_aliases();
    let Some(path) = model_alias_history_path(sessions_dir) else {
        return Ok(aliases);
    };
    if !path.exists() {
        return Ok(aliases);
    }

    let content = std::fs::read_to_string(&path)?;
    let history: ModelAliasHistory = serde_json::from_str(&content)?;
    aliases.extend(history.aliases.into_iter().filter(|(key, entry)| {
        key.to_ascii_lowercase().starts_with("model")
            && entry.raw_model_id.to_ascii_lowercase().starts_with("model")
    }).map(|(key, entry)| {
        (
            key,
            ModelAlias {
                raw_model_id: entry.raw_model_id,
                model_id: entry.model_id,
                label: entry.label,
                source: entry.source,
            },
        )
    }));
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
        let mut loaded = serde_json::from_str::<ModelAliasHistory>(&content).unwrap_or_else(|_| ModelAliasHistory {
            version: MODEL_ALIAS_HISTORY_VERSION,
            updated_at: now.clone(),
            aliases: BTreeMap::new(),
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
            let target_model_id = if let Some(normalized) = format_normalization_fallback(&alias.model_id) {
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
    if !flash_a_normalized && (normalized.contains('.') || normalized.contains('_') || normalized.contains(' ')) {
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

    // 4. Uniformly strip thinking/performance level tokens (high, low, medium, thinking)
    let mut tier_stripped = false;
    if !flash_a_normalized && (normalized.contains("-high")
        || normalized.contains("-low")
        || normalized.contains("-medium")
        || normalized.contains("-thinking"))
    {
        let tokens: Vec<&str> = normalized.split('-').collect();
        let filtered: Vec<&str> = tokens
            .into_iter()
            .filter(|&t| t != "high" && t != "low" && t != "medium" && t != "thinking")
            .collect();
        normalized = filtered.join("-");
        tier_stripped = true;
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

    if prefix_stripped || chars_replaced || tier_stripped || version_formatted || preview_processed || flash_a_normalized
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

pub fn detect_antigravity_connections() -> Result<Vec<AntigravityConnection>> {
    let candidates = detect_process_candidates()?;
    let mut connections = Vec::new();

    for candidate in candidates {
        let ports = candidate_probe_ports(&candidate, find_listening_ports(candidate.pid)?);
        for port in ports {
            if let Some(scheme) = probe_heartbeat(port, candidate.csrf_token.as_deref()) {
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
    let mut dirs = Vec::new();
    if active_kinds.contains(&AntigravityRuntimeKind::Desktop) {
        dirs.push((
            home.join(".gemini")
                .join("antigravity")
                .join("conversations"),
            AntigravityRuntimeKind::Desktop,
        ));
    }
    if active_kinds.contains(&AntigravityRuntimeKind::Cli) {
        dirs.push((
            home.join(".gemini")
                .join("antigravity-cli")
                .join("conversations"),
            AntigravityRuntimeKind::Cli,
        ));
    }

    let mut session_ids = Vec::new();
    for (dir, runtime_kind) in dirs {
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
                                    let mut modified_ms = path
                                        .metadata()
                                        .ok()
                                        .and_then(|metadata| metadata.modified().ok())
                                        .and_then(system_time_to_millis);
                                    if ext == "db" {
                                        for extra_ext in &["db-wal", "db-shm"] {
                                            let extra_path = path.with_extension(extra_ext);
                                            if extra_path.exists() {
                                                if let Some(extra_ms) = extra_path
                                                    .metadata()
                                                    .ok()
                                                    .and_then(|metadata| metadata.modified().ok())
                                                    .and_then(system_time_to_millis)
                                                {
                                                    modified_ms = match modified_ms {
                                                        Some(m) => Some(m.max(extra_ms)),
                                                        None => Some(extra_ms),
                                                    };
                                                }
                                            }
                                        }
                                    }
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

fn probe_heartbeat(port: u16, csrf_token: Option<&str>) -> Option<&'static str> {
    for scheme in ["https", "http"] {
        if probe_heartbeat_with_scheme(scheme, port, csrf_token) {
            return Some(scheme);
        }
    }
    None
}

fn probe_heartbeat_with_scheme(scheme: &'static str, port: u16, csrf_token: Option<&str>) -> bool {
    let body = json!({ "uuid": "00000000-0000-0000-0000-000000000000" });
    let Ok((status, text)) = language_server_request_text(
        scheme,
        port,
        csrf_token,
        "Heartbeat",
        &body,
        Duration::from_secs(2),
        ANTIGRAVITY_RPC_BODY_CAP,
    ) else {
        return false;
    };

    status == 200
        && heartbeat_response_looks_well_formed(&text)
        && probe_endpoint_identity(scheme, port, csrf_token)
}

fn heartbeat_response_looks_well_formed(body: &str) -> bool {
    let trimmed = body.trim_start();
    let json_start = trimmed.find(['{', '[']).map(|idx| &trimmed[idx..]);
    let Some(slice) = json_start else {
        return false;
    };
    serde_json::from_str::<Value>(slice).is_ok()
}

fn probe_endpoint_identity(scheme: &'static str, port: u16, csrf_token: Option<&str>) -> bool {
    for method in [
        "GetCascadeTrajectoryGeneratorMetadata",
        "GetAllCascadeTrajectories",
    ] {
        if let Some(body) = identity_probe_request(scheme, port, csrf_token, method) {
            if response_contains_antigravity_marker(method, &body) {
                return true;
            }
        }
    }
    false
}

fn identity_probe_request(
    scheme: &'static str,
    port: u16,
    csrf_token: Option<&str>,
    method: &str,
) -> Option<String> {
    let (status, text) = language_server_request_text(
        scheme,
        port,
        csrf_token,
        method,
        &json!({}),
        Duration::from_secs(2),
        ANTIGRAVITY_RPC_BODY_CAP,
    )
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

fn language_server_request_text(
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
    let csrf_token = csrf_token
        .filter(|token| !token.trim().is_empty())
        .map(String::from);

    std::thread::spawn(move || -> Result<(u16, String)> {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .danger_accept_invalid_certs(true)
            .build()?;
        let mut request = client
            .post(url)
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .body(body_text);
        if let Some(csrf_token) = csrf_token {
            request = request.header("X-Codeium-Csrf-Token", csrf_token);
        }
        let response = request.send()?;
        let status_code = response.status().as_u16();

        if let Some(length) = response.content_length() {
            if length > max_body_bytes as u64 {
                anyhow::bail!("RPC body of {length} bytes exceeds {max_body_bytes} cap");
            }
        }

        let bytes = response.bytes()?;
        if bytes.len() > max_body_bytes {
            anyhow::bail!(
                "RPC body of {} bytes exceeds {} cap",
                bytes.len(),
                max_body_bytes
            );
        }
        Ok((status_code, String::from_utf8(bytes.to_vec())?))
    })
    .join()
    .map_err(|_| anyhow::anyhow!("Antigravity language server request thread panicked"))?
}

fn rpc_request(connection: &AntigravityConnection, method: &str, body: &Value) -> Result<Value> {
    let (status_code, response_body) = language_server_request_text(
        &connection.scheme,
        connection.port,
        connection.csrf_token.as_deref(),
        method,
        body,
        Duration::from_secs(10),
        ANTIGRAVITY_RPC_BODY_CAP,
    )?;

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
                150_i64, 50_i64, 20_i64, 0_i64, 10_i64, "resp-456", "2023-01-01", "antigravity-v2"
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
                150_i64, 50_i64, 20_i64, 0_i64, 10_i64, "resp-456", "2023-01-01", "antigravity-v2"
            ],
        ).unwrap();

        conn.execute(
            "INSERT OR REPLACE INTO session_usage (id, session_id, client, model_id, provider_id, timestamp, step_index, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, response_id, pricing_day, parser_version)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                "resp-456", "sess-123", "antigravity-cli", "gemini-3-pro-preview", "google", 1672531202000_i64, 0_i64,
                200_i64, 60_i64, 30_i64, 0_i64, 15_i64, "resp-456", "2023-01-01", "antigravity-v2"
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
                    150_i64, 50_i64, 20_i64, 0_i64, 10_i64, &logical_key, "2023-01-01", "antigravity-v2"
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
            rusqlite::params!["msg-1", "sess-1", "antigravity-cli", "MODEL_PLACEHOLDER_M26", "anthropic", 1672531201000_i64, 0_i64, 150_i64, 50_i64, 0_i64, 0_i64, 0_i64, "2023-01-01", "antigravity-v2"],
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
            rusqlite::params!["msg-2", "sess-2", "antigravity-cli", "MODEL_PLACEHOLDER_M20", "google", 1672531202000_i64, 0_i64, 200_i64, 60_i64, 0_i64, 0_i64, 0_i64, "2023-01-01", "antigravity-v2"],
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
            rusqlite::params!["msg-3", "sess-3", "antigravity-cli", "claude-opus-4.6-thinking", "anthropic", 1672531203000_i64, 0_i64, 300_i64, 70_i64, 0_i64, 0_i64, 0_i64, "2023-01-01", "antigravity-v2"],
        ).unwrap();

        normalize_cached_antigravity_artifacts(&sessions_dir, &HashMap::new()).unwrap();

        let parser = AntigravitySessionParser::new()
            .with_custom_paths(vec![sessions_dir])
            .with_skip_sync(true);
        let messages = parser.parse_sessions(None).unwrap();

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].model_id, "claude-opus-4-6");
        assert_eq!(messages[0].provider_id, "anthropic");
        assert_eq!(messages[1].model_id, "gemini-3.5-flash");
        assert_eq!(messages[1].provider_id, "google");
        assert_eq!(messages[2].model_id, "claude-opus-4-6");
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
            rusqlite::params!["msg-1", "sess-1", "antigravity-cli", "MODEL_PLACEHOLDER_M20", "google", 1672531201000_i64, 0_i64, 1_i64, 2_i64, 0_i64, 0_i64, 0_i64, "2023-01-01", "antigravity-v2"],
        ).unwrap();

        normalize_cached_antigravity_artifacts(&sessions_dir, &HashMap::new()).unwrap();

        let mut stmt = conn
            .prepare("SELECT model_id FROM sessions WHERE session_id = 'sess-1'")
            .unwrap();
        let session_model: String = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(session_model, "gemini-3.1-pro-preview");

        let mut stmt2 = conn
            .prepare("SELECT model_id FROM session_usage WHERE id = 'msg-1'")
            .unwrap();
        let usage_model: String = stmt2.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(usage_model, "gemini-3.5-flash");
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
            "gemini-3.5-flash"
        );

        let loaded = load_model_alias_history_map(&sessions_dir).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M132", &loaded),
            "gemini-3.5-flash"
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
            "claude-opus-4-6"
        );
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M37", &saved),
            "gemini-3.1-pro-preview"
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
            "gemini-3-pro-preview"
        );
        assert_eq!(
            resolve_antigravity_model_id("antigravity-gemini-3-pro-low"),
            "gemini-3-pro-preview"
        );
        assert_eq!(
            resolve_antigravity_model_id("gemini-3.0-pro-high"),
            "gemini-3-pro-preview"
        );
        assert_eq!(
            resolve_antigravity_model_id("gemini-3.1-pro-high"),
            "gemini-3.1-pro-preview"
        );
        assert_eq!(
            resolve_antigravity_model_id("gemini-3-1-pro-preview-low"),
            "gemini-3.1-pro-preview"
        );
        assert_eq!(
            resolve_antigravity_model_id("gemini-3-pro-image"),
            "gemini-3-pro-preview-image"
        );
    }

    #[test]
    fn test_antigravity_label_to_model_id_handles_future_label_shapes() {
        assert_eq!(
            antigravity_label_to_model_id("Claude Opus 4.5 (Thinking)").as_deref(),
            Some("claude-opus-4-5")
        );
        assert_eq!(
            antigravity_label_to_model_id("Claude Sonnet 4.5 (Thinking)").as_deref(),
            Some("claude-sonnet-4-5")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3.0 Pro Preview (High)").as_deref(),
            Some("gemini-3-pro-preview")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3.0 Pro (High)").as_deref(),
            Some("gemini-3-pro-preview")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3.1 Pro (High)").as_deref(),
            Some("gemini-3.1-pro-preview")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3.1 Pro Preview (Low)").as_deref(),
            Some("gemini-3.1-pro-preview")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3 Pro Preview (Low)").as_deref(),
            Some("gemini-3-pro-preview")
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
        let wal_mtime = wal_file_path.metadata().unwrap().modified().unwrap();

        std::fs::write(anti_dir.join(non_uuid), "mock").unwrap();
        std::fs::write(cli_dir.join(txt_file), "mock").unwrap();

        let discovered = discover_local_conversation_ids_from_home(
            temp_dir.path(),
            &[AntigravityRuntimeKind::Desktop, AntigravityRuntimeKind::Cli],
        );

        assert_eq!(discovered.len(), 2);
        assert!(discovered
            .iter()
            .any(|entry| entry.session_id == "12345678901234567890"
                && entry.modified_ms.is_some()
                && entry.runtime_kind == AntigravityRuntimeKind::Desktop));

        let db_entry = discovered
            .iter()
            .find(|entry| entry.session_id == "abcdefabcdefabcdefabcdef")
            .unwrap();
        assert_eq!(db_entry.runtime_kind, AntigravityRuntimeKind::Cli);
        let db_entry_mtime_ms = db_entry.modified_ms.unwrap();
        let wal_mtime_ms = system_time_to_millis(wal_mtime).unwrap();
        assert_eq!(db_entry_mtime_ms, wal_mtime_ms);
    }
}
