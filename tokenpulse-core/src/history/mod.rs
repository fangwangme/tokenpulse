//! Durable observation history for quota polls and Keeper executions.
//!
//! The quota *cache* keeps exactly one row per provider and overwrites it on
//! every poll, so nothing survives for later analysis. This store lives in the
//! same `tokenpulse.db` file but only ever appends, giving an evenly-spaced
//! time series of every window's usage plus a record of every Keeper ping.

use crate::keeper::{KeeperExecutionRecord, KeeperTriggerType};
use crate::provider::QuotaSnapshot;
use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use rusqlite::{params, Connection};
use std::fs;
use std::path::PathBuf;

/// Bumped whenever `SCHEMA_V1` below gains a migration step.
const SCHEMA_VERSION: i32 = 1;

const SCHEMA_V1: &str = "
CREATE TABLE IF NOT EXISTS quota_observations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    plan TEXT,
    account TEXT,
    window_label TEXT NOT NULL,
    used_percent REAL NOT NULL,
    resets_at TEXT,
    period_duration_ms INTEGER
);
CREATE INDEX IF NOT EXISTS idx_quota_observations_provider_time
    ON quota_observations(provider, observed_at);
CREATE INDEX IF NOT EXISTS idx_quota_observations_window_time
    ON quota_observations(provider, window_label, observed_at);

CREATE TABLE IF NOT EXISTS quota_credit_observations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    used REAL NOT NULL,
    credit_limit REAL,
    currency TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_quota_credit_observations_provider_time
    ON quota_credit_observations(provider, observed_at);

CREATE TABLE IF NOT EXISTS quota_fetch_failures (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    provider TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    error TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_quota_fetch_failures_provider_time
    ON quota_fetch_failures(provider, observed_at);

CREATE TABLE IF NOT EXISTS keeper_executions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    trigger_type TEXT NOT NULL,
    executed_at TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    command TEXT NOT NULL,
    duration_ms INTEGER NOT NULL,
    success INTEGER NOT NULL,
    exit_code INTEGER,
    output_snippet TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_keeper_executions_time
    ON keeper_executions(executed_at);
CREATE INDEX IF NOT EXISTS idx_keeper_executions_agent_time
    ON keeper_executions(agent, executed_at);
";

pub struct HistoryStore {
    db_path: PathBuf,
}

impl HistoryStore {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let data_dir = home.join(".local").join("share").join("tokenpulse");
        Self {
            db_path: data_dir.join("tokenpulse.db"),
        }
    }

    /// Opens a store at an explicit path — used by the quota cache so both write
    /// to the same file, and by tests so they stay inside a temp dir.
    pub fn with_path(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    pub fn db_path(&self) -> &PathBuf {
        &self.db_path
    }

    fn open(&self) -> Result<Connection> {
        if let Some(parent) = self.db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&self.db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // WAL keeps the TUI's writes from blocking a concurrent CLI read.
        conn.execute_batch("PRAGMA journal_mode = WAL;")?;
        ensure_schema(&conn)?;
        Ok(conn)
    }

    /// Appends one row per rate window, plus a credits row when the provider
    /// reports them. Called once per successful poll.
    pub fn record_quota_snapshot(
        &self,
        provider: &str,
        observed_at: DateTime<Utc>,
        snapshot: &QuotaSnapshot,
    ) -> Result<()> {
        let mut conn = self.open()?;
        let tx = conn.transaction()?;

        {
            let mut stmt = tx.prepare(
                "INSERT INTO quota_observations (
                    provider, observed_at, fetched_at, plan, account,
                    window_label, used_percent, resets_at, period_duration_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for window in &snapshot.windows {
                stmt.execute(params![
                    provider,
                    observed_at.to_rfc3339(),
                    snapshot.fetched_at.to_rfc3339(),
                    snapshot.plan,
                    snapshot.account,
                    window.label,
                    window.used_percent,
                    window.resets_at.map(|t| t.to_rfc3339()),
                    window.period_duration_ms,
                ])?;
            }
        }

        if let Some(credits) = &snapshot.credits {
            tx.execute(
                "INSERT INTO quota_credit_observations (
                    provider, observed_at, used, credit_limit, currency
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    provider,
                    observed_at.to_rfc3339(),
                    credits.used,
                    credits.limit,
                    credits.currency,
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    /// Records a poll that failed, so recurring auth or network problems stay
    /// visible after the status-bar message has scrolled away.
    pub fn record_quota_failure(
        &self,
        provider: &str,
        observed_at: DateTime<Utc>,
        error: &str,
    ) -> Result<()> {
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO quota_fetch_failures (provider, observed_at, error)
             VALUES (?1, ?2, ?3)",
            params![provider, observed_at.to_rfc3339(), error],
        )?;
        Ok(())
    }

    pub fn record_keeper_execution(&self, record: &KeeperExecutionRecord) -> Result<()> {
        let conn = self.open()?;
        conn.execute(
            "INSERT INTO keeper_executions (
                agent, trigger_type, executed_at, model, prompt, command,
                duration_ms, success, exit_code, output_snippet
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                record.agent,
                record.trigger_type.as_str(),
                record.timestamp.to_rfc3339(),
                record.model,
                record.prompt,
                record.command_executed,
                record.duration_ms as i64,
                record.success as i32,
                record.exit_code,
                record.output_snippet,
            ],
        )?;
        Ok(())
    }

    /// Returns the newest `limit` executions in chronological order, ready to
    /// seed the TUI's log panel on startup.
    pub fn recent_keeper_executions(&self, limit: usize) -> Result<Vec<KeeperExecutionRecord>> {
        if !self.db_path.exists() {
            return Ok(Vec::new());
        }
        let conn = self.open()?;
        let mut stmt = conn.prepare(
            "SELECT agent, trigger_type, executed_at, model, prompt, command,
                    duration_ms, success, exit_code, output_snippet
             FROM keeper_executions
             ORDER BY executed_at DESC, id DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            let executed_at: String = row.get(2)?;
            let timestamp = DateTime::parse_from_rfc3339(&executed_at)
                .map(|t| t.with_timezone(&Local))
                .unwrap_or_else(|_| Local::now());
            Ok(KeeperExecutionRecord {
                agent: row.get(0)?,
                trigger_type: KeeperTriggerType::from_db_str(&row.get::<_, String>(1)?),
                timestamp,
                model: row.get(3)?,
                prompt: row.get(4)?,
                command_executed: row.get(5)?,
                duration_ms: row.get::<_, i64>(6)? as u64,
                success: row.get::<_, i32>(7)? != 0,
                exit_code: row.get(8)?,
                output_snippet: row.get(9)?,
            })
        })?;

        let mut records: Vec<KeeperExecutionRecord> = rows.collect::<rusqlite::Result<_>>()?;
        records.reverse(); // oldest first, matching the in-memory log order
        Ok(records)
    }
}

impl Default for HistoryStore {
    fn default() -> Self {
        Self::new()
    }
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    conn.execute_batch(SCHEMA_V1)?;
    // PRAGMA does not accept bound parameters.
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{CreditInfo, RateWindow};

    fn store() -> (tempfile::TempDir, HistoryStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::with_path(dir.path().join("tokenpulse.db"));
        (dir, store)
    }

    fn snapshot(used: f64) -> QuotaSnapshot {
        QuotaSnapshot {
            provider: "claude".to_string(),
            plan: Some("Max".to_string()),
            account: Some("me@example.com".to_string()),
            windows: vec![
                RateWindow {
                    label: "Session (5h)".to_string(),
                    used_percent: used,
                    resets_at: Some(Utc::now()),
                    period_duration_ms: Some(5 * 60 * 60 * 1000),
                },
                RateWindow {
                    label: "Weekly (7d)".to_string(),
                    used_percent: 71.0,
                    resets_at: None,
                    period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
                },
            ],
            credits: Some(CreditInfo {
                used: 12.5,
                limit: Some(100.0),
                currency: "USD".to_string(),
            }),
            rate_limit_reset_credits: vec![],
            fetched_at: Utc::now(),
        }
    }

    #[test]
    fn quota_observations_append_one_row_per_window() {
        let (_dir, store) = store();
        let t0 = Utc::now();

        store
            .record_quota_snapshot("claude", t0, &snapshot(0.0))
            .unwrap();
        store
            .record_quota_snapshot("claude", t0 + chrono::Duration::minutes(5), &snapshot(2.0))
            .unwrap();

        let conn = store.open().unwrap();
        let windows: i64 = conn
            .query_row("SELECT COUNT(*) FROM quota_observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(windows, 4, "two polls x two windows");

        // Repeated values are kept: the series is an evenly spaced grid.
        let weekly: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM quota_observations
                 WHERE window_label = 'Weekly (7d)' AND used_percent = 71.0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(weekly, 2);

        let credits: i64 = conn
            .query_row("SELECT COUNT(*) FROM quota_credit_observations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(credits, 2);
    }

    #[test]
    fn quota_failures_are_recorded_per_provider() {
        let (_dir, store) = store();
        store
            .record_quota_failure("codex", Utc::now(), "401 Unauthorized")
            .unwrap();

        let conn = store.open().unwrap();
        let (provider, error): (String, String) = conn
            .query_row(
                "SELECT provider, error FROM quota_fetch_failures",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(provider, "codex");
        assert_eq!(error, "401 Unauthorized");
    }

    #[test]
    fn keeper_executions_round_trip_newest_last() {
        let (_dir, store) = store();

        for i in 0..5 {
            let mut record = crate::keeper::failed_ping_record(
                "claude",
                &crate::config::AgentKeeperConfig::default(),
                KeeperTriggerType::Daily,
                &format!("run-{i}"),
            );
            record.timestamp = Local::now() + chrono::Duration::seconds(i);
            record.success = i % 2 == 0;
            record.duration_ms = i as u64 * 100;
            store.record_keeper_execution(&record).unwrap();
        }

        let recent = store.recent_keeper_executions(3).unwrap();
        assert_eq!(recent.len(), 3);
        // Oldest first, so pushing them onto the in-memory log preserves order.
        assert_eq!(recent[0].output_snippet, "run-2");
        assert_eq!(recent[2].output_snippet, "run-4");
        assert_eq!(recent[2].trigger_type.as_str(), "daily");
        assert!(recent[2].success);
        assert_eq!(recent[2].duration_ms, 400);
    }

    #[test]
    fn recent_executions_on_a_missing_database_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = HistoryStore::with_path(dir.path().join("nope.db"));
        assert!(store.recent_keeper_executions(50).unwrap().is_empty());
    }

    #[test]
    fn schema_is_created_once_and_is_idempotent() {
        let (_dir, store) = store();
        store.open().unwrap();
        let conn = store.open().unwrap();
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }
}
