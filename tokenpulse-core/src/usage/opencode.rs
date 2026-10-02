use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};

use anyhow::Result;
use chrono::{Local, LocalResult, NaiveDate, TimeZone};
use rusqlite::{params, params_from_iter, Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

const PARSER_VERSION: &str = "opencode-v3";

/// Token fields read from a message's `data` JSON, in the column order
/// `OpenCodeRow::from_sql` expects. Only these few fields are extracted: an
/// assistant's `data` also carries its full content, typically several KB.
const TOKEN_FIELDS: [&str; 5] = ["input", "output", "reasoning", "cache.read", "cache.write"];

pub struct OpenCodeSessionParser;

impl OpenCodeSessionParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OpenCodeSessionParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionParser for OpenCodeSessionParser {
    fn provider_name(&self) -> &str {
        "opencode"
    }

    fn session_paths(&self) -> Vec<PathBuf> {
        discover_databases(&opencode_data_dir(), std::env::var_os("OPENCODE_DB"))
    }

    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        Ok(parse_databases(&self.session_paths(), since))
    }

    fn parser_version(&self) -> &str {
        PARSER_VERSION
    }
}

/// `$XDG_DATA_HOME/opencode`, else `~/.local/share/opencode`. OpenCode uses the
/// XDG layout on every platform, macOS included.
fn opencode_data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(".local")
                .join("share")
        })
        .join("opencode")
}

/// Every OpenCode database to scan.
///
/// `OPENCODE_DB` replaces discovery, as it does in OpenCode itself; a relative
/// value resolves against the data dir. Otherwise `opencode.db` (the
/// latest/dev/beta/next/prod channels) plus each `opencode-<channel>.db`
/// sibling, which also covers the 2.x preview's `opencode-next.db`.
fn discover_databases(data_dir: &Path, db_override: Option<OsString>) -> Vec<PathBuf> {
    if let Some(value) = db_override.filter(|value| !value.is_empty()) {
        // `join` keeps an absolute override as-is.
        let path = data_dir.join(value);
        return if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
    }

    let mut channels: Vec<PathBuf> = std::fs::read_dir(data_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            // `.db` as the suffix also rules out `-wal`, `-shm` and `-journal`.
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("opencode-") && name.ends_with(".db"))
        })
        .filter(|path| path.is_file())
        .collect();
    channels.sort();

    let mut paths = Vec::new();
    let default = data_dir.join("opencode.db");
    if default.is_file() {
        paths.push(default);
    }
    paths.extend(channels);
    paths
}

/// Parse every database, counting each message id once across all of them.
fn parse_databases(paths: &[PathBuf], since: Option<NaiveDate>) -> Vec<UnifiedMessage> {
    let since_timestamp_ms = since.and_then(start_of_day_timestamp_ms);
    let mut seen = HashSet::new();
    let mut messages = Vec::new();

    for path in paths {
        debug!("Parsing OpenCode database: {:?}", path);
        let conn = match open_read_only(path) {
            Ok(connection) => connection,
            Err(error) => {
                warn!("Failed to open database {:?}: {}", path, error);
                continue;
            }
        };

        for row in load_rows(&conn, since_timestamp_ms) {
            if seen.insert(row.id.clone()) {
                messages.push(row.into_message());
            }
        }
    }

    if let Some(since) = since {
        messages.retain(|message| message_on_or_after(message, since));
    }
    messages.sort_by_key(|message| message.timestamp);
    messages
}

/// OpenCode owns these databases, and may be writing to them while we read.
fn open_read_only(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// Token-bearing rows of one database: OpenCode 2.x `session_message` first,
/// then 1.x `message` rows whose id 2.x does not have.
///
/// v1 is read even after the v1 → v2 migration has completed. The migration
/// never deletes v1 rows but drops some assistant messages (the one that
/// produced a compaction summary, subtask parents, rows failing validation),
/// so their usage exists only in v1 — and a user who downgrades to 1.x writes
/// to v1 again. Migrated messages keep their id, which is what makes the
/// message-level fallback exact.
fn load_rows(conn: &Connection, since_timestamp_ms: Option<i64>) -> Vec<OpenCodeRow> {
    let has_v2 = table_exists(conn, "session_message");
    let mut rows = Vec::new();

    if has_v2 {
        match load_v2_rows(conn, since_timestamp_ms) {
            Ok(v2_rows) => rows.extend(v2_rows),
            Err(error) => warn!("Failed to read OpenCode session_message: {}", error),
        }
    }
    if table_exists(conn, "message") {
        match load_v1_rows(conn, since_timestamp_ms, has_v2) {
            Ok(v1_rows) => rows.extend(v1_rows),
            Err(error) => warn!("Failed to read OpenCode message: {}", error),
        }
    }

    rows
}

fn load_v2_rows(
    conn: &Connection,
    since_timestamp_ms: Option<i64>,
) -> rusqlite::Result<Vec<OpenCodeRow>> {
    let has_sessions = table_exists(conn, "session_v2");
    let cutoffs = if has_sessions {
        fork_copy_cutoffs(conn)?
    } else {
        HashMap::new()
    };

    let mut stmt = conn.prepare(&v2_query(since_timestamp_ms.is_some(), has_sessions))?;
    let rows = stmt
        .query_map(params_from_iter(since_timestamp_ms), |row| {
            Ok((OpenCodeRow::from_sql(row)?, row.get::<_, i64>(11)?))
        })?
        .flatten()
        .filter(|(row, seq)| {
            cutoffs
                .get(&row.session_id)
                .is_none_or(|cutoff| seq > cutoff)
        })
        .map(|(row, _)| row)
        .collect();
    Ok(rows)
}

/// Assistant messages, plus completed or failed compactions, that carry
/// tokens. A failed compaction has no `model`, so the session's model stands
/// in for it.
fn v2_query(windowed: bool, has_sessions: bool) -> String {
    let (session_model, session_provider, join) = if has_sessions {
        (
            "json_extract(s.model, '$.id')",
            "json_extract(s.model, '$.providerID')",
            "\nLEFT JOIN session_v2 s ON s.id = sm.session_id",
        )
    } else {
        ("NULL", "NULL", "")
    };
    format!(
        "SELECT sm.id, sm.session_id, sm.time_created,
       COALESCE(json_extract(sm.data, '$.model.id'), {session_model}),
       COALESCE(json_extract(sm.data, '$.model.providerID'), {session_provider}),
       {tokens},
       {data_time},
       sm.seq
FROM session_message sm{join}
WHERE sm.type IN ('assistant', 'compaction')
  AND json_valid(sm.data)
  AND json_type(sm.data, '$.tokens') = 'object'{window}",
        tokens = token_columns("sm.data"),
        data_time = data_time_column("sm.data"),
        window = if windowed {
            "\n  AND sm.time_created >= ?1"
        } else {
            ""
        },
    )
}

fn load_v1_rows(
    conn: &Connection,
    since_timestamp_ms: Option<i64>,
    has_v2: bool,
) -> rusqlite::Result<Vec<OpenCodeRow>> {
    // Early 1.x schemas had no `time_created` column; their rows are dated from
    // `data.time` and filtered by `since` after parsing.
    let has_time = column_exists(conn, "message", "time_created");
    let window = since_timestamp_ms.filter(|_| has_time);

    let mut stmt = conn.prepare(&v1_query(window.is_some(), has_time, has_v2))?;
    let rows = stmt
        .query_map(params_from_iter(window), OpenCodeRow::from_sql)?
        .flatten()
        .collect();
    Ok(rows)
}

/// Assistant rows of the 1.x `message` table that 2.x has no copy of.
///
/// The time window goes through `rowid IN (...)` so SQLite answers it from the
/// covering index `(session_id, time_created, id)` instead of scanning every
/// row's `data`.
fn v1_query(windowed: bool, has_time: bool, has_v2: bool) -> String {
    format!(
        "SELECT m.id, m.session_id, {time_created},
       json_extract(m.data, '$.modelID'),
       json_extract(m.data, '$.providerID'),
       {tokens},
       {data_time}
FROM message m
WHERE {window}json_valid(m.data)
  AND json_extract(m.data, '$.role') = 'assistant'
  AND json_type(m.data, '$.tokens') = 'object'{dedup}",
        time_created = if has_time { "m.time_created" } else { "NULL" },
        tokens = token_columns("m.data"),
        data_time = data_time_column("m.data"),
        window = if windowed {
            "m.rowid IN (SELECT rowid FROM message WHERE time_created >= ?1)\n  AND "
        } else {
            ""
        },
        dedup = if has_v2 {
            "\n  AND NOT EXISTS (SELECT 1 FROM session_message s WHERE s.id = m.id)"
        } else {
            ""
        },
    )
}

fn token_columns(data: &str) -> String {
    TOKEN_FIELDS
        .iter()
        .map(|field| format!("CAST(json_extract({data}, '$.tokens.{field}') AS INTEGER)"))
        .collect::<Vec<_>>()
        .join(",\n       ")
}

fn data_time_column(data: &str) -> String {
    format!(
        "COALESCE(json_extract({data}, '$.time.created'), \
         json_extract({data}, '$.time.completed'))"
    )
}

/// The `seq` up to which each forked session's rows are copies of its parent.
///
/// Forking copies the parent's rows into the new session under new ids but
/// with the same `seq` and identical `data`, tokens included, so counting them
/// would bill the parent's history twice. A boundary that cannot be resolved —
/// parent or boundary message missing, unparsable JSON — leaves the fork with
/// no cutoff, so all of its rows are kept.
fn fork_copy_cutoffs(conn: &Connection) -> rusqlite::Result<HashMap<String, i64>> {
    if !column_exists(conn, "session_v2", "fork_session_id")
        || !column_exists(conn, "session_v2", "fork_boundary")
    {
        return Ok(HashMap::new());
    }

    let forks: Vec<(String, String, String)> = conn
        .prepare(
            "SELECT id, fork_session_id, fork_boundary FROM session_v2
             WHERE fork_session_id IS NOT NULL AND fork_boundary IS NOT NULL",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .flatten()
        .collect();

    let mut boundary_seq =
        conn.prepare("SELECT seq FROM session_message WHERE id = ?1 AND session_id = ?2")?;
    let mut last_seq_before =
        conn.prepare("SELECT MAX(seq) FROM session_message WHERE session_id = ?1 AND seq < ?2")?;

    let mut cutoffs = HashMap::new();
    for (fork_id, parent_id, boundary) in forks {
        let Ok(boundary) = serde_json::from_str::<ForkBoundary>(&boundary) else {
            continue;
        };
        let Some(seq) = boundary_seq
            .query_row(params![boundary.message_id, parent_id], |row| row.get(0))
            .optional()?
        else {
            continue;
        };
        let cutoff: Option<i64> = match boundary.kind.as_str() {
            "through" => Some(seq),
            // No earlier row means nothing was copied.
            "before" => last_seq_before.query_row(params![parent_id, seq], |row| row.get(0))?,
            _ => None,
        };
        if let Some(cutoff) = cutoff {
            cutoffs.insert(fork_id, cutoff);
        }
    }
    Ok(cutoffs)
}

#[derive(Debug, Deserialize)]
struct ForkBoundary {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "messageID")]
    message_id: String,
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |_| Ok(()),
    )
    .is_ok()
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2",
        [table, column],
        |_| Ok(()),
    )
    .is_ok()
}

fn start_of_day_timestamp_ms(date: NaiveDate) -> Option<i64> {
    let start = date.and_hms_opt(0, 0, 0)?;
    match Local.from_local_datetime(&start) {
        LocalResult::Single(dt) => Some(dt.timestamp_millis()),
        LocalResult::Ambiguous(early, _) => Some(early.timestamp_millis()),
        LocalResult::None => None,
    }
}

fn message_on_or_after(message: &UnifiedMessage, since: NaiveDate) -> bool {
    NaiveDate::parse_from_str(&message.date, "%Y-%m-%d")
        .map(|date| date >= since)
        .unwrap_or(false)
}

/// `data.time` values are milliseconds, or seconds in some older rows.
fn epoch_millis(timestamp: f64) -> i64 {
    if timestamp > 10_000_000_000.0 {
        timestamp as i64
    } else {
        (timestamp * 1000.0) as i64
    }
}

/// One token-bearing message, as selected by `v1_query` / `v2_query`.
#[derive(Debug)]
struct OpenCodeRow {
    id: String,
    session_id: String,
    time_created: Option<i64>,
    model_id: Option<String>,
    provider_id: Option<String>,
    tokens: TokenBreakdown,
    data_time_created: Option<f64>,
}

impl OpenCodeRow {
    fn from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let token = |index: usize| -> rusqlite::Result<i64> {
            Ok(row.get::<_, Option<i64>>(index)?.unwrap_or(0).max(0))
        };
        Ok(Self {
            id: row.get(0)?,
            session_id: row.get(1)?,
            time_created: row.get(2)?,
            model_id: row.get(3)?,
            provider_id: row.get(4)?,
            tokens: TokenBreakdown {
                input: token(5)?,
                output: token(6)?,
                reasoning: token(7)?,
                cache_read: token(8)?,
                cache_write: token(9)?,
            },
            data_time_created: row.get(10)?,
        })
    }

    fn into_message(self) -> UnifiedMessage {
        let timestamp = self
            .time_created
            .or_else(|| self.data_time_created.map(epoch_millis))
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

        // The message id is the ledger key in both layouts, so a message keeps
        // its key when its session migrates from v1 to v2.
        UnifiedMessage::new(
            "opencode",
            self.model_id.unwrap_or_else(|| "unknown".to_string()),
            self.provider_id.unwrap_or_else(|| "unknown".to_string()),
            self.session_id,
            self.id,
            timestamp,
            self.tokens,
        )
        .with_parser_version(PARSER_VERSION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const V1_SCHEMA: &str = "
        CREATE TABLE `message` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL
        );
        CREATE INDEX `message_session_time_created_id_idx` ON `message` (`session_id`,`time_created`,`id`);";

    const V2_SCHEMA: &str = "
        CREATE TABLE `session_v2` (
          `id` text PRIMARY KEY,
          `parent_id` text,
          `fork_session_id` text,
          `fork_boundary` text,
          `model` text,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL
        );
        CREATE TABLE `session_message` (
          `id` text PRIMARY KEY,
          `session_id` text NOT NULL,
          `type` text NOT NULL,
          `seq` integer NOT NULL,
          `time_created` integer NOT NULL,
          `time_updated` integer NOT NULL,
          `data` text NOT NULL
        );
        CREATE UNIQUE INDEX `session_message_session_seq_idx` ON `session_message` (`session_id`,`seq`);
        CREATE INDEX `session_message_session_type_seq_idx` ON `session_message` (`session_id`,`type`,`seq`);
        CREATE INDEX `session_message_session_time_created_id_idx` ON `session_message` (`session_id`,`time_created`,`id`);
        CREATE INDEX `session_message_time_created_idx` ON `session_message` (`time_created`);";

    const T0: i64 = 1_700_000_000_000;

    struct Db {
        path: PathBuf,
        conn: Connection,
    }

    impl Db {
        fn create(dir: &Path, name: &str, schemas: &[&str]) -> Self {
            let path = dir.join(name);
            let conn = Connection::open(&path).unwrap();
            for schema in schemas {
                conn.execute_batch(schema).unwrap();
            }
            Self { path, conn }
        }

        fn v1(&self, id: &str, session: &str, time_created: i64, data: Value) {
            self.conn
                .execute(
                    "INSERT INTO message (id, session_id, time_created, time_updated, data)
                     VALUES (?1, ?2, ?3, ?3, ?4)",
                    params![id, session, time_created, data.to_string()],
                )
                .unwrap();
        }

        fn v1_raw(&self, id: &str, session: &str, time_created: i64, data: &str) {
            self.conn
                .execute(
                    "INSERT INTO message (id, session_id, time_created, time_updated, data)
                     VALUES (?1, ?2, ?3, ?3, ?4)",
                    params![id, session, time_created, data],
                )
                .unwrap();
        }

        fn session(&self, id: &str, model: Option<Value>, fork: Option<(&str, &str)>) {
            self.conn
                .execute(
                    "INSERT INTO session_v2 (id, fork_session_id, fork_boundary, model, time_created, time_updated)
                     VALUES (?1, ?2, ?3, ?4, 0, 0)",
                    params![
                        id,
                        fork.map(|(parent, _)| parent),
                        fork.map(|(_, boundary)| boundary),
                        model.map(|model| model.to_string())
                    ],
                )
                .unwrap();
        }

        fn v2(&self, id: &str, session: &str, kind: &str, seq: i64, data: Value) {
            self.v2_at(id, session, kind, seq, T0, data);
        }

        fn v2_at(
            &self,
            id: &str,
            session: &str,
            kind: &str,
            seq: i64,
            time_created: i64,
            data: Value,
        ) {
            self.conn
                .execute(
                    "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                    params![id, session, kind, seq, time_created, data.to_string()],
                )
                .unwrap();
        }

        fn v2_raw(&self, id: &str, session: &str, kind: &str, seq: i64, data: &str) {
            self.conn
                .execute(
                    "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                    params![id, session, kind, seq, T0, data],
                )
                .unwrap();
        }
    }

    fn tokens(input: i64) -> Value {
        json!({
            "input": input,
            "output": 10,
            "reasoning": 1,
            "cache": { "read": 5, "write": 2 }
        })
    }

    fn v1_assistant(model: &str, input: i64) -> Value {
        json!({
            "role": "assistant",
            "modelID": model,
            "providerID": "anthropic",
            "cost": 0.05,
            "tokens": tokens(input),
            "time": { "created": T0 }
        })
    }

    fn v2_assistant(model: &str, input: i64) -> Value {
        json!({
            "agent": "build",
            "model": { "id": model, "providerID": "opencode", "variant": "high" },
            "content": [{ "type": "text", "text": "hello" }],
            "cost": 0.01,
            "tokens": tokens(input),
            "time": { "created": T0, "completed": T0 + 1000 }
        })
    }

    fn parse(paths: &[&Db]) -> Vec<UnifiedMessage> {
        let paths: Vec<PathBuf> = paths.iter().map(|db| db.path.clone()).collect();
        parse_databases(&paths, None)
    }

    fn keys(messages: &[UnifiedMessage]) -> Vec<&str> {
        let mut keys: Vec<&str> = messages.iter().map(|m| m.message_key.as_str()).collect();
        keys.sort();
        keys
    }

    fn by_key<'a>(messages: &'a [UnifiedMessage], key: &str) -> &'a UnifiedMessage {
        messages
            .iter()
            .find(|message| message.message_key == key)
            .unwrap_or_else(|| panic!("{key} missing from {:?}", keys(messages)))
    }

    fn local_noon_ms(date: NaiveDate) -> i64 {
        Local
            .from_local_datetime(&date.and_hms_opt(12, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn test_parse_opencode_structure() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V1_SCHEMA]);
        db.v1(
            "msg_123",
            "ses_456",
            T0,
            json!({
                "id": "msg_123",
                "sessionID": "ses_456",
                "role": "assistant",
                "modelID": "claude-sonnet-4",
                "providerID": "anthropic",
                "cost": 0.05,
                "tokens": {
                    "input": 1000,
                    "output": 500,
                    "reasoning": 100,
                    "cache": { "read": 200, "write": 50 }
                },
                "time": { "created": 1700000000000.0 }
            }),
        );

        let messages = parse(&[&db]);

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.model_id, "claude-sonnet-4");
        assert_eq!(msg.provider_id, "anthropic");
        assert_eq!(msg.session_id, "ses_456");
        assert_eq!(msg.message_key, "msg_123");
        assert_eq!(msg.timestamp, T0);
        assert_eq!(msg.tokens.input, 1000);
        assert_eq!(msg.tokens.output, 500);
        assert_eq!(msg.tokens.reasoning, 100);
        assert_eq!(msg.tokens.cache_read, 200);
        assert_eq!(msg.tokens.cache_write, 50);
        assert_eq!(msg.parser_version, PARSER_VERSION);
    }

    #[test]
    fn test_negative_values_clamped_to_zero() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V1_SCHEMA]);
        db.v1(
            "msg_negative",
            "ses_negative",
            T0,
            json!({
                "role": "assistant",
                "modelID": "claude-sonnet-4",
                "providerID": "anthropic",
                "cost": -0.05,
                "tokens": {
                    "input": -100,
                    "output": -50,
                    "reasoning": -25,
                    "cache": { "read": -200, "write": -10 }
                },
                "time": { "created": 1700000000000.0 }
            }),
        );

        let messages = parse(&[&db]);

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.tokens.input, 0);
        assert_eq!(msg.tokens.output, 0);
        assert_eq!(msg.tokens.cache_read, 0);
        assert_eq!(msg.tokens.cache_write, 0);
        assert_eq!(msg.tokens.reasoning, 0);
    }

    #[test]
    fn test_open_code_time_helper() {
        assert_eq!(epoch_millis(1700000000000.0), 1700000000000);
        assert_eq!(epoch_millis(1700000000.0), 1700000000000);
    }

    #[test]
    fn parse_row_ignores_source_cost_for_centralized_pricing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V1_SCHEMA]);
        db.v1(
            "msg_source_cost",
            "ses_source_cost",
            T0,
            v1_assistant("claude-sonnet-4", 1000),
        );

        let messages = parse(&[&db]);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].cost, 0.0);
    }

    #[test]
    fn parse_row_leaves_missing_source_cost_for_store_level_pricing() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V1_SCHEMA]);
        db.v1(
            "msg_missing_cost",
            "ses_missing_cost",
            T0,
            json!({
                "role": "assistant",
                "modelID": "deepseek-ai/deepseek-v4-flash",
                "providerID": "nvidia",
                "tokens": {
                    "input": 1000,
                    "output": 500,
                    "reasoning": 0,
                    "cache": { "read": 0, "write": 0 }
                },
                "time": { "created": 1700000000000.0 }
            }),
        );

        let messages = parse(&[&db]);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].cost, 0.0);
        assert_eq!(messages[0].model_id, "deepseek-ai/deepseek-v4-flash");
        assert_eq!(messages[0].provider_id, "nvidia");
    }

    #[test]
    fn v1_only_database_parses_assistant_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V1_SCHEMA]);
        db.v1("msg_a", "ses_1", T0, v1_assistant("claude-sonnet-4", 100));
        db.v1(
            "msg_user",
            "ses_1",
            T0,
            json!({ "role": "user", "time": { "created": T0 } }),
        );
        // A failed assistant reply with no usage.
        db.v1(
            "msg_err",
            "ses_1",
            T0,
            json!({ "role": "assistant", "modelID": "claude-sonnet-4" }),
        );
        db.v1_raw("msg_bad", "ses_1", T0, "{not json");

        let messages = parse(&[&db]);

        assert_eq!(keys(&messages), ["msg_a"]);
        let msg = &messages[0];
        assert_eq!(msg.client, "opencode");
        assert_eq!(msg.model_id, "claude-sonnet-4");
        assert_eq!(msg.provider_id, "anthropic");
        assert_eq!(msg.session_id, "ses_1");
        assert_eq!(msg.tokens.input, 100);
        assert_eq!(msg.tokens.output, 10);
        assert_eq!(msg.tokens.reasoning, 1);
        assert_eq!(msg.tokens.cache_read, 5);
        assert_eq!(msg.tokens.cache_write, 2);
    }

    /// Pre-`time_created` 1.x schemas are dated from `data.time`.
    #[test]
    fn v1_schema_without_time_created_dates_rows_from_data() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(
            dir.path(),
            "opencode.db",
            &["CREATE TABLE message (id text PRIMARY KEY, session_id text, data text);"],
        );
        db.conn
            .execute(
                "INSERT INTO message (id, session_id, data) VALUES ('msg_old', 'ses_old', ?1)",
                [json!({
                    "role": "assistant",
                    "modelID": "claude-sonnet-4",
                    "providerID": "anthropic",
                    "tokens": tokens(40),
                    "time": { "created": 1_700_000_000.0 }
                })
                .to_string()],
            )
            .unwrap();

        let messages = parse_databases(&[db.path.clone()], NaiveDate::from_ymd_opt(2020, 1, 1));

        assert_eq!(keys(&messages), ["msg_old"]);
        assert_eq!(messages[0].timestamp, T0);
    }

    #[test]
    fn v2_only_database_parses_nested_model_and_ignores_non_token_types() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V2_SCHEMA]);
        db.session("ses_1", None, None);
        db.v2(
            "msg_user",
            "ses_1",
            "user",
            1,
            json!({ "text": "hi", "time": { "created": T0 } }),
        );
        db.v2(
            "msg_a",
            "ses_1",
            "assistant",
            2,
            v2_assistant("muse-spark-1.3", 100),
        );
        // Errored assistant: no tokens.
        db.v2(
            "msg_err",
            "ses_1",
            "assistant",
            3,
            json!({
                "model": { "id": "muse-spark-1.3", "providerID": "opencode" },
                "time": { "created": T0 }
            }),
        );
        // Usage-free types, even when they carry a `tokens` key.
        for (seq, kind) in ["synthetic", "system", "shell", "idle", "model-switched"]
            .into_iter()
            .enumerate()
        {
            db.v2(
                &format!("msg_{kind}"),
                "ses_1",
                kind,
                10 + seq as i64,
                json!({ "tokens": tokens(999), "time": { "created": T0 } }),
            );
        }
        db.v2_raw("msg_bad", "ses_1", "assistant", 20, "{not json");

        let messages = parse(&[&db]);

        assert_eq!(keys(&messages), ["msg_a"]);
        let msg = &messages[0];
        assert_eq!(msg.model_id, "muse-spark-1.3");
        assert_eq!(msg.provider_id, "opencode");
        assert_eq!(msg.session_id, "ses_1");
        assert_eq!(msg.timestamp, T0);
        assert_eq!(msg.tokens.input, 100);
        assert_eq!(msg.tokens.cache_read, 5);
        assert_eq!(msg.cost, 0.0);
    }

    /// A session migrated from 1.x keeps its message ids in `session_message`,
    /// but the migration drops some assistant messages whose usage then lives
    /// only in `message`. Unmigrated sessions live only in `message` too.
    #[test]
    fn migrated_database_prefers_v2_and_keeps_v1_only_assistants() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V1_SCHEMA, V2_SCHEMA]);
        // Migrated session: msg_a is in both layouts, msg_dropped only in v1.
        db.v1("msg_a", "ses_migrated", T0, v1_assistant("v1-model", 100));
        db.v1(
            "msg_dropped",
            "ses_migrated",
            T0 + 1,
            v1_assistant("v1-model", 7),
        );
        db.session("ses_migrated", None, None);
        db.v2(
            "msg_a",
            "ses_migrated",
            "assistant",
            2,
            v2_assistant("v2-model", 100),
        );
        // Unmigrated session: v1 only.
        db.v1(
            "msg_unmigrated",
            "ses_old",
            T0 + 2,
            v1_assistant("v1-model", 3),
        );

        let messages = parse(&[&db]);

        assert_eq!(keys(&messages), ["msg_a", "msg_dropped", "msg_unmigrated"]);
        assert_eq!(
            by_key(&messages, "msg_a").model_id,
            "v2-model",
            "a message present in both layouts is read from v2"
        );
        assert_eq!(by_key(&messages, "msg_dropped").tokens.input, 7);
        assert_eq!(by_key(&messages, "msg_unmigrated").session_id, "ses_old");
        let total_input: i64 = messages.iter().map(|m| m.tokens.input).sum();
        assert_eq!(total_input, 110);
    }

    #[test]
    fn v2_compaction_with_tokens_counted_running_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V2_SCHEMA]);
        db.session(
            "ses_1",
            Some(json!({ "id": "session-model", "providerID": "session-provider" })),
            None,
        );
        db.v2(
            "cmp_done",
            "ses_1",
            "compaction",
            1,
            json!({
                "status": "completed",
                "reason": "auto",
                "model": { "id": "compact-model", "providerID": "anthropic" },
                "summary": "...",
                "cost": 0.02,
                "tokens": tokens(50),
                "time": { "created": T0 }
            }),
        );
        db.v2(
            "cmp_failed",
            "ses_1",
            "compaction",
            2,
            json!({
                "status": "failed",
                "reason": "manual",
                "error": { "type": "compaction.failed", "message": "boom" },
                "tokens": tokens(20),
                "time": { "created": T0 }
            }),
        );
        db.v2(
            "cmp_running",
            "ses_1",
            "compaction",
            3,
            json!({ "status": "running", "reason": "auto", "time": { "created": T0 } }),
        );
        // What the v1 → v2 migration writes for a compaction: no usage.
        db.v2(
            "cmp_migrated",
            "ses_1",
            "compaction",
            4,
            json!({ "status": "completed", "summary": "...", "time": { "created": T0 } }),
        );

        let messages = parse(&[&db]);

        assert_eq!(keys(&messages), ["cmp_done", "cmp_failed"]);
        let done = by_key(&messages, "cmp_done");
        assert_eq!(done.model_id, "compact-model");
        assert_eq!(done.provider_id, "anthropic");
        assert_eq!(done.tokens.input, 50);
        let failed = by_key(&messages, "cmp_failed");
        assert_eq!(
            failed.model_id, "session-model",
            "falls back to the session model"
        );
        assert_eq!(failed.provider_id, "session-provider");
        assert_eq!(failed.tokens.input, 20);
    }

    #[test]
    fn v2_compaction_without_any_model_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V2_SCHEMA]);
        db.session("ses_1", None, None);
        db.v2(
            "cmp_failed",
            "ses_1",
            "compaction",
            1,
            json!({ "status": "failed", "tokens": tokens(20), "time": { "created": T0 } }),
        );

        let messages = parse(&[&db]);

        assert_eq!(keys(&messages), ["cmp_failed"]);
        assert_eq!(messages[0].model_id, "unknown");
        assert_eq!(messages[0].provider_id, "unknown");
    }

    /// Forking copies the parent's rows under new ids with the same `seq` and
    /// identical `data`; only the fork's own later rows are new usage.
    #[test]
    fn v2_fork_copies_are_skipped_and_own_rows_counted() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V2_SCHEMA]);
        let parent = [
            ("p_user1", "user", 1),
            ("p_a1", "assistant", 2),
            ("p_user2", "user", 3),
            ("p_a2", "assistant", 4),
        ];
        db.session("ses_parent", None, None);
        for (id, kind, seq) in parent {
            db.v2(id, "ses_parent", kind, seq, v2_assistant("m", seq * 100));
        }

        // `through` p_a1: copies seq 1..=2.
        db.session(
            "ses_through",
            None,
            Some(("ses_parent", r#"{"type":"through","messageID":"p_a1"}"#)),
        );
        db.v2("evt1_1", "ses_through", "user", 1, v2_assistant("m", 100));
        db.v2(
            "evt1_2",
            "ses_through",
            "assistant",
            2,
            v2_assistant("m", 200),
        );
        db.v2("t_own", "ses_through", "assistant", 3, v2_assistant("m", 5));

        // `before` p_a2 (seq 4): copies through the parent's last seq below 4.
        db.session(
            "ses_before",
            None,
            Some(("ses_parent", r#"{"type":"before","messageID":"p_a2"}"#)),
        );
        db.v2("evt2_1", "ses_before", "user", 1, v2_assistant("m", 100));
        db.v2(
            "evt2_2",
            "ses_before",
            "assistant",
            2,
            v2_assistant("m", 200),
        );
        db.v2("evt2_3", "ses_before", "user", 3, v2_assistant("m", 300));
        db.v2("b_own", "ses_before", "assistant", 4, v2_assistant("m", 6));

        let messages = parse(&[&db]);

        // evt1_2 and evt2_2 are copies of p_a1: same seq, same tokens.
        assert_eq!(keys(&messages), ["b_own", "p_a1", "p_a2", "t_own"]);
        let total_input: i64 = messages.iter().map(|m| m.tokens.input).sum();
        assert_eq!(total_input, 200 + 400 + 5 + 6);
    }

    #[test]
    fn v2_unresolvable_fork_boundary_keeps_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V2_SCHEMA]);
        db.session("ses_parent", None, None);
        db.v2("p_a1", "ses_parent", "assistant", 1, v2_assistant("m", 1));

        db.session(
            "ses_missing_msg",
            None,
            Some(("ses_parent", r#"{"type":"through","messageID":"gone"}"#)),
        );
        db.v2(
            "m_copy",
            "ses_missing_msg",
            "assistant",
            1,
            v2_assistant("m", 1),
        );
        db.session(
            "ses_missing_parent",
            None,
            Some(("ses_gone", r#"{"type":"through","messageID":"p_a1"}"#)),
        );
        db.v2(
            "p_copy",
            "ses_missing_parent",
            "assistant",
            1,
            v2_assistant("m", 1),
        );
        db.session("ses_garbled", None, Some(("ses_parent", "not json")));
        db.v2(
            "g_copy",
            "ses_garbled",
            "assistant",
            1,
            v2_assistant("m", 1),
        );

        let messages = parse(&[&db]);

        assert_eq!(keys(&messages), ["g_copy", "m_copy", "p_a1", "p_copy"]);
    }

    #[test]
    fn overlapping_ids_across_two_databases_are_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let main = Db::create(dir.path(), "opencode.db", &[V1_SCHEMA, V2_SCHEMA]);
        main.session("ses_1", None, None);
        main.v2("msg_shared", "ses_1", "assistant", 1, v2_assistant("m", 10));
        main.v2("msg_main", "ses_1", "assistant", 2, v2_assistant("m", 20));
        // The 2.x preview database is imported with unchanged ids.
        let next = Db::create(dir.path(), "opencode-next.db", &[V2_SCHEMA]);
        next.session("ses_1", None, None);
        next.v2("msg_shared", "ses_1", "assistant", 1, v2_assistant("m", 10));
        next.v2("msg_next", "ses_1", "assistant", 3, v2_assistant("m", 30));
        // A v1-only database repeating a v2 id.
        let beta = Db::create(dir.path(), "opencode-beta.db", &[V1_SCHEMA]);
        beta.v1("msg_main", "ses_1", T0, v1_assistant("m", 20));

        let paths = discover_databases(dir.path(), None);
        let messages = parse_databases(&paths, None);

        assert_eq!(keys(&messages), ["msg_main", "msg_next", "msg_shared"]);
        let total_input: i64 = messages.iter().map(|m| m.tokens.input).sum();
        assert_eq!(total_input, 60);
    }

    #[test]
    fn since_is_respected_for_v1_and_v2_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V1_SCHEMA, V2_SCHEMA]);
        let since = NaiveDate::from_ymd_opt(2026, 9, 10).unwrap();
        let before = local_noon_ms(since.pred_opt().unwrap());
        let on = local_noon_ms(since);
        db.v1("v1_old", "ses_v1", before, v1_assistant("m", 1));
        db.v1("v1_new", "ses_v1", on, v1_assistant("m", 1));
        db.session("ses_v2", None, None);
        db.v2_at(
            "v2_old",
            "ses_v2",
            "assistant",
            1,
            before,
            v2_assistant("m", 1),
        );
        db.v2_at("v2_new", "ses_v2", "assistant", 2, on, v2_assistant("m", 1));

        let messages = parse_databases(&[db.path.clone()], Some(since));

        assert_eq!(keys(&messages), ["v1_new", "v2_new"]);
        assert_eq!(parse(&[&db]).len(), 4);
    }

    #[test]
    fn discovery_finds_channel_databases_but_not_sqlite_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "opencode.db",
            "opencode-next.db",
            "opencode-dev.db",
            "opencode-dev.db-wal",
            "opencode-dev.db-shm",
            "opencode-x.db-journal",
            "opencode.db-wal",
            "other.db",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }

        let found = discover_databases(dir.path(), None);

        assert_eq!(
            found,
            vec![
                dir.path().join("opencode.db"),
                dir.path().join("opencode-dev.db"),
                dir.path().join("opencode-next.db"),
            ]
        );
    }

    #[test]
    fn opencode_db_override_replaces_discovery() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("opencode.db"), b"").unwrap();
        std::fs::write(dir.path().join("custom.db"), b"").unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let absolute = elsewhere.path().join("abs.db");
        std::fs::write(&absolute, b"").unwrap();

        assert_eq!(
            discover_databases(dir.path(), Some("custom.db".into())),
            vec![dir.path().join("custom.db")],
            "relative values resolve against the data dir"
        );
        assert_eq!(
            discover_databases(dir.path(), Some(absolute.clone().into_os_string())),
            vec![absolute]
        );
        assert!(discover_databases(dir.path(), Some("missing.db".into())).is_empty());
    }

    #[test]
    fn databases_are_opened_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V1_SCHEMA]);
        drop(db.conn);

        let conn = open_read_only(&db.path).unwrap();

        assert!(conn.is_readonly(rusqlite::DatabaseName::Main).unwrap());
        assert!(conn.execute("DELETE FROM message", []).is_err());
    }

    fn query_plan(conn: &Connection, sql: &str) -> String {
        let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        stmt.query_map([T0], |row| row.get::<_, String>(3))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn windowed_queries_use_the_time_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(dir.path(), "opencode.db", &[V1_SCHEMA, V2_SCHEMA]);

        let v1_plan = query_plan(&db.conn, &v1_query(true, true, true));
        assert!(
            v1_plan.contains("USING COVERING INDEX message_session_time_created_id_idx"),
            "{v1_plan}"
        );

        let v2_plan = query_plan(&db.conn, &v2_query(true, true));
        assert!(
            v2_plan.contains("session_message_time_created_idx"),
            "{v2_plan}"
        );
    }
}
