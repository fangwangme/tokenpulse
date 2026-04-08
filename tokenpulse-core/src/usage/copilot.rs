use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};
use crate::usage::scanner;

use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use tracing::debug;

const PARSER_VERSION: &str = "copilot-v1";
const EVENT_NAME: &str = "gen_ai.client.inference.operation.details";

pub struct CopilotSessionParser;

impl CopilotSessionParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_file(&self, path: &PathBuf) -> Vec<UnifiedMessage> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return Vec::new(),
        };

        let mut messages = Vec::new();
        let mut seen_response_ids = HashSet::new();
        // Cache estimation: track previous input per (session_id, model)
        let mut prev_input: HashMap<(String, String), i64> = HashMap::new();

        for line in BufReader::new(file).lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => continue,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let value: Value = match serde_json::from_str(trimmed) {
                Ok(value) => value,
                Err(error) => {
                    debug!("Failed to parse Copilot OTEL line in {:?}: {}", path, error);
                    continue;
                }
            };

            let attrs = match value.get("attributes") {
                Some(attrs) => attrs,
                None => continue,
            };

            let event_name = attrs
                .get("event.name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if event_name != EVENT_NAME {
                continue;
            }

            let input_tokens = attrs
                .get("gen_ai.usage.input_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let output_tokens = attrs
                .get("gen_ai.usage.output_tokens")
                .and_then(Value::as_i64)
                .unwrap_or(0);

            if input_tokens == 0 && output_tokens == 0 {
                continue;
            }

            // Deduplicate by response_id
            let response_id = attrs
                .get("gen_ai.response.id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !response_id.is_empty() && !seen_response_ids.insert(response_id.to_string()) {
                continue;
            }

            let model_id = attrs
                .get("gen_ai.response.model")
                .or_else(|| attrs.get("gen_ai.request.model"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();

            let session_id = extract_session_id(&value).unwrap_or_else(|| "unknown".to_string());

            let timestamp = extract_hr_time(&value).unwrap_or(0);

            let provider_id = detect_provider(&model_id);

            // Cache estimation within a session using the same model
            let cache_key = (session_id.clone(), model_id.clone());
            let mut cache_read = 0i64;
            let mut effective_input = input_tokens;

            if let Some(&prev) = prev_input.get(&cache_key) {
                if input_tokens >= prev {
                    cache_read = prev;
                    effective_input = input_tokens - prev;
                }
            }
            prev_input.insert(cache_key, input_tokens);

            let tokens = TokenBreakdown {
                input: effective_input.max(0),
                output: output_tokens.max(0),
                cache_read: cache_read.max(0),
                cache_write: 0,
                reasoning: 0,
            };

            let message_key = if !response_id.is_empty() {
                format!("copilot:{}", response_id)
            } else {
                format!(
                    "copilot:{}:{}:{}:{}:{}",
                    session_id, timestamp, model_id, input_tokens, output_tokens
                )
            };

            messages.push(
                UnifiedMessage::new(
                    "copilot",
                    model_id,
                    provider_id,
                    session_id,
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

impl Default for CopilotSessionParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionParser for CopilotSessionParser {
    fn provider_name(&self) -> &str {
        "copilot"
    }

    fn session_paths(&self) -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        let data_dir = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".local").join("share"));
        vec![data_dir.join("github-copilot")]
    }

    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        let mut all_messages = Vec::new();
        for root in self.session_paths() {
            if !root.exists() {
                continue;
            }
            let files = scanner::discover_files(&root, "jsonl", since);
            for file in files {
                all_messages.extend(self.parse_file(&file));
            }
        }
        all_messages.sort_by_key(|message| message.timestamp);
        Ok(all_messages)
    }

    fn parser_version(&self) -> &str {
        PARSER_VERSION
    }
}

fn extract_session_id(value: &Value) -> Option<String> {
    let raw_attrs = value.get("resource")?.get("_rawAttributes")?.as_array()?;
    for entry in raw_attrs {
        let arr = entry.as_array()?;
        if arr.len() >= 2 && arr[0].as_str() == Some("session.id") {
            return arr[1]
                .get("value")
                .and_then(Value::as_str)
                .or_else(|| arr[1].as_str())
                .map(String::from);
        }
    }
    None
}

fn extract_hr_time(value: &Value) -> Option<i64> {
    let hr_time = value.get("hrTime")?.as_array()?;
    if hr_time.len() >= 2 {
        let seconds = hr_time[0].as_i64()?;
        let nanos = hr_time[1].as_i64().unwrap_or(0);
        Some(seconds * 1000 + nanos / 1_000_000)
    } else {
        None
    }
}

fn detect_provider(model: &str) -> String {
    let lower = model.to_lowercase();
    if lower.starts_with("gpt")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
    {
        "openai".to_string()
    } else if lower.starts_with("claude") {
        "anthropic".to_string()
    } else if lower.starts_with("gemini") {
        "google".to_string()
    } else {
        "unknown".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_provider_openai_models() {
        assert_eq!(detect_provider("gpt-4o-mini-2024-07-18"), "openai");
        assert_eq!(detect_provider("gpt-4o"), "openai");
        assert_eq!(detect_provider("o1-preview"), "openai");
        assert_eq!(detect_provider("o3-mini"), "openai");
        assert_eq!(detect_provider("o4-mini"), "openai");
    }

    #[test]
    fn detect_provider_anthropic_models() {
        assert_eq!(detect_provider("claude-3.5-sonnet"), "anthropic");
        assert_eq!(detect_provider("claude-sonnet-4-20250514"), "anthropic");
    }

    #[test]
    fn detect_provider_google_models() {
        assert_eq!(detect_provider("gemini-2.0-flash"), "google");
    }

    #[test]
    fn detect_provider_unknown() {
        assert_eq!(detect_provider("some-model"), "unknown");
    }

    #[test]
    fn extract_hr_time_parses_correctly() {
        let value: Value = serde_json::from_str(r#"{"hrTime": [1700000000, 500000000]}"#).unwrap();
        assert_eq!(extract_hr_time(&value), Some(1700000000500));
    }

    #[test]
    fn extract_hr_time_missing() {
        let value: Value = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(extract_hr_time(&value), None);
    }

    #[test]
    fn extract_session_id_from_raw_attributes() {
        let value: Value = serde_json::from_str(
            r#"{
                "resource": {
                    "_rawAttributes": [
                        ["session.id", {"value": "abc-123"}]
                    ]
                }
            }"#,
        )
        .unwrap();
        assert_eq!(extract_session_id(&value), Some("abc-123".to_string()));
    }

    #[test]
    fn extract_session_id_missing() {
        let value: Value = serde_json::from_str(r#"{"resource": {}}"#).unwrap();
        assert_eq!(extract_session_id(&value), None);
    }

    #[test]
    fn parse_otel_event_line() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("events.jsonl");

        let event = serde_json::json!({
            "hrTime": [1700000000, 123456789],
            "resource": {
                "_rawAttributes": [
                    ["session.id", {"value": "sess-1"}]
                ]
            },
            "attributes": {
                "event.name": "gen_ai.client.inference.operation.details",
                "gen_ai.response.model": "gpt-4o-mini-2024-07-18",
                "gen_ai.request.model": "gpt-4o-mini",
                "gen_ai.usage.input_tokens": 1500,
                "gen_ai.usage.output_tokens": 200,
                "gen_ai.response.id": "chatcmpl-abc123"
            }
        });

        std::fs::write(&file, format!("{}\n", event)).unwrap();
        let messages = parser.parse_file(&file);

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.client, "copilot");
        assert_eq!(msg.model_id, "gpt-4o-mini-2024-07-18");
        assert_eq!(msg.provider_id, "openai");
        assert_eq!(msg.session_id, "sess-1");
        assert_eq!(msg.tokens.input, 1500);
        assert_eq!(msg.tokens.output, 200);
        assert_eq!(msg.tokens.cache_read, 0);
        assert_eq!(msg.message_key, "copilot:chatcmpl-abc123");
    }

    #[test]
    fn skips_non_matching_events() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("events.jsonl");

        let event = serde_json::json!({
            "hrTime": [1700000000, 0],
            "attributes": {
                "event.name": "some.other.event",
                "gen_ai.usage.input_tokens": 100,
                "gen_ai.usage.output_tokens": 50
            }
        });

        std::fs::write(&file, format!("{}\n", event)).unwrap();
        let messages = parser.parse_file(&file);
        assert!(messages.is_empty());
    }

    #[test]
    fn skips_zero_token_events() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("events.jsonl");

        let event = serde_json::json!({
            "hrTime": [1700000000, 0],
            "attributes": {
                "event.name": "gen_ai.client.inference.operation.details",
                "gen_ai.usage.input_tokens": 0,
                "gen_ai.usage.output_tokens": 0,
                "gen_ai.response.id": "resp-zero"
            }
        });

        std::fs::write(&file, format!("{}\n", event)).unwrap();
        let messages = parser.parse_file(&file);
        assert!(messages.is_empty());
    }

    #[test]
    fn deduplicates_by_response_id() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("events.jsonl");

        let event = serde_json::json!({
            "hrTime": [1700000000, 0],
            "resource": { "_rawAttributes": [["session.id", {"value": "s1"}]] },
            "attributes": {
                "event.name": "gen_ai.client.inference.operation.details",
                "gen_ai.response.model": "gpt-4o",
                "gen_ai.usage.input_tokens": 100,
                "gen_ai.usage.output_tokens": 50,
                "gen_ai.response.id": "dup-id"
            }
        });

        let content = format!("{}\n{}\n", event, event);
        std::fs::write(&file, content).unwrap();
        let messages = parser.parse_file(&file);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn cache_estimation_monotonic_input() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("events.jsonl");

        let event1 = serde_json::json!({
            "hrTime": [1700000000, 0],
            "resource": { "_rawAttributes": [["session.id", {"value": "s1"}]] },
            "attributes": {
                "event.name": "gen_ai.client.inference.operation.details",
                "gen_ai.response.model": "gpt-4o",
                "gen_ai.usage.input_tokens": 1000,
                "gen_ai.usage.output_tokens": 200,
                "gen_ai.response.id": "r1"
            }
        });
        let event2 = serde_json::json!({
            "hrTime": [1700000001, 0],
            "resource": { "_rawAttributes": [["session.id", {"value": "s1"}]] },
            "attributes": {
                "event.name": "gen_ai.client.inference.operation.details",
                "gen_ai.response.model": "gpt-4o",
                "gen_ai.usage.input_tokens": 1500,
                "gen_ai.usage.output_tokens": 300,
                "gen_ai.response.id": "r2"
            }
        });

        let content = format!("{}\n{}\n", event1, event2);
        std::fs::write(&file, content).unwrap();
        let messages = parser.parse_file(&file);

        assert_eq!(messages.len(), 2);
        // First message: no previous, so no cache
        assert_eq!(messages[0].tokens.input, 1000);
        assert_eq!(messages[0].tokens.cache_read, 0);
        // Second message: input grew from 1000 to 1500, so 1000 cached
        assert_eq!(messages[1].tokens.input, 500);
        assert_eq!(messages[1].tokens.cache_read, 1000);
    }

    #[test]
    fn cache_estimation_resets_on_smaller_input() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("events.jsonl");

        let event1 = serde_json::json!({
            "hrTime": [1700000000, 0],
            "resource": { "_rawAttributes": [["session.id", {"value": "s1"}]] },
            "attributes": {
                "event.name": "gen_ai.client.inference.operation.details",
                "gen_ai.response.model": "gpt-4o",
                "gen_ai.usage.input_tokens": 2000,
                "gen_ai.usage.output_tokens": 100,
                "gen_ai.response.id": "r1"
            }
        });
        let event2 = serde_json::json!({
            "hrTime": [1700000001, 0],
            "resource": { "_rawAttributes": [["session.id", {"value": "s1"}]] },
            "attributes": {
                "event.name": "gen_ai.client.inference.operation.details",
                "gen_ai.response.model": "gpt-4o",
                "gen_ai.usage.input_tokens": 500,
                "gen_ai.usage.output_tokens": 50,
                "gen_ai.response.id": "r2"
            }
        });

        let content = format!("{}\n{}\n", event1, event2);
        std::fs::write(&file, content).unwrap();
        let messages = parser.parse_file(&file);

        assert_eq!(messages.len(), 2);
        // Second message: input < previous, new context, no cache
        assert_eq!(messages[1].tokens.input, 500);
        assert_eq!(messages[1].tokens.cache_read, 0);
    }
}
