use crate::provider::{
    local_date_string_from_timestamp, SessionParser, TokenBreakdown, UnifiedMessage,
};
use crate::usage::scanner;
use crate::usage::utils::detect_provider_from_model;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Deserialize;
use std::path::PathBuf;
use tracing::{debug, warn};

pub struct PiSessionParser {}

impl PiSessionParser {
    pub fn new() -> Self {
        Self {}
    }

    fn parse_file(&self, path: PathBuf) -> Vec<UnifiedMessage> {
        let mut messages = Vec::new();
        let mut current_session: Option<String> = None;
        let mut current_model: Option<String> = None;

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

            match serde_json::from_str::<PiEntry>(line) {
                Ok(entry) => match entry.entry_type.as_str() {
                    "header" => {
                        current_session = entry.session_id;
                        current_model = entry.model;
                    }
                    "message" | "assistant" => {
                        if let (Some(session_id), Some(model)) = (&current_session, &current_model)
                        {
                            if let Some(usage) = entry.usage {
                                let timestamp = entry
                                    .timestamp
                                    .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
                                let date = local_date_string_from_timestamp(timestamp);

                                let tokens = TokenBreakdown {
                                    input: usage.input_tokens.unwrap_or(0),
                                    output: usage.output_tokens.unwrap_or(0),
                                    cache_read: usage.cache_read.unwrap_or(0),
                                    cache_write: usage.cache_write.unwrap_or(0),
                                    reasoning: 0,
                                };

                                let msg = UnifiedMessage::new(
                                    "pi",
                                    model.clone(),
                                    detect_provider_from_model(model),
                                    session_id.clone(),
                                    format!("{}:{}:{}", session_id, timestamp, model),
                                    timestamp,
                                    tokens,
                                )
                                .with_cost(0.0)
                                .with_pricing_day(date)
                                .with_parser_version("pi-v2");

                                messages.push(msg);
                            }
                        }
                    }
                    _ => {}
                },
                Err(e) => {
                    debug!("Failed to parse line: {}", e);
                }
            }
        }

        messages
    }
}

impl Default for PiSessionParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionParser for PiSessionParser {
    fn provider_name(&self) -> &str {
        "pi"
    }

    fn session_paths(&self) -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        vec![home.join(".pi").join("agent").join("sessions")]
    }

    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        let mut all_messages = Vec::new();

        for root in self.session_paths() {
            if !root.exists() {
                continue;
            }

            let files = scanner::discover_files(&root, "jsonl", since);
            debug!("Found {} files for PI", files.len());

            for file in files {
                let msgs = self.parse_file(file);
                all_messages.extend(msgs);
            }
        }

        all_messages.sort_by_key(|m| m.timestamp);
        Ok(all_messages)
    }
}

#[derive(Debug, Deserialize)]
struct PiEntry {
    #[serde(rename = "type")]
    entry_type: String,
    session_id: Option<String>,
    model: Option<String>,
    usage: Option<PiUsage>,
    timestamp: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct PiUsage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_uses_model_based_provider_detection() {
        let parser = PiSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"header","session_id":"s1","model":"gpt-4.1"}
{"type":"assistant","timestamp":1710000000000,"usage":{"input_tokens":10,"output_tokens":20}}"#,
        )
        .unwrap();

        let messages = parser.parse_file(path);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].provider_id, "openai");
    }
}
