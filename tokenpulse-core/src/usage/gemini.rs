use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};
use crate::usage::scanner;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Deserialize;
use std::path::PathBuf;
use tracing::debug;

const PARSER_VERSION: &str = "gemini-v1";

pub struct GeminiSessionParser;

impl GeminiSessionParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_file(&self, path: PathBuf) -> Vec<UnifiedMessage> {
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(_) => return Vec::new(),
        };

        let session: GeminiSession = match serde_json::from_str(&content) {
            Ok(session) => session,
            Err(error) => {
                debug!("Failed to parse Gemini session {:?}: {}", path, error);
                return Vec::new();
            }
        };

        let session_id = session.session_id.clone().unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("unknown")
                .to_string()
        });

        session
            .messages
            .into_iter()
            .enumerate()
            .filter_map(|(index, message)| {
                if message.message_type.as_deref() != Some("gemini") {
                    return None;
                }

                let tokens = message.tokens?;
                let model_id = message.model?;
                let timestamp = message
                    .timestamp
                    .as_deref()
                    .and_then(parse_rfc3339_ms)
                    .or_else(|| session.last_updated.as_deref().and_then(parse_rfc3339_ms))
                    .or_else(|| session.start_time.as_deref().and_then(parse_rfc3339_ms))
                    .unwrap_or_default();

                let message_key = message.id.unwrap_or_else(|| {
                    format!(
                        "{}:{}:{}:{}",
                        session_id,
                        index,
                        model_id,
                        tokens.input.unwrap_or(0)
                    )
                });

                Some(
                    UnifiedMessage::new(
                        "gemini",
                        model_id,
                        "google",
                        session_id.clone(),
                        message_key,
                        timestamp,
                        TokenBreakdown {
                            input: tokens.input.unwrap_or(0).max(0),
                            output: tokens.output.unwrap_or(0).max(0),
                            cache_read: tokens.cached.unwrap_or(0).max(0),
                            cache_write: 0,
                            reasoning: tokens.thoughts.unwrap_or(0).max(0),
                        },
                    )
                    .with_parser_version(PARSER_VERSION),
                )
            })
            .collect()
    }
}

impl Default for GeminiSessionParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionParser for GeminiSessionParser {
    fn provider_name(&self) -> &str {
        "gemini"
    }

    fn session_paths(&self) -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        vec![home.join(".gemini").join("tmp")]
    }

    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        let mut all_messages = Vec::new();
        for root in self.session_paths() {
            if !root.exists() {
                continue;
            }
            let files = scanner::discover_files(&root, "json", since);
            for file in files {
                if file
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with("session-"))
                    .unwrap_or(false)
                {
                    all_messages.extend(self.parse_file(file));
                }
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
struct GeminiSession {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "startTime")]
    start_time: Option<String>,
    #[serde(rename = "lastUpdated")]
    last_updated: Option<String>,
    #[serde(default)]
    messages: Vec<GeminiMessage>,
}

#[derive(Debug, Deserialize)]
struct GeminiMessage {
    id: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "type")]
    message_type: Option<String>,
    model: Option<String>,
    tokens: Option<GeminiTokens>,
}

#[derive(Debug, Deserialize)]
struct GeminiTokens {
    input: Option<i64>,
    output: Option<i64>,
    cached: Option<i64>,
    thoughts: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_file_extracts_gemini_token_breakdown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-gemini.json");
        std::fs::write(
            &path,
            r#"{
                "sessionId":"gem-session",
                "startTime":"2026-04-01T12:00:00Z",
                "lastUpdated":"2026-04-01T12:05:00Z",
                "messages":[
                    {"id":"user-1","type":"user"},
                    {"id":"gem-1","type":"gemini","timestamp":"2026-04-01T12:01:00Z","model":"gemini-2.5-pro","tokens":{"input":120,"output":45,"cached":30,"thoughts":12}}
                ]
            }"#,
        )
        .unwrap();

        let messages = GeminiSessionParser::new().parse_file(path);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].session_id, "gem-session");
        assert_eq!(messages[0].model_id, "gemini-2.5-pro");
        assert_eq!(messages[0].tokens.input, 120);
        assert_eq!(messages[0].tokens.output, 45);
        assert_eq!(messages[0].tokens.cache_read, 30);
        assert_eq!(messages[0].tokens.reasoning, 12);
    }

    #[test]
    fn parse_file_skips_non_gemini_messages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-gemini.json");
        std::fs::write(
            &path,
            r#"{
                "sessionId":"gem-session",
                "messages":[
                    {"id":"user-1","type":"user","model":"gemini-2.5-pro","tokens":{"input":1,"output":1}}
                ]
            }"#,
        )
        .unwrap();

        let messages = GeminiSessionParser::new().parse_file(path);

        assert!(messages.is_empty());
    }
}
