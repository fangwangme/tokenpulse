use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};
use crate::usage::scanner;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;
use tracing::debug;

const PARSER_VERSION: &str = "claude-v2";

pub struct ClaudeSessionParser;

impl ClaudeSessionParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_file(&self, path: PathBuf, seen_keys: &mut HashSet<String>) -> Vec<UnifiedMessage> {
        let mut messages = Vec::new();
        let session_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string();

        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => return messages,
        };

        for (line_index, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let entry: ClaudeEntry = match serde_json::from_str(trimmed) {
                Ok(entry) => entry,
                Err(error) => {
                    debug!("Failed to parse Claude entry in {:?}: {}", path, error);
                    continue;
                }
            };

            if entry.entry_type != "assistant" {
                continue;
            }

            let message = match entry.message {
                Some(message) => message,
                None => continue,
            };
            let usage = match message.usage {
                Some(usage) => usage,
                None => continue,
            };
            let model_id = match message.model {
                Some(model_id) => model_id,
                None => continue,
            };

            let timestamp = entry
                .timestamp
                .as_deref()
                .and_then(parse_rfc3339_ms)
                .unwrap_or_default();

            let message_key = match (message.id.as_deref(), entry.request_id.as_deref()) {
                (Some(message_id), Some(request_id)) if !request_id.is_empty() => {
                    format!("{message_id}:{request_id}")
                }
                (Some(message_id), _) => message_id.to_string(),
                _ => format!(
                    "{}:{}:{}:{}:{}:{}:{}",
                    session_id,
                    timestamp,
                    model_id,
                    line_index,
                    usage.input_tokens.unwrap_or(0),
                    usage.output_tokens.unwrap_or(0),
                    usage.cache_read_input_tokens.unwrap_or(0)
                ),
            };

            if !seen_keys.insert(message_key.clone()) {
                continue;
            }

            let tokens = TokenBreakdown {
                input: usage.input_tokens.unwrap_or(0).max(0),
                output: usage.output_tokens.unwrap_or(0).max(0),
                cache_read: usage.cache_read_input_tokens.unwrap_or(0).max(0),
                cache_write: usage.cache_creation_input_tokens.unwrap_or(0).max(0),
                reasoning: 0,
            };

            messages.push(
                UnifiedMessage::new(
                    "claude",
                    model_id,
                    "anthropic",
                    entry.session_id.unwrap_or_else(|| session_id.clone()),
                    message_key,
                    timestamp,
                    tokens,
                )
                .with_parser_version(PARSER_VERSION),
            );
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
        vec![
            home.join(".claude").join("projects"),
            home.join(".claude").join("transcripts"),
        ]
    }

    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        let mut all_messages = Vec::new();
        let mut seen_keys = HashSet::new();
        for root in self.session_paths() {
            if !root.exists() {
                continue;
            }
            let files = scanner::discover_files(&root, "jsonl", since);
            for file in files {
                all_messages.extend(self.parse_file(file, &mut seen_keys));
            }
        }
        all_messages.sort_by_key(|message| message.timestamp);
        Ok(all_messages)
    }

    fn parser_version(&self) -> &str {
        PARSER_VERSION
    }
}

fn parse_rfc3339_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

#[derive(Debug, Deserialize)]
struct ClaudeEntry {
    #[serde(rename = "type")]
    entry_type: String,
    #[serde(rename = "requestId", alias = "request_id")]
    request_id: Option<String>,
    session_id: Option<String>,
    timestamp: Option<String>,
    message: Option<ClaudeMessage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
}
