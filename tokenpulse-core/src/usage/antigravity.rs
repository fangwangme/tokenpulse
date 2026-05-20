use crate::provider::{
    local_date_string_from_timestamp, SessionParser, TokenBreakdown, UnifiedMessage,
};
use crate::usage::scanner;
use crate::usage::utils::detect_provider_from_model;

use anyhow::Result;
use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, info, warn};

const PARSER_VERSION: &str = "antigravity-v2";
const MODEL_ALIAS_HISTORY_VERSION: u32 = 1;
const MODEL_ALIAS_HISTORY_FILE_NAME: &str = "model-aliases.json";

pub struct AntigravitySessionParser;

impl AntigravitySessionParser {
    pub fn new() -> Self {
        Self
    }

    fn parse_file(&self, path: PathBuf) -> Vec<UnifiedMessage> {
        let mut messages = Vec::new();
        let mut message_positions = std::collections::HashMap::new();
        let mut current_model: Option<String> = None;

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read {:?}: {}", path, e);
                return messages;
            }
        };

        for (line_index, line) in content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let entry: Value = match serde_json::from_str(trimmed) {
                Ok(e) => e,
                Err(e) => {
                    debug!("Failed to parse Antigravity entry: {}", e);
                    continue;
                }
            };

            let entry_type = entry.get("type").and_then(Value::as_str).unwrap_or("");
            match entry_type {
                "session_meta" => {
                    if let Some(model_id) = entry.get("modelId").and_then(Value::as_str) {
                        current_model = Some(model_id.to_string());
                    }
                }
                "usage" => {
                    let model = entry
                        .get("modelId")
                        .and_then(Value::as_str)
                        .map(String::from)
                        .or_else(|| current_model.clone())
                        .unwrap_or_else(|| "unknown".to_string());
                    let model = resolve_antigravity_model_id(&model);

                    let session_id = entry
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            path.file_stem()
                                .and_then(|stem| stem.to_str())
                                .unwrap_or("unknown")
                        })
                        .to_string();

                    let timestamp = entry
                        .get("timestamp")
                        .and_then(Value::as_i64)
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

                    let input = to_safe_i64(entry.get("input"));
                    let output = to_safe_i64(entry.get("output"));
                    let cache_read = to_safe_i64(entry.get("cacheRead"));
                    let cache_write = to_safe_i64(entry.get("cacheWrite"));
                    let reasoning = to_safe_i64(entry.get("reasoning"));

                    let tokens = TokenBreakdown {
                        input,
                        output,
                        cache_read,
                        cache_write,
                        reasoning,
                    };

                    if tokens.is_empty() {
                        continue;
                    }

                    let provider_id = detect_provider_from_model(&model);
                    let date = local_date_string_from_timestamp(timestamp);

                    let response_id = entry.get("responseId").and_then(Value::as_str);
                    let message_key = response_id.map(String::from).unwrap_or_else(|| {
                        format!("{}:{}:{}:{}", session_id, timestamp, model, line_index)
                    });

                    let msg = UnifiedMessage::new(
                        "antigravity",
                        model,
                        provider_id,
                        session_id,
                        message_key.clone(),
                        timestamp,
                        tokens,
                    )
                    .with_pricing_day(date)
                    .with_parser_version(PARSER_VERSION);

                    if let Some(position) = message_positions.get(&message_key).copied() {
                        messages[position] = msg;
                    } else {
                        message_positions.insert(message_key, messages.len());
                        messages.push(msg);
                    }
                }
                _ => {}
            }
        }

        messages
    }
}

impl Default for AntigravitySessionParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionParser for AntigravitySessionParser {
    fn provider_name(&self) -> &str {
        "antigravity"
    }

    fn session_paths(&self) -> Vec<PathBuf> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        vec![home
            .join(".config")
            .join("tokenpulse")
            .join("antigravity-cache")
            .join("sessions")]
    }

    fn parse_sessions(&self, since: Option<NaiveDate>) -> Result<Vec<UnifiedMessage>> {
        let mut all_messages = Vec::new();

        for root in self.session_paths() {
            if !root.exists() {
                let _ = std::fs::create_dir_all(&root);
            }

            // Sync first!
            if let Err(e) = sync_antigravity(&root) {
                debug!("Failed to sync Antigravity: {}", e);
            }

            let alias_history = match load_model_alias_history_map(&root) {
                Ok(aliases) => aliases,
                Err(e) => {
                    debug!("Failed to load Antigravity model alias history: {}", e);
                    HashMap::new()
                }
            };
            if let Err(e) = normalize_cached_antigravity_artifacts(&root, &alias_history) {
                debug!("Failed to normalize Antigravity cache: {}", e);
            }

            let files = scanner::discover_files(&root, "jsonl", since);
            debug!("Found {} files for Antigravity", files.len());

            for file in files {
                let msgs = self.parse_file(file);
                all_messages.extend(msgs);
            }
        }

        all_messages.sort_by_key(|m| m.timestamp);
        Ok(all_messages)
    }

    fn parser_version(&self) -> &str {
        PARSER_VERSION
    }
}

#[derive(Debug, Clone)]
pub struct AntigravityConnection {
    pub pid: u32,
    pub port: u16,
    pub csrf_token: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone)]
struct ProcessCandidate {
    pid: u32,
    ppid: u32,
    declared_port: Option<u16>,
    csrf_token: String,
}

pub fn sync_antigravity(sessions_dir: &Path) -> Result<()> {
    let connections = match detect_antigravity_connections() {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "Warning: Antigravity language server process discovery failed: {}",
                e
            );
            return Ok(());
        }
    };

    if connections.is_empty() {
        if let Err(e) = merge_and_save_model_alias_history(sessions_dir, &HashMap::new()) {
            debug!("Failed to seed Antigravity model alias history: {}", e);
        }
        debug!("No running Antigravity language servers detected; skipping sync and reading cache");
        return Ok(());
    }

    let dynamic_model_aliases = fetch_dynamic_model_aliases(&connections);
    let model_aliases =
        match merge_and_save_model_alias_history(sessions_dir, &dynamic_model_aliases) {
            Ok(aliases) => aliases,
            Err(e) => {
                debug!("Failed to update Antigravity model alias history: {}", e);
                dynamic_model_aliases
            }
        };

    let cached_files_before = match std::fs::read_dir(sessions_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "jsonl"))
            .count(),
        Err(_) => 0,
    };
    let mut synced_sessions_count = 0;

    let mut summaries = Vec::new();
    for connection in &connections {
        let response = match rpc_request(connection, "GetAllCascadeTrajectories", &json!({})) {
            Ok(r) => r,
            Err(e) => {
                debug!(
                    "Failed to query GetAllCascadeTrajectories from {}: {}",
                    connection.port, e
                );
                continue;
            }
        };

        // The response can be:
        // 1. An object with a "trajectorySummaries" or "cascadeTrajectories" key containing an array
        // 2. A flat object map where keys are cascade IDs and values are session objects
        let trajectory_entries = extract_trajectory_entries(&response);

        for (key, item) in trajectory_entries {
            let session_id = if !key.is_empty() {
                key
            } else {
                item.get("cascadeId")
                    .or_else(|| item.get("trajectoryId"))
                    .or_else(|| item.get("id"))
                    .or_else(|| item.get("sessionId"))
                    .and_then(Value::as_str)
                    .map(String::from)
                    .unwrap_or_default()
            };

            if session_id.is_empty() {
                continue;
            }

            let last_modified_ms = item
                .get("lastModifiedTime")
                .or_else(|| item.get("lastModified"))
                .or_else(|| item.get("updatedAt"))
                .and_then(parse_timestamp_value);

            summaries.push((session_id, last_modified_ms, connection.clone()));
        }
    }

    // Discover conversation IDs from local folders
    let local_conversation_ids = discover_local_conversation_ids();
    debug!(
        "Discovered {} local conversation files",
        local_conversation_ids.len()
    );
    for session_id in local_conversation_ids {
        if !summaries.iter().any(|(id, _, _)| id == &session_id) {
            for connection in &connections {
                summaries.push((session_id.clone(), None, connection.clone()));
            }
        }
    }

    let mut unique_summaries: HashMap<String, (Option<i64>, Vec<AntigravityConnection>)> =
        HashMap::new();
    for (session_id, last_modified_ms, conn) in summaries {
        if let Some((existing_lm, conns)) = unique_summaries.get_mut(&session_id) {
            if last_modified_ms.unwrap_or(0) > existing_lm.unwrap_or(0) {
                *existing_lm = last_modified_ms;
            }
            if !conns
                .iter()
                .any(|c| c.pid == conn.pid && c.port == conn.port)
            {
                conns.push(conn);
            }
        } else {
            unique_summaries.insert(session_id, (last_modified_ms, vec![conn]));
        }
    }

    let total_detected_sessions = unique_summaries.len();
    let max_summary_ms = unique_summaries
        .values()
        .filter_map(|(lm, _)| *lm)
        .max()
        .unwrap_or_else(|| Local::now().timestamp_millis());

    let sync_threshold_existing_ms = max_summary_ms - 2 * 24 * 3600 * 1000; // 2 days
    let sync_threshold_new_ms = max_summary_ms - 7 * 24 * 3600 * 1000; // 7 days

    for (session_id, (last_modified_ms, conns)) in unique_summaries {
        let lm = last_modified_ms.unwrap_or(max_summary_ms);
        let file_name = session_artifact_file_stem(&session_id);
        let path = sessions_dir.join(format!("{}.jsonl", file_name));
        let exists = path.exists();

        // 1. Skip if older than 7 days (even if file does not exist)
        if lm < sync_threshold_new_ms {
            continue;
        }

        // 2. Skip if older than 2 days and the file already exists
        if lm < sync_threshold_existing_ms && exists {
            continue;
        }

        let mut metadata_response = None;
        for conn in &conns {
            debug!(
                "Syncing Antigravity session {} (modified: {:?}) from port {}",
                session_id, last_modified_ms, conn.port
            );
            match rpc_request(
                conn,
                "GetCascadeTrajectoryGeneratorMetadata",
                &json!({ "cascadeId": session_id }),
            ) {
                Ok(r) => {
                    metadata_response = Some(r);
                    break;
                }
                Err(e) => {
                    debug!(
                        "Failed to fetch generator metadata for session {} from port {}: {}",
                        session_id, conn.port, e
                    );
                }
            }
        }

        let Some(metadata_response) = metadata_response else {
            continue;
        };

        let metadata = metadata_response
            .get("generatorMetadata")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        if metadata.is_empty() {
            continue;
        }

        let mut lines = Vec::new();
        for meta in &metadata {
            let chat_model = meta.get("chatModel").unwrap_or(meta);
            let raw_model_id = chat_model
                .get("responseModel")
                .or_else(|| chat_model.get("model"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let model_id = resolve_antigravity_model_id_with_aliases(raw_model_id, &model_aliases);

            let created_at = chat_model
                .get("chatStartMetadata")
                .and_then(|v| v.get("createdAt"))
                .and_then(parse_timestamp_value);

            lines.push(
                json!({
                    "type": "session_meta",
                    "sessionId": session_id,
                    "modelId": model_id,
                    "timestamp": created_at,
                })
                .to_string(),
            );

            if let Some(retry_infos) = chat_model.get("retryInfos").and_then(Value::as_array) {
                for retry in retry_infos {
                    let usage = retry.get("usage").unwrap_or(retry);
                    let input = to_safe_i64(usage.get("inputTokens"));
                    let output = to_safe_i64(usage.get("outputTokens"));
                    let cache_read = to_safe_i64(usage.get("cacheReadTokens"));
                    let reasoning = to_safe_i64(usage.get("thinkingOutputTokens"));
                    let timestamp = usage
                        .get("createdAt")
                        .or_else(|| usage.get("timestamp"))
                        .and_then(parse_timestamp_value)
                        .or(created_at);

                    if input == 0 && output == 0 && cache_read == 0 && reasoning == 0 {
                        continue;
                    }

                    lines.push(
                        json!({
                            "type": "usage",
                            "sessionId": session_id,
                            "modelId": model_id,
                            "timestamp": timestamp,
                            "input": input,
                            "output": output,
                            "cacheRead": cache_read,
                            "cacheWrite": 0,
                            "reasoning": reasoning,
                            "responseId": usage.get("responseId").and_then(Value::as_str),
                        })
                        .to_string(),
                    );
                }
            }
        }

        if !lines.is_empty() {
            let contents = format!("{}\n", lines.join("\n"));
            match std::fs::write(&path, contents) {
                Ok(_) => {
                    synced_sessions_count += 1;
                }
                Err(e) => {
                    warn!("Failed to write cache file {:?}: {}", path, e);
                }
            }
        }
    }

    let cached_files_after = match std::fs::read_dir(sessions_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|entry| entry.path().extension().map_or(false, |ext| ext == "jsonl"))
            .count(),
        Err(_) => 0,
    };

    info!("Antigravity sync: Synced local Antigravity cache from running language servers.");
    info!("detected connections: {}", connections.len());
    info!("total detected sessions: {}", total_detected_sessions);
    info!("synced sessions this run: {}", synced_sessions_count);
    info!(
        "cached sessions: {} -> {}",
        cached_files_before, cached_files_after
    );

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelAliasHistory {
    version: u32,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    aliases: BTreeMap<String, ModelAliasHistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelAliasHistoryEntry {
    #[serde(rename = "rawModelId")]
    raw_model_id: String,
    #[serde(rename = "modelId")]
    model_id: String,
    label: Option<String>,
    source: String,
    #[serde(rename = "firstSeenAt")]
    first_seen_at: String,
    #[serde(rename = "lastSeenAt")]
    last_seen_at: String,
}

#[derive(Debug, Clone)]
struct ModelAlias {
    raw_model_id: String,
    model_id: String,
    label: Option<String>,
    source: String,
}

struct StaticModelAlias {
    raw_model_id: &'static str,
    model_id: &'static str,
    label: Option<&'static str>,
    source: &'static str,
}

const STATIC_MODEL_ALIASES: &[StaticModelAlias] = &[
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M26",
        model_id: "claude-opus-4-6-thinking",
        label: Some("Claude Opus 4.6 (Thinking)"),
        source: "captured-antigravity-get-user-status;tokscale",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M35",
        model_id: "claude-sonnet-4-6-thinking",
        label: Some("Claude Sonnet 4.6 (Thinking)"),
        source: "captured-antigravity-get-user-status;tokscale",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M36",
        model_id: "gemini-3.1-pro-low",
        label: Some("Gemini 3.1 Pro (Low)"),
        source: "captured-antigravity-get-user-status;tokscale",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M37",
        model_id: "gemini-3.1-pro-high",
        label: Some("Gemini 3.1 Pro (High)"),
        source: "tokscale;antigravity-mobility-cli",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M47",
        model_id: "gemini-3-flash-preview",
        label: Some("Gemini 3 Flash"),
        source: "tokscale;antigravity-mobility-cli",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_OPENAI_GPT_OSS_120B_MEDIUM",
        model_id: "gpt-oss-120b-medium",
        label: Some("GPT-OSS 120B (Medium)"),
        source: "captured-antigravity-get-user-status;tokscale",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M132",
        model_id: "gemini-3.5-flash-high",
        label: Some("Gemini 3.5 Flash (High)"),
        source: "captured-antigravity-get-user-status",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M20",
        model_id: "gemini-3.5-flash-medium",
        label: Some("Gemini 3.5 Flash (Medium)"),
        source: "captured-antigravity-get-user-status",
    },
    StaticModelAlias {
        raw_model_id: "MODEL_PLACEHOLDER_M16",
        model_id: "gemini-3.1-pro-high",
        label: Some("Gemini 3.1 Pro (High)"),
        source: "captured-antigravity-get-user-status",
    },
    StaticModelAlias {
        raw_model_id: "antigravity-claude-opus-4-6-thinking",
        model_id: "claude-opus-4-6-thinking",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "claude-opus-4.6-thinking",
        model_id: "claude-opus-4-6-thinking",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "antigravity-claude-sonnet-4-6-thinking",
        model_id: "claude-sonnet-4-6-thinking",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "claude-sonnet-4.6-thinking",
        model_id: "claude-sonnet-4-6-thinking",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "antigravity-claude-opus-4-5-thinking",
        model_id: "claude-opus-4-5-thinking",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "antigravity-claude-opus-4-5-thinking-high",
        model_id: "claude-opus-4-5-thinking-high",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "antigravity-claude-opus-4-5-thinking-medium",
        model_id: "claude-opus-4-5-thinking-medium",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "antigravity-claude-sonnet-4-5-thinking",
        model_id: "claude-sonnet-4-5-thinking",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "antigravity-claude-sonnet-4-5-thinking-high",
        model_id: "claude-sonnet-4-5-thinking-high",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "antigravity-claude-sonnet-4-5-thinking-medium",
        model_id: "claude-sonnet-4-5-thinking-medium",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "claude-opus-4.6",
        model_id: "claude-opus-4-6",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "claude-sonnet-4.6",
        model_id: "claude-sonnet-4-6",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "claude-haiku-4.6",
        model_id: "claude-haiku-4-6",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "claude-opus-4.5",
        model_id: "claude-opus-4-5",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "claude-sonnet-4.5",
        model_id: "claude-sonnet-4-5",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "claude-haiku-4.5",
        model_id: "claude-haiku-4-5",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "claude-opus-4.5-thinking",
        model_id: "claude-opus-4-5-thinking",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "claude-sonnet-4.5-thinking",
        model_id: "claude-sonnet-4-5-thinking",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "gemini-3-flash-c",
        model_id: "gemini-3-flash-preview",
        label: None,
        source: "tokscale;format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "gemini-3-pro-preview-high",
        model_id: "gemini-3-pro-preview-high",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "gemini-3-pro-preview-low",
        model_id: "gemini-3-pro-preview-low",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "gemini-3.0-pro-preview-high",
        model_id: "gemini-3-pro-preview-high",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "gemini-3.0-pro-preview-low",
        model_id: "gemini-3-pro-preview-low",
        label: None,
        source: "format-normalization",
    },
    StaticModelAlias {
        raw_model_id: "gemini-3-flash-a",
        model_id: "gemini-3.5-flash",
        label: Some("Gemini 3.5 Flash"),
        source: "captured-antigravity-runtime",
    },
    StaticModelAlias {
        raw_model_id: "antigravity-gemini-3-flash-a",
        model_id: "gemini-3.5-flash",
        label: Some("Gemini 3.5 Flash"),
        source: "captured-antigravity-runtime",
    },
];

fn static_model_aliases() -> HashMap<String, ModelAlias> {
    let mut aliases = HashMap::new();
    for alias in STATIC_MODEL_ALIASES {
        aliases.insert(
            normalize_alias_key(alias.raw_model_id),
            ModelAlias {
                raw_model_id: alias.raw_model_id.to_string(),
                model_id: alias.model_id.to_string(),
                label: alias.label.map(str::to_string),
                source: alias.source.to_string(),
            },
        );
    }
    aliases
}

fn fetch_dynamic_model_aliases(
    connections: &[AntigravityConnection],
) -> HashMap<String, ModelAlias> {
    let mut aliases = HashMap::new();

    for connection in connections {
        let response = match rpc_request(
            connection,
            "GetUserStatus",
            &json!({
                "metadata": {
                    "ideName": "antigravity",
                    "extensionName": "antigravity",
                    "ideVersion": "unknown",
                    "locale": "en",
                }
            }),
        ) {
            Ok(response) => response,
            Err(e) => {
                debug!(
                    "Failed to fetch Antigravity model labels from port {}: {}",
                    connection.port, e
                );
                continue;
            }
        };

        collect_model_aliases_from_value(&response, &mut aliases);
    }

    aliases
}

fn collect_model_aliases_from_value(value: &Value, aliases: &mut HashMap<String, ModelAlias>) {
    if let Some(configs) = value.get("clientModelConfigs").and_then(Value::as_array) {
        collect_model_aliases_from_configs(configs, aliases);
    }

    if let Some(configs) = value
        .get("cascadeModelConfigData")
        .and_then(|data| data.get("clientModelConfigs"))
        .and_then(Value::as_array)
    {
        collect_model_aliases_from_configs(configs, aliases);
    }

    if let Some(configs) = value
        .get("userStatus")
        .and_then(|status| status.get("cascadeModelConfigData"))
        .and_then(|data| data.get("clientModelConfigs"))
        .and_then(Value::as_array)
    {
        collect_model_aliases_from_configs(configs, aliases);
    }
}

fn collect_model_aliases_from_configs(
    configs: &[Value],
    aliases: &mut HashMap<String, ModelAlias>,
) {
    for config in configs {
        let Some(raw_model) = config
            .get("modelOrAlias")
            .and_then(|model| model.get("model"))
            .and_then(Value::as_str)
            .filter(|model| !model.trim().is_empty())
        else {
            continue;
        };
        let Some(label) = config
            .get("label")
            .and_then(Value::as_str)
            .filter(|label| !label.trim().is_empty())
        else {
            continue;
        };
        let Some(model_id) = antigravity_label_to_model_id(label) else {
            continue;
        };
        aliases.insert(
            normalize_alias_key(raw_model),
            ModelAlias {
                raw_model_id: raw_model.to_string(),
                model_id,
                label: Some(label.to_string()),
                source: "antigravity-get-user-status".to_string(),
            },
        );
    }
}

fn model_alias_history_path(sessions_dir: &Path) -> Option<PathBuf> {
    sessions_dir
        .parent()
        .map(|cache_dir| cache_dir.join(MODEL_ALIAS_HISTORY_FILE_NAME))
}

fn load_model_alias_history_map(sessions_dir: &Path) -> Result<HashMap<String, ModelAlias>> {
    let mut aliases = static_model_aliases();
    let Some(path) = model_alias_history_path(sessions_dir) else {
        return Ok(aliases);
    };
    if !path.exists() {
        return Ok(aliases);
    }

    let content = std::fs::read_to_string(&path)?;
    let history: ModelAliasHistory = serde_json::from_str(&content)?;
    aliases.extend(history.aliases.into_iter().map(|(key, entry)| {
        (
            key,
            ModelAlias {
                raw_model_id: entry.raw_model_id,
                model_id: entry.model_id,
                label: entry.label,
                source: entry.source,
            },
        )
    }));
    Ok(aliases)
}

fn merge_and_save_model_alias_history(
    sessions_dir: &Path,
    dynamic_aliases: &HashMap<String, ModelAlias>,
) -> Result<HashMap<String, ModelAlias>> {
    let now = chrono::Utc::now().to_rfc3339();
    let Some(path) = model_alias_history_path(sessions_dir) else {
        return Ok(dynamic_aliases.clone());
    };

    let mut history = if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        serde_json::from_str::<ModelAliasHistory>(&content).unwrap_or_else(|_| ModelAliasHistory {
            version: MODEL_ALIAS_HISTORY_VERSION,
            updated_at: now.clone(),
            aliases: BTreeMap::new(),
        })
    } else {
        ModelAliasHistory {
            version: MODEL_ALIAS_HISTORY_VERSION,
            updated_at: now.clone(),
            aliases: BTreeMap::new(),
        }
    };

    history.version = MODEL_ALIAS_HISTORY_VERSION;
    history.updated_at = now.clone();

    let mut merged_aliases = static_model_aliases();
    for (key, entry) in &history.aliases {
        merged_aliases.insert(
            key.clone(),
            ModelAlias {
                raw_model_id: entry.raw_model_id.clone(),
                model_id: entry.model_id.clone(),
                label: entry.label.clone(),
                source: entry.source.clone(),
            },
        );
    }
    for (key, alias) in dynamic_aliases {
        merged_aliases.insert(key.clone(), alias.clone());
    }

    for (key, alias) in &merged_aliases {
        let entry = history
            .aliases
            .entry(key.clone())
            .or_insert_with(|| ModelAliasHistoryEntry {
                raw_model_id: alias.raw_model_id.clone(),
                model_id: alias.model_id.clone(),
                label: alias.label.clone(),
                source: alias.source.clone(),
                first_seen_at: now.clone(),
                last_seen_at: now.clone(),
            });
        entry.raw_model_id = alias.raw_model_id.clone();
        entry.model_id = alias.model_id.clone();
        entry.label = alias.label.clone();
        entry.source = alias.source.clone();
        entry.last_seen_at = now.clone();
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&history)?),
    )?;

    Ok(merged_aliases)
}

fn normalize_cached_antigravity_artifacts(
    sessions_dir: &Path,
    model_aliases: &HashMap<String, ModelAlias>,
) -> Result<()> {
    if !sessions_dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(sessions_dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                debug!("Failed to read Antigravity cache entry: {}", e);
                continue;
            }
        };
        let path = entry.path();
        if !path.is_file() || path.extension().map_or(true, |ext| ext != "jsonl") {
            continue;
        }
        normalize_cached_antigravity_artifact(&path, model_aliases)?;
    }

    Ok(())
}

fn normalize_cached_antigravity_artifact(
    path: &Path,
    model_aliases: &HashMap<String, ModelAlias>,
) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let mut changed = false;
    let mut lines = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            lines.push(line.to_string());
            continue;
        }

        let mut entry: Value = match serde_json::from_str(trimmed) {
            Ok(entry) => entry,
            Err(_) => {
                lines.push(line.to_string());
                continue;
            }
        };

        if normalize_model_id_field(&mut entry, model_aliases) {
            changed = true;
            lines.push(entry.to_string());
        } else {
            lines.push(line.to_string());
        }
    }

    if changed {
        std::fs::write(path, format!("{}\n", lines.join("\n")))?;
    }

    Ok(())
}

fn normalize_model_id_field(
    entry: &mut Value,
    model_aliases: &HashMap<String, ModelAlias>,
) -> bool {
    let Some(model_id) = entry.get("modelId").and_then(Value::as_str) else {
        return false;
    };

    let resolved = resolve_antigravity_model_id_with_aliases(model_id, model_aliases);
    if resolved == model_id {
        return false;
    }

    if let Some(object) = entry.as_object_mut() {
        object.insert("modelId".to_string(), Value::String(resolved));
        true
    } else {
        false
    }
}

fn resolve_antigravity_model_id(model_id: &str) -> String {
    resolve_antigravity_model_id_with_aliases(model_id, &HashMap::new())
}

fn resolve_antigravity_model_id_with_aliases(
    model_id: &str,
    dynamic_aliases: &HashMap<String, ModelAlias>,
) -> String {
    dynamic_aliases
        .get(&normalize_alias_key(model_id))
        .map(|alias| alias.model_id.as_str())
        .or_else(|| antigravity_model_alias(model_id))
        .unwrap_or(model_id)
        .to_string()
}

fn normalize_alias_key(model_id: &str) -> String {
    model_id.trim().to_ascii_lowercase().replace('-', "_")
}

fn antigravity_label_to_model_id(label: &str) -> Option<String> {
    let cleaned = label.trim();
    if cleaned.is_empty() {
        return None;
    }

    let lower = cleaned.to_ascii_lowercase();
    if !(lower.starts_with("claude ")
        || lower.starts_with("gemini ")
        || lower.starts_with("gpt-oss "))
    {
        return None;
    }

    let normalized = lower.replace("(", " ").replace(")", " ");
    let tokens = normalized
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(|token| token.replace('.', "-"))
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    let model_id = tokens.join("-");
    match model_id.as_str() {
        "gemini-3-0-pro-preview-high" => Some("gemini-3-pro-preview-high".to_string()),
        "gemini-3-0-pro-preview-low" => Some("gemini-3-pro-preview-low".to_string()),
        "gemini-3-0-pro-preview" => Some("gemini-3-pro-preview".to_string()),
        _ => Some(model_id),
    }
}

fn antigravity_model_alias(model_id: &str) -> Option<&'static str> {
    let key = normalize_alias_key(model_id);
    STATIC_MODEL_ALIASES
        .iter()
        .find(|alias| normalize_alias_key(alias.raw_model_id) == key)
        .map(|alias| alias.model_id)
}

/// Extracts trajectory entries from the RPC response.
/// Supports these formats:
/// 1. `{ "trajectorySummaries": [ ... ] }` — array of objects
/// 2. `{ "trajectorySummaries": { "id1": {...}, "id2": {...} } }` — object map (keys = cascade IDs)
/// 3. `{ "cascadeId1": { ... }, "cascadeId2": { ... } }` — flat object map at root level
fn extract_trajectory_entries(response: &Value) -> Vec<(String, Value)> {
    let known_keys = ["trajectorySummaries", "cascadeTrajectories"];

    // Try known container keys first
    for key in &known_keys {
        if let Some(container) = response.get(*key) {
            // Container is an array
            if let Some(arr) = container.as_array() {
                return arr
                    .iter()
                    .map(|item| (String::new(), item.clone()))
                    .collect();
            }
            // Container is an object map (keys = cascade IDs)
            if let Some(obj) = container.as_object() {
                return obj
                    .iter()
                    .filter(|(_, v)| v.is_object())
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
            }
        }
    }

    // Fall back to flat object map at root level
    if let Some(obj) = response.as_object() {
        let entries: Vec<_> = obj
            .iter()
            .filter(|(k, v)| v.is_object() && !known_keys.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !entries.is_empty() {
            return entries;
        }
    }

    Vec::new()
}

fn sanitize_session_id(session_id: &str) -> String {
    let sanitized: String = session_id
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();

    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "session".to_string()
    } else {
        trimmed.to_string()
    }
}

fn session_artifact_file_stem(session_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let sanitized = sanitize_session_id(session_id);
    let mut hasher = DefaultHasher::new();
    session_id.hash(&mut hasher);
    let hash = hasher.finish();
    format!("{}-{:016x}", sanitized, hash)
}

fn to_safe_i64(value: Option<&Value>) -> i64 {
    value
        .and_then(|inner| {
            inner
                .as_i64()
                .or_else(|| inner.as_u64().and_then(|number| i64::try_from(number).ok()))
                .or_else(|| inner.as_str().and_then(|text| text.parse::<i64>().ok()))
        })
        .unwrap_or(0)
        .max(0)
}

fn parse_timestamp_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| {
            value.as_str().and_then(|text| {
                text.parse::<i64>().ok().or_else(|| {
                    chrono::DateTime::parse_from_rfc3339(text)
                        .ok()
                        .map(|datetime| datetime.timestamp_millis())
                })
            })
        })
        .filter(|timestamp| *timestamp > 0)
}

pub fn detect_antigravity_connections() -> Result<Vec<AntigravityConnection>> {
    let candidates = detect_process_candidates()?;
    let mut connections = Vec::new();

    for candidate in candidates {
        let ports = candidate_probe_ports(&candidate, find_listening_ports(candidate.pid)?);
        for port in ports {
            if probe_heartbeat(port, &candidate.csrf_token) {
                connections.push(AntigravityConnection {
                    pid: candidate.pid,
                    port,
                    csrf_token: candidate.csrf_token.clone(),
                    fingerprint: format!("pid:{}:port:{}", candidate.pid, port),
                });
                break;
            }
        }
    }

    connections.sort_by(|left, right| {
        right
            .pid
            .cmp(&left.pid)
            .then_with(|| left.port.cmp(&right.port))
    });
    connections.dedup_by(|left, right| left.pid == right.pid && left.port == right.port);

    Ok(connections)
}

fn candidate_probe_ports(candidate: &ProcessCandidate, mut ports: Vec<u16>) -> Vec<u16> {
    if let Some(declared_port) = candidate.declared_port {
        if !ports.contains(&declared_port) {
            ports.push(declared_port);
        }
    }

    ports.sort_unstable();
    ports.dedup();
    ports
}

fn detect_process_candidates() -> Result<Vec<ProcessCandidate>> {
    let output = run_command("ps", &["-ww", "-eo", "pid,ppid,args"])?;
    let mut candidates = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }

        let Ok(pid) = parts[0].parse::<u32>() else {
            continue;
        };
        let Ok(ppid) = parts[1].parse::<u32>() else {
            continue;
        };
        let command = parts[2..].join(" ");
        if !is_antigravity_process(&command) {
            continue;
        }

        let exe_ok = process_executable_path(pid)
            .map(|path| {
                let lower = path.to_string_lossy().to_lowercase();
                lower.contains("antigravity") || lower.contains("language_server")
            })
            .unwrap_or(true);
        if !exe_ok {
            continue;
        }

        let Some(csrf_token) = extract_csrf_token(&command) else {
            continue;
        };
        let declared_port = extract_declared_port(&command);

        candidates.push(ProcessCandidate {
            pid,
            ppid,
            declared_port,
            csrf_token,
        });
    }

    candidates.sort_by(|left, right| {
        right
            .pid
            .cmp(&left.pid)
            .then_with(|| right.ppid.cmp(&left.ppid))
            .then_with(|| right.declared_port.cmp(&left.declared_port))
    });
    candidates.dedup_by(|left, right| left.pid == right.pid);

    Ok(candidates)
}

fn is_antigravity_process(command: &str) -> bool {
    let lower = command.to_lowercase();
    (lower.contains("language_server")
        && (lower.contains("antigravity")
            || lower.contains("--app_data_dir") && lower.contains("antigravity")))
        || lower.contains("/antigravity/")
        || lower.contains("\\antigravity\\")
        || lower.contains("/antigravity-cli/")
        || lower.contains("\\antigravity-cli\\")
}

fn discover_local_conversation_ids() -> Vec<String> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    discover_local_conversation_ids_from_home(&home)
}

fn discover_local_conversation_ids_from_home(home: &Path) -> Vec<String> {
    let dirs = vec![
        home.join(".gemini")
            .join("antigravity")
            .join("conversations"),
        home.join(".gemini")
            .join("antigravity-cli")
            .join("conversations"),
    ];

    let mut session_ids = Vec::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        if ext == "pb" || ext == "db" {
                            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                                if stem.len() >= 20 {
                                    session_ids.push(stem.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    session_ids.sort();
    session_ids.dedup();
    session_ids
}

#[cfg(target_os = "macos")]
fn process_executable_path(pid: u32) -> Option<PathBuf> {
    let pid_str = pid.to_string();
    let output = run_command("lsof", &["-p", &pid_str, "-Fn"]).ok()?;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix('n') {
            if rest.contains(".app/Contents/MacOS/") {
                return Some(PathBuf::from(rest));
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
fn process_executable_path(_pid: u32) -> Option<PathBuf> {
    None
}

fn extract_csrf_token(command: &str) -> Option<String> {
    let token = extract_flag_value(command, "--csrf_token")?;
    if token.len() >= 32 && token.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-') {
        Some(token)
    } else {
        None
    }
}

fn extract_declared_port(command: &str) -> Option<u16> {
    extract_flag_value(command, "--extension_server_port")?
        .parse::<u16>()
        .ok()
}

fn extract_flag_value(command: &str, flag: &str) -> Option<String> {
    let compact = format!("{}=", flag);
    if let Some(idx) = command.find(&compact) {
        let rest = &command[idx + compact.len()..];
        return rest
            .split_whitespace()
            .next()
            .map(|value| value.to_string());
    }

    let idx = command.find(flag)?;
    let rest = &command[idx + flag.len()..];
    rest.split_whitespace()
        .find(|value| !value.is_empty())
        .map(|value| value.trim().to_string())
}

fn find_listening_ports(pid: u32) -> Result<Vec<u16>> {
    let pid_str = pid.to_string();
    let mut ports = run_port_query("lsof", &["-Pan", "-p", &pid_str, "-iTCP", "-sTCP:LISTEN"])?;

    if ports.is_empty() {
        ports = run_port_query("lsof", &["-Pan", "-p", &pid_str, "-i"])?;
    }

    ports.sort_unstable();
    ports.dedup();
    Ok(ports)
}

fn run_port_query(program: &str, args: &[&str]) -> Result<Vec<u16>> {
    match run_command(program, args) {
        Ok(output) => Ok(parse_ports(&output)),
        Err(_) => Ok(Vec::new()),
    }
}

fn parse_ports(output: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    for line in output.lines() {
        if let Some(port) = parse_port_from_line(line) {
            ports.push(port);
        }
    }
    ports
}

fn parse_port_from_line(line: &str) -> Option<u16> {
    for token in line.split_whitespace() {
        if let Some(port) = token
            .strip_prefix("127.0.0.1:")
            .or_else(|| token.strip_prefix("localhost:"))
            .or_else(|| token.strip_prefix("*:"))
            .or_else(|| token.strip_prefix("::1:"))
        {
            let cleaned = port.trim_end_matches("(LISTEN)").trim_end_matches(',');
            if let Ok(parsed) = cleaned.parse::<u16>() {
                return Some(parsed);
            }
        }
    }

    if let Some(idx) = line.rfind(':') {
        let rest = line[idx + 1..].trim();
        let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return digits.parse::<u16>().ok();
        }
    }

    None
}

fn probe_heartbeat(port: u16, csrf_token: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };

    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    let body = r#"{"uuid":"00000000-0000-0000-0000-000000000000"}"#;
    let request = format!(
        "POST /exa.language_server_pb.LanguageServerService/Heartbeat HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnect-Protocol-Version: 1\r\nX-Codeium-Csrf-Token: {}\r\nConnection: close\r\n\r\n{}",
        port,
        body.len(),
        csrf_token,
        body
    );

    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    if reader.read_line(&mut status_line).is_err() {
        return false;
    }

    let status_ok = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .is_some_and(|status| status == 200);
    if !status_ok {
        return false;
    }

    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() {
            return false;
        }
        if header.trim().is_empty() {
            break;
        }
    }

    let mut buffer = String::new();
    let _ = reader.by_ref().take(4096).read_to_string(&mut buffer);

    if !heartbeat_response_looks_well_formed(&buffer) {
        return false;
    }

    probe_endpoint_identity(port, csrf_token)
}

fn heartbeat_response_looks_well_formed(body: &str) -> bool {
    let trimmed = body.trim_start();
    let json_start = trimmed.find(['{', '[']).map(|idx| &trimmed[idx..]);
    let Some(slice) = json_start else {
        return false;
    };
    serde_json::from_str::<Value>(slice).is_ok()
}

fn probe_endpoint_identity(port: u16, csrf_token: &str) -> bool {
    for method in [
        "GetCascadeTrajectoryGeneratorMetadata",
        "GetAllCascadeTrajectories",
    ] {
        if let Some(body) = identity_probe_request(port, csrf_token, method) {
            if response_contains_antigravity_marker(&body) {
                return true;
            }
        }
    }
    false
}

fn identity_probe_request(port: u16, csrf_token: &str, method: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    let body = r#"{}"#;
    let request = format!(
        "POST /exa.language_server_pb.LanguageServerService/{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnect-Protocol-Version: 1\r\nX-Codeium-Csrf-Token: {}\r\nConnection: close\r\n\r\n{}",
        method,
        port,
        body.len(),
        csrf_token,
        body
    );

    if stream.write_all(request.as_bytes()).is_err() {
        return None;
    }

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    if reader.read_line(&mut status_line).is_err() {
        return None;
    }

    let status_ok = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .is_some_and(|status| status == 200);

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() {
            return None;
        }
        let trimmed = header.trim();
        if trimmed.is_empty() {
            break;
        }

        let lower = trimmed.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse::<usize>().ok();
        }
        if lower.contains("transfer-encoding") && lower.contains("chunked") {
            chunked = true;
        }
    }

    if !status_ok {
        return None;
    }

    if chunked {
        return read_chunked_body_prefix(&mut reader, 4096).ok();
    }

    if let Some(length) = content_length {
        let read_length = length.min(4096);
        let mut bytes = vec![0_u8; read_length];
        reader.read_exact(&mut bytes).ok()?;
        return String::from_utf8(bytes).ok();
    }

    let mut buffer = String::new();
    reader
        .by_ref()
        .take(4096)
        .read_to_string(&mut buffer)
        .ok()?;
    Some(buffer)
}

fn response_contains_antigravity_marker(body: &str) -> bool {
    let trimmed = body.trim_start();
    let json_start = trimmed.find(['{', '[']);
    let Some(idx) = json_start else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&trimmed[idx..]) else {
        return prefix_contains_antigravity_marker(&trimmed[idx..]);
    };
    contains_antigravity_marker(&value)
}

fn prefix_contains_antigravity_marker(body: &str) -> bool {
    let trimmed = body.trim_start();
    if !trimmed.starts_with(['{', '[']) {
        return false;
    }

    [
        "\"cascadeId\"",
        "\"cascadeTrajectories\"",
        "\"trajectorySummaries\"",
        "\"generatorMetadata\"",
        "\"serverInfo\"",
        "\"serverCapabilities\"",
    ]
    .iter()
    .any(|marker| {
        trimmed
            .split(marker)
            .skip(1)
            .any(|suffix| suffix.trim_start().starts_with(':'))
    })
}

fn contains_antigravity_marker(value: &Value) -> bool {
    const MARKERS: &[&str] = &[
        "cascadeId",
        "cascadeTrajectories",
        "trajectorySummaries",
        "generatorMetadata",
        "serverInfo",
        "serverCapabilities",
    ];
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                if MARKERS.iter().any(|m| m.eq_ignore_ascii_case(key)) {
                    return true;
                }
                if contains_antigravity_marker(val) {
                    return true;
                }
            }
            false
        }
        Value::Array(items) => items.iter().any(contains_antigravity_marker),
        _ => false,
    }
}

fn read_chunked_body_prefix(
    reader: &mut BufReader<TcpStream>,
    max_body_bytes: usize,
) -> Result<String> {
    let mut body = Vec::new();
    while body.len() < max_body_bytes {
        let mut size_line = String::new();
        reader.read_line(&mut size_line)?;
        let chunk_size = parse_chunk_size_line(&size_line)?;
        if chunk_size == 0 {
            break;
        }

        let remaining = max_body_bytes - body.len();
        let read_size = chunk_size.min(remaining);
        let mut chunk = vec![0_u8; read_size];
        reader.read_exact(&mut chunk)?;
        body.extend_from_slice(&chunk);

        if read_size < chunk_size {
            break;
        }

        let mut crlf = [0_u8; 2];
        reader.read_exact(&mut crlf)?;
    }

    Ok(String::from_utf8(body)?)
}

fn read_chunked_body_with_cap(
    reader: &mut BufReader<TcpStream>,
    max_body_bytes: usize,
) -> Result<String> {
    let mut body = Vec::new();
    loop {
        let mut size_line = String::new();
        reader.read_line(&mut size_line)?;
        let chunk_size = parse_chunk_size_line(&size_line)?;
        if chunk_size == 0 {
            break;
        }

        if chunk_size > max_body_bytes || body.len().saturating_add(chunk_size) > max_body_bytes {
            anyhow::bail!(
                "RPC body of {} bytes exceeds {} cap",
                body.len().saturating_add(chunk_size),
                max_body_bytes
            );
        }

        let mut chunk = vec![0_u8; chunk_size];
        reader.read_exact(&mut chunk)?;
        body.extend_from_slice(&chunk);

        let mut crlf = [0_u8; 2];
        reader.read_exact(&mut crlf)?;
    }

    Ok(String::from_utf8(body)?)
}

fn parse_chunk_size_line(size_line: &str) -> Result<usize> {
    let trimmed = size_line.trim();
    let chunk_size = trimmed
        .split(';')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Missing chunk size"))?;

    usize::from_str_radix(chunk_size, 16).map_err(|e| anyhow::anyhow!("Invalid chunk size: {}", e))
}

fn run_command(program: &str, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new(program).args(args).output()?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn rpc_request(connection: &AntigravityConnection, method: &str, body: &Value) -> Result<Value> {
    let mut stream = TcpStream::connect(("127.0.0.1", connection.port))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let body_text = serde_json::to_string(body)?;
    let request = format!(
        "POST /exa.language_server_pb.LanguageServerService/{} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnect-Protocol-Version: 1\r\nX-Codeium-Csrf-Token: {}\r\nConnection: close\r\n\r\n{}",
        method,
        connection.port,
        body_text.len(),
        connection.csrf_token,
        body_text
    );

    stream.write_all(request.as_bytes())?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader.read_line(&mut status_line)?;

    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| anyhow::anyhow!("Malformed HTTP response from Antigravity RPC"))?;

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        let trimmed = header.trim();
        if trimmed.is_empty() {
            break;
        }

        let lower = trimmed.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse::<usize>().ok();
        }
        if lower.contains("transfer-encoding") && lower.contains("chunked") {
            chunked = true;
        }
    }

    let response_body = if chunked {
        read_chunked_body_with_cap(&mut reader, 16 * 1024 * 1024)?
    } else if let Some(length) = content_length {
        if length > 16 * 1024 * 1024 {
            anyhow::bail!("RPC body of {length} bytes exceeds 16MB cap");
        }
        let mut bytes = vec![0_u8; length];
        reader.read_exact(&mut bytes)?;
        String::from_utf8(bytes)?
    } else {
        let mut text = String::new();
        reader
            .by_ref()
            .take(16 * 1024 * 1024 + 1)
            .read_to_string(&mut text)?;
        text
    };

    if status_code != 200 {
        return Err(anyhow::anyhow!(
            "RPC {} failed with status {}: {}",
            method,
            status_code,
            response_body
        ));
    }

    let trimmed = response_body.trim_start();
    let json_start = trimmed.find(['{', '[']).unwrap_or(0);
    Ok(serde_json::from_str(&trimmed[json_start..])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_file_antigravity() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test_session.jsonl");

        let log_content = r#"
{"type": "session_meta", "sessionId": "sess-123", "modelId": "antigravity-gemini-3-pro", "timestamp": 1672531200000}
{"type": "usage", "sessionId": "sess-123", "modelId": "antigravity-gemini-3-pro", "timestamp": 1672531201000, "input": 150, "output": 50, "cacheRead": 20, "cacheWrite": 0, "reasoning": 10, "responseId": "resp-456"}
"#;

        std::fs::write(&file_path, log_content).unwrap();

        let parser = AntigravitySessionParser::new();
        let messages = parser.parse_file(file_path);

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.session_id, "sess-123");
        assert_eq!(msg.model_id, "antigravity-gemini-3-pro");
        assert_eq!(msg.provider_id, "google");
        assert_eq!(msg.message_key, "resp-456");
        assert_eq!(msg.timestamp, 1672531201000);
        assert_eq!(msg.tokens.input, 150);
        assert_eq!(msg.tokens.output, 50);
        assert_eq!(msg.tokens.cache_read, 20);
        assert_eq!(msg.tokens.cache_write, 0);
        assert_eq!(msg.tokens.reasoning, 10);
    }

    #[test]
    fn test_parse_file_antigravity_deduplication() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test_session.jsonl");

        let log_content = r#"
{"type": "session_meta", "sessionId": "sess-123", "modelId": "antigravity-gemini-3-pro", "timestamp": 1672531200000}
{"type": "usage", "sessionId": "sess-123", "modelId": "antigravity-gemini-3-pro", "timestamp": 1672531201000, "input": 150, "output": 50, "cacheRead": 20, "cacheWrite": 0, "reasoning": 10, "responseId": "resp-456"}
{"type": "usage", "sessionId": "sess-123", "modelId": "antigravity-gemini-3-pro", "timestamp": 1672531202000, "input": 200, "output": 60, "cacheRead": 30, "cacheWrite": 0, "reasoning": 15, "responseId": "resp-456"}
"#;

        std::fs::write(&file_path, log_content).unwrap();

        let parser = AntigravitySessionParser::new();
        let messages = parser.parse_file(file_path);

        assert_eq!(messages.len(), 1);
        let msg = &messages[0];
        assert_eq!(msg.session_id, "sess-123");
        assert_eq!(msg.message_key, "resp-456");
        assert_eq!(msg.tokens.input, 200);
        assert_eq!(msg.tokens.output, 60);
    }

    #[test]
    fn test_parse_file_resolves_antigravity_model_aliases() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test_session.jsonl");

        let log_content = r#"
{"type": "usage", "sessionId": "sess-1", "modelId": "MODEL_PLACEHOLDER_M26", "timestamp": 1672531201000, "input": 150, "output": 50}
{"type": "usage", "sessionId": "sess-2", "modelId": "gemini-3-flash-a", "timestamp": 1672531202000, "input": 200, "output": 60}
{"type": "usage", "sessionId": "sess-3", "modelId": "claude-opus-4.6-thinking", "timestamp": 1672531203000, "input": 300, "output": 70}
"#;

        std::fs::write(&file_path, log_content).unwrap();

        let parser = AntigravitySessionParser::new();
        let messages = parser.parse_file(file_path);

        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].model_id, "claude-opus-4-6-thinking");
        assert_eq!(messages[0].provider_id, "anthropic");
        assert_eq!(messages[1].model_id, "gemini-3.5-flash");
        assert_eq!(messages[1].provider_id, "google");
        assert_eq!(messages[2].model_id, "claude-opus-4-6-thinking");
        assert_eq!(messages[2].provider_id, "anthropic");
    }

    #[test]
    fn test_normalize_cached_artifact_rewrites_known_model_aliases() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("test_session.jsonl");

        std::fs::write(
            &file_path,
            r#"{"type":"session_meta","sessionId":"sess-1","modelId":"MODEL_PLACEHOLDER_M37","timestamp":1672531200000}
{"type":"usage","sessionId":"sess-1","modelId":"gemini-3-flash-a","timestamp":1672531201000,"input":1,"output":2}
"#,
        )
        .unwrap();

        normalize_cached_antigravity_artifact(&file_path, &HashMap::new()).unwrap();

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert!(content.contains(r#""modelId":"gemini-3.1-pro-high""#));
        assert!(content.contains(r#""modelId":"gemini-3.5-flash""#));
        assert!(!content.contains("MODEL_PLACEHOLDER_M37"));
        assert!(!content.contains("gemini-3-flash-a"));
    }

    #[test]
    fn test_dynamic_model_aliases_are_saved_and_reused() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let mut dynamic_aliases = HashMap::new();
        dynamic_aliases.insert(
            normalize_alias_key("MODEL_PLACEHOLDER_M132"),
            ModelAlias {
                raw_model_id: "MODEL_PLACEHOLDER_M132".to_string(),
                model_id: "gemini-3.5-flash-high".to_string(),
                label: Some("Gemini 3.5 Flash (High)".to_string()),
                source: "antigravity-get-user-status".to_string(),
            },
        );

        let saved = merge_and_save_model_alias_history(&sessions_dir, &dynamic_aliases).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M132", &saved),
            "gemini-3.5-flash-high"
        );

        let loaded = load_model_alias_history_map(&sessions_dir).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M132", &loaded),
            "gemini-3.5-flash-high"
        );

        let history_path = model_alias_history_path(&sessions_dir).unwrap();
        let content = std::fs::read_to_string(history_path).unwrap();
        assert!(content.contains("MODEL_PLACEHOLDER_M132"));
        assert!(content.contains("Gemini 3.5 Flash (High)"));
        assert!(content.contains("firstSeenAt"));
        assert!(content.contains("lastSeenAt"));
    }

    #[test]
    fn test_model_alias_history_is_seeded_from_static_sources() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let saved = merge_and_save_model_alias_history(&sessions_dir, &HashMap::new()).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M26", &saved),
            "claude-opus-4-6-thinking"
        );
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M37", &saved),
            "gemini-3.1-pro-high"
        );

        let history_path = model_alias_history_path(&sessions_dir).unwrap();
        let content = std::fs::read_to_string(history_path).unwrap();
        assert!(content.contains("MODEL_PLACEHOLDER_M26"));
        assert!(content.contains("captured-antigravity-get-user-status;tokscale"));
    }

    #[test]
    fn test_dynamic_model_aliases_override_static_seed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let sessions_dir = temp_dir.path().join("antigravity-cache").join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let mut dynamic_aliases = HashMap::new();
        dynamic_aliases.insert(
            normalize_alias_key("MODEL_PLACEHOLDER_M37"),
            ModelAlias {
                raw_model_id: "MODEL_PLACEHOLDER_M37".to_string(),
                model_id: "gemini-3.1-pro-thinking-high".to_string(),
                label: Some("Gemini 3.1 Pro Thinking High".to_string()),
                source: "antigravity-get-user-status".to_string(),
            },
        );

        let saved = merge_and_save_model_alias_history(&sessions_dir, &dynamic_aliases).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M37", &saved),
            "gemini-3.1-pro-thinking-high"
        );

        let loaded = load_model_alias_history_map(&sessions_dir).unwrap();
        assert_eq!(
            resolve_antigravity_model_id_with_aliases("MODEL_PLACEHOLDER_M37", &loaded),
            "gemini-3.1-pro-thinking-high"
        );
    }

    #[test]
    fn test_antigravity_label_to_model_id_handles_future_label_shapes() {
        assert_eq!(
            antigravity_label_to_model_id("Claude Opus 4.5 (Thinking)").as_deref(),
            Some("claude-opus-4-5-thinking")
        );
        assert_eq!(
            antigravity_label_to_model_id("Claude Sonnet 4.5 (Thinking)").as_deref(),
            Some("claude-sonnet-4-5-thinking")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3.0 Pro Preview (High)").as_deref(),
            Some("gemini-3-pro-preview-high")
        );
        assert_eq!(
            antigravity_label_to_model_id("Gemini 3 Pro Preview (Low)").as_deref(),
            Some("gemini-3-pro-preview-low")
        );
        assert_eq!(antigravity_label_to_model_id("Unknown Beta"), None);
    }

    #[test]
    fn test_discover_local_conversation_ids() {
        let temp_dir = tempfile::tempdir().unwrap();
        let gemini_dir = temp_dir.path().join(".gemini");
        let anti_dir = gemini_dir.join("antigravity").join("conversations");
        let cli_dir = gemini_dir.join("antigravity-cli").join("conversations");

        std::fs::create_dir_all(&anti_dir).unwrap();
        std::fs::create_dir_all(&cli_dir).unwrap();

        let uuid_1 = "12345678901234567890.pb";
        let uuid_2 = "abcdefabcdefabcdefabcdef.db";
        let non_uuid = "short.pb";
        let txt_file = "12345678901234567890.txt";

        std::fs::write(anti_dir.join(uuid_1), "mock").unwrap();
        std::fs::write(cli_dir.join(uuid_2), "mock").unwrap();
        std::fs::write(anti_dir.join(non_uuid), "mock").unwrap();
        std::fs::write(cli_dir.join(txt_file), "mock").unwrap();

        let discovered = discover_local_conversation_ids_from_home(temp_dir.path());

        assert_eq!(discovered.len(), 2);
        assert!(discovered.contains(&"12345678901234567890".to_string()));
        assert!(discovered.contains(&"abcdefabcdefabcdefabcdef".to_string()));
    }
}
