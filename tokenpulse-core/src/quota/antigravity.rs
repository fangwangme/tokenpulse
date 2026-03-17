use crate::auth::antigravity::AntigravityAuth;
use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;
use tracing::{debug, info, warn};

const REQUEST_TIMEOUT_SECS: u64 = 20;
const LS_PROBE_TIMEOUT_SECS: u64 = 3;

// Language Server process discovery
const LS_PROCESS_NAME: &str = "language_server_macos";
const LS_MARKER: &str = "antigravity";

// Connect-RPC service
const LS_SERVICE: &str = "exa.language_server_pb.LanguageServerService";

// Cloud Code API (fallback)
const CLOUD_CODE_URLS: &[&str] = &[
    "https://daily-cloudcode-pa.googleapis.com",
    "https://cloudcode-pa.googleapis.com",
];
const FETCH_MODELS_PATH: &str = "/v1internal:fetchAvailableModels";

const CC_MODEL_BLACKLIST: &[&str] = &[
    "MODEL_CHAT_20706",
    "MODEL_CHAT_23310",
    "MODEL_GOOGLE_GEMINI_2_5_FLASH",
    "MODEL_GOOGLE_GEMINI_2_5_FLASH_THINKING",
    "MODEL_GOOGLE_GEMINI_2_5_FLASH_LITE",
    "MODEL_GOOGLE_GEMINI_2_5_PRO",
    "MODEL_PLACEHOLDER_M19",
    "MODEL_PLACEHOLDER_M9",
    "MODEL_PLACEHOLDER_M12",
];

// ── LS Discovery types ──

#[derive(Debug)]
struct LsDiscovery {
    pid: i32,
    csrf_token: String,
    ports: Vec<u16>,
    extension_port: Option<u16>,
}

// ── LS RPC response types ──

#[derive(Debug, Deserialize)]
struct LsUserStatusResponse {
    #[serde(default, rename = "cascadeModelConfigData")]
    cascade_model_config_data: Option<CascadeModelConfigData>,
    #[serde(default, rename = "userStatus")]
    user_status: Option<UserStatus>,
}

#[derive(Debug, Deserialize)]
struct CascadeModelConfigData {
    #[serde(default, rename = "clientModelConfigs")]
    client_model_configs: Vec<ClientModelConfig>,
}

#[derive(Debug, Deserialize)]
struct ClientModelConfig {
    #[serde(default)]
    label: Option<String>,
    #[serde(default, rename = "modelOrAlias")]
    model_or_alias: Option<ModelOrAlias>,
    #[serde(default, rename = "quotaInfo")]
    quota_info: Option<LsQuotaInfo>,
}

#[derive(Debug, Deserialize)]
struct ModelOrAlias {
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LsQuotaInfo {
    #[serde(default, rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(default, rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserStatus {
    #[serde(default, rename = "planStatus")]
    plan_status: Option<PlanStatus>,
}

#[derive(Debug, Deserialize)]
struct PlanStatus {
    #[serde(default, rename = "planInfo")]
    plan_info: Option<PlanInfo>,
}

#[derive(Debug, Deserialize)]
struct PlanInfo {
    #[serde(default, rename = "planName")]
    plan_name: Option<String>,
}

// ── Cloud Code API response types ──

#[derive(Debug, Deserialize)]
struct FetchModelsResponse {
    #[serde(default)]
    models: Option<HashMap<String, CloudModelInfo>>,
}

#[derive(Debug, Deserialize)]
struct CloudModelInfo {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    is_internal: Option<bool>,
    #[serde(default)]
    quota_info: Option<CloudQuotaInfo>,
}

#[derive(Debug, Deserialize)]
struct CloudQuotaInfo {
    #[serde(default)]
    remaining_fraction: Option<f64>,
    #[serde(default)]
    reset_time: Option<String>,
}

// ── Unified pool data ──

struct PoolData {
    pools: HashMap<String, (f64, Option<String>)>,
    plan: Option<String>,
}

// ── Main fetcher ──

pub struct AntigravityQuotaFetcher {
    client: Client,
    ls_client: Client,
    auth: AntigravityAuth,
}

impl AntigravityQuotaFetcher {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| Client::new());
        let ls_client = Client::builder()
            .timeout(Duration::from_secs(LS_PROBE_TIMEOUT_SECS))
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            ls_client,
            auth: AntigravityAuth::new(),
        }
    }

    // ── Layer 1: Language Server Discovery ──

    fn discover_ls(&self) -> Option<LsDiscovery> {
        debug!("Discovering Antigravity Language Server...");

        let output = Command::new("ps")
            .args(["-ax", "-o", "pid=,command="])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            let line = line.trim();
            if !line.contains(LS_PROCESS_NAME) {
                continue;
            }
            // Verify it belongs to Antigravity
            let line_lower = line.to_lowercase();
            if !line_lower.contains(LS_MARKER) {
                continue;
            }

            // Extract PID
            let pid: i32 = match line.split_whitespace().next().and_then(|s| s.parse().ok()) {
                Some(p) => p,
                None => continue,
            };

            // Extract CSRF token
            let csrf_token = match extract_flag(line, "--csrf_token") {
                Some(t) => t,
                None => {
                    debug!("Found LS process {} but no CSRF token", pid);
                    continue;
                }
            };

            // Extract extension port (optional)
            let extension_port = extract_flag(line, "--extension_server_port")
                .and_then(|p| p.parse::<u16>().ok());

            // Find listening ports via lsof
            let ports = find_listening_ports(pid);

            if ports.is_empty() && extension_port.is_none() {
                debug!("Found LS process {} but no listening ports", pid);
                continue;
            }

            info!("Discovered Antigravity LS: pid={}, ports={:?}, ext_port={:?}", pid, ports, extension_port);

            return Some(LsDiscovery {
                pid,
                csrf_token,
                ports,
                extension_port,
            });
        }

        debug!("No Antigravity Language Server found");
        None
    }

    async fn probe_ls(&self, discovery: &LsDiscovery) -> Option<PoolData> {
        // Try all discovered ports, find a working one
        let working = self.find_working_port(discovery).await?;

        debug!("Using LS at {}:{}", working.0, working.1);

        // Call GetUserStatus
        let url = format!(
            "{}://127.0.0.1:{}/{}/GetUserStatus",
            working.0, working.1, LS_SERVICE
        );

        // Load API key from SQLite for the request (optional)
        let api_key = self.auth.load_credentials().ok()
            .and_then(|c| c.api_key);

        let mut metadata = serde_json::json!({
            "ideName": "antigravity",
            "extensionName": "antigravity",
            "extensionVersion": "1.0.0",
        });
        if let Some(ref key) = api_key {
            metadata["apiKey"] = serde_json::Value::String(key.clone());
        }

        let body = serde_json::json!({ "metadata": metadata });

        let response = self.ls_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .header("x-codeium-csrf-token", &discovery.csrf_token)
            .json(&body)
            .send()
            .await
            .ok()?;

        if !response.status().is_success() {
            debug!("GetUserStatus failed: {}", response.status());
            // Try GetCommandModelConfigs as fallback
            return self.probe_ls_model_configs(&working, discovery).await;
        }

        let data: LsUserStatusResponse = response.json().await.ok()?;
        self.parse_ls_response(data)
    }

    async fn probe_ls_model_configs(
        &self,
        working: &(&str, u16),
        discovery: &LsDiscovery,
    ) -> Option<PoolData> {
        let url = format!(
            "{}://127.0.0.1:{}/{}/GetCommandModelConfigs",
            working.0, working.1, LS_SERVICE
        );

        let body = serde_json::json!({
            "metadata": {
                "ideName": "antigravity",
                "extensionName": "antigravity",
            }
        });

        let response = self.ls_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .header("x-codeium-csrf-token", &discovery.csrf_token)
            .json(&body)
            .send()
            .await
            .ok()?;

        if !response.status().is_success() {
            debug!("GetCommandModelConfigs failed: {}", response.status());
            return None;
        }

        let data: LsUserStatusResponse = response.json().await.ok()?;
        self.parse_ls_response(data)
    }

    async fn find_working_port<'a>(&self, discovery: &'a LsDiscovery) -> Option<(&'static str, u16)> {
        // Try all discovered ports: HTTPS first, then HTTP
        for &port in &discovery.ports {
            for scheme in &["https", "http"] {
                let url = format!(
                    "{}://127.0.0.1:{}/{}/GetUnleashData",
                    scheme, port, LS_SERVICE
                );
                let result = self.ls_client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Connect-Protocol-Version", "1")
                    .header("x-codeium-csrf-token", &discovery.csrf_token)
                    .json(&serde_json::json!({}))
                    .send()
                    .await;

                match result {
                    Ok(_) => {
                        // Any response means port is alive
                        debug!("LS port alive: {}:{}", scheme, port);
                        let scheme_str: &'static str = if *scheme == "https" { "https" } else { "http" };
                        return Some((scheme_str, port));
                    }
                    Err(e) => {
                        debug!("LS probe {}:{} failed: {}", scheme, port, e);
                    }
                }
            }
        }

        // Try extension port as fallback
        if let Some(ext_port) = discovery.extension_port {
            for scheme in &["https", "http"] {
                let url = format!(
                    "{}://127.0.0.1:{}/{}/GetUnleashData",
                    scheme, ext_port, LS_SERVICE
                );
                if self.ls_client
                    .post(&url)
                    .header("Content-Type", "application/json")
                    .header("Connect-Protocol-Version", "1")
                    .header("x-codeium-csrf-token", &discovery.csrf_token)
                    .json(&serde_json::json!({}))
                    .send()
                    .await
                    .is_ok()
                {
                    let scheme_str: &'static str = if *scheme == "https" { "https" } else { "http" };
                    return Some((scheme_str, ext_port));
                }
            }
        }

        None
    }

    fn parse_ls_response(&self, data: LsUserStatusResponse) -> Option<PoolData> {
        let configs = data.cascade_model_config_data?.client_model_configs;
        if configs.is_empty() {
            return None;
        }

        let plan = data.user_status
            .and_then(|u| u.plan_status)
            .and_then(|p| p.plan_info)
            .and_then(|i| i.plan_name);

        let mut pools: HashMap<String, (f64, Option<String>)> = HashMap::new();

        for config in &configs {
            let label = config.label.as_deref().unwrap_or("");
            let model = config.model_or_alias.as_ref()
                .and_then(|m| m.model.as_deref())
                .unwrap_or("");

            // Skip if no quota info
            let quota = match &config.quota_info {
                Some(q) => q,
                None => continue,
            };

            let frac = quota.remaining_fraction.unwrap_or(1.0);
            let reset_time = quota.reset_time.clone();

            // Determine pool from label or model name
            let pool_name = self.pool_label_from_ls(label, model);

            let entry = pools.entry(pool_name).or_insert((frac, reset_time.clone()));
            if frac < entry.0 {
                *entry = (frac, reset_time);
            }
        }

        if pools.is_empty() {
            return None;
        }

        Some(PoolData { pools, plan })
    }

    fn pool_label_from_ls(&self, label: &str, model: &str) -> String {
        let combined = format!("{} {}", label, model).to_lowercase();
        if combined.contains("gemini") && combined.contains("pro") {
            "Gemini Pro".to_string()
        } else if combined.contains("gemini") && combined.contains("flash") {
            "Gemini Flash".to_string()
        } else {
            "Claude".to_string()
        }
    }

    // ── Layer 2: Cloud Code API (fallback) ──

    async fn fetch_via_cloud_api(&self) -> Result<PoolData> {
        let creds = self.auth.load_credentials()?;

        let mut tokens_to_try: Vec<String> = Vec::new();
        if let Some(ref access) = creds.access_token {
            tokens_to_try.push(access.clone());
        }
        if let Some(ref api_key) = creds.api_key {
            if !tokens_to_try.contains(api_key) {
                tokens_to_try.push(api_key.clone());
            }
        }

        if tokens_to_try.is_empty() {
            return Err(anyhow!("No Antigravity credentials found. Please open Antigravity to login."));
        }

        for token in &tokens_to_try {
            for base_url in CLOUD_CODE_URLS {
                let url = format!("{}{}", base_url, FETCH_MODELS_PATH);

                let response = match self.client
                    .post(&url)
                    .bearer_auth(token)
                    .header("Content-Type", "application/json")
                    .header("User-Agent", "antigravity")
                    .json(&serde_json::json!({}))
                    .send()
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        debug!("Cloud Code {} failed: {}", base_url, e);
                        continue;
                    }
                };

                let status = response.status().as_u16();
                debug!("Cloud Code {} status: {}", base_url, status);

                if status == 401 || status == 403 {
                    continue;
                }

                if !response.status().is_success() {
                    continue;
                }

                let data: FetchModelsResponse = response.json().await?;
                let pools = self.parse_cloud_response(data);
                if !pools.is_empty() {
                    return Ok(PoolData { pools, plan: None });
                }
            }
        }

        Err(anyhow!("Antigravity session expired. Please open Antigravity to refresh your session."))
    }

    fn parse_cloud_response(&self, data: FetchModelsResponse) -> HashMap<String, (f64, Option<String>)> {
        let mut pools: HashMap<String, (f64, Option<String>)> = HashMap::new();

        let models = match data.models {
            Some(m) => m,
            None => return pools,
        };

        for (model_id, info) in models {
            if CC_MODEL_BLACKLIST.contains(&model_id.as_str()) {
                continue;
            }
            if info.is_internal.unwrap_or(false) {
                continue;
            }
            let display_name = match info.display_name {
                Some(ref n) => n.clone(),
                None => continue,
            };

            let normalized = display_name.replace(|c| c == '(' || c == ')', "").trim().to_string();
            let pool = self.pool_label_from_ls(&normalized, &model_id);

            let frac = info.quota_info.as_ref().and_then(|q| q.remaining_fraction).unwrap_or(0.0);
            let reset_time = info.quota_info.as_ref().and_then(|q| q.reset_time.clone());

            let entry = pools.entry(pool).or_insert((frac, reset_time.clone()));
            if frac < entry.0 {
                *entry = (frac, reset_time);
            }
        }

        pools
    }

    // ── Common: convert pool data to QuotaSnapshot ──

    fn pools_to_snapshot(&self, pool_data: PoolData) -> QuotaSnapshot {
        let mut windows = Vec::new();

        let mut sorted_pools: Vec<_> = pool_data.pools.into_iter().collect();
        sorted_pools.sort_by(|a, b| {
            let key = |name: &str| -> &str {
                if name.contains("Pro") { "0" }
                else if name.contains("Flash") { "1" }
                else { "2" }
            };
            key(&a.0).cmp(key(&b.0))
        });

        for (pool, (frac, reset_time)) in sorted_pools {
            let used = ((1.0 - frac.clamp(0.0, 1.0)) * 100.0).round();
            let resets_at = reset_time.and_then(|s| {
                DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc))
            });

            windows.push(RateWindow {
                label: pool,
                used_percent: used,
                resets_at,
                period_duration_ms: Some(5 * 60 * 60 * 1000),
            });
        }

        if windows.is_empty() {
            windows.push(RateWindow {
                label: "Usage".to_string(),
                used_percent: 0.0,
                resets_at: None,
                period_duration_ms: None,
            });
        }

        QuotaSnapshot {
            provider: "antigravity".to_string(),
            plan: pool_data.plan,
            windows,
            credits: None,
            fetched_at: Utc::now(),
        }
    }
}

impl Default for AntigravityQuotaFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QuotaFetcher for AntigravityQuotaFetcher {
    fn provider_name(&self) -> &str {
        "antigravity"
    }

    fn provider_display_name(&self) -> &str {
        "Antigravity"
    }

    async fn fetch_quota(&self) -> Result<QuotaSnapshot> {
        // Layer 1: Try Language Server discovery (local, zero network cost)
        if let Some(discovery) = self.discover_ls() {
            if let Some(pool_data) = self.probe_ls(&discovery).await {
                info!("Antigravity quota fetched via Language Server (pid={})", discovery.pid);
                return Ok(self.pools_to_snapshot(pool_data));
            }
            warn!("LS discovered but probe failed, falling back to Cloud API");
        }

        // Layer 2: Fallback to Cloud Code API
        debug!("Falling back to Cloud Code API for Antigravity quota");
        let pool_data = self.fetch_via_cloud_api().await?;
        info!("Antigravity quota fetched via Cloud Code API");
        Ok(self.pools_to_snapshot(pool_data))
    }
}

// ── Helper functions ──

fn extract_flag(command_line: &str, flag: &str) -> Option<String> {
    // Handle --flag=value
    let eq_prefix = format!("{}=", flag);
    if let Some(pos) = command_line.find(&eq_prefix) {
        let start = pos + eq_prefix.len();
        let rest = &command_line[start..];
        let value = rest.split_whitespace().next().unwrap_or("");
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    // Handle --flag value
    let parts: Vec<&str> = command_line.split_whitespace().collect();
    for i in 0..parts.len().saturating_sub(1) {
        if parts[i] == flag {
            return Some(parts[i + 1].to_string());
        }
    }

    None
}

fn find_listening_ports(pid: i32) -> Vec<u16> {
    let output = Command::new("lsof")
        .args([
            "-nP",
            "-iTCP",
            "-sTCP:LISTEN",
            "-a",
            "-p",
            &pid.to_string(),
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut ports = std::collections::BTreeSet::new();

    for line in stdout.lines().skip(1) {
        // lsof output: COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME
        // NAME is like "127.0.0.1:42150" or "*:42150"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if let Some(name) = parts.last() {
            if let Some(colon_pos) = name.rfind(':') {
                if let Ok(port) = name[colon_pos + 1..].parse::<u16>() {
                    ports.insert(port);
                }
            }
        }
    }

    ports.into_iter().collect()
}
