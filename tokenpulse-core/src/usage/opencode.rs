use crate::pricing::{calculate_cost, lookup_model_pricing_or_warn, PricingCache};
use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::Connection;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, warn};

pub struct OpenCodeSessionParser {
    pricing_cache: PricingCache,
}

impl OpenCodeSessionParser {
    pub fn new() -> Self {
        Self {
            pricing_cache: PricingCache::new(),
        }
    }

    fn parse_database(
        &self,
        db_path: &PathBuf,
        pricing: &HashMap<String, crate::pricing::ModelPricing>,
        since: Option<NaiveDate>,
    ) -> Vec<UnifiedMessage> {
        let conn = match Connection::open(db_path) {
            Ok(connection) => connection,
            Err(error) => {
                warn!("Failed to open database {:?}: {}", db_path, error);
                return Vec::new();
            }
        };

        let since_timestamp_ms = since.and_then(start_of_day_timestamp_ms);
        let mut rows = self
            .load_rows_with_timestamp(&conn, since_timestamp_ms)
            .or_else(|| self.load_rows_without_timestamp(&conn))
            .unwrap_or_default();
        rows.sort_by_key(|row| row.timestamp.unwrap_or_default());

        let mut messages: Vec<UnifiedMessage> = rows
            .into_iter()
            .filter_map(|row| self.parse_row(row, pricing))
            .collect();

        if let Some(since) = since {
            messages.retain(|message| message_on_or_after(message, since));
        }

        messages
    }

    fn load_rows_with_timestamp(
        &self,
        conn: &Connection,
        since_timestamp_ms: Option<i64>,
    ) -> Option<Vec<OpenCodeRow>> {
        let sql = if since_timestamp_ms.is_some() {
            "SELECT id, session_id, data, timestamp FROM message WHERE timestamp >= ?1 ORDER BY timestamp"
        } else {
            "SELECT id, session_id, data, timestamp FROM message ORDER BY timestamp"
        };
        let mut stmt = conn.prepare(sql).ok()?;

        if let Some(since_timestamp_ms) = since_timestamp_ms {
            let rows = stmt
                .query_map([since_timestamp_ms], |row| {
                    Ok(OpenCodeRow {
                        message_id: row.get::<_, Option<String>>(0)?,
                        session_id: row.get::<_, Option<String>>(1)?,
                        data: row.get(2)?,
                        timestamp: row.get::<_, Option<i64>>(3)?,
                    })
                })
                .ok()?;
            Some(rows.flatten().collect())
        } else {
            let rows = stmt
                .query_map([], |row| {
                    Ok(OpenCodeRow {
                        message_id: row.get::<_, Option<String>>(0)?,
                        session_id: row.get::<_, Option<String>>(1)?,
                        data: row.get(2)?,
                        timestamp: row.get::<_, Option<i64>>(3)?,
                    })
                })
                .ok()?;
            Some(rows.flatten().collect())
        }
    }

    fn load_rows_without_timestamp(&self, conn: &Connection) -> Option<Vec<OpenCodeRow>> {
        let mut stmt = conn
            .prepare("SELECT id, session_id, data FROM message")
            .ok()?;

        let rows = stmt
            .query_map([], |row| {
                Ok(OpenCodeRow {
                    message_id: row.get::<_, Option<String>>(0)?,
                    session_id: row.get::<_, Option<String>>(1)?,
                    data: row.get(2)?,
                    timestamp: None,
                })
            })
            .ok()?;

        Some(rows.flatten().collect())
    }

    fn parse_row(
        &self,
        row: OpenCodeRow,
        pricing: &std::collections::HashMap<String, crate::pricing::ModelPricing>,
    ) -> Option<UnifiedMessage> {
        let msg_data: OpenCodeMessageData = serde_json::from_str(&row.data).ok()?;

        if msg_data.role.as_deref() != Some("assistant") {
            return None;
        }

        let model_id = msg_data
            .model_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let provider_id = msg_data
            .provider_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let session_id = msg_data
            .session_id
            .clone()
            .or(row.session_id.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let message_id = msg_data.id.clone().or(row.message_id.clone());
        let source_cost = msg_data.cost.unwrap_or(0.0).max(0.0);
        let timestamp = row
            .timestamp
            .or_else(|| {
                msg_data
                    .time
                    .as_ref()
                    .and_then(OpenCodeTime::created_timestamp)
            })
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
        let tokens = msg_data.tokens?;
        let date = chrono::DateTime::from_timestamp_millis(timestamp)
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let message_key = message_id
            .clone()
            .unwrap_or_else(|| format!("{}:{}:{}", session_id, timestamp, model_id));

        let token_breakdown = TokenBreakdown {
            input: tokens.input.max(0),
            output: tokens.output.max(0),
            cache_read: tokens.cache.read.max(0),
            cache_write: tokens.cache.write.max(0),
            reasoning: tokens.reasoning.unwrap_or(0).max(0),
        };

        let cost = lookup_model_pricing_or_warn(&model_id, pricing)
            .map(|pricing| calculate_cost(&token_breakdown, pricing))
            .unwrap_or(source_cost);

        Some(
            UnifiedMessage::new(
                "opencode",
                model_id,
                provider_id,
                session_id,
                message_key,
                timestamp,
                token_breakdown,
            )
            .with_cost(cost)
            .with_pricing_day(date)
            .with_parser_version(self.parser_version()),
        )
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
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        vec![home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db")]
    }

    fn parse_sessions(&self, _since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        let pricing = match self.pricing_cache.get_pricing_sync() {
            Ok(pricing) => pricing,
            Err(error) => {
                warn!(
                    "Failed to load pricing data for OpenCode usage parsing; continuing without refreshed pricing: {}",
                    error
                );
                HashMap::new()
            }
        };
        let mut all_messages = Vec::new();

        for db_path in self.session_paths() {
            if !db_path.exists() {
                continue;
            }

            debug!("Parsing OpenCode database: {:?}", db_path);
            let msgs = self.parse_database(&db_path, &pricing, _since);
            all_messages.extend(msgs);
        }

        all_messages.sort_by_key(|message| message.timestamp);
        Ok(all_messages)
    }
}

fn start_of_day_timestamp_ms(date: NaiveDate) -> Option<i64> {
    date.and_hms_opt(0, 0, 0)
        .map(|dt| dt.and_utc().timestamp_millis())
}

fn message_on_or_after(message: &UnifiedMessage, since: NaiveDate) -> bool {
    NaiveDate::parse_from_str(&message.date, "%Y-%m-%d")
        .map(|date| date >= since)
        .unwrap_or(false)
}

#[derive(Debug)]
struct OpenCodeRow {
    message_id: Option<String>,
    session_id: Option<String>,
    data: String,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct OpenCodeMessageData {
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "sessionID", default)]
    session_id: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(rename = "modelID", alias = "model", default)]
    model_id: Option<String>,
    #[serde(rename = "providerID", alias = "provider", default)]
    provider_id: Option<String>,
    #[serde(default)]
    cost: Option<f64>,
    #[serde(default)]
    tokens: Option<OpenCodeTokens>,
    #[serde(default)]
    time: Option<OpenCodeTime>,
}

#[derive(Debug, Deserialize)]
struct OpenCodeTokens {
    input: i64,
    output: i64,
    reasoning: Option<i64>,
    cache: OpenCodeCache,
}

#[derive(Debug, Deserialize)]
struct OpenCodeCache {
    read: i64,
    write: i64,
}

#[derive(Debug, Deserialize)]
struct OpenCodeTime {
    created: Option<f64>,
    completed: Option<f64>,
}

impl OpenCodeTime {
    fn created_timestamp(&self) -> Option<i64> {
        self.created.or(self.completed).map(|ts| {
            if ts > 10_000_000_000.0 {
                ts as i64
            } else {
                (ts * 1000.0) as i64
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_opencode_structure() {
        let json = r#"{
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
        }"#;

        let mut bytes = json.as_bytes().to_vec();
        let msg: OpenCodeMessageData = serde_json::from_slice(&mut bytes).unwrap();

        assert_eq!(msg.model_id, Some("claude-sonnet-4".to_string()));
        assert_eq!(msg.tokens.unwrap().input, 1000);
    }

    #[test]
    fn test_negative_values_clamped_to_zero() {
        let json = r#"{
            "id": "msg_negative",
            "sessionID": "ses_negative",
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
        }"#;

        let mut temp_file = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
        temp_file.write_all(json.as_bytes()).unwrap();

        let parser = OpenCodeSessionParser::new();
        let msg = parser.parse_row(
            OpenCodeRow {
                message_id: Some("msg_negative".to_string()),
                session_id: Some("ses_negative".to_string()),
                data: json.to_string(),
                timestamp: None,
            },
            &std::collections::HashMap::new(),
        );

        assert!(msg.is_some());
        let msg = msg.unwrap();
        assert_eq!(msg.tokens.input, 0);
        assert_eq!(msg.tokens.output, 0);
        assert_eq!(msg.tokens.cache_read, 0);
        assert_eq!(msg.tokens.cache_write, 0);
        assert_eq!(msg.tokens.reasoning, 0);
    }

    #[test]
    fn test_open_code_time_helper() {
        let time = OpenCodeTime {
            created: Some(1700000000000.0),
            completed: None,
        };
        assert_eq!(time.created_timestamp(), Some(1700000000000));
    }
}
