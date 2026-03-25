use crate::pricing::{calculate_cost, lookup_model_pricing_or_warn, ModelPricing, PricingCache};
use crate::provider::{TokenBreakdown, UnifiedMessage};
use crate::usage::{DashboardDay, ModelSummary, ProviderSummary};
use anyhow::Result;
use chrono::{Duration, NaiveDate, Utc};
use rusqlite::{
    params, params_from_iter, types::Value, Connection, OptionalExtension, Transaction,
};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct DailyUsageRow {
    pub date: String,
    pub source: String,
    pub provider_id: String,
    pub model_id: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub cost_usd: f64,
    pub message_count: i64,
    pub session_count: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct DateRange {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

impl DateRange {
    pub fn contains(&self, date: NaiveDate) -> bool {
        date >= self.start && date <= self.end
    }
}

#[derive(Debug, Clone)]
pub struct UsageStore {
    path: PathBuf,
}

impl UsageStore {
    pub fn new() -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("tokenpulse");
        Self {
            path: cache_dir.join("usage.sqlite3"),
        }
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn latest_message_date(&self, source: &str) -> Result<Option<NaiveDate>> {
        let conn = self.open()?;
        let value: Option<String> = conn
            .query_row(
                "SELECT MAX(date) FROM usage_messages WHERE source = ?1",
                params![source],
                |row| row.get(0),
            )
            .optional()?
            .flatten();

        Ok(value.and_then(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok()))
    }

    pub fn default_since(
        &self,
        source: &str,
        requested: Option<NaiveDate>,
    ) -> Result<Option<NaiveDate>> {
        let inferred = self
            .latest_message_date(source)?
            .map(|date| date - Duration::days(1));

        Ok(match (requested, inferred) {
            (Some(requested), Some(inferred)) => Some(requested.max(inferred)),
            (Some(requested), None) => Some(requested),
            (None, Some(inferred)) => Some(inferred),
            (None, None) => None,
        })
    }

    pub fn ingest_messages(
        &self,
        messages: &[UnifiedMessage],
        refresh_pricing: bool,
    ) -> Result<BTreeSet<String>> {
        if messages.is_empty() {
            return Ok(BTreeSet::new());
        }

        let pricing_cache = PricingCache::new();
        let pricing = match pricing_cache.get_pricing_sync() {
            Ok(pricing) => Some(pricing),
            Err(error) if !refresh_pricing => {
                warn!(
                    "Failed to load pricing data during usage ingest; continuing without refreshed pricing: {}",
                    error
                );
                None
            }
            Err(error) => return Err(error),
        };

        let mut conn = self.open()?;
        let tx = conn.transaction()?;
        let now = Utc::now().timestamp_millis();
        let mut affected_dates = BTreeSet::new();

        for message in messages {
            let snapshot =
                ensure_pricing_snapshot(&tx, pricing.as_ref(), message, refresh_pricing)?;
            let cost = snapshot
                .as_ref()
                .map(|pricing| calculate_cost(&message.tokens, pricing))
                .unwrap_or_else(|| message.cost.max(0.0));

            tx.execute(
                r#"
                INSERT INTO usage_messages (
                    source, provider_id, model_id, session_id, message_key,
                    timestamp_ms, date, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, reasoning_tokens,
                    total_tokens, cost_usd, pricing_day, parser_version
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5,
                    ?6, ?7, ?8, ?9,
                    ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16
                )
                ON CONFLICT(source, message_key) DO UPDATE SET
                    provider_id = excluded.provider_id,
                    model_id = excluded.model_id,
                    session_id = excluded.session_id,
                    timestamp_ms = excluded.timestamp_ms,
                    date = excluded.date,
                    input_tokens = excluded.input_tokens,
                    output_tokens = excluded.output_tokens,
                    cache_read_tokens = excluded.cache_read_tokens,
                    cache_write_tokens = excluded.cache_write_tokens,
                    reasoning_tokens = excluded.reasoning_tokens,
                    total_tokens = excluded.total_tokens,
                    cost_usd = excluded.cost_usd,
                    pricing_day = excluded.pricing_day,
                    parser_version = excluded.parser_version
                "#,
                params![
                    message.client,
                    message.provider_id,
                    message.model_id,
                    message.session_id,
                    message.message_key,
                    message.timestamp,
                    message.date,
                    message.tokens.input,
                    message.tokens.output,
                    message.tokens.cache_read,
                    message.tokens.cache_write,
                    message.tokens.reasoning,
                    message.total_tokens(),
                    cost,
                    message.pricing_day,
                    message.parser_version,
                ],
            )?;

            affected_dates.insert(message.date.clone());
        }

        for date in &affected_dates {
            rebuild_daily_for_date(&tx, date, now)?;
        }

        tx.commit()?;
        Ok(affected_dates)
    }

    pub fn delete_sources_in_date_range(
        &self,
        range: DateRange,
        sources: &[String],
        refresh_pricing: bool,
    ) -> Result<()> {
        self.delete_scoped(Some(range), sources, refresh_pricing)
    }

    pub fn clear_sources(&self, sources: &[String], refresh_pricing: bool) -> Result<()> {
        self.delete_scoped(None, sources, refresh_pricing)
    }

    pub fn delete_date_range(&self, range: DateRange, refresh_pricing: bool) -> Result<()> {
        let mut conn = self.open()?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM usage_messages WHERE date >= ?1 AND date <= ?2",
            params![range.start.to_string(), range.end.to_string()],
        )?;
        tx.execute(
            "DELETE FROM daily_model_usage WHERE date >= ?1 AND date <= ?2",
            params![range.start.to_string(), range.end.to_string()],
        )?;
        if refresh_pricing {
            tx.execute(
                "DELETE FROM daily_pricing_snapshots WHERE date >= ?1 AND date <= ?2",
                params![range.start.to_string(), range.end.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn clear_all(&self, refresh_pricing: bool) -> Result<()> {
        let mut conn = self.open()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM usage_messages", [])?;
        tx.execute("DELETE FROM daily_model_usage", [])?;
        if refresh_pricing {
            tx.execute("DELETE FROM daily_pricing_snapshots", [])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn rebuild_all_daily(&self) -> Result<()> {
        let mut conn = self.open()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM daily_model_usage", [])?;
        let mut stmt = tx.prepare("SELECT DISTINCT date FROM usage_messages ORDER BY date")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let dates: Vec<String> = rows.flatten().collect();
        drop(stmt);
        let now = Utc::now().timestamp_millis();
        for date in dates {
            rebuild_daily_for_date(&tx, &date, now)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_messages(
        &self,
        since: Option<NaiveDate>,
        sources: &[String],
    ) -> Result<Vec<UnifiedMessage>> {
        let conn = self.open()?;
        let mut sql = String::from(
            r#"
            SELECT source, provider_id, model_id, session_id, message_key,
                   timestamp_ms, date, input_tokens, output_tokens,
                   cache_read_tokens, cache_write_tokens, reasoning_tokens,
                   cost_usd, pricing_day, parser_version
            FROM usage_messages
            WHERE 1=1
            "#,
        );
        let params = append_common_filters(&mut sql, since, sources);
        sql.push_str(" ORDER BY timestamp_ms ASC");

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), row_to_message)?;

        let mut messages: Vec<UnifiedMessage> = rows.flatten().collect();
        messages.sort_by_key(|message| message.timestamp);
        Ok(messages)
    }

    pub fn load_dashboard_days(
        &self,
        since: Option<NaiveDate>,
        sources: &[String],
    ) -> Result<Vec<DashboardDay>> {
        let conn = self.open()?;
        let mut sql = String::from(
            r#"
            SELECT date,
                   SUM(input_tokens),
                   SUM(output_tokens),
                   SUM(cache_read_tokens),
                   SUM(cache_write_tokens),
                   SUM(reasoning_tokens),
                   SUM(total_tokens),
                   SUM(cost_usd),
                   COUNT(*),
                   COUNT(DISTINCT source || '::' || session_id)
            FROM usage_messages
            WHERE 1=1
            "#,
        );
        let params = append_common_filters(&mut sql, since, sources);
        sql.push_str(" GROUP BY date ORDER BY date ASC");

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), row_to_dashboard_day)?;
        Ok(rows.flatten().collect())
    }

    pub fn load_daily_rows(
        &self,
        since: Option<NaiveDate>,
        sources: &[String],
    ) -> Result<Vec<DailyUsageRow>> {
        let conn = self.open()?;
        let mut sql = String::from(
            r#"
            SELECT date, source, provider_id, model_id,
                   input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                   reasoning_tokens, total_tokens, cost_usd, message_count, session_count
            FROM daily_model_usage
            WHERE 1=1
            "#,
        );
        let params = append_common_filters(&mut sql, since, sources);
        sql.push_str(" ORDER BY date ASC, cost_usd DESC");

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), row_to_daily)?;

        Ok(rows.flatten().collect())
    }

    pub fn load_provider_summaries(
        &self,
        since: Option<NaiveDate>,
        sources: &[String],
    ) -> Result<Vec<ProviderSummary>> {
        let conn = self.open()?;
        let mut sql = String::from(
            r#"
            SELECT source,
                   SUM(cost_usd),
                   SUM(total_tokens),
                   COUNT(*),
                   COUNT(DISTINCT source || '::' || session_id)
            FROM usage_messages
            WHERE 1=1
            "#,
        );
        let params = append_common_filters(&mut sql, since, sources);
        sql.push_str(" GROUP BY source ORDER BY SUM(total_tokens) DESC, source ASC");

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), row_to_provider_summary)?;
        Ok(rows.flatten().collect())
    }

    pub fn load_model_summaries(
        &self,
        since: Option<NaiveDate>,
        sources: &[String],
    ) -> Result<Vec<ModelSummary>> {
        let conn = self.open()?;
        let mut sql = String::from(
            r#"
            SELECT model_id,
                   provider_id,
                   source,
                   SUM(cost_usd),
                   SUM(total_tokens),
                   COUNT(*),
                   COUNT(DISTINCT source || '::' || session_id)
            FROM usage_messages
            WHERE 1=1
            "#,
        );
        let params = append_common_filters(&mut sql, since, sources);
        sql.push_str(
            " GROUP BY source, provider_id, model_id ORDER BY SUM(total_tokens) DESC, model_id ASC",
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), row_to_model_summary)?;
        Ok(rows.flatten().collect())
    }

    fn open(&self) -> Result<Connection> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&self.path)?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS usage_messages (
                source TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                model_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                message_key TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                date TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cache_read_tokens INTEGER NOT NULL,
                cache_write_tokens INTEGER NOT NULL,
                reasoning_tokens INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL,
                cost_usd REAL NOT NULL,
                pricing_day TEXT NOT NULL,
                parser_version TEXT NOT NULL,
                PRIMARY KEY (source, message_key)
            );
            CREATE INDEX IF NOT EXISTS idx_usage_messages_date ON usage_messages(date);
            CREATE INDEX IF NOT EXISTS idx_usage_messages_source_date ON usage_messages(source, date);

            CREATE TABLE IF NOT EXISTS daily_model_usage (
                date TEXT NOT NULL,
                source TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                model_id TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cache_read_tokens INTEGER NOT NULL,
                cache_write_tokens INTEGER NOT NULL,
                reasoning_tokens INTEGER NOT NULL,
                total_tokens INTEGER NOT NULL,
                cost_usd REAL NOT NULL,
                message_count INTEGER NOT NULL,
                session_count INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (date, source, provider_id, model_id)
            );
            CREATE INDEX IF NOT EXISTS idx_daily_model_usage_date ON daily_model_usage(date);

            CREATE TABLE IF NOT EXISTS daily_pricing_snapshots (
                date TEXT NOT NULL,
                provider_id TEXT NOT NULL,
                model_id TEXT NOT NULL,
                input_cost_per_token REAL NOT NULL,
                output_cost_per_token REAL NOT NULL,
                cache_read_input_token_cost REAL,
                cache_creation_input_token_cost REAL,
                captured_at INTEGER NOT NULL,
                pricing_source TEXT NOT NULL,
                pricing_version TEXT NOT NULL,
                PRIMARY KEY (date, provider_id, model_id)
            );
            "#,
        )?;
        Ok(conn)
    }

    fn delete_scoped(
        &self,
        range: Option<DateRange>,
        sources: &[String],
        refresh_pricing: bool,
    ) -> Result<()> {
        if sources.is_empty() {
            return match range {
                Some(range) => self.delete_date_range(range, refresh_pricing),
                None => self.clear_all(refresh_pricing),
            };
        }

        let mut conn = self.open()?;
        let tx = conn.transaction()?;
        let snapshot_keys = if refresh_pricing {
            load_pricing_snapshot_keys(&tx, range, sources)?
        } else {
            Vec::new()
        };

        let mut message_sql = String::from("DELETE FROM usage_messages WHERE 1=1");
        let message_params = append_range_and_source_filters(&mut message_sql, range, sources);
        tx.execute(&message_sql, params_from_iter(message_params))?;

        let mut daily_sql = String::from("DELETE FROM daily_model_usage WHERE 1=1");
        let daily_params = append_range_and_source_filters(&mut daily_sql, range, sources);
        tx.execute(&daily_sql, params_from_iter(daily_params))?;

        if refresh_pricing {
            for (date, provider_id, model_id) in snapshot_keys {
                tx.execute(
                    "DELETE FROM daily_pricing_snapshots WHERE date = ?1 AND provider_id = ?2 AND model_id = ?3",
                    params![date, provider_id, model_id],
                )?;
            }
        }

        tx.commit()?;
        Ok(())
    }
}

impl Default for UsageStore {
    fn default() -> Self {
        Self::new()
    }
}

fn append_common_filters(
    sql: &mut String,
    since: Option<NaiveDate>,
    sources: &[String],
) -> Vec<Value> {
    let mut params = Vec::new();

    if let Some(since) = since {
        sql.push_str(" AND date >= ?");
        params.push(Value::from(since.to_string()));
    }

    if !sources.is_empty() {
        sql.push_str(" AND source IN (");
        for idx in 0..sources.len() {
            if idx > 0 {
                sql.push_str(", ");
            }
            sql.push('?');
            params.push(Value::from(sources[idx].clone()));
        }
        sql.push(')');
    }

    params
}

fn append_range_and_source_filters(
    sql: &mut String,
    range: Option<DateRange>,
    sources: &[String],
) -> Vec<Value> {
    let mut params = Vec::new();

    if let Some(range) = range {
        sql.push_str(" AND date >= ?");
        params.push(Value::from(range.start.to_string()));
        sql.push_str(" AND date <= ?");
        params.push(Value::from(range.end.to_string()));
    }

    if !sources.is_empty() {
        sql.push_str(" AND source IN (");
        for (idx, source) in sources.iter().enumerate() {
            if idx > 0 {
                sql.push_str(", ");
            }
            sql.push('?');
            params.push(Value::from(source.clone()));
        }
        sql.push(')');
    }

    params
}

fn load_pricing_snapshot_keys(
    tx: &Transaction<'_>,
    range: Option<DateRange>,
    sources: &[String],
) -> Result<Vec<(String, String, String)>> {
    let mut sql =
        String::from("SELECT DISTINCT date, provider_id, model_id FROM usage_messages WHERE 1=1");
    let params = append_range_and_source_filters(&mut sql, range, sources);
    let mut stmt = tx.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    Ok(rows.flatten().collect())
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<UnifiedMessage> {
    let input: i64 = row.get(7)?;
    let output: i64 = row.get(8)?;
    let cache_read: i64 = row.get(9)?;
    let cache_write: i64 = row.get(10)?;
    let reasoning: i64 = row.get(11)?;
    Ok(UnifiedMessage {
        client: row.get(0)?,
        provider_id: row.get(1)?,
        model_id: row.get(2)?,
        session_id: row.get(3)?,
        message_key: row.get(4)?,
        timestamp: row.get(5)?,
        date: row.get(6)?,
        tokens: TokenBreakdown {
            input,
            output,
            cache_read,
            cache_write,
            reasoning,
        },
        cost: row.get(12)?,
        pricing_day: row.get(13)?,
        parser_version: row.get(14)?,
    })
}

fn row_to_dashboard_day(row: &rusqlite::Row<'_>) -> rusqlite::Result<DashboardDay> {
    Ok(DashboardDay {
        date: row.get(0)?,
        input_tokens: row.get(1)?,
        output_tokens: row.get(2)?,
        cache_read_tokens: row.get(3)?,
        cache_write_tokens: row.get(4)?,
        reasoning_tokens: row.get(5)?,
        total_tokens: row.get(6)?,
        total_cost_usd: row.get(7)?,
        message_count: row.get(8)?,
        session_count: row.get(9)?,
        intensity_tokens: 0,
        intensity_cost: 0,
    })
}

fn row_to_daily(row: &rusqlite::Row<'_>) -> rusqlite::Result<DailyUsageRow> {
    Ok(DailyUsageRow {
        date: row.get(0)?,
        source: row.get(1)?,
        provider_id: row.get(2)?,
        model_id: row.get(3)?,
        input_tokens: row.get(4)?,
        output_tokens: row.get(5)?,
        cache_read_tokens: row.get(6)?,
        cache_write_tokens: row.get(7)?,
        reasoning_tokens: row.get(8)?,
        total_tokens: row.get(9)?,
        cost_usd: row.get(10)?,
        message_count: row.get(11)?,
        session_count: row.get(12)?,
    })
}

fn row_to_provider_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderSummary> {
    Ok(ProviderSummary {
        provider: row.get(0)?,
        cost: row.get(1)?,
        tokens: row.get(2)?,
        message_count: row.get::<_, i64>(3)? as usize,
        session_count: row.get::<_, i64>(4)? as usize,
        percent: 0.0,
    })
}

fn row_to_model_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModelSummary> {
    Ok(ModelSummary {
        model: row.get(0)?,
        provider: row.get(1)?,
        source: row.get(2)?,
        cost: row.get(3)?,
        tokens: row.get(4)?,
        message_count: row.get::<_, i64>(5)? as usize,
        session_count: row.get::<_, i64>(6)? as usize,
        percent: 0.0,
    })
}

fn rebuild_daily_for_date(tx: &Transaction<'_>, date: &str, now: i64) -> Result<()> {
    tx.execute(
        "DELETE FROM daily_model_usage WHERE date = ?1",
        params![date],
    )?;
    tx.execute(
        r#"
        INSERT INTO daily_model_usage (
            date, source, provider_id, model_id,
            input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
            reasoning_tokens, total_tokens, cost_usd, message_count, session_count, updated_at
        )
        SELECT
            date,
            source,
            provider_id,
            model_id,
            SUM(input_tokens),
            SUM(output_tokens),
            SUM(cache_read_tokens),
            SUM(cache_write_tokens),
            SUM(reasoning_tokens),
            SUM(total_tokens),
            SUM(cost_usd),
            COUNT(*),
            COUNT(DISTINCT session_id),
            ?2
        FROM usage_messages
        WHERE date = ?1
        GROUP BY date, source, provider_id, model_id
        "#,
        params![date, now],
    )?;
    Ok(())
}

fn ensure_pricing_snapshot(
    tx: &Transaction<'_>,
    pricing: Option<&HashMap<String, ModelPricing>>,
    message: &UnifiedMessage,
    replace_existing: bool,
) -> Result<Option<ModelPricing>> {
    if replace_existing {
        tx.execute(
            "DELETE FROM daily_pricing_snapshots WHERE date = ?1 AND provider_id = ?2 AND model_id = ?3",
            params![message.date, message.provider_id, message.model_id],
        )?;
    }

    let existing = tx
        .query_row(
            r#"
            SELECT input_cost_per_token, output_cost_per_token,
                   cache_read_input_token_cost, cache_creation_input_token_cost
            FROM daily_pricing_snapshots
            WHERE date = ?1 AND provider_id = ?2 AND model_id = ?3
            "#,
            params![message.date, message.provider_id, message.model_id],
            |row| {
                Ok(ModelPricing::new(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                ))
            },
        )
        .optional()?;

    if existing.is_some() {
        return Ok(existing);
    }

    let Some(pricing) = pricing else {
        return Ok(None);
    };

    let looked_up = lookup_model_pricing_or_warn(&message.model_id, pricing).cloned();
    let snapshot = looked_up.unwrap_or_else(|| ModelPricing::simple(0.0, 0.0));
    let pricing_source =
        if snapshot.input_cost_per_token > 0.0 || snapshot.output_cost_per_token > 0.0 {
            "litellm"
        } else {
            "missing"
        };

    tx.execute(
        r#"
        INSERT INTO daily_pricing_snapshots (
            date, provider_id, model_id, input_cost_per_token,
            output_cost_per_token, cache_read_input_token_cost,
            cache_creation_input_token_cost, captured_at, pricing_source, pricing_version
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        "#,
        params![
            message.date,
            message.provider_id,
            message.model_id,
            snapshot.input_cost_per_token,
            snapshot.output_cost_per_token,
            snapshot.cache_read_input_token_cost,
            snapshot.cache_creation_input_token_cost,
            Utc::now().timestamp_millis(),
            pricing_source,
            "litellm-cache-v1",
        ],
    )?;

    if pricing_source == "missing" {
        Ok(None)
    } else {
        Ok(Some(snapshot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::UnifiedMessage;

    fn sample_message(date: &str, key: &str) -> UnifiedMessage {
        UnifiedMessage::new(
            "claude",
            "claude-3-opus",
            "anthropic",
            "session-1",
            key,
            NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis(),
            TokenBreakdown {
                input: 100,
                output: 50,
                cache_read: 10,
                cache_write: 5,
                reasoning: 0,
            },
        )
        .with_cost(1.0)
    }

    #[test]
    fn default_since_prefers_recent_lookback() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));
        store
            .ingest_messages(&[sample_message("2024-03-10", "m1")], false)
            .unwrap();
        let since = store.default_since("claude", None).unwrap().unwrap();
        assert_eq!(since, NaiveDate::from_ymd_opt(2024, 3, 9).unwrap());
    }

    #[test]
    fn delete_sources_in_date_range_preserves_other_sources() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));

        let mut claude = sample_message("2024-03-10", "claude-m1");
        claude.client = "claude".to_string();
        let mut codex = sample_message("2024-03-10", "codex-m1");
        codex.client = "codex".to_string();
        codex.provider_id = "openai".to_string();

        store
            .ingest_messages(&[claude.clone(), codex.clone()], false)
            .unwrap();

        store
            .delete_sources_in_date_range(
                DateRange {
                    start: NaiveDate::from_ymd_opt(2024, 3, 10).unwrap(),
                    end: NaiveDate::from_ymd_opt(2024, 3, 10).unwrap(),
                },
                &["claude".to_string()],
                false,
            )
            .unwrap();

        let remaining_codex = store.load_messages(None, &["codex".to_string()]).unwrap();
        let remaining_claude = store.load_messages(None, &["claude".to_string()]).unwrap();

        assert_eq!(remaining_codex.len(), 1);
        assert_eq!(remaining_codex[0].client, "codex");
        assert!(remaining_claude.is_empty());
    }
}
