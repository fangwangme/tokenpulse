use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};
use crate::usage::scanner;
use crate::usage::utils::{detect_provider_from_model, parse_timestamp_str};

use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use tracing::debug;

const PARSER_VERSION: &str = "copilot-v3";
const EVENT_NAME: &str = "gen_ai.client.inference.operation.details";

pub struct CopilotSessionParser;

impl CopilotSessionParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_file(&self, path: &PathBuf) -> Vec<UnifiedMessage> {
        if is_agent_session_file(path) {
            return self.parse_agent_session_file(path);
        }

        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return Vec::new(),
        };

        let mut messages = Vec::new();
        let mut seen_response_ids = HashSet::new();
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

            if let Some(message) = parse_otel_event(&value, &mut seen_response_ids, &mut prev_input)
            {
                messages.push(message);
            }
        }

        messages
    }

    fn parse_agent_session_file(&self, path: &PathBuf) -> Vec<UnifiedMessage> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return Vec::new(),
        };

        let default_session_id = path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();
        let mut current_session_id = default_session_id;
        let mut current_model = "unknown".to_string();
        let mut fallback_messages = Vec::new();
        let mut latest_shutdown = None;

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
                    debug!(
                        "Failed to parse Copilot session-state line in {:?}: {}",
                        path, error
                    );
                    continue;
                }
            };

            if let Some((session_id, model_id)) = extract_agent_session_start(&value) {
                current_session_id = session_id;
                if !is_unknown_model(&model_id) {
                    current_model = model_id;
                }
                continue;
            }

            if let Some(model_id) = extract_agent_model_change(&value) {
                if !is_unknown_model(&model_id) {
                    current_model = model_id;
                }
                continue;
            }

            if let Some(messages) = parse_session_shutdown(&value, &current_session_id) {
                latest_shutdown = Some(messages);
                continue;
            }

            if let Some(message) =
                parse_agent_session_event(&value, &current_session_id, &current_model)
            {
                fallback_messages.push(message);
            }
        }

        latest_shutdown.unwrap_or(fallback_messages)
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
        vec![
            data_dir.join("github-copilot"),
            home.join(".copilot").join("session-state"),
        ]
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

fn extract_agent_session_start(value: &Value) -> Option<(String, String)> {
    if value.get("type")?.as_str()? != "session.start" {
        return None;
    }
    let data = value.get("data")?;
    let session_id = data.get("sessionId")?.as_str()?.to_string();
    let model_id = extract_model_id(data).unwrap_or_else(|| "unknown".to_string());
    Some((session_id, model_id))
}

fn extract_agent_model_change(value: &Value) -> Option<String> {
    if value.get("type")?.as_str()? != "session.model_change" {
        return None;
    }
    extract_model_id(value.get("data")?)
}

fn extract_model_id(data: &Value) -> Option<String> {
    data.get("model")
        .or_else(|| data.get("selectedModel"))
        .or_else(|| data.get("newModel"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned)
}

fn is_unknown_model(model_id: &str) -> bool {
    model_id.trim().is_empty() || model_id.trim().eq_ignore_ascii_case("unknown")
}

fn parse_session_shutdown(value: &Value, session_id: &str) -> Option<Vec<UnifiedMessage>> {
    if value.get("type")?.as_str()? != "session.shutdown" {
        return None;
    }

    let data = value.get("data")?;
    let timestamp = parse_timestamp_str(value.get("timestamp")?.as_str()?)?;
    let model_metrics = data.get("modelMetrics")?.as_object()?;
    let mut messages = Vec::new();

    for (model_id, metric) in model_metrics {
        let requests = metric.get("requests")?;
        let usage = metric.get("usage")?;
        let request_count = requests
            .get("count")
            .and_then(Value::as_u64)
            .map(|count| count as usize)
            .unwrap_or(0)
            .max(1);
        let total_input_tokens = usage
            .get("inputTokens")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
        let output_tokens = usage
            .get("outputTokens")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
        let cache_read_tokens = usage
            .get("cacheReadTokens")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);
        let cache_write_tokens = usage
            .get("cacheWriteTokens")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);

        if total_input_tokens == 0
            && output_tokens == 0
            && cache_read_tokens == 0
            && cache_write_tokens == 0
        {
            continue;
        }

        let effective_input_tokens =
            split_total_input_tokens(total_input_tokens, cache_read_tokens, cache_write_tokens);

        let provider_id = detect_provider_from_model(model_id);
        for idx in 0..request_count {
            messages.push(
                UnifiedMessage::new(
                    "copilot",
                    model_id.clone(),
                    provider_id.clone(),
                    session_id.to_string(),
                    format!("copilot-session:{}:{}:{}", session_id, model_id, idx),
                    timestamp + idx as i64,
                    TokenBreakdown {
                        input: distribute_i64(effective_input_tokens, request_count, idx),
                        output: distribute_i64(output_tokens, request_count, idx),
                        cache_read: distribute_i64(cache_read_tokens, request_count, idx),
                        cache_write: distribute_i64(cache_write_tokens, request_count, idx),
                        reasoning: 0,
                    },
                )
                .with_parser_version(PARSER_VERSION),
            );
        }
    }

    if messages.is_empty() {
        None
    } else {
        Some(messages)
    }
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

fn parse_otel_event(
    value: &Value,
    seen_response_ids: &mut HashSet<String>,
    prev_input: &mut HashMap<(String, String), i64>,
) -> Option<UnifiedMessage> {
    let attrs = value.get("attributes")?;

    let event_name = attrs
        .get("event.name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if event_name != EVENT_NAME {
        return None;
    }

    let total_input_tokens = attrs
        .get("gen_ai.usage.input_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);
    let output_tokens = attrs
        .get("gen_ai.usage.output_tokens")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        .max(0);

    let response_id = attrs
        .get("gen_ai.response.id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !response_id.is_empty() && !seen_response_ids.insert(response_id.to_string()) {
        return None;
    }

    let model_id = attrs
        .get("gen_ai.response.model")
        .or_else(|| attrs.get("gen_ai.request.model"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let session_id = extract_session_id(value).unwrap_or_else(|| "unknown".to_string());
    let timestamp = extract_hr_time(value).unwrap_or(0);
    let provider_id = detect_provider_from_model(&model_id);

    let cache_key = (session_id.clone(), model_id.clone());
    let (effective_input, cache_read, cache_write) =
        match extract_otel_cache_tokens(attrs).map(|(read, write)| {
            (
                split_total_input_tokens(total_input_tokens, read, write),
                read,
                write,
            )
        }) {
            Some(tokens) => tokens,
            None => {
                let (effective_input, cache_read) =
                    estimate_cache_read(prev_input, cache_key, total_input_tokens);
                (effective_input, cache_read, 0)
            }
        };

    if effective_input == 0 && output_tokens == 0 && cache_read == 0 && cache_write == 0 {
        return None;
    }

    let tokens = TokenBreakdown {
        input: effective_input,
        output: output_tokens,
        cache_read,
        cache_write,
        reasoning: 0,
    };

    let message_key = if !response_id.is_empty() {
        format!("copilot:{}", response_id)
    } else {
        format!(
            "copilot:{}:{}:{}:{}:{}",
            session_id, timestamp, model_id, total_input_tokens, output_tokens
        )
    };

    Some(
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
    )
}

fn extract_otel_cache_tokens(attrs: &Value) -> Option<(i64, i64)> {
    let cache_read = attrs
        .get("gen_ai.usage.cache_read.input_tokens")
        .and_then(Value::as_i64);
    let cache_write = attrs
        .get("gen_ai.usage.cache_creation.input_tokens")
        .and_then(Value::as_i64);

    match (cache_read, cache_write) {
        (None, None) => None,
        _ => Some((
            cache_read.unwrap_or(0).max(0),
            cache_write.unwrap_or(0).max(0),
        )),
    }
}

fn split_total_input_tokens(total_input: i64, cache_read: i64, cache_write: i64) -> i64 {
    total_input
        .saturating_sub(cache_read.max(0))
        .saturating_sub(cache_write.max(0))
        .max(0)
}

fn estimate_cache_read(
    prev_input: &mut HashMap<(String, String), i64>,
    cache_key: (String, String),
    total_input_tokens: i64,
) -> (i64, i64) {
    let mut cache_read = 0i64;
    let mut effective_input = total_input_tokens.max(0);

    if let Some(&prev) = prev_input.get(&cache_key) {
        if total_input_tokens >= prev {
            cache_read = prev;
            effective_input = total_input_tokens - prev;
        }
    }

    prev_input.insert(cache_key, total_input_tokens);
    (effective_input.max(0), cache_read.max(0))
}

fn parse_agent_session_event(
    value: &Value,
    session_id: &str,
    model_id: &str,
) -> Option<UnifiedMessage> {
    if value.get("type")?.as_str()? != "assistant.message" {
        return None;
    }

    let data = value.get("data")?;
    let output_tokens = data
        .get("outputTokens")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if output_tokens <= 0 {
        return None;
    }

    let timestamp = parse_timestamp_str(value.get("timestamp")?.as_str()?)?;
    let event_model = extract_model_id(data);
    let model_id = event_model.as_deref().unwrap_or(model_id);
    if is_unknown_model(model_id) {
        return None;
    }
    let message_id = data
        .get("messageId")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let interaction_id = data
        .get("interactionId")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let provider_id = detect_provider_from_model(model_id);

    Some(
        UnifiedMessage::new(
            "copilot",
            model_id.to_string(),
            provider_id,
            session_id.to_string(),
            format!("copilot-agent:{}:{}", interaction_id, message_id),
            timestamp,
            TokenBreakdown {
                input: 0,
                output: output_tokens,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
        )
        .with_parser_version(PARSER_VERSION),
    )
}

fn distribute_i64(total: i64, parts: usize, idx: usize) -> i64 {
    if parts == 0 {
        return 0;
    }
    let base = total / parts as i64;
    let remainder = (total % parts as i64).max(0);
    base + i64::from((idx as i64) < remainder)
}

fn is_agent_session_file(path: &PathBuf) -> bool {
    path.file_name().and_then(|name| name.to_str()) == Some("events.jsonl")
        && path.ancestors().any(|ancestor| {
            ancestor.file_name().and_then(|name| name.to_str()) == Some("session-state")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_provider_openai_models() {
        assert_eq!(
            detect_provider_from_model("gpt-4o-mini-2024-07-18"),
            "openai"
        );
        assert_eq!(detect_provider_from_model("gpt-4o"), "openai");
        assert_eq!(detect_provider_from_model("codex-mini-latest"), "openai");
        assert_eq!(detect_provider_from_model("o1-preview"), "openai");
        assert_eq!(detect_provider_from_model("o3-mini"), "openai");
        assert_eq!(detect_provider_from_model("o4-mini"), "openai");
    }

    #[test]
    fn detect_provider_anthropic_models() {
        assert_eq!(detect_provider_from_model("claude-3.5-sonnet"), "anthropic");
        assert_eq!(
            detect_provider_from_model("claude-sonnet-4-20250514"),
            "anthropic"
        );
    }

    #[test]
    fn detect_provider_google_models() {
        assert_eq!(detect_provider_from_model("gemini-2.0-flash"), "google");
    }

    #[test]
    fn detect_provider_nvidia_bucket_models() {
        assert_eq!(detect_provider_from_model("deepseek-r1"), "other");
        assert_eq!(detect_provider_from_model("glm-4.7"), "other");
        assert_eq!(detect_provider_from_model("MiniMax-M2.5"), "other");
        assert_eq!(
            detect_provider_from_model("nvidia/llama-3.1-nemotron"),
            "other"
        );
    }

    #[test]
    fn detect_provider_unknown() {
        assert_eq!(detect_provider_from_model("some-model"), "other");
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
    fn parse_otel_event_uses_official_cache_fields() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("events.jsonl");

        let event1 = serde_json::json!({
            "hrTime": [1700000000, 0],
            "resource": {
                "_rawAttributes": [
                    ["session.id", {"value": "sess-1"}]
                ]
            },
            "attributes": {
                "event.name": "gen_ai.client.inference.operation.details",
                "gen_ai.response.model": "claude-sonnet-4.6",
                "gen_ai.usage.input_tokens": 1000,
                "gen_ai.usage.output_tokens": 100,
                "gen_ai.response.id": "resp-1"
            }
        });
        let event2 = serde_json::json!({
            "hrTime": [1700000001, 0],
            "resource": {
                "_rawAttributes": [
                    ["session.id", {"value": "sess-1"}]
                ]
            },
            "attributes": {
                "event.name": "gen_ai.client.inference.operation.details",
                "gen_ai.response.model": "claude-sonnet-4.6",
                "gen_ai.usage.input_tokens": 1500,
                "gen_ai.usage.output_tokens": 200,
                "gen_ai.usage.cache_read.input_tokens": 200,
                "gen_ai.usage.cache_creation.input_tokens": 50,
                "gen_ai.response.id": "resp-2"
            }
        });

        std::fs::write(&file, format!("{}\n{}\n", event1, event2)).unwrap();
        let messages = parser.parse_file(&file);

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].tokens.input, 1250);
        assert_eq!(messages[1].tokens.output, 200);
        assert_eq!(messages[1].tokens.cache_read, 200);
        assert_eq!(messages[1].tokens.cache_write, 50);
    }

    #[test]
    fn parse_agent_session_event_line() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session-state").join("sess-agent-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let file = session_dir.join("events.jsonl");

        let session_start = serde_json::json!({
            "type": "session.start",
            "data": {
                "sessionId": "sess-agent-1",
                "selectedModel": "claude-opus-4.6"
            },
            "timestamp": "2026-04-08T18:12:06.646Z"
        });
        let assistant_message = serde_json::json!({
            "type": "assistant.message",
            "data": {
                "messageId": "msg-1",
                "interactionId": "interaction-1",
                "outputTokens": 482
            },
            "timestamp": "2026-04-08T18:12:19.679Z"
        });

        std::fs::write(&file, format!("{}\n{}\n", session_start, assistant_message)).unwrap();
        let messages = parser.parse_file(&file);

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.client, "copilot");
        assert_eq!(msg.model_id, "claude-opus-4.6");
        assert_eq!(msg.provider_id, "anthropic");
        assert_eq!(msg.session_id, "sess-agent-1");
        assert_eq!(msg.tokens.input, 0);
        assert_eq!(msg.tokens.output, 482);
        assert_eq!(msg.message_key, "copilot-agent:interaction-1:msg-1");
    }

    #[test]
    fn parse_agent_session_event_uses_model_change_when_start_has_no_model() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session-state").join("sess-agent-2");
        std::fs::create_dir_all(&session_dir).unwrap();
        let file = session_dir.join("events.jsonl");

        let session_start = serde_json::json!({
            "type": "session.start",
            "data": {
                "sessionId": "sess-agent-2"
            },
            "timestamp": "2026-04-08T18:12:06.646Z"
        });
        let model_change = serde_json::json!({
            "type": "session.model_change",
            "data": {
                "newModel": "claude-sonnet-4.6"
            },
            "timestamp": "2026-04-08T18:12:08.000Z"
        });
        let assistant_message = serde_json::json!({
            "type": "assistant.message",
            "data": {
                "messageId": "msg-2",
                "interactionId": "interaction-2",
                "outputTokens": 321
            },
            "timestamp": "2026-04-08T18:12:19.679Z"
        });

        std::fs::write(
            &file,
            format!(
                "{}\n{}\n{}\n",
                session_start, model_change, assistant_message
            ),
        )
        .unwrap();
        let messages = parser.parse_file(&file);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].model_id, "claude-sonnet-4.6");
        assert_eq!(messages[0].provider_id, "anthropic");
        assert_eq!(messages[0].tokens.output, 321);
    }

    #[test]
    fn parse_agent_session_event_skips_unknown_model_fallback() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session-state").join("sess-agent-unknown");
        std::fs::create_dir_all(&session_dir).unwrap();
        let file = session_dir.join("events.jsonl");

        let session_start = serde_json::json!({
            "type": "session.start",
            "data": {
                "sessionId": "sess-agent-unknown"
            },
            "timestamp": "2026-04-08T18:12:06.646Z"
        });
        let assistant_message = serde_json::json!({
            "type": "assistant.message",
            "data": {
                "messageId": "msg-unknown",
                "interactionId": "interaction-unknown",
                "outputTokens": 123
            },
            "timestamp": "2026-04-08T18:12:19.679Z"
        });

        std::fs::write(&file, format!("{}\n{}\n", session_start, assistant_message)).unwrap();
        let messages = parser.parse_file(&file);

        assert!(messages.is_empty());
    }

    #[test]
    fn parse_session_shutdown_summary() {
        let parser = CopilotSessionParser::new();
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("session-state").join("sess-agent-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let file = session_dir.join("events.jsonl");

        let session_start = serde_json::json!({
            "type": "session.start",
            "data": {
                "sessionId": "sess-agent-1",
                "selectedModel": "claude-opus-4.6"
            },
            "timestamp": "2026-04-08T18:12:06.646Z"
        });
        let shutdown = serde_json::json!({
            "type": "session.shutdown",
            "data": {
                "modelMetrics": {
                    "claude-opus-4.6": {
                        "requests": {"count": 3, "cost": 3.0},
                        "usage": {
                            "inputTokens": 9,
                            "outputTokens": 6,
                            "cacheReadTokens": 3,
                            "cacheWriteTokens": 0
                        }
                    }
                }
            },
            "timestamp": "2026-04-08T19:48:47.935Z"
        });

        std::fs::write(&file, format!("{}\n{}\n", session_start, shutdown)).unwrap();
        let messages = parser.parse_file(&file);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages.iter().map(|m| m.tokens.input).sum::<i64>(), 6);
        assert_eq!(messages.iter().map(|m| m.tokens.output).sum::<i64>(), 6);
        assert_eq!(messages.iter().map(|m| m.tokens.cache_read).sum::<i64>(), 3);
        assert_eq!(messages[0].session_id, "sess-agent-1");
        assert_eq!(messages[0].model_id, "claude-opus-4.6");
        assert_eq!(messages[0].provider_id, "anthropic");
        assert_eq!(
            messages[0].message_key,
            "copilot-session:sess-agent-1:claude-opus-4.6:0"
        );
        assert!(messages.iter().all(|message| message.cost == 0.0));
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
    fn cache_estimation_fallback_uses_monotonic_input_when_cache_attrs_absent() {
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
    fn cache_estimation_fallback_resets_on_smaller_input() {
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
