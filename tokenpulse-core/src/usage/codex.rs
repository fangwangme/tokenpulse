use crate::pricing::{calculate_cost, lookup_model_pricing, PricingCache};
use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};
use crate::usage::scanner;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Deserialize;
use std::path::PathBuf;
use tracing::{debug, warn};

pub struct CodexSessionParser {
    pricing_cache: PricingCache,
}

impl CodexSessionParser {
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
        let mut current_model: Option<String> = None;
        let mut accumulated_tokens = CodexTokens::default();
        let mut session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read {:?}: {}", path, e);
                return messages;
            }
        };

        let mut last_timestamp: i64 = 0;

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<CodexEntry>(line) {
                Ok(entry) => match entry.entry_type.as_str() {
                    "model" => {
                        if let Some(model) = entry.model {
                            current_model = Some(model);
                        }
                    }
                    "tokens" => {
                        if let Some(tokens) = entry.tokens {
                            accumulated_tokens.input += tokens.input.unwrap_or(0);
                            accumulated_tokens.output += tokens.output.unwrap_or(0);
                            accumulated_tokens.cache_read += tokens.cache_read.unwrap_or(0);
                        }
                    }
                    "session" => {
                        if let Some(id) = entry.session_id {
                            session_id = id;
                        }
                    }
                    "message" | "response" => {
                        if let Some(ts) = entry.timestamp {
                            last_timestamp = ts;
                        }
                    }
                    "end" | "complete" => {
                        if let Some(model) = current_model.take() {
                            let date = chrono::DateTime::from_timestamp_millis(last_timestamp)
                                .map(|dt| dt.format("%Y-%m-%d").to_string())
                                .unwrap_or_else(|| "unknown".to_string());

                            let tokens = TokenBreakdown {
                                input: accumulated_tokens.input,
                                output: accumulated_tokens.output,
                                cache_read: accumulated_tokens.cache_read,
                                cache_write: 0,
                                reasoning: 0,
                            };

                            let cost = match lookup_model_pricing(&model, pricing) {
                                Some(p) => calculate_cost(&tokens, p),
                                None => 0.0,
                            };

                            let msg = UnifiedMessage {
                                client: "codex".to_string(),
                                model_id: model,
                                provider_id: "openai".to_string(),
                                session_id: session_id.clone(),
                                timestamp: last_timestamp,
                                date,
                                tokens,
                                cost,
                            };

                            messages.push(msg);
                            accumulated_tokens = CodexTokens::default();
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    debug!("Failed to parse line: {}", e);
                }
            }
        }

        if current_model.is_some()
            && (accumulated_tokens.input > 0 || accumulated_tokens.output > 0)
        {
            let model = current_model.unwrap();
            let date = chrono::DateTime::from_timestamp_millis(last_timestamp)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "unknown".to_string());

            let tokens = TokenBreakdown {
                input: accumulated_tokens.input,
                output: accumulated_tokens.output,
                cache_read: accumulated_tokens.cache_read,
                cache_write: 0,
                reasoning: 0,
            };

            let cost = match lookup_model_pricing(&model, pricing) {
                Some(p) => calculate_cost(&tokens, p),
                None => 0.0,
            };

            let msg = UnifiedMessage {
                client: "codex".to_string(),
                model_id: model,
                provider_id: "openai".to_string(),
                session_id,
                timestamp: last_timestamp,
                date,
                tokens,
                cost,
            };

            messages.push(msg);
        }

        messages
    }
}

impl Default for CodexSessionParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionParser for CodexSessionParser {
    fn provider_name(&self) -> &str {
        "codex"
    }

    fn session_paths(&self) -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        vec![
            home.join(".codex").join("sessions"),
            std::env::var("CODEX_HOME")
                .map(PathBuf::from)
                .map(|p| p.join("sessions"))
                .unwrap_or_else(|_| home.join(".codex").join("sessions")),
        ]
    }

    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        let pricing = tokio::runtime::Handle::try_current()
            .map(|handle| handle.block_on(self.pricing_cache.get_pricing()))
            .unwrap_or_else(|_| {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(self.pricing_cache.get_pricing())
            })?;

        let mut all_messages = Vec::new();

        for root in self.session_paths() {
            if !root.exists() {
                continue;
            }

            let files = scanner::discover_files(&root, "jsonl", since);
            debug!("Found {} files for Codex", files.len());

            for file in files {
                let msgs = self.parse_file(file, &pricing);
                all_messages.extend(msgs);
            }
        }

        all_messages.sort_by_key(|m| m.timestamp);
        Ok(all_messages)
    }
}

#[derive(Debug, Deserialize, Default)]
struct CodexEntry {
    #[serde(rename = "type")]
    entry_type: String,
    model: Option<String>,
    tokens: Option<CodexTokensRaw>,
    session_id: Option<String>,
    timestamp: Option<i64>,
}

#[derive(Debug, Default)]
struct CodexTokens {
    input: i64,
    output: i64,
    cache_read: i64,
}

#[derive(Debug, Deserialize)]
struct CodexTokensRaw {
    input: Option<i64>,
    output: Option<i64>,
    cache_read: Option<i64>,
}
