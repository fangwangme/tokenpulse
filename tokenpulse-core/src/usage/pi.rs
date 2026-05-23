use crate::provider::{
    local_date_string_from_timestamp, IncrementalIngestMode, SessionParser, TokenBreakdown,
    UnifiedMessage,
};
use crate::usage::scanner;
use crate::usage::utils::detect_provider_from_model;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Deserialize;
use std::path::PathBuf;
use tracing::{debug, warn};

const PARSER_VERSION: &str = "pi-v3";

pub struct PiSessionParser {}

impl PiSessionParser {
    pub fn new() -> Self {
        Self {}
    }

    fn parse_file(&self, path: PathBuf) -> Vec<UnifiedMessage> {
        let mut messages = Vec::new();
        let mut current_session: Option<String> = None;
        let mut current_model: Option<String> = None;
        let mut current_provider: Option<String> = None;

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
                    "session" | "header" => {
                        current_session =
                            entry.id.clone().or(entry.session_id.clone()).or_else(|| {
                                path.file_stem()
                                    .and_then(|stem| stem.to_str())
                                    .map(ToOwned::to_owned)
                            });
                        if entry.model_id.is_some() {
                            current_model = entry.model_id.clone();
                        } else if entry.model.is_some() {
                            current_model = entry.model.clone();
                        }
                        if entry.provider.is_some() {
                            current_provider = entry.provider.clone();
                        }
                    }
                    "model_change" => {
                        if entry.model_id.is_some() {
                            current_model = entry.model_id.clone();
                        } else if entry.model.is_some() {
                            current_model = entry.model.clone();
                        }
                        if entry.provider.is_some() {
                            current_provider = entry.provider.clone();
                        }
                    }
                    "message" | "assistant" => {
                        let Some(ref message) = entry.message else {
                            continue;
                        };
                        if message.role.as_deref() != Some("assistant") {
                            continue;
                        }
                        let Some(ref usage) = message.usage else {
                            continue;
                        };
                        let tokens = TokenBreakdown {
                            input: usage
                                .input
                                .unwrap_or_else(|| usage.input_tokens.unwrap_or(0))
                                .max(0),
                            output: usage
                                .output
                                .unwrap_or_else(|| usage.output_tokens.unwrap_or(0))
                                .max(0),
                            cache_read: usage.cache_read.unwrap_or(0).max(0),
                            cache_write: usage.cache_write.unwrap_or(0).max(0),
                            reasoning: 0,
                        };
                        if tokens.is_empty() {
                            continue;
                        }

                        let model = message
                            .model
                            .clone()
                            .or_else(|| current_model.clone())
                            .unwrap_or_else(|| "unknown".to_string());
                        let provider_id = message
                            .provider
                            .clone()
                            .or_else(|| current_provider.clone())
                            .unwrap_or_else(|| detect_provider_from_model(&model));
                        let session_id = current_session
                            .clone()
                            .or(entry.id.clone())
                            .unwrap_or_else(|| {
                                path.file_stem()
                                    .and_then(|stem| stem.to_str())
                                    .unwrap_or("unknown")
                                    .to_string()
                            });
                        let timestamp = entry
                            .timestamp_ms()
                            .or(message.timestamp)
                            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
                        let date = local_date_string_from_timestamp(timestamp);
                        let message_key = message
                            .response_id
                            .clone()
                            .or(entry.id.clone())
                            .unwrap_or_else(|| format!("{}:{}:{}", session_id, timestamp, model));

                        let msg = UnifiedMessage::new(
                            "pi",
                            model,
                            provider_id,
                            session_id,
                            message_key,
                            timestamp,
                            tokens,
                        )
                        .with_pricing_day(date)
                        .with_parser_version(PARSER_VERSION);

                        messages.push(msg);
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

            all_messages.extend(scanner::parse_files_parallel(files, |file| {
                self.parse_file(file)
            }));
        }

        all_messages.sort_by_key(|m| m.timestamp);
        Ok(all_messages)
    }

    fn parser_version(&self) -> &str {
        PARSER_VERSION
    }

    fn incremental_ingest_mode(&self) -> IncrementalIngestMode {
        IncrementalIngestMode::ReplaceChangedSessions
    }
}

#[derive(Debug, Deserialize)]
struct PiEntry {
    #[serde(rename = "type")]
    entry_type: String,
    id: Option<String>,
    session_id: Option<String>,
    model: Option<String>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    provider: Option<String>,
    message: Option<PiMessage>,
    timestamp: Option<PiTimestamp>,
}

impl PiEntry {
    fn timestamp_ms(&self) -> Option<i64> {
        self.timestamp.as_ref().and_then(PiTimestamp::to_millis)
    }
}

#[derive(Debug, Deserialize)]
struct PiMessage {
    role: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    usage: Option<PiUsage>,
    timestamp: Option<i64>,
    #[serde(rename = "responseId")]
    response_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PiUsage {
    input: Option<i64>,
    output: Option<i64>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    #[serde(rename = "cacheRead")]
    cache_read: Option<i64>,
    #[serde(rename = "cacheWrite")]
    cache_write: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PiTimestamp {
    String(String),
    Number(i64),
}

impl PiTimestamp {
    fn to_millis(&self) -> Option<i64> {
        match self {
            Self::String(value) => chrono::DateTime::parse_from_rfc3339(value)
                .ok()
                .map(|dt| dt.timestamp_millis()),
            Self::Number(value) => Some(*value),
        }
    }
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
            r#"{"type":"session","id":"s1"}
{"type":"model_change","provider":"nvidia-nim","modelId":"gpt-4.1"}
{"type":"message","id":"msg-1","timestamp":"2026-05-12T08:50:30.109Z","message":{"role":"assistant","provider":"nvidia-nim","model":"gpt-4.1","usage":{"input":10,"output":20,"cacheRead":0,"cacheWrite":0},"timestamp":1778575830241}}"#,
        )
        .unwrap();

        let messages = parser.parse_file(path);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].provider_id, "nvidia-nim");
        assert_eq!(messages[0].tokens.input, 10);
        assert_eq!(messages[0].tokens.output, 20);
    }

    #[test]
    fn pi_skips_zero_token_error_messages() {
        let parser = PiSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"session","id":"s1"}
{"type":"model_change","provider":"nvidia-nim","modelId":"deepseek-ai/deepseek-v4-pro"}
{"type":"message","id":"msg-1","timestamp":"2026-05-12T08:50:30.109Z","message":{"role":"assistant","provider":"nvidia-nim","model":"deepseek-ai/deepseek-v4-pro","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"timestamp":1778575830241,"errorMessage":"429"}}"#,
        )
        .unwrap();

        let messages = parser.parse_file(path);

        assert!(messages.is_empty());
    }
}
