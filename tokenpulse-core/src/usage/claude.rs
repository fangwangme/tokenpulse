use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};
use crate::usage::scanner;
use crate::usage::utils::detect_provider_from_model;

use anyhow::Result;
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::debug;

const PARSER_VERSION: &str = "claude-v4";

pub struct ClaudeSessionParser;

impl ClaudeSessionParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_file(&self, path: PathBuf) -> Vec<UnifiedMessage> {
        let mut messages = Vec::new();
        let mut message_positions = HashMap::new();
        let session_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string();
        let mut latest_route = ClaudeModelRoute::default();

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

            if let Some(route) = ClaudeModelRoute::from_local_command(
                entry.entry_type.as_str(),
                entry.message.as_ref(),
            ) {
                latest_route = route;
            }

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

            // Claude reports uncached input separately from cache read/write tokens.
            // Do not subtract cache fields from input, or cached-heavy requests go to zero.
            let tokens = TokenBreakdown {
                input: usage.input_tokens.unwrap_or(0).max(0),
                output: usage.output_tokens.unwrap_or(0).max(0),
                cache_read: usage.cache_read_input_tokens.unwrap_or(0).max(0),
                cache_write: usage.cache_creation_input_tokens.unwrap_or(0).max(0),
                reasoning: 0,
            };
            if tokens.is_empty() {
                continue;
            }

            let provider_id = latest_route
                .provider_hint(message.provider.as_deref())
                .unwrap_or_else(|| detect_provider_from_model(&model_id));

            upsert_message(
                &mut messages,
                &mut message_positions,
                UnifiedMessage::new(
                    "claude",
                    model_id,
                    provider_id,
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
        let mut message_positions = HashMap::new();
        for root in self.session_paths() {
            if !root.exists() {
                continue;
            }
            let files = scanner::discover_files(&root, "jsonl", since);
            for file in files {
                for message in self.parse_file(file) {
                    upsert_message(&mut all_messages, &mut message_positions, message);
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

fn upsert_message(
    messages: &mut Vec<UnifiedMessage>,
    message_positions: &mut HashMap<String, usize>,
    message: UnifiedMessage,
) {
    let key = message.message_key.clone();
    if let Some(position) = message_positions.get(&key).copied() {
        if should_replace_message(&messages[position], &message) {
            messages[position] = message;
        }
    } else {
        message_positions.insert(key, messages.len());
        messages.push(message);
    }
}

fn should_replace_message(existing: &UnifiedMessage, candidate: &UnifiedMessage) -> bool {
    candidate.timestamp > existing.timestamp
        || (candidate.timestamp == existing.timestamp
            && candidate.total_tokens() >= existing.total_tokens())
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
    provider: Option<String>,
    content: Option<Value>,
    usage: Option<ClaudeUsage>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
}

#[derive(Debug, Clone, Default)]
struct ClaudeModelRoute {
    provider_hint: Option<String>,
}

impl ClaudeModelRoute {
    fn from_local_command(entry_type: &str, message: Option<&ClaudeMessage>) -> Option<Self> {
        if entry_type != "user" {
            return None;
        }

        let message = message?;
        let text = extract_content_text(message.content.as_ref()?)?;

        let route = extract_model_route_from_text(text)?;
        let segments: Vec<&str> = route
            .split('/')
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
            .collect();
        // Standard two-segment routes like "anthropic/claude-sonnet-4" do not
        // carry an extra provider hop. Only multi-hop routes include the
        // backend provider hint we want to preserve for pricing lookup.
        let provider_hint = if segments.len() >= 3 {
            segments
                .get(segments.len() - 3)
                .map(|segment| segment.to_ascii_lowercase().replace('_', "-"))
        } else {
            None
        };

        Some(Self { provider_hint })
    }

    fn provider_hint(&self, message_provider: Option<&str>) -> Option<String> {
        self.provider_hint
            .clone()
            .or_else(|| message_provider.map(|provider| provider.to_ascii_lowercase()))
    }
}

fn extract_content_text(content: &Value) -> Option<&str> {
    match content {
        Value::String(text) => Some(text.as_str()),
        Value::Array(blocks) => blocks.iter().find_map(|block| {
            if block.get("type").and_then(Value::as_str) == Some("text") {
                block.get("text").and_then(Value::as_str)
            } else {
                None
            }
        }),
        _ => None,
    }
}

fn extract_model_route_from_text(text: &str) -> Option<String> {
    let cleaned = strip_ansi_escapes(text);
    let marker = "Set model to ";
    let start = cleaned.find(marker)? + marker.len();
    let mut candidate = cleaned[start..].trim();
    if let Some(end) = candidate.find("</local-command-stdout>") {
        candidate = &candidate[..end];
    }
    if let Some((model, _)) = candidate.split_once(" with ") {
        candidate = model;
    }
    let candidate = candidate.trim();
    if candidate.is_empty() {
        None
    } else {
        Some(candidate.to_string())
    }
}

fn strip_ansi_escapes(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(next) = chars.next() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        cleaned.push(ch);
    }

    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_file_keeps_latest_duplicate_message_and_request_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"assistant","requestId":"req-1","timestamp":"2026-04-01T12:00:00Z","message":{"id":"msg-1","model":"claude-sonnet-4","usage":{"input_tokens":10,"output_tokens":5}}}
{"type":"assistant","requestId":"req-1","timestamp":"2026-04-01T12:00:01Z","message":{"id":"msg-1","model":"claude-sonnet-4","usage":{"input_tokens":99,"output_tokens":99}}}"#,
        )
        .unwrap();

        let messages = ClaudeSessionParser::new().parse_file(path);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_key, "msg-1:req-1");
        assert_eq!(messages[0].tokens.input, 99);
        assert_eq!(messages[0].tokens.output, 99);
    }

    #[test]
    fn parse_file_builds_fallback_key_and_clamps_negative_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"assistant","session_id":"session-42","timestamp":"2026-04-01T12:00:00Z","message":{"model":"claude-opus-4","usage":{"input_tokens":-10,"output_tokens":20,"cache_read_input_tokens":5,"cache_creation_input_tokens":2}}}"#,
        )
        .unwrap();

        let messages = ClaudeSessionParser::new().parse_file(path);

        assert_eq!(messages.len(), 1);
        assert!(messages[0].message_key.starts_with("session:"));
        assert_eq!(messages[0].session_id, "session-42");
        assert_eq!(messages[0].tokens.input, 0);
        assert_eq!(messages[0].tokens.output, 20);
        assert_eq!(messages[0].tokens.cache_read, 5);
        assert_eq!(messages[0].tokens.cache_write, 2);
    }

    #[test]
    fn parse_file_keeps_claude_cache_tokens_separate_from_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"assistant","requestId":"req-1","timestamp":"2026-04-01T12:00:00Z","message":{"id":"msg-1","model":"claude-opus-4-6","usage":{"input_tokens":1,"output_tokens":117,"cache_read_input_tokens":55725,"cache_creation_input_tokens":3911}}}"#,
        )
        .unwrap();

        let messages = ClaudeSessionParser::new().parse_file(path);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tokens.input, 1);
        assert_eq!(messages[0].tokens.output, 117);
        assert_eq!(messages[0].tokens.cache_read, 55_725);
        assert_eq!(messages[0].tokens.cache_write, 3_911);
        assert_eq!(messages[0].tokens.total(), 59_754);
    }

    #[test]
    fn parse_file_uses_local_model_route_for_provider_and_pricing_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"<local-command-stdout>Set model to anthropic/nvidia_nim/deepseek-ai/deepseek-v4-pro</local-command-stdout>\"}]}}\n{\"type\":\"assistant\",\"timestamp\":\"2026-04-01T12:00:00Z\",\"message\":{\"id\":\"msg-1\",\"model\":\"deepseek-ai/deepseek-v4-pro\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5}}}",
        )
        .unwrap();

        let messages = ClaudeSessionParser::new().parse_file(path);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].provider_id, "nvidia-nim");
        assert_eq!(messages[0].model_id, "deepseek-ai/deepseek-v4-pro");
    }

    #[test]
    fn upsert_message_keeps_more_complete_duplicate_with_same_timestamp() {
        let mut messages = Vec::new();
        let mut positions = HashMap::new();

        upsert_message(
            &mut messages,
            &mut positions,
            UnifiedMessage::new(
                "claude",
                "claude-opus-4-1",
                "anthropic",
                "session-1",
                "msg-1:req-1",
                1_743_508_800_000,
                TokenBreakdown {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
            )
            .with_parser_version(PARSER_VERSION),
        );

        upsert_message(
            &mut messages,
            &mut positions,
            UnifiedMessage::new(
                "claude",
                "claude-opus-4-1",
                "anthropic",
                "session-1",
                "msg-1:req-1",
                1_743_508_800_000,
                TokenBreakdown {
                    input: 10,
                    output: 99,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
            )
            .with_parser_version(PARSER_VERSION),
        );

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].tokens.output, 99);
    }

    #[test]
    fn upsert_message_does_not_replace_newer_duplicate_with_older_copy() {
        let mut messages = Vec::new();
        let mut positions = HashMap::new();

        upsert_message(
            &mut messages,
            &mut positions,
            UnifiedMessage::new(
                "claude",
                "claude-opus-4-1",
                "anthropic",
                "session-1",
                "msg-1:req-1",
                1_743_508_800_100,
                TokenBreakdown {
                    input: 10,
                    output: 99,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
            )
            .with_parser_version(PARSER_VERSION),
        );

        upsert_message(
            &mut messages,
            &mut positions,
            UnifiedMessage::new(
                "claude",
                "claude-opus-4-1",
                "anthropic",
                "session-1",
                "msg-1:req-1",
                1_743_508_800_000,
                TokenBreakdown {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                },
            )
            .with_parser_version(PARSER_VERSION),
        );

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].timestamp, 1_743_508_800_100);
        assert_eq!(messages[0].tokens.output, 99);
    }
}
