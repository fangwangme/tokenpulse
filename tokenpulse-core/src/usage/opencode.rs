use crate::pricing::{calculate_cost, lookup_model_pricing, PricingCache};
use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::Connection;
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
        pricing: &std::collections::HashMap<String, crate::pricing::ModelPricing>,
    ) -> Vec<UnifiedMessage> {
        let mut messages = Vec::new();

        let conn = match Connection::open(db_path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to open database {:?}: {}", db_path, e);
                return messages;
            }
        };

        let mut stmt = match conn
            .prepare("SELECT id, session_id, data, timestamp FROM message ORDER BY timestamp")
        {
            Ok(s) => s,
            Err(e) => {
                warn!("Failed to prepare statement: {}", e);
                return messages;
            }
        };

        let rows = stmt.query_map([], |row| {
            Ok(OpenCodeRow {
                id: row.get(0)?,
                session_id: row.get(1)?,
                data: row.get(2)?,
                timestamp: row.get(3)?,
            })
        });

        if let Ok(rows) = rows {
            for row in rows.flatten() {
                if let Ok(msg_data) = serde_json::from_str::<OpenCodeMessageData>(&row.data) {
                    let model_id = msg_data.model.unwrap_or_else(|| "unknown".to_string());

                    let tokens = TokenBreakdown {
                        input: msg_data.input_tokens.unwrap_or(0),
                        output: msg_data.output_tokens.unwrap_or(0),
                        cache_read: msg_data.cache_read_tokens.unwrap_or(0),
                        cache_write: msg_data.cache_write_tokens.unwrap_or(0),
                        reasoning: 0,
                    };

                    let date = chrono::DateTime::from_timestamp_millis(row.timestamp)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    let cost = match lookup_model_pricing(&model_id, pricing) {
                        Some(p) => calculate_cost(&tokens, p),
                        None => 0.0,
                    };

                    let msg = UnifiedMessage {
                        client: "opencode".to_string(),
                        model_id,
                        provider_id: msg_data.provider.unwrap_or_else(|| "anthropic".to_string()),
                        session_id: row.session_id,
                        timestamp: row.timestamp,
                        date,
                        tokens,
                        cost,
                    };

                    messages.push(msg);
                }
            }
        }

        messages
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

    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        let pricing = tokio::runtime::Handle::try_current()
            .map(|handle| handle.block_on(self.pricing_cache.get_pricing()))
            .unwrap_or_else(|_| {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(self.pricing_cache.get_pricing())
            })?;

        let mut all_messages = Vec::new();

        for db_path in self.session_paths() {
            if !db_path.exists() {
                continue;
            }

            debug!("Parsing OpenCode database: {:?}", db_path);
            let msgs = self.parse_database(&db_path, &pricing);
            all_messages.extend(msgs);
        }

        all_messages.sort_by_key(|m| m.timestamp);
        Ok(all_messages)
    }
}

#[derive(Debug)]
struct OpenCodeRow {
    id: String,
    session_id: String,
    data: String,
    timestamp: i64,
}

#[derive(Debug, serde::Deserialize)]
struct OpenCodeMessageData {
    model: Option<String>,
    provider: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
}
