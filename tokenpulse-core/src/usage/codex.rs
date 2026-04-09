use crate::provider::{SessionParser, TokenBreakdown, UnifiedMessage};
use crate::usage::scanner;

use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use tracing::debug;

const PARSER_VERSION: &str = "codex-v2";

pub struct CodexSessionParser;

impl CodexSessionParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_file(&self, path: &PathBuf) -> Vec<UnifiedMessage> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return Vec::new(),
        };

        let mut messages = Vec::new();
        let mut current_model: Option<String> = None;
        let mut previous_totals: Option<CodexUsage> = None;
        let mut provider_id = String::from("openai");
        let mut session_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string();

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
                    debug!("Failed to parse Codex line in {:?}: {}", path, error);
                    continue;
                }
            };

            let entry_type = value
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let payload = value.get("payload");

            if entry_type == "session_meta" {
                if let Some(id) = payload
                    .and_then(|payload| payload.get("id"))
                    .and_then(Value::as_str)
                {
                    session_id = id.to_string();
                }
                if let Some(provider) = payload
                    .and_then(|payload| payload.get("model_provider"))
                    .and_then(Value::as_str)
                {
                    provider_id = provider.to_string();
                }
                if let Some(model) = extract_model(payload.or(Some(&value))) {
                    current_model = Some(model);
                }
                continue;
            }

            if let Some(model) = extract_model(payload.or(Some(&value))) {
                current_model = Some(model);
            }

            if entry_type != "event_msg"
                || payload
                    .and_then(|payload| payload.get("type"))
                    .and_then(Value::as_str)
                    != Some("token_count")
            {
                continue;
            }

            let info = match payload.and_then(|payload| payload.get("info")) {
                Some(info) => info,
                None => continue,
            };

            if let Some(model) = extract_model(Some(info)) {
                current_model = Some(model);
            }

            let model_id = current_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let timestamp = extract_timestamp_ms(&value)
                .or_else(|| payload.and_then(extract_timestamp_ms))
                .unwrap_or_default();

            let last_usage = info
                .get("last_token_usage")
                .and_then(parse_usage)
                .filter(|usage| usage.total() > 0);
            let total_usage = info
                .get("total_token_usage")
                .and_then(parse_usage)
                .filter(|usage| usage.total() > 0);

            let usage = if let Some(last_usage) = last_usage {
                if let (Some(previous), Some(total_usage)) = (previous_totals, total_usage) {
                    if total_usage.total() == previous.total() {
                        continue;
                    }
                    if total_usage.total() < previous.total()
                        && total_usage.total() + last_usage.total() >= previous.total()
                    {
                        continue;
                    }
                }
                if let Some(total_usage) = total_usage {
                    previous_totals = Some(total_usage);
                }
                last_usage
            } else if let (Some(total_usage), Some(previous)) = (total_usage, previous_totals) {
                let delta = total_usage.delta(previous);
                previous_totals = Some(total_usage);
                match delta {
                    Some(delta) if delta.total() > 0 => delta,
                    _ => continue,
                }
            } else if let Some(total_usage) = total_usage {
                previous_totals = Some(total_usage);
                total_usage
            } else {
                continue;
            };

            let tokens = usage.to_tokens();
            if tokens.is_empty() {
                continue;
            }

            let message_key = format!(
                "{}:{}:{}:{}:{}:{}:{}",
                session_id,
                timestamp,
                model_id,
                tokens.input,
                tokens.output,
                tokens.cache_read,
                tokens.reasoning
            );

            messages.push(
                UnifiedMessage::new(
                    "codex",
                    model_id,
                    provider_id.clone(),
                    session_id.clone(),
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
        let codex_home = std::env::var("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home.join(".codex"));

        vec![
            home.join(".codex").join("sessions"),
            home.join(".codex").join("archived_sessions"),
            codex_home.join("sessions"),
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

#[derive(Clone, Copy, Debug, Default)]
struct CodexUsage {
    input: i64,
    output: i64,
    cached: i64,
    reasoning: i64,
}

impl CodexUsage {
    fn total(self) -> i64 {
        self.input + self.output + self.cached + self.reasoning
    }

    fn delta(self, previous: Self) -> Option<Self> {
        if self.input < previous.input
            || self.output < previous.output
            || self.cached < previous.cached
            || self.reasoning < previous.reasoning
        {
            return None;
        }

        Some(Self {
            input: self.input - previous.input,
            output: self.output - previous.output,
            cached: self.cached - previous.cached,
            reasoning: self.reasoning - previous.reasoning,
        })
    }

    fn to_tokens(self) -> TokenBreakdown {
        let cached = self.cached.min(self.input).max(0);
        TokenBreakdown {
            input: (self.input - cached).max(0),
            output: self.output.max(0),
            cache_read: cached,
            cache_write: 0,
            reasoning: self.reasoning.max(0),
        }
    }
}

fn parse_usage(value: &Value) -> Option<CodexUsage> {
    let input = value
        .get("input_tokens")
        .and_then(Value::as_i64)
        .or_else(|| value.get("input").and_then(Value::as_i64))?
        .max(0);
    Some(CodexUsage {
        input,
        output: value
            .get("output_tokens")
            .and_then(Value::as_i64)
            .or_else(|| value.get("output").and_then(Value::as_i64))
            .unwrap_or(0)
            .max(0),
        cached: value
            .get("cached_input_tokens")
            .and_then(Value::as_i64)
            .or_else(|| value.get("cache_read_input_tokens").and_then(Value::as_i64))
            .or_else(|| value.get("cached_tokens").and_then(Value::as_i64))
            .unwrap_or(0)
            .max(0),
        reasoning: value
            .get("reasoning_output_tokens")
            .and_then(Value::as_i64)
            .or_else(|| value.get("reasoning_tokens").and_then(Value::as_i64))
            .unwrap_or(0)
            .max(0),
    })
}

fn extract_model(value: Option<&Value>) -> Option<String> {
    let value = value?;
    value
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| value.get("model_name").and_then(Value::as_str))
        .or_else(|| {
            value
                .get("model_info")
                .and_then(|info| info.get("slug"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            value
                .get("payload")
                .and_then(|payload| payload.get("model"))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned)
}

fn extract_timestamp_ms(value: &Value) -> Option<i64> {
    if let Some(raw) = value.get("timestamp").and_then(Value::as_i64) {
        return Some(raw);
    }
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|dt| dt.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_usage_reads_aliases_and_clamps_negative_values() {
        let value: Value = serde_json::from_str(
            r#"{
                "input": 100,
                "output_tokens": 25,
                "cache_read_input_tokens": 150,
                "reasoning_tokens": -10
            }"#,
        )
        .unwrap();

        let usage = parse_usage(&value).unwrap();

        assert_eq!(usage.input, 100);
        assert_eq!(usage.output, 25);
        assert_eq!(usage.cached, 150);
        assert_eq!(usage.reasoning, 0);
    }

    #[test]
    fn parse_file_prefers_last_usage_and_clamps_cache_to_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"session-1","model_provider":"openai","model":"gpt-5"}}
{"type":"event_msg","timestamp":"2026-04-01T12:00:00Z","payload":{"type":"token_count","info":{"model":"gpt-5","last_token_usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":120,"reasoning_output_tokens":5},"total_token_usage":{"input_tokens":100,"output_tokens":20,"cached_input_tokens":120,"reasoning_output_tokens":5}}}}"#,
        )
        .unwrap();

        let messages = CodexSessionParser::new().parse_file(&path);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].session_id, "session-1");
        assert_eq!(messages[0].provider_id, "openai");
        assert_eq!(messages[0].tokens.input, 0);
        assert_eq!(messages[0].tokens.output, 20);
        assert_eq!(messages[0].tokens.cache_read, 100);
        assert_eq!(messages[0].tokens.reasoning, 5);
    }
}
