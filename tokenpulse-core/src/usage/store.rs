use super::normalize_model_name;
use crate::pricing::{calculate_cost, ModelPricing, PricingCache, PricingCatalog};
use crate::provider::{TokenBreakdown, UnifiedMessage};
use crate::usage::{DashboardDay, ModelSummary, ProviderSummary};
use anyhow::{anyhow, Result};
use chrono::{Duration, NaiveDate, Utc};
use rusqlite::{
    params, params_from_iter, types::Value, Connection, OptionalExtension, Transaction,
};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tracing::{info, warn};

const CANONICAL_SOURCE_SQL: &str =
    "CASE WHEN source IN ('antigravity-cli', 'antigravity-desktop') THEN 'antigravity' ELSE source END";

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
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let data_dir = home.join(".local").join("share").join("tokenpulse");
        Self {
            path: data_dir.join("usage.db"),
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
        let value: Option<String> = if source == "antigravity" {
            conn.query_row(
                "SELECT MAX(date) FROM usage_messages WHERE source IN ('antigravity', 'antigravity-cli', 'antigravity-desktop')",
                [],
                |row| row.get(0),
            )
            .optional()?
            .flatten()
        } else {
            conn.query_row(
                "SELECT MAX(date) FROM usage_messages WHERE source = ?1",
                params![source],
                |row| row.get(0),
            )
            .optional()?
            .flatten()
        };

        Ok(value.and_then(|date| NaiveDate::parse_from_str(&date, "%Y-%m-%d").ok()))
    }

    pub fn source_has_stale_parser_version(
        &self,
        source: &str,
        parser_version: &str,
    ) -> Result<bool> {
        let conn = self.open()?;
        let has_stale = if source == "antigravity" {
            conn.query_row(
                r#"
                SELECT 1
                FROM usage_messages
                WHERE source IN ('antigravity', 'antigravity-cli', 'antigravity-desktop') AND parser_version != ?1
                LIMIT 1
                "#,
                params![parser_version],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        } else {
            conn.query_row(
                r#"
                SELECT 1
                FROM usage_messages
                WHERE source = ?1 AND parser_version != ?2
                LIMIT 1
                "#,
                params![source, parser_version],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        };
        Ok(has_stale)
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
        let mut pricing = match load_pricing_for_usage(&pricing_cache, refresh_pricing) {
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
            let snapshot = ensure_pricing_snapshot(
                &tx,
                &pricing_cache,
                &mut pricing,
                message,
                refresh_pricing,
            )?;
            let cost = derive_message_cost(message, snapshot.as_ref(), pricing.is_some())?;

            tx.execute(
                r#"
                INSERT INTO usage_messages (
                    source, client, provider_id, model_id, session_id, message_key,
                    timestamp_ms, date, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, reasoning_tokens,
                    total_tokens, cost_usd, pricing_day, parser_version
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6,
                    ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17
                )
                ON CONFLICT(source, client, message_key) DO UPDATE SET
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
                    message.client_detail.as_deref().unwrap_or(&message.client),
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

    pub fn replace_sessions_messages(
        &self,
        messages: &[UnifiedMessage],
        refresh_pricing: bool,
    ) -> Result<BTreeSet<String>> {
        if messages.is_empty() {
            return Ok(BTreeSet::new());
        }

        let pricing_cache = PricingCache::new();
        let mut pricing = match load_pricing_for_usage(&pricing_cache, refresh_pricing) {
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
        let session_keys: BTreeSet<(String, String, String)> = messages
            .iter()
            .map(|message| {
                (
                    message.client.clone(),
                    message
                        .client_detail
                        .clone()
                        .unwrap_or_else(|| message.client.clone()),
                    message.session_id.clone(),
                )
            })
            .collect();

        for (source, client, session_id) in &session_keys {
            let mut stmt = tx.prepare(
                "SELECT DISTINCT date FROM usage_messages WHERE source = ?1 AND client = ?2 AND session_id = ?3",
            )?;
            let rows = stmt.query_map(params![source, client, session_id], |row| {
                row.get::<_, String>(0)
            })?;
            for row in rows.flatten() {
                affected_dates.insert(row);
            }
            drop(stmt);

            tx.execute(
                "DELETE FROM usage_messages WHERE source = ?1 AND client = ?2 AND session_id = ?3",
                params![source, client, session_id],
            )?;
        }

        for message in messages {
            let snapshot = ensure_pricing_snapshot(
                &tx,
                &pricing_cache,
                &mut pricing,
                message,
                refresh_pricing,
            )?;
            let cost = derive_message_cost(message, snapshot.as_ref(), pricing.is_some())?;

            tx.execute(
                r#"
                INSERT INTO usage_messages (
                    source, client, provider_id, model_id, session_id, message_key,
                    timestamp_ms, date, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, reasoning_tokens,
                    total_tokens, cost_usd, pricing_day, parser_version
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6,
                    ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17
                )
                ON CONFLICT(source, client, message_key) DO UPDATE SET
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
                    message.client_detail.as_deref().unwrap_or(&message.client),
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

    pub fn replace_source_messages(
        &self,
        source: &str,
        messages: &[UnifiedMessage],
        refresh_pricing: bool,
    ) -> Result<BTreeSet<String>> {
        if messages.is_empty() {
            return Ok(BTreeSet::new());
        }

        let pricing_cache = PricingCache::new();
        let mut pricing = match load_pricing_for_usage(&pricing_cache, refresh_pricing) {
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

        let existing_dates = load_source_dates(&tx, source)?;
        for date in &existing_dates {
            affected_dates.insert(date.clone());
        }

        delete_scoped_tx(&tx, None, &[source.to_string()], refresh_pricing)?;

        for message in messages {
            let snapshot = ensure_pricing_snapshot(
                &tx,
                &pricing_cache,
                &mut pricing,
                message,
                refresh_pricing,
            )?;
            let cost = derive_message_cost(message, snapshot.as_ref(), pricing.is_some())?;

            tx.execute(
                r#"
                INSERT INTO usage_messages (
                    source, client, provider_id, model_id, session_id, message_key,
                    timestamp_ms, date, input_tokens, output_tokens,
                    cache_read_tokens, cache_write_tokens, reasoning_tokens,
                    total_tokens, cost_usd, pricing_day, parser_version
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6,
                    ?7, ?8, ?9, ?10,
                    ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17
                )
                ON CONFLICT(source, client, message_key) DO UPDATE SET
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
                    message.client_detail.as_deref().unwrap_or(&message.client),
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

    pub fn repair_zero_costs(&self, since: Option<NaiveDate>, sources: &[String]) -> Result<usize> {
        let mut conn = self.open()?;
        if !has_zero_cost_repairs_pending(&conn, since, sources)? {
            return Ok(0);
        }
        let pricing = PricingCache::new().get_pricing_allow_stale_sync()?;
        let tx = conn.transaction()?;

        let mut sql = format!(
            r#"
            SELECT source, client, message_key, provider_id, model_id, date,
                   input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens
            FROM usage_messages
            WHERE cost_usd <= 0 AND total_tokens > 0 AND {NOT_PSEUDO_MODEL_SQL}
            "#,
        );
        let params = append_common_filters(&mut sql, since, sources);
        let mut stmt = tx.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                TokenBreakdown {
                    input: row.get(6)?,
                    output: row.get(7)?,
                    cache_read: row.get(8)?,
                    cache_write: row.get(9)?,
                    reasoning: row.get(10)?,
                },
            ))
        })?;

        let mut affected_dates = BTreeSet::new();
        let mut repaired = 0usize;

        for row in rows.flatten() {
            let (source, client, message_key, provider_id, model_id, date, tokens) = row;
            let Some(pricing_row) = pricing.lookup(&model_id, Some(provider_id.as_str())) else {
                continue;
            };
            let cost = calculate_cost(&tokens, pricing_row.pricing);
            if cost <= 0.0 {
                continue;
            }

            tx.execute(
                "UPDATE usage_messages SET cost_usd = ?1 WHERE source = ?2 AND client = ?3 AND message_key = ?4",
                params![cost, source, client, message_key],
            )?;

            // Save the newly found valid non-zero snapshot for this date/model
            let snapshot = pricing_row.pricing;
            tx.execute(
                r#"
                INSERT INTO daily_pricing_snapshots (
                    date, provider_id, model_id, input_cost_per_token,
                    output_cost_per_token, cache_read_input_token_cost,
                    cache_creation_input_token_cost, captured_at, pricing_source, pricing_version
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(date, provider_id, model_id) DO UPDATE SET
                    input_cost_per_token = excluded.input_cost_per_token,
                    output_cost_per_token = excluded.output_cost_per_token,
                    cache_read_input_token_cost = excluded.cache_read_input_token_cost,
                    cache_creation_input_token_cost = excluded.cache_creation_input_token_cost,
                    captured_at = excluded.captured_at,
                    pricing_source = excluded.pricing_source,
                    pricing_version = excluded.pricing_version
                "#,
                params![
                    date,
                    provider_id,
                    model_id,
                    snapshot.input_cost_per_token,
                    snapshot.output_cost_per_token,
                    snapshot.cache_read_input_token_cost,
                    snapshot.cache_creation_input_token_cost,
                    Utc::now().timestamp_millis(),
                    pricing_row.source,
                    pricing_row.version,
                ],
            )?;

            affected_dates.insert(date);
            repaired += 1;
        }
        drop(stmt);

        let now = Utc::now().timestamp_millis();
        for date in &affected_dates {
            rebuild_daily_for_date(&tx, date, now)?;
        }

        tx.commit()?;
        Ok(repaired)
    }

    pub fn load_summary_counts(
        &self,
        since: Option<NaiveDate>,
        sources: &[String],
    ) -> Result<(usize, usize)> {
        let conn = self.open()?;
        let mut subquery_filters = String::new();
        let params = append_common_filters(&mut subquery_filters, since, sources);
        let sql = format!(
            r#"
            SELECT COUNT(*),
                   COUNT(DISTINCT source || '::' || session_id)
            FROM (
                SELECT
                    date,
                    {CANONICAL_SOURCE_SQL} AS source,
                    session_id,
                    message_key
                FROM usage_messages
                WHERE 1=1 {subquery_filters}
                GROUP BY date, {CANONICAL_SOURCE_SQL}, session_id, message_key
            )
            "#,
        );

        conn.query_row(&sql, params_from_iter(params), |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
            ))
        })
        .map_err(Into::into)
    }

    pub fn load_messages(
        &self,
        since: Option<NaiveDate>,
        sources: &[String],
    ) -> Result<Vec<UnifiedMessage>> {
        let conn = self.open()?;
        let mut sql = String::from(
            r#"
            SELECT source,
                   client,
                   provider_id,
                   model_id,
                   session_id,
                   message_key,
                   timestamp_ms,
                   date,
                   input_tokens,
                   output_tokens,
                   cache_read_tokens,
                   cache_write_tokens,
                   reasoning_tokens,
                   cost_usd,
                   pricing_day,
                   parser_version
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
        let mut sql = format!(
            r#"
            SELECT date,
                   SUM(input_tokens),
                   SUM(output_tokens),
                   SUM(cache_read_tokens),
                   SUM(cache_write_tokens),
                   SUM(reasoning_tokens),
                   SUM(total_tokens),
                   SUM(cost_usd),
                   SUM(message_count),
                   SUM(session_count)
            FROM daily_model_usage
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
        let mut sql = format!(
            r#"
            SELECT date,
                   {CANONICAL_SOURCE_SQL} AS source,
                   provider_id,
                   model_id,
                   SUM(input_tokens),
                   SUM(output_tokens),
                   SUM(cache_read_tokens),
                   SUM(cache_write_tokens),
                   SUM(reasoning_tokens),
                   SUM(total_tokens),
                   SUM(cost_usd),
                   SUM(message_count),
                   SUM(session_count)
            FROM daily_model_usage
            WHERE 1=1
            "#,
        );
        let params = append_common_filters(&mut sql, since, sources);
        sql.push_str(&format!(
            " GROUP BY date, {CANONICAL_SOURCE_SQL}, provider_id, model_id ORDER BY date ASC, SUM(cost_usd) DESC",
        ));

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
        let mut sql = format!(
            r#"
            SELECT source,
                   SUM(cost_usd),
                   SUM(total_tokens),
                   SUM(message_count),
                   SUM(session_count)
            FROM daily_model_usage
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
        let mut subquery_filters = String::new();
        let params = append_common_filters(&mut subquery_filters, since, sources);
        let sql = format!(
            r#"
            SELECT model_id,
                   provider_id,
                   source,
                   session_id,
                   SUM(cost_usd),
                   SUM(total_tokens),
                   COUNT(*),
                   SUM(input_tokens),
                   SUM(output_tokens),
                   SUM(cache_read_tokens + cache_write_tokens)
            FROM (
                SELECT
                    date,
                    {CANONICAL_SOURCE_SQL} AS source,
                    session_id,
                    message_key,
                    MAX(provider_id) AS provider_id,
                    MAX(model_id) AS model_id,
                    MAX(cost_usd) AS cost_usd,
                    MAX(total_tokens) AS total_tokens,
                    MAX(input_tokens) AS input_tokens,
                    MAX(output_tokens) AS output_tokens,
                    MAX(cache_read_tokens) AS cache_read_tokens,
                    MAX(cache_write_tokens) AS cache_write_tokens
                FROM usage_messages
                WHERE 1=1 {subquery_filters}
                GROUP BY date, {CANONICAL_SOURCE_SQL}, session_id, message_key
            )
            GROUP BY source, provider_id, model_id, session_id ORDER BY SUM(total_tokens) DESC, model_id ASC
            "#,
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(params), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, f64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
            ))
        })?;

        let mut grouped: BTreeMap<String, AggregatedModelSummary> = BTreeMap::new();
        for row in rows.flatten() {
            let (
                model_id,
                provider_id,
                source,
                session_id,
                cost,
                tokens,
                message_count,
                input_tokens,
                output_tokens,
                cache_tokens,
            ) = row;
            let normalized = normalize_model_name(&model_id);
            let entry = grouped.entry(normalized).or_default();
            entry.providers.insert(provider_id);
            entry.sources.insert(source);
            entry.sessions.insert(session_id);
            entry.cost += cost;
            entry.tokens += tokens;
            entry.message_count += message_count as usize;
            entry.input_tokens += input_tokens;
            entry.output_tokens += output_tokens;
            entry.cache_tokens += cache_tokens;
        }

        let mut summaries: Vec<ModelSummary> = grouped
            .into_iter()
            .filter(|(_, summary)| summary.tokens > 0)
            .map(|(model, summary)| ModelSummary {
                model,
                provider: summary.providers.into_iter().collect::<Vec<_>>().join(","),
                source: summary.sources.into_iter().collect::<Vec<_>>().join(","),
                cost: summary.cost,
                tokens: summary.tokens,
                input_tokens: summary.input_tokens,
                output_tokens: summary.output_tokens,
                cache_tokens: summary.cache_tokens,
                message_count: summary.message_count,
                session_count: summary.sessions.len(),
                percent: 0.0,
            })
            .collect();
        summaries.sort_by(|left, right| right.tokens.cmp(&left.tokens));
        Ok(summaries)
    }

    fn open(&self) -> Result<Connection> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut conn = Connection::open(&self.path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode = WAL;")?;
        ensure_schema_initialized(&self.path, &mut conn)?;
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
        delete_scoped_tx(&tx, range, sources, refresh_pricing)?;

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
        let mut first = true;
        for source in sources {
            if source == "antigravity" {
                for s in &["antigravity", "antigravity-cli", "antigravity-desktop"] {
                    if !first {
                        sql.push_str(", ");
                    }
                    sql.push('?');
                    params.push(Value::from((*s).to_string()));
                    first = false;
                }
            } else {
                if !first {
                    sql.push_str(", ");
                }
                sql.push('?');
                params.push(Value::from(source.clone()));
                first = false;
            }
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
        let mut first = true;
        for source in sources {
            if source == "antigravity" {
                for s in &["antigravity", "antigravity-cli", "antigravity-desktop"] {
                    if !first {
                        sql.push_str(", ");
                    }
                    sql.push('?');
                    params.push(Value::from((*s).to_string()));
                    first = false;
                }
            } else {
                if !first {
                    sql.push_str(", ");
                }
                sql.push('?');
                params.push(Value::from(source.clone()));
                first = false;
            }
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

fn load_source_dates(tx: &Transaction<'_>, source: &str) -> Result<Vec<String>> {
    let mapper = |row: &rusqlite::Row<'_>| row.get::<_, String>(0);
    let dates = if source == "antigravity" {
        let mut stmt = tx.prepare("SELECT DISTINCT date FROM usage_messages WHERE source IN ('antigravity', 'antigravity-cli', 'antigravity-desktop') ORDER BY date ASC")?;
        let res = stmt
            .query_map([], mapper)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        res
    } else {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT date FROM usage_messages WHERE source = ?1 ORDER BY date ASC",
        )?;
        let res = stmt
            .query_map(params![source], mapper)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        res
    };
    Ok(dates)
}

fn delete_scoped_tx(
    tx: &Transaction<'_>,
    range: Option<DateRange>,
    sources: &[String],
    refresh_pricing: bool,
) -> Result<()> {
    let snapshot_keys = if refresh_pricing {
        load_pricing_snapshot_keys(tx, range, sources)?
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

    Ok(())
}

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<UnifiedMessage> {
    let input: i64 = row.get(8)?;
    let output: i64 = row.get(9)?;
    let cache_read: i64 = row.get(10)?;
    let cache_write: i64 = row.get(11)?;
    let reasoning: i64 = row.get(12)?;
    Ok(UnifiedMessage {
        client: row.get(0)?,
        client_detail: row.get(1)?,
        provider_id: row.get(2)?,
        model_id: row.get(3)?,
        session_id: row.get(4)?,
        message_key: row.get(5)?,
        timestamp: row.get(6)?,
        date: row.get(7)?,
        tokens: TokenBreakdown {
            input,
            output,
            cache_read,
            cache_write,
            reasoning,
        },
        cost: row.get(13)?,
        pricing_day: row.get(14)?,
        parser_version: row.get(15)?,
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

fn rebuild_daily_for_date(tx: &Transaction<'_>, date: &str, now: i64) -> Result<()> {
    tx.execute(
        "DELETE FROM daily_model_usage WHERE date = ?1",
        params![date],
    )?;
    tx.execute(
        &format!(
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
        FROM (
            SELECT
                date,
                {CANONICAL_SOURCE_SQL} AS source,
                session_id,
                message_key,
                MAX(provider_id) AS provider_id,
                MAX(model_id) AS model_id,
                MAX(input_tokens) AS input_tokens,
                MAX(output_tokens) AS output_tokens,
                MAX(cache_read_tokens) AS cache_read_tokens,
                MAX(cache_write_tokens) AS cache_write_tokens,
                MAX(reasoning_tokens) AS reasoning_tokens,
                MAX(total_tokens) AS total_tokens,
                MAX(cost_usd) AS cost_usd
            FROM usage_messages
            WHERE date = ?1
            GROUP BY date, {CANONICAL_SOURCE_SQL}, session_id, message_key
        )
        GROUP BY date, source, provider_id, model_id
        "#,
        ),
        params![date, now],
    )?;
    Ok(())
}

fn ensure_pricing_snapshot(
    tx: &Transaction<'_>,
    pricing_cache: &PricingCache,
    pricing: &mut Option<PricingCatalog>,
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

    let mut looked_up = pricing
        .as_ref()
        .and_then(|p| p.lookup(&message.model_id, Some(message.provider_id.as_str())));

    if looked_up.is_none()
        && !is_pseudo_model_id(&message.model_id)
        && !PricingCache::has_refreshed_this_run()
    {
        match pricing_cache.lazy_refresh_sync() {
            Ok(Some(new_catalog)) => {
                info!(
                    "Pricing for model {} was missing or zero-cost; refreshed pricing catalog on demand",
                    message.model_id
                );
                *pricing = Some(new_catalog);
                looked_up = pricing
                    .as_ref()
                    .and_then(|p| p.lookup(&message.model_id, Some(message.provider_id.as_str())));
            }
            Ok(None) => {}
            Err(error) => {
                warn!("Failed to lazy-refresh pricing cache: {}", error);
            }
        }
    }

    if let Some(resolved) = looked_up {
        let snapshot = resolved.pricing.clone();
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
                resolved.source,
                resolved.version,
            ],
        )?;
        Ok(Some(snapshot))
    } else {
        Ok(None)
    }
}

fn load_pricing_for_usage(
    pricing_cache: &PricingCache,
    refresh_pricing: bool,
) -> Result<PricingCatalog> {
    if refresh_pricing {
        pricing_cache.get_pricing_sync()
    } else {
        pricing_cache.get_pricing_allow_stale_sync()
    }
}

fn derive_message_cost(
    message: &UnifiedMessage,
    snapshot: Option<&ModelPricing>,
    pricing_available: bool,
) -> Result<f64> {
    if let Some(snapshot) = snapshot {
        // Cost is always derived from the centralized daily pricing snapshot.
        // Source-reported message.cost is intentionally ignored so all agents
        // share one pricing policy and historical repairs use the same logic.
        return Ok(calculate_cost(&message.tokens, snapshot));
    }

    if !pricing_available {
        return Err(anyhow!(
            "Pricing data unavailable for {}:{} on {}. Re-run with connectivity or use --refresh-pricing when pricing is reachable.",
            message.client,
            message.model_id,
            message.date
        ));
    }

    Ok(0.0)
}

#[derive(Default)]
struct AggregatedModelSummary {
    providers: BTreeSet<String>,
    sources: BTreeSet<String>,
    sessions: BTreeSet<String>,
    cost: f64,
    tokens: i64,
    message_count: usize,
    input_tokens: i64,
    output_tokens: i64,
    cache_tokens: i64,
}

fn ensure_schema_initialized(path: &PathBuf, conn: &mut Connection) -> Result<()> {
    if initialized_paths()
        .lock()
        .map_err(|_| anyhow!("Usage store schema mutex poisoned"))?
        .contains(path)
    {
        return Ok(());
    }

    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='usage_messages'",
            [],
            |row| {
                let count: i64 = row.get(0)?;
                Ok(count > 0)
            },
        )
        .unwrap_or(false);

    if table_exists {
        let client_is_pk: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('usage_messages') WHERE name = 'client' AND pk > 0",
                [],
                |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count > 0)
                },
            )
            .unwrap_or(false);

        if !client_is_pk {
            let _ = conn.execute("DROP TABLE IF EXISTS usage_messages;", []);
            let _ = conn.execute("DROP TABLE IF EXISTS daily_model_usage;", []);
            let _ = conn.execute("DROP TABLE IF EXISTS daily_pricing_snapshots;", []);
        }
    }

    conn.execute_batch(USAGE_SCHEMA_SQL)?;

    initialized_paths()
        .lock()
        .map_err(|_| anyhow!("Usage store schema mutex poisoned"))?
        .insert(path.clone());
    Ok(())
}

fn initialized_paths() -> &'static Mutex<HashSet<PathBuf>> {
    static PATHS: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    PATHS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Pseudo-model ids reported by agent logs that do not correspond to a real,
/// purchasable model: routing aliases ("auto-gemini-3", "gemini-default"),
/// internal features ("codex-auto-review"), and parser fallbacks ("unknown").
/// They can never resolve against the pricing catalog, so they must not
/// trigger on-demand pricing refreshes or keep cost repairs pending forever.
fn is_pseudo_model_id(model_id: &str) -> bool {
    let id = model_id.to_ascii_lowercase();
    id.is_empty()
        || id == "unknown"
        || id.starts_with("auto-")
        || id.ends_with("-auto-review")
        || id.ends_with("-default")
}

/// SQL twin of [`is_pseudo_model_id`]; keep both in sync.
const NOT_PSEUDO_MODEL_SQL: &str = "model_id <> '' \
    AND lower(model_id) <> 'unknown' \
    AND lower(model_id) NOT LIKE 'auto-%' \
    AND lower(model_id) NOT LIKE '%-auto-review' \
    AND lower(model_id) NOT LIKE '%-default'";

fn has_zero_cost_repairs_pending(
    conn: &Connection,
    since: Option<NaiveDate>,
    sources: &[String],
) -> Result<bool> {
    let mut sql = format!(
        r#"
        SELECT 1
        FROM usage_messages
        WHERE cost_usd <= 0 AND total_tokens > 0 AND {NOT_PSEUDO_MODEL_SQL}
        "#,
    );
    let params = append_common_filters(&mut sql, since, sources);
    sql.push_str(" LIMIT 1");

    Ok(conn
        .query_row(&sql, params_from_iter(params), |_| Ok(()))
        .optional()?
        .is_some())
}

const USAGE_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS usage_messages (
    source TEXT NOT NULL,
    client TEXT NOT NULL,
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
    PRIMARY KEY (source, client, message_key)
);
CREATE INDEX IF NOT EXISTS idx_usage_messages_date ON usage_messages(date);
CREATE INDEX IF NOT EXISTS idx_usage_messages_source_date ON usage_messages(source, date);
CREATE INDEX IF NOT EXISTS idx_usage_messages_zero_cost
    ON usage_messages(date, source)
    WHERE cost_usd <= 0 AND total_tokens > 0;

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
"#;

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

    fn sample_derived_cost_message(date: &str, key: &str) -> UnifiedMessage {
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
    fn source_has_stale_parser_version_detects_mismatches() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));
        let mut message = sample_message("2024-03-10", "m1");
        message.client = "gemini".to_string();
        message.provider_id = "google".to_string();
        message.parser_version = "gemini-v2".to_string();

        store.ingest_messages(&[message], false).unwrap();

        assert!(store
            .source_has_stale_parser_version("gemini", "gemini-v3")
            .unwrap());
        assert!(!store
            .source_has_stale_parser_version("gemini", "gemini-v2")
            .unwrap());
        assert!(!store
            .source_has_stale_parser_version("claude", "claude-v2")
            .unwrap());
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

    #[test]
    fn replace_source_messages_keeps_existing_rows_when_new_parse_is_empty() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));

        let mut message = sample_message("2024-03-10", "gemini-m1");
        message.client = "gemini".to_string();
        message.provider_id = "google".to_string();
        message.parser_version = "gemini-v2".to_string();

        store.ingest_messages(&[message], false).unwrap();
        store.replace_source_messages("gemini", &[], false).unwrap();

        let remaining = store.load_messages(None, &["gemini".to_string()]).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].parser_version, "gemini-v2");
    }

    #[test]
    fn replace_source_messages_replaces_old_rows_after_new_parse_succeeds() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));

        let mut old = sample_message("2024-03-10", "gemini-old");
        old.client = "gemini".to_string();
        old.provider_id = "google".to_string();
        old.parser_version = "gemini-v2".to_string();
        old.session_id = "old-session".to_string();

        let mut replacement = sample_message("2024-03-11", "gemini-new");
        replacement.client = "gemini".to_string();
        replacement.provider_id = "google".to_string();
        replacement.parser_version = "gemini-v3".to_string();
        replacement.session_id = "new-session".to_string();

        store.ingest_messages(&[old], false).unwrap();
        store
            .replace_source_messages("gemini", &[replacement], false)
            .unwrap();

        let remaining = store.load_messages(None, &["gemini".to_string()]).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].message_key, "gemini-new");
        assert_eq!(remaining[0].date, "2024-03-11");
        assert_eq!(remaining[0].parser_version, "gemini-v3");
    }

    #[test]
    fn replace_sessions_messages_replaces_only_changed_sessions() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));

        let mut old_changed = sample_message("2024-03-10", "changed-old");
        old_changed.session_id = "changed-session".to_string();

        let mut untouched = sample_message("2024-03-11", "untouched");
        untouched.session_id = "untouched-session".to_string();

        store
            .ingest_messages(&[old_changed, untouched], false)
            .unwrap();

        let mut replacement = sample_message("2024-03-12", "changed-new");
        replacement.session_id = "changed-session".to_string();

        store
            .replace_sessions_messages(&[replacement], false)
            .unwrap();

        let remaining = store.load_messages(None, &["claude".to_string()]).unwrap();
        let keys = remaining
            .iter()
            .map(|message| message.message_key.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(remaining.len(), 2);
        assert!(keys.contains("changed-new"));
        assert!(keys.contains("untouched"));
        assert!(!keys.contains("changed-old"));

        let days = store
            .load_dashboard_days(None, &["claude".to_string()])
            .unwrap();
        assert_eq!(
            days.iter().map(|day| day.date.as_str()).collect::<Vec<_>>(),
            vec!["2024-03-11", "2024-03-12"]
        );
    }

    #[test]
    fn derive_message_cost_errors_when_pricing_fetch_failed_and_cost_is_missing() {
        let message = sample_derived_cost_message("2024-03-10", "missing-price");

        let error = derive_message_cost(&message, None, false).unwrap_err();

        assert!(error
            .to_string()
            .contains("Pricing data unavailable for claude:claude-3-opus"));
    }

    #[test]
    fn derive_message_cost_uses_snapshot_when_available() {
        let message = sample_derived_cost_message("2024-03-10", "priced");
        let pricing = ModelPricing::simple(0.01, 0.02);

        let cost = derive_message_cost(&message, Some(&pricing), true).unwrap();

        assert!(cost > 0.0);
    }

    #[test]
    fn derive_message_cost_ignores_parser_supplied_cost_when_snapshot_exists() {
        let mut message = sample_derived_cost_message("2024-03-10", "m1");
        message.cost = 42.5;
        let pricing = ModelPricing::simple(0.01, 0.02);

        let cost = derive_message_cost(&message, Some(&pricing), true).unwrap();

        assert!(cost > 0.0);
        assert!(cost < 42.5);
    }

    #[test]
    fn load_model_summaries_normalizes_and_merges_variants() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));

        let mut first = sample_message("2024-03-10", "m1");
        first.model_id = "antigravity-claude-opus-4-5-thinking-high".to_string();
        first.session_id = "shared-session".to_string();
        first.cost = 2.0;

        let mut second = sample_message("2024-03-10", "m2");
        second.client = "codex".to_string();
        second.model_id = "claude-opus-4.5".to_string();
        second.session_id = "shared-session".to_string();
        second.cost = 3.0;

        store.ingest_messages(&[first, second], false).unwrap();

        let summaries = store.load_model_summaries(None, &[]).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].model, "claude-opus-4-5");
        assert_eq!(summaries[0].source, "claude,codex");
        assert_eq!(summaries[0].provider, "anthropic");
        assert_eq!(summaries[0].session_count, 1);
        assert_eq!(summaries[0].message_count, 2);
        assert!(summaries[0].cost >= 0.0);
    }

    #[test]
    fn test_load_model_summaries_filters_out_zero_tokens() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));

        // Message with tokens > 0
        let mut first = sample_message("2024-03-10", "m1");
        first.model_id = "claude-3-opus".to_string();
        first.tokens = TokenBreakdown {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        };

        // Message with tokens = 0
        let mut second = sample_message("2024-03-10", "m2");
        second.model_id = "gpt-4".to_string();
        second.tokens = TokenBreakdown {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        };

        store.ingest_messages(&[first, second], false).unwrap();

        let summaries = store.load_model_summaries(None, &[]).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].model, "claude-3-opus");
    }

    #[test]
    fn load_summary_counts_returns_message_and_session_totals() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));

        let mut first = sample_message("2024-03-10", "m1");
        first.session_id = "session-a".to_string();
        let mut second = sample_message("2024-03-10", "m2");
        second.session_id = "session-b".to_string();

        store.ingest_messages(&[first, second], false).unwrap();

        let (message_count, session_count) = store.load_summary_counts(None, &[]).unwrap();
        assert_eq!(message_count, 2);
        assert_eq!(session_count, 2);
    }

    #[test]
    fn test_antigravity_source_names_handling() {
        let tempdir = tempfile::tempdir().unwrap();
        let db_path = tempdir.path().join("usage.sqlite3");

        let store = UsageStore::with_path(db_path);

        // Ingest three messages with distinct antigravity source kinds and distinct session IDs
        let mut msg1 = sample_message("2026-05-22", "key1");
        msg1.client = "antigravity".to_string();
        msg1.client_detail = Some("antigravity-cli".to_string());
        msg1.session_id = "session-1".to_string();
        let mut msg2 = sample_message("2026-05-22", "key2");
        msg2.client = "antigravity".to_string();
        msg2.client_detail = Some("antigravity-desktop".to_string());
        msg2.session_id = "session-2".to_string();
        let mut msg3 = sample_message("2026-05-22", "key3");
        msg3.client = "antigravity".to_string();
        msg3.client_detail = Some("antigravity".to_string());
        msg3.session_id = "session-3".to_string();

        store.ingest_messages(&[msg1, msg2, msg3], false).unwrap();

        // 1. Verify that they remain distinct in the database under client detail
        let conn = store.open().unwrap();
        let count_cli: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_messages WHERE client = 'antigravity-cli'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let count_desktop: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_messages WHERE client = 'antigravity-desktop'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let count_antigravity: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_messages WHERE client = 'antigravity'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(count_cli, 1);
        assert_eq!(count_desktop, 1);
        assert_eq!(count_antigravity, 1);

        // 2. Verify that querying with source filter "antigravity" returns all three sources combined
        let summaries = store
            .load_model_summaries(None, &["antigravity".to_string()])
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].message_count, 3);
        assert_eq!(summaries[0].session_count, 3);

        // 3. Verify that load_summary_counts with source filter "antigravity" also aggregates them
        let (msg_cnt, session_cnt) = store
            .load_summary_counts(None, &["antigravity".to_string()])
            .unwrap();
        assert_eq!(msg_cnt, 3);
        assert_eq!(session_cnt, 3);

        // 4. Verify that duplicate message keys on the same platform and session are deduplicated in aggregation
        let mut msg4 = sample_message("2026-05-22", "dup_key");
        msg4.client = "antigravity".to_string();
        msg4.client_detail = Some("antigravity-cli".to_string());
        msg4.session_id = "session-dup".to_string();
        msg4.tokens.input = 100;

        let mut msg5 = sample_message("2026-05-22", "dup_key");
        msg5.client = "antigravity".to_string();
        msg5.client_detail = Some("antigravity-desktop".to_string());
        msg5.session_id = "session-dup".to_string();
        msg5.tokens.input = 100;

        store.ingest_messages(&[msg4, msg5], false).unwrap();

        let total_db_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_messages WHERE message_key = 'dup_key'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total_db_count, 2);

        let (msg_cnt_dedup, session_cnt_dedup) = store
            .load_summary_counts(None, &["antigravity".to_string()])
            .unwrap();
        assert_eq!(msg_cnt_dedup, 4);
        assert_eq!(session_cnt_dedup, 4);
        assert_eq!(msg_cnt, 3);
        assert_eq!(session_cnt, 3);
    }

    #[test]
    fn test_zero_cost_not_saved_in_pricing_snapshots() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));

        // Ingest a message with a model that is missing/zero-priced
        let mut message = sample_message("2024-03-10", "m1");
        message.model_id = "completely-nonexistent-model-id-xyz".to_string();

        store.ingest_messages(&[message], false).unwrap();

        // Query daily_pricing_snapshots to ensure it remains empty
        let conn = store.open().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM daily_pricing_snapshots", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(
            count, 0,
            "No snapshots should be saved for missing/zero-cost models"
        );
    }

    #[test]
    fn pseudo_model_ids_are_detected() {
        for id in [
            "",
            "unknown",
            "Unknown",
            "auto-gemini-3",
            "codex-auto-review",
            "gemini-default",
        ] {
            assert!(is_pseudo_model_id(id), "{id:?} should be pseudo");
        }
        for id in [
            "gpt-5.4",
            "claude-opus-4-6",
            "moonshotai/kimi-k2.5",
            "gemini-3-flash-preview",
        ] {
            assert!(!is_pseudo_model_id(id), "{id:?} should not be pseudo");
        }
    }

    #[test]
    fn pseudo_models_do_not_keep_repairs_pending() {
        let tempdir = tempfile::tempdir().unwrap();
        let store = UsageStore::with_path(tempdir.path().join("usage.sqlite3"));

        let mut message = sample_message("2024-03-10", "m1");
        message.model_id = "codex-auto-review".to_string();

        store.ingest_messages(&[message], false).unwrap();

        let conn = store.open().unwrap();
        assert!(
            !has_zero_cost_repairs_pending(&conn, None, &[]).unwrap(),
            "pseudo-model zero-cost rows must not keep cost repairs pending"
        );
        assert_eq!(store.repair_zero_costs(None, &[]).unwrap(), 0);
    }
}
