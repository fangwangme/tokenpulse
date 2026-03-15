use crate::pricing::{calculate_cost, lookup_model_pricing, PricingCache};
use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};
use crate::usage::scanner;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;
use tracing::{debug, warn};

pub struct ClaudeSessionParser {
    pricing_cache: PricingCache,
}

impl ClaudeSessionParser {
    pub fn new() -> Self {
        Self {
            pricing_cache: PricingCache::new(),
        }
    }

    fn parse_file(
        &self,
        path: PathBuf,
        pricing: &std::collections::HashMap<String, crate::pricing::ModelPricing>,
    ) -> Vec<UnifiedMessage> {
        let mut messages = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read {:?}: {}", path, e);
                return messages;
            }
        };

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<ClaudeEntry>(line) {
                Ok(entry) => {
                    if entry.entry_type != "assistant" {
                        continue;
                    }

                    let message = match entry.message {
                        Some(ref m) => m,
                        None => continue,
                    };

                    let message_id = message.id.clone();
                    let request_id = entry.request_id.clone().unwrap_or_default();
                    let dedup_key = format!("{}:{}", message_id, request_id);

                    if seen_ids.contains(&dedup_key) {
                        continue;
                    }
                    seen_ids.insert(dedup_key);

                    let model_id = message.model.clone();
                    let usage = match &message.usage {
                        Some(u) => u,
                        None => continue,
                    };

                    let session_id = entry.session_id.clone().unwrap_or_else(|| {
                        path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string()
                    });

                    // Parse ISO timestamp
                    let timestamp = entry
                        .timestamp
                        .map(|ts| {
                            chrono::DateTime::parse_from_rfc3339(&ts)
                                .map(|dt| dt.timestamp_millis())
                                .unwrap_or_else(|_| chrono::Utc::now().timestamp_millis())
                        })
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let date = chrono::DateTime::from_timestamp_millis(timestamp)
                        .map(|dt| dt.format("%Y-%m-%d").to_string())
                        .unwrap_or_else(|| "unknown".to_string());

                    let tokens = TokenBreakdown {
                        input: usage.input_tokens.unwrap_or(0),
                        output: usage.output_tokens.unwrap_or(0),
                        cache_read: usage.cache_read_input_tokens.unwrap_or(0),
                        cache_write: usage.cache_creation_input_tokens.unwrap_or(0),
                        reasoning: 0,
                    };

                    let cost = match lookup_model_pricing(&model_id, pricing) {
                        Some(p) => calculate_cost(&tokens, p),
                        None => {
                            debug!("No pricing found for model: {}", model_id);
                            0.0
                        }
                    };

                    let msg = UnifiedMessage {
                        client: "claude".to_string(),
                        model_id,
                        provider_id: "anthropic".to_string(),
                        session_id,
                        timestamp,
                        date,
                        tokens,
                        cost,
                    };

                    messages.push(msg);
                }
                Err(e) => {
                    debug!("Failed to parse line: {}", e);
                }
            }
        }

        messages
    }
}

impl Default for ClaudeSessionParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionParser for ClaudeSessionParser {
    fn provider_name(&self) -> &str {
        "claude"
    }

    fn session_paths(&self) -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        vec![home.join(".claude").join("projects")]
    }

    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        let pricing = self.pricing_cache.get_pricing_sync()?;

        let mut all_messages = Vec::new();

        for root in self.session_paths() {
            if !root.exists() {
                continue;
            }

            let files = scanner::discover_files(&root, "jsonl", since);
            debug!("Found {} files for Claude", files.len());

            for file in files {
                let msgs = self.parse_file(file, &pricing);
                all_messages.extend(msgs);
            }
        }

        all_messages.sort_by_key(|m| m.timestamp);
        Ok(all_messages)
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeEntry {
    #[serde(rename = "type")]
    entry_type: String,
    message: Option<ClaudeMessage>,
    request_id: Option<String>,
    session_id: Option<String>,
    timestamp: Option<String>, // ISO format
}

#[derive(Debug, Deserialize)]
struct ClaudeMessage {
    id: String,
    model: String,
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
}
