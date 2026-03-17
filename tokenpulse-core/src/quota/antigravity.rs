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
const LOAD_CODE_ASSIST_PATH: &str = "/v1internal:loadCodeAssist";
const ONBOARD_USER_PATH: &str = "/v1internal:onboardUser";
const FETCH_MODELS_PATH: &str = "/v1internal:fetchAvailableModels";
const GOOGLE_OAUTH_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const GOOGLE_CLIENT_SECRET: &str = "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";
const USER_AGENT: &str = "antigravity";

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
    #[serde(default, rename = "clientModelConfigs")]
    client_model_configs: Vec<ClientModelConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct CascadeModelConfigData {
    #[serde(default, rename = "clientModelConfigs")]
    client_model_configs: Vec<ClientModelConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct ClientModelConfig {
    #[serde(default)]
    label: Option<String>,
    #[serde(default, rename = "modelOrAlias")]
    model_or_alias: Option<ModelOrAlias>,
    #[serde(default, rename = "quotaInfo")]
    quota_info: Option<LsQuotaInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelOrAlias {
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
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
    #[serde(default, rename = "cascadeModelConfigData")]
    cascade_model_config_data: Option<CascadeModelConfigData>,
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
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
    #[serde(default, rename = "isInternal")]
    is_internal: Option<bool>,
    #[serde(default, rename = "quotaInfo")]
    quota_info: Option<CloudQuotaInfo>,
}

#[derive(Debug, Deserialize)]
struct CloudQuotaInfo {
    #[serde(default, rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(default, rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadCodeAssistResponse {
    #[serde(default, rename = "currentTier")]
    current_tier: Option<Tier>,
    #[serde(default, rename = "paidTier")]
    paid_tier: Option<Tier>,
    #[serde(default, rename = "allowedTiers")]
    allowed_tiers: Option<Vec<AllowedTier>>,
    #[serde(default, rename = "cloudaicompanionProject")]
    project: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct Tier {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AllowedTier {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "isDefault")]
    is_default: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct OnboardUserResponse {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    done: Option<bool>,
    #[serde(default)]
    response: Option<OnboardResponse>,
}

#[derive(Debug, Deserialize)]
struct OnboardResponse {
    #[serde(default, rename = "cloudaicompanionProject")]
    project: Option<serde_json::Value>,
}

// ── Unified pool data ──

#[derive(Debug, Clone)]
struct PoolQuota {
    remaining_fraction: f64,
    reset_time: Option<String>,
    period_duration_ms: i64,
}

struct PoolData {
    pools: HashMap<String, PoolQuota>,
    plan: Option<String>,
}

struct CloudCodeContext {
    project_id: Option<String>,
    plan: Option<String>,
    tier_id: Option<String>,
}

// ── Main fetcher ──

pub struct AntigravityQuotaFetcher {
    client: Client,
    ls_client: Client,
    auth: AntigravityAuth,
}

impl AntigravityQuotaFetcher {
    const FIVE_HOURS_MS: i64 = 5 * 60 * 60 * 1000;
    const SEVEN_DAYS_MS: i64 = 7 * 24 * 60 * 60 * 1000;

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
            "ideVersion": "unknown",
            "locale": "en",
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
                "ideVersion": "unknown",
                "locale": "en",
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
                    Ok(response) if response.status().is_success() => {
                        debug!("LS port alive: {}:{} ({})", scheme, port, response.status());
                        let scheme_str: &'static str = if *scheme == "https" { "https" } else { "http" };
                        return Some((scheme_str, port));
                    }
                    Ok(response) => {
                        debug!("LS probe {}:{} returned {}", scheme, port, response.status());
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
                    .map(|response| response.status().is_success())
                    .unwrap_or(false)
                {
                    let scheme_str: &'static str = if *scheme == "https" { "https" } else { "http" };
                    return Some((scheme_str, ext_port));
                }
            }
        }

        None
    }

    fn parse_ls_response(&self, data: LsUserStatusResponse) -> Option<PoolData> {
        let configs = data
            .user_status
            .as_ref()
            .and_then(|u| u.cascade_model_config_data.as_ref())
            .map(|c| c.client_model_configs.clone())
            .or_else(|| {
                data.cascade_model_config_data
                    .as_ref()
                    .map(|c| c.client_model_configs.clone())
            })
            .unwrap_or_else(|| data.client_model_configs.clone());
        if configs.is_empty() {
            return None;
        }

        let plan = data.user_status
            .and_then(|u| u.plan_status)
            .and_then(|p| p.plan_info)
            .and_then(|i| i.plan_name);

        let mut pools: HashMap<String, PoolQuota> = HashMap::new();

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
            let period_duration_ms = self.infer_period_duration_ms(reset_time.as_deref(), &pool_name);

            let entry = pools.entry(pool_name).or_insert(PoolQuota {
                remaining_fraction: frac,
                reset_time: reset_time.clone(),
                period_duration_ms,
            });
            if frac < entry.remaining_fraction {
                *entry = PoolQuota {
                    remaining_fraction: frac,
                    reset_time,
                    period_duration_ms,
                };
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

    fn pool_period_duration_ms(&self, pool_label: &str) -> i64 {
        if pool_label.to_lowercase().contains("flash") {
            Self::FIVE_HOURS_MS
        } else {
            Self::SEVEN_DAYS_MS
        }
    }

    fn infer_period_duration_ms(&self, reset_time: Option<&str>, pool_label: &str) -> i64 {
        let Some(reset_time) = reset_time else {
            return self.pool_period_duration_ms(pool_label);
        };

        let Some(reset_at) = DateTime::parse_from_rfc3339(reset_time)
            .ok()
            .map(|dt| dt.with_timezone(&Utc)) else {
            return self.pool_period_duration_ms(pool_label);
        };

        if reset_at.signed_duration_since(Utc::now()).num_milliseconds() <= Self::FIVE_HOURS_MS {
            Self::FIVE_HOURS_MS
        } else {
            Self::SEVEN_DAYS_MS
        }
    }

    // ── Layer 2: Cloud Code API (fallback) ──

    async fn refresh_google_access_token(&self, refresh_token: &str) -> Result<String> {
        let response = self.client
            .post(GOOGLE_OAUTH_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("client_id", GOOGLE_CLIENT_ID),
                ("client_secret", GOOGLE_CLIENT_SECRET),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Google OAuth refresh failed {}: {}", status, text));
        }

        let value: serde_json::Value = response.json().await?;
        value
            .get("access_token")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Google OAuth refresh response missing access_token"))
    }

    fn build_load_code_assist_payload(&self, project_id: Option<&str>) -> serde_json::Value {
        let mut payload = serde_json::json!({
            "metadata": {
                "ideType": "ANTIGRAVITY",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        });

        if let Some(project_id) = project_id.filter(|id| !id.trim().is_empty()) {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert(
                    "cloudaicompanionProject".to_string(),
                    serde_json::Value::String(project_id.to_string()),
                );
            }
        }

        payload
    }

    fn extract_project_id(&self, value: &serde_json::Value) -> Option<String> {
        if let Some(text) = value.as_str() {
            if !text.trim().is_empty() {
                return Some(text.to_string());
            }
        }
        if let Some(obj) = value.as_object() {
            for key in ["id", "projectId"] {
                if let Some(text) = obj.get(key).and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        return Some(text.to_string());
                    }
                }
            }
        }
        None
    }

    fn pick_onboard_tier(&self, allowed_tiers: &[AllowedTier]) -> Option<String> {
        if let Some(default) = allowed_tiers.iter().find(|tier| tier.is_default.unwrap_or(false)) {
            if let Some(id) = default.id.clone() {
                return Some(id);
            }
        }
        allowed_tiers.iter().find_map(|tier| tier.id.clone())
    }

    async fn load_cloud_code_context(
        &self,
        access_token: &str,
        base_url: &str,
    ) -> Result<CloudCodeContext> {
        let response = self.client
            .post(format!("{}{}", base_url, LOAD_CODE_ASSIST_PATH))
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("User-Agent", USER_AGENT)
            .json(&self.build_load_code_assist_payload(None))
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || response.status() == reqwest::StatusCode::FORBIDDEN
        {
            return Err(anyhow!("auth_failed"));
        }

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("loadCodeAssist failed {}: {}", status, text));
        }

        let data: LoadCodeAssistResponse = response.json().await?;
        let plan = data
            .paid_tier
            .as_ref()
            .and_then(|tier| tier.id.clone())
            .or_else(|| data.current_tier.as_ref().and_then(|tier| tier.id.clone()));
        let tier_id = data
            .paid_tier
            .as_ref()
            .and_then(|tier| tier.id.clone())
            .or_else(|| data.current_tier.as_ref().and_then(|tier| tier.id.clone()))
            .or_else(|| {
                data.allowed_tiers
                    .as_ref()
                    .and_then(|tiers| self.pick_onboard_tier(tiers))
            });
        let project_id = data.project.as_ref().and_then(|value| self.extract_project_id(value));

        Ok(CloudCodeContext {
            project_id,
            plan,
            tier_id,
        })
    }

    async fn try_onboard_user(
        &self,
        access_token: &str,
        base_url: &str,
        tier_id: &str,
        project_id: Option<&str>,
    ) -> Result<Option<String>> {
        let mut payload = serde_json::json!({
            "tierId": tier_id,
            "metadata": {
                "ideType": "ANTIGRAVITY",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        });
        if let Some(project_id) = project_id.filter(|id| !id.trim().is_empty()) {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert(
                    "cloudaicompanionProject".to_string(),
                    serde_json::Value::String(project_id.to_string()),
                );
            }
        }

        let response = self.client
            .post(format!("{}{}", base_url, ONBOARD_USER_PATH))
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("User-Agent", USER_AGENT)
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("onboardUser failed {}: {}", status, text));
        }

        let mut data: OnboardUserResponse = response.json().await?;
        loop {
            if data.done.unwrap_or(false) {
                return Ok(data
                    .response
                    .and_then(|resp| resp.project)
                    .as_ref()
                    .and_then(|value| self.extract_project_id(value)));
            }

            let op_name = data
                .name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("onboardUser missing operation name"))?;

            let response = self.client
                .get(format!("{}/v1internal/{}", base_url, op_name))
                .bearer_auth(access_token)
                .header("Content-Type", "application/json")
                .header("User-Agent", USER_AGENT)
                .send()
                .await?;

            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                return Err(anyhow!("onboardUser poll failed {}: {}", status, text));
            }

            data = response.json().await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn fetch_cloud_models_with_token(
        &self,
        access_token: &str,
    ) -> Result<Option<PoolData>> {
        for base_url in CLOUD_CODE_URLS {
            let mut context = match self.load_cloud_code_context(access_token, base_url).await {
                Ok(ctx) => ctx,
                Err(e) if e.to_string() == "auth_failed" => return Err(e),
                Err(e) => {
                    debug!("Cloud Code context failed {}: {}", base_url, e);
                    continue;
                }
            };

            if context.project_id.is_none() {
                if let Some(tier_id) = context.tier_id.clone() {
                    match self
                        .try_onboard_user(
                            access_token,
                            base_url,
                            &tier_id,
                            context.project_id.as_deref(),
                        )
                        .await
                    {
                        Ok(project_id) => context.project_id = project_id,
                        Err(e) => debug!("onboardUser failed {}: {}", base_url, e),
                    }
                }
            }

            let payload = context
                .project_id
                .as_ref()
                .map(|id| serde_json::json!({ "project": id }))
                .unwrap_or_else(|| serde_json::json!({}));

            let response = match self.client
                .post(format!("{}{}", base_url, FETCH_MODELS_PATH))
                .bearer_auth(access_token)
                .header("Content-Type", "application/json")
                .header("User-Agent", USER_AGENT)
                .json(&payload)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    debug!("Cloud Code fetchAvailableModels {} failed: {}", base_url, e);
                    continue;
                }
            };

            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                || response.status() == reqwest::StatusCode::FORBIDDEN
            {
                return Err(anyhow!("auth_failed"));
            }

            if !response.status().is_success() {
                debug!("Cloud Code fetchAvailableModels {} status {}", base_url, response.status());
                continue;
            }

            let data: FetchModelsResponse = response.json().await?;
            let pools = self.parse_cloud_response(data);
            if !pools.is_empty() {
                return Ok(Some(PoolData {
                    pools,
                    plan: context.plan,
                }));
            }
        }

        Ok(None)
    }

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
            match self.fetch_cloud_models_with_token(token).await {
                Ok(Some(pool_data)) => return Ok(pool_data),
                Ok(None) => {}
                Err(e) if e.to_string() == "auth_failed" => {}
                Err(e) => debug!("Cloud Code token attempt failed: {}", e),
            }
        }

        if let Some(refresh_token) = creds.refresh_token.as_deref() {
            let refreshed_access_token = self.refresh_google_access_token(refresh_token).await?;
            if let Some(pool_data) = self.fetch_cloud_models_with_token(&refreshed_access_token).await? {
                return Ok(pool_data);
            }
        }

        Err(anyhow!("Antigravity session expired. Please open Antigravity to refresh your session."))
    }

    fn parse_cloud_response(&self, data: FetchModelsResponse) -> HashMap<String, PoolQuota> {
        let mut pools: HashMap<String, PoolQuota> = HashMap::new();

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
            let period_duration_ms = self.infer_period_duration_ms(reset_time.as_deref(), &pool);

            let entry = pools.entry(pool).or_insert(PoolQuota {
                remaining_fraction: frac,
                reset_time: reset_time.clone(),
                period_duration_ms,
            });
            if frac < entry.remaining_fraction {
                *entry = PoolQuota {
                    remaining_fraction: frac,
                    reset_time,
                    period_duration_ms,
                };
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

        for (pool, quota) in sorted_pools {
            let used = ((1.0 - quota.remaining_fraction.clamp(0.0, 1.0)) * 100.0).round();
            let resets_at = quota.reset_time.and_then(|s| {
                DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc))
            });

            windows.push(RateWindow {
                label: pool,
                used_percent: used,
                resets_at,
                period_duration_ms: Some(quota.period_duration_ms),
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
        // lsof output ends with "127.0.0.1:42150 (LISTEN)".
        let Some(colon_pos) = line.rfind(':') else {
            continue;
        };
        let port_text: String = line[colon_pos + 1..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        if let Ok(port) = port_text.parse::<u16>() {
            ports.insert(port);
        }
    }

    ports.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_period_duration_matches_pool_type() {
        let fetcher = AntigravityQuotaFetcher::new();

        assert_eq!(
            fetcher.pool_period_duration_ms("Gemini Flash"),
            AntigravityQuotaFetcher::FIVE_HOURS_MS
        );
        assert_eq!(
            fetcher.pool_period_duration_ms("Gemini Pro"),
            AntigravityQuotaFetcher::SEVEN_DAYS_MS
        );
        assert_eq!(
            fetcher.pool_period_duration_ms("Claude"),
            AntigravityQuotaFetcher::SEVEN_DAYS_MS
        );
    }

    #[test]
    fn infer_period_duration_uses_reset_time_before_pool_default() {
        let fetcher = AntigravityQuotaFetcher::new();
        let short_reset = (Utc::now() + chrono::Duration::hours(4)).to_rfc3339();
        let long_reset = (Utc::now() + chrono::Duration::hours(6)).to_rfc3339();

        assert_eq!(
            fetcher.infer_period_duration_ms(Some(&short_reset), "Claude"),
            AntigravityQuotaFetcher::FIVE_HOURS_MS
        );
        assert_eq!(
            fetcher.infer_period_duration_ms(Some(&long_reset), "Gemini Flash"),
            AntigravityQuotaFetcher::SEVEN_DAYS_MS
        );
        assert_eq!(
            fetcher.infer_period_duration_ms(None, "Gemini Flash"),
            AntigravityQuotaFetcher::FIVE_HOURS_MS
        );
    }

    #[test]
    fn pools_to_snapshot_preserves_per_pool_periods() {
        let fetcher = AntigravityQuotaFetcher::new();
        let snapshot = fetcher.pools_to_snapshot(PoolData {
            pools: HashMap::from([
                (
                    "Gemini Flash".to_string(),
                    PoolQuota {
                        remaining_fraction: 0.9,
                        reset_time: Some("2026-03-18T00:00:00Z".to_string()),
                        period_duration_ms: AntigravityQuotaFetcher::FIVE_HOURS_MS,
                    },
                ),
                (
                    "Claude".to_string(),
                    PoolQuota {
                        remaining_fraction: 0.2,
                        reset_time: Some("2026-03-24T00:00:00Z".to_string()),
                        period_duration_ms: AntigravityQuotaFetcher::SEVEN_DAYS_MS,
                    },
                ),
            ]),
            plan: Some("test".to_string()),
        });

        let flash = snapshot
            .windows
            .iter()
            .find(|window| window.label == "Gemini Flash")
            .unwrap();
        let claude = snapshot
            .windows
            .iter()
            .find(|window| window.label == "Claude")
            .unwrap();

        assert_eq!(
            flash.period_duration_ms,
            Some(AntigravityQuotaFetcher::FIVE_HOURS_MS)
        );
        assert_eq!(
            claude.period_duration_ms,
            Some(AntigravityQuotaFetcher::SEVEN_DAYS_MS)
        );
    }
}
