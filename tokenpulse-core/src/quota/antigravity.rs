use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow};
use crate::usage::antigravity::{
    detect_antigravity_connections, AntigravityConnection, AntigravityRuntimeKind,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::time::Duration;
use tracing::{debug, info};

// The Language Server proxies RetrieveUserQuotaSummary to Antigravity's backend,
// so this is a real network round-trip, not a local liveness check. Keep it in
// line with the other providers (20s) instead of a tight 3s probe budget.
const LS_REQUEST_TIMEOUT_SECS: u64 = 20;

// Connect-RPC service exposed by the Antigravity Language Server.
const LS_SERVICE: &str = "exa.language_server_pb.LanguageServerService";

const FIVE_HOURS_MS: i64 = 5 * 60 * 60 * 1000;
const SEVEN_DAYS_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const AGY_CLI_TIMEOUT_SECS: u64 = 15;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityCredentials {
    pub token: AntigravityToken,
    #[serde(default)]
    pub auth_method: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityToken {
    pub access_token: String,
    pub token_type: Option<String>,
    pub refresh_token: Option<String>,
    pub expiry: String,
}

fn is_antigravity_token_expired(expiry_str: &str) -> bool {
    if let Ok(expiry_dt) = DateTime::parse_from_rfc3339(expiry_str) {
        let expiry_utc = expiry_dt.with_timezone(&Utc);
        let now = Utc::now();
        let buffer = chrono::Duration::minutes(5);
        expiry_utc <= now + buffer
    } else {
        true
    }
}

// ── RetrieveUserQuotaSummary response ──

#[derive(Debug, Deserialize)]
struct LsQuotaSummaryResponse {
    #[serde(default)]
    response: Option<LsQuotaSummary>,
}

#[derive(Debug, Deserialize)]
struct LsQuotaSummary {
    #[serde(default)]
    groups: Vec<LsQuotaGroup>,
}

#[derive(Debug, Deserialize)]
struct LsQuotaGroup {
    #[serde(default, rename = "displayName")]
    display_name: String,
    #[serde(default)]
    buckets: Vec<LsQuotaBucket>,
}

#[derive(Debug, Deserialize)]
struct LsQuotaBucket {
    #[serde(default)]
    window: String,
    #[serde(default, rename = "remainingFraction")]
    remaining_fraction: f64,
    #[serde(default, rename = "resetTime")]
    reset_time: Option<String>,
}

// ── GetUserStatus response (account + plan only) ──

#[derive(Debug, Deserialize)]
struct LsUserStatusResponse {
    #[serde(default, rename = "userStatus")]
    user_status: Option<UserStatus>,
}

#[derive(Debug, Deserialize)]
struct UserStatus {
    #[serde(default)]
    email: Option<String>,
    #[serde(default, rename = "planStatus")]
    plan_status: Option<PlanStatus>,
    #[serde(default, rename = "userTier")]
    user_tier: Option<UserTier>,
}

#[derive(Debug, Deserialize)]
struct UserTier {
    #[serde(default)]
    name: Option<String>,
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

#[derive(Debug, Default)]
struct UserMetadata {
    account: Option<String>,
    plan: Option<String>,
}

// ── Main fetcher ──

pub struct AntigravityQuotaFetcher {
    ls_client: Client,
}

impl AntigravityQuotaFetcher {
    pub fn new() -> Self {
        let ls_client = Client::builder()
            .timeout(Duration::from_secs(LS_REQUEST_TIMEOUT_SECS))
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { ls_client }
    }

    /// POST a standard IDE-metadata request to a Language Server RPC method and
    /// deserialize the JSON response. Returns `None` on any transport, status,
    /// or decode error so callers can fall back to the next connection.
    async fn rpc_post<T: serde::de::DeserializeOwned>(
        &self,
        connection: &AntigravityConnection,
        method: &str,
    ) -> Option<T> {
        let url = format!(
            "{}://127.0.0.1:{}/{}/{}",
            connection.scheme, connection.port, LS_SERVICE, method
        );

        let body = serde_json::json!({
            "metadata": {
                "ideName": "antigravity",
                "extensionName": "antigravity",
                "ideVersion": "unknown",
                "locale": "en",
            }
        });

        let mut request = self
            .ls_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .json(&body);
        if let Some(csrf_token) = connection.csrf_token.as_deref() {
            request = request.header("x-codeium-csrf-token", csrf_token);
        }

        let response = request.send().await.ok()?;
        if !response.status().is_success() {
            debug!("{} failed: {}", method, response.status());
            return None;
        }
        response.json::<T>().await.ok()
    }

    /// Probe a single connection for the user's quota summary, returning a
    /// fully built snapshot when the server reports usable quota buckets.
    async fn probe_connection(&self, connection: &AntigravityConnection) -> Option<QuotaSnapshot> {
        debug!(
            "Probing {} Antigravity LS at {}:{}",
            connection.runtime_kind.as_str(),
            connection.scheme,
            connection.port
        );

        let summary: LsQuotaSummaryResponse = self
            .rpc_post(connection, "RetrieveUserQuotaSummary")
            .await?;
        let windows = windows_from_summary(&summary.response?);
        if windows.is_empty() {
            return None;
        }

        // Account/plan are not part of the quota summary; fetch them best-effort.
        let metadata = self
            .rpc_post::<LsUserStatusResponse>(connection, "GetUserStatus")
            .await
            .map(user_metadata_from_status)
            .unwrap_or_default();

        Some(QuotaSnapshot {
            provider: "antigravity".to_string(),
            plan: metadata.plan,
            account: metadata.account,
            windows,
            credits: None,
            rate_limit_reset_credits: vec![],
            fetched_at: Utc::now(),
        })
    }

    async fn load_antigravity_creds(&self) -> Result<AntigravityCredentials> {
        // 1. Try to read from cache
        let cache_path = antigravity_token_cache_path();
        let mut cached_creds: Option<AntigravityCredentials> = None;
        if cache_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&cache_path) {
                if let Ok(creds) = serde_json::from_str::<AntigravityCredentials>(&content) {
                    cached_creds = Some(creds);
                }
            }
        }

        // If cached creds are valid and not expired, return them
        if let Some(ref creds) = cached_creds {
            if !is_antigravity_token_expired(&creds.token.expiry) {
                return Ok(creds.clone());
            }
        }

        // 2. Read from Keychain
        #[cfg(target_os = "macos")]
        {
            let output = Command::new("security")
                .args([
                    "find-generic-password",
                    "-s",
                    "gemini",
                    "-a",
                    "antigravity",
                    "-w",
                ])
                .output()?;
            if output.status.success() {
                let pwd_data = String::from_utf8_lossy(&output.stdout);
                let pwd_data = pwd_data.trim();
                if let Some(b64_payload) = pwd_data.strip_prefix("go-keyring-base64:") {
                    let decoded = crate::auth::decode_base64(b64_payload)?;
                    let keychain_creds: AntigravityCredentials = serde_json::from_slice(&decoded)?;

                    // If keychain token is not expired, we can cache and return it
                    if !is_antigravity_token_expired(&keychain_creds.token.expiry) {
                        if let Some(parent) = cache_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if let Ok(json_data) = serde_json::to_string(&keychain_creds) {
                            let _ = std::fs::write(&cache_path, json_data);
                        }
                        return Ok(keychain_creds);
                    }

                    // If keychain is expired, but we have a refresh token, we refresh it
                    if let Some(ref refresh) = keychain_creds.token.refresh_token {
                        return self
                            .refresh_antigravity_token(&keychain_creds, refresh)
                            .await;
                    }
                }
            }
        }

        // If we couldn't get a valid token from keychain but have a cached token with refresh token:
        if let Some(ref creds) = cached_creds {
            if let Some(ref refresh) = creds.token.refresh_token {
                return self.refresh_antigravity_token(creds, refresh).await;
            }
        }

        Err(anyhow!(
            "No valid Antigravity credentials found or refresh token missing"
        ))
    }

    async fn refresh_antigravity_token(
        &self,
        original_creds: &AntigravityCredentials,
        refresh_token: &str,
    ) -> Result<AntigravityCredentials> {
        info!("Refreshing Antigravity OAuth token");

        let client_id = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
        let client_secret = ["GOCSPX-", "K58FWR486LdLJ1mLB8sXC4z6qDAf"].concat();

        #[derive(Deserialize)]
        struct RefreshResponse {
            access_token: String,
            expires_in: Option<i64>,
            refresh_token: Option<String>,
        }

        let params = [
            ("client_id", client_id),
            ("client_secret", &client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];

        let response = self
            .ls_client
            .post("https://oauth2.googleapis.com/token")
            .form(&params)
            .send()
            .await
            .map_err(|e| anyhow!("Google OAuth token refresh failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Google OAuth token refresh failed with HTTP {}: {}",
                status,
                body
            ));
        }

        let parsed: RefreshResponse = response
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse Google OAuth token refresh response: {}", e))?;

        if parsed.access_token.is_empty() {
            return Err(anyhow!("Received empty access token from Google OAuth"));
        }

        let expires_in = parsed.expires_in.unwrap_or(3599);
        let expiry_dt = Utc::now() + chrono::Duration::seconds(expires_in);
        let expiry_str = expiry_dt.to_rfc3339_opts(chrono::SecondsFormat::Micros, true);

        let mut new_creds = original_creds.clone();
        new_creds.token.access_token = parsed.access_token;
        new_creds.token.expiry = expiry_str;
        if let Some(new_refresh) = parsed.refresh_token {
            if !new_refresh.is_empty() {
                new_creds.token.refresh_token = Some(new_refresh);
            }
        }

        // Save to cache
        let cache_path = antigravity_token_cache_path();
        if let Some(parent) = cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json_data = serde_json::to_string(&new_creds)?;
        std::fs::write(&cache_path, &json_data)?;
        info!("Saved refreshed Antigravity token to cache");

        Ok(new_creds)
    }

    async fn check_agy_version(&self) -> bool {
        let mut ver_cmd = tokio::process::Command::new("agy");
        ver_cmd
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);

        let Ok(child) = ver_cmd.spawn() else {
            return false;
        };

        let Ok(Ok(output)) =
            tokio::time::timeout(std::time::Duration::from_secs(2), child.wait_with_output()).await
        else {
            return false;
        };

        if !output.status.success() {
            return false;
        }

        let ver_str = String::from_utf8_lossy(&output.stdout);
        is_supported_agy_version(&ver_str)
    }

    async fn probe_agy_cli(&self) -> Option<QuotaSnapshot> {
        if !self.check_agy_version().await {
            debug!(
                "agy CLI not found or version does not support /usage (< 1.4); skipping CLI probe"
            );
            return None;
        }

        let mut command = tokio::process::Command::new("agy");
        command
            .args(["-p", "/usage", "--output-format", "json"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                debug!("Failed to spawn agy CLI process: {}", e);
                return None;
            }
        };

        let output = match tokio::time::timeout(
            std::time::Duration::from_secs(AGY_CLI_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                debug!("Error waiting for agy CLI output: {}", e);
                return None;
            }
            Err(_) => {
                debug!(
                    "agy CLI usage command timed out after {}s; process killed via kill_on_drop",
                    AGY_CLI_TIMEOUT_SECS
                );
                return None;
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!(
                "agy CLI usage command exited with status: {:?}, stderr: {}",
                output.status.code(),
                stderr.trim()
            );
            return None;
        }

        let json_str = match String::from_utf8(output.stdout) {
            Ok(s) => s,
            Err(e) => {
                debug!("agy CLI output is not valid UTF-8: {}", e);
                return None;
            }
        };

        let mut snapshot = match parse_agy_cli_output(&json_str) {
            Ok(s) => s,
            Err(e) => {
                debug!("Failed to parse agy CLI output: {}", e);
                return None;
            }
        };

        // Best effort: enrich account email from cached id_token without triggering OAuth refresh
        if let Some(id_token) = read_cached_id_token() {
            snapshot.account = crate::auth::decode_jwt_email(&id_token);
        }

        Some(snapshot)
    }

    async fn query_load_code_assist(
        &self,
        access_token: &str,
        user_agent: &str,
    ) -> (Option<String>, Option<String>) {
        let load_url = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
        let load_body = serde_json::json!({
            "metadata": {
                "ideType": "ANTIGRAVITY",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI"
            }
        });

        let mut project_id: Option<String> = None;
        let mut plan: Option<String> = None;

        let load_res = self
            .ls_client
            .post(load_url)
            .bearer_auth(access_token)
            .header("User-Agent", user_agent)
            .header("Content-Type", "application/json")
            .json(&load_body)
            .send()
            .await;

        match load_res {
            Ok(resp) => {
                if resp.status().is_success() {
                    if let Ok(load_data) = resp.json::<LoadCodeAssistResponse>().await {
                        if let Some(proj) = load_data
                            .cloudaicompanion_project
                            .as_ref()
                            .and_then(parse_project_id)
                        {
                            project_id = Some(proj);
                        }
                        plan = load_data
                            .paid_tier
                            .and_then(|t| t.name)
                            .filter(|n| !n.trim().is_empty())
                            .or_else(|| {
                                load_data
                                    .current_tier
                                    .and_then(|t| t.name)
                                    .filter(|n| !n.trim().is_empty())
                            });
                    }
                } else {
                    debug!("loadCodeAssist returned status: {}", resp.status());
                }
            }
            Err(e) => {
                debug!("loadCodeAssist network/transport error: {}", e);
            }
        }
        (project_id, plan)
    }

    async fn fetch_quota_via_cloud_code(&self) -> Result<QuotaSnapshot> {
        let mut creds = self.load_antigravity_creds().await?;
        let user_agent = cloud_code_user_agent();

        // 1. Query loadCodeAssist to resolve project ID and plan
        let (mut project_id, mut plan) = self
            .query_load_code_assist(&creds.token.access_token, &user_agent)
            .await;

        let api_url = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
        let mut req_body = build_cloud_code_quota_body(project_id.as_deref());

        let mut response = self
            .ls_client
            .post(api_url)
            .bearer_auth(&creds.token.access_token)
            .header("User-Agent", &user_agent)
            .header("Content-Type", "application/json")
            .json(&req_body)
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(ref refresh) = creds.token.refresh_token {
                debug!("Antigravity returned 401 Unauthorized; attempting token refresh");
                if let Ok(refreshed) = self.refresh_antigravity_token(&creds, refresh).await {
                    creds = refreshed;
                    // Re-query loadCodeAssist after token refresh to resolve project ID and plan
                    let (refreshed_project_id, refreshed_plan) = self
                        .query_load_code_assist(&creds.token.access_token, &user_agent)
                        .await;
                    project_id = refreshed_project_id;
                    if refreshed_plan.is_some() {
                        plan = refreshed_plan;
                    }
                    req_body = build_cloud_code_quota_body(project_id.as_deref());

                    response = self
                        .ls_client
                        .post(api_url)
                        .bearer_auth(&creds.token.access_token)
                        .header("User-Agent", &user_agent)
                        .header("Content-Type", "application/json")
                        .json(&req_body)
                        .send()
                        .await?;
                }
            }
        }

        let status = response.status();
        let body = response.text().await?;
        debug!(
            "Antigravity Cloud Code quota response status: {}, {} bytes",
            status,
            body.len()
        );

        if !status.is_success() {
            return Err(anyhow!("Antigravity Quota API error {}: {}", status, body));
        }

        // Try to parse the response as wrapped or unwrapped LsQuotaSummary
        let parsed_summary =
            if let Ok(wrapped) = serde_json::from_str::<LsQuotaSummaryResponse>(&body) {
                if let Some(summary) = wrapped.response {
                    summary
                } else {
                    serde_json::from_str::<LsQuotaSummary>(&body).map_err(|e| {
                        anyhow!(
                            "Failed to parse Antigravity quota response: {}. Body: {}",
                            e,
                            body
                        )
                    })?
                }
            } else {
                serde_json::from_str::<LsQuotaSummary>(&body).map_err(|e| {
                    anyhow!(
                        "Failed to parse Antigravity quota response: {}. Body: {}",
                        e,
                        body
                    )
                })?
            };

        let windows = windows_from_summary(&parsed_summary);
        if windows.is_empty() {
            return Err(anyhow!("No active quota windows found in API response"));
        }

        let account = creds
            .id_token
            .as_deref()
            .and_then(crate::auth::decode_jwt_email);

        Ok(QuotaSnapshot {
            provider: "antigravity".to_string(),
            plan,
            account,
            windows,
            credits: None,
            rate_limit_reset_credits: vec![],
            fetched_at: Utc::now(),
        })
    }
}

/// Map a quota summary into sorted `RateWindow`s: Gemini before Claude, and
/// within each group the 5-hour limit before the weekly limit.
fn windows_from_summary(summary: &LsQuotaSummary) -> Vec<RateWindow> {
    let mut built: Vec<(u8, u8, RateWindow)> = Vec::new();

    for group in &summary.groups {
        let group_label = clean_group_label(&group.display_name);
        let group_rank = group_rank(&group_label);

        for bucket in &group.buckets {
            let (window_suffix, window_rank, period_duration_ms) =
                window_descriptor(&bucket.window);
            // Keep full precision; the UI rounds for display.
            let used = (1.0 - bucket.remaining_fraction.clamp(0.0, 1.0)) * 100.0;
            let resets_at = bucket.reset_time.as_deref().and_then(|s| {
                DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            });

            built.push((
                group_rank,
                window_rank,
                RateWindow {
                    label: format!("{} ({})", group_label, window_suffix),
                    // Antigravity meters each model family separately, and the
                    // group label already is that family.
                    model_family: model_family_from_group(&group_label),
                    used_percent: used,
                    resets_at,
                    period_duration_ms,
                },
            ));
        }
    }

    built.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    built.into_iter().map(|(_, _, window)| window).collect()
}

/// Turn a server group display name into a short, clean label.
/// `clean_group_label` falls back to "Usage" when a group has no usable name;
/// that is a placeholder, not a model family, so it is reported as absent.
fn model_family_from_group(group_label: &str) -> Option<String> {
    if group_label == "Usage" {
        None
    } else {
        Some(group_label.to_string())
    }
}

fn clean_group_label(display_name: &str) -> String {
    let lower = display_name.to_lowercase();
    if lower.contains("gemini") {
        "Gemini".to_string()
    } else if lower.contains("claude") || lower.contains("gpt") {
        "Claude".to_string()
    } else {
        let trimmed = display_name.trim();
        let cleaned = trimmed
            .strip_suffix(" Models")
            .or_else(|| trimmed.strip_suffix(" models"))
            .unwrap_or(trimmed)
            .trim();
        if cleaned.is_empty() {
            "Usage".to_string()
        } else {
            cleaned.to_string()
        }
    }
}

fn group_rank(group_label: &str) -> u8 {
    match group_label {
        "Gemini" => 0,
        "Claude" => 1,
        _ => 2,
    }
}

/// Map a server window identifier to a display suffix, sort rank, and period.
fn window_descriptor(window: &str) -> (String, u8, Option<i64>) {
    match window.to_lowercase().as_str() {
        "5h" => ("5h".to_string(), 0, Some(FIVE_HOURS_MS)),
        "weekly" => ("7d".to_string(), 1, Some(SEVEN_DAYS_MS)),
        _ => (window.to_string(), 2, None),
    }
}

fn user_metadata_from_status(status: LsUserStatusResponse) -> UserMetadata {
    let user = status.user_status;
    let account = user
        .as_ref()
        .and_then(|u| u.email.clone())
        .filter(|email| !email.trim().is_empty());
    let plan = user
        .as_ref()
        .and_then(|u| u.user_tier.as_ref())
        .and_then(|t| t.name.clone())
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            user.and_then(|u| u.plan_status)
                .and_then(|p| p.plan_info)
                .and_then(|i| i.plan_name)
                .filter(|name| !name.trim().is_empty())
        });
    UserMetadata { account, plan }
}

/// Order detected connections so the Desktop server is tried first, then CLI,
/// then any other runtime kinds, preserving discovery order within each group.
fn ordered_connections(connections: &[AntigravityConnection]) -> Vec<&AntigravityConnection> {
    let mut ordered: Vec<&AntigravityConnection> = Vec::with_capacity(connections.len());
    for kind in [AntigravityRuntimeKind::Desktop, AntigravityRuntimeKind::Cli] {
        ordered.extend(connections.iter().filter(|conn| conn.runtime_kind == kind));
    }
    ordered.extend(connections.iter().filter(|conn| {
        conn.runtime_kind != AntigravityRuntimeKind::Cli
            && conn.runtime_kind != AntigravityRuntimeKind::Desktop
    }));
    ordered
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
        let connections = detect_antigravity_connections().await.unwrap_or_else(|e| {
            debug!("Antigravity language server discovery failed: {}", e);
            Vec::new()
        });

        for connection in ordered_connections(&connections) {
            if let Some(snapshot) = self.probe_connection(connection).await {
                info!(
                    "Antigravity quota fetched via {} Language Server (pid={})",
                    connection.runtime_kind.as_str(),
                    connection.pid
                );
                return Ok(snapshot);
            }
        }

        if connections.is_empty() {
            debug!("No running Language Server processes detected. Attempting agy CLI fallback.");
        } else {
            debug!(
                "{} Language Server connection(s) detected, but quota RPC probes failed. Attempting agy CLI fallback.",
                connections.len()
            );
        }

        // 2. Fall back to agy CLI if installed
        if let Some(snapshot) = self.probe_agy_cli().await {
            info!("Antigravity quota fetched via agy CLI usage command");
            return Ok(snapshot);
        }

        // 3. Fall back to Cloud Code API using cached OAuth credentials
        debug!("Attempting direct Cloud Code API fallback.");
        match self.fetch_quota_via_cloud_code().await {
            Ok(snapshot) => {
                info!("Antigravity quota fetched directly via Cloud Code API");
                return Ok(snapshot);
            }
            Err(e) => {
                debug!("Cloud Code API fallback failed: {}", e);
            }
        }

        Err(anyhow!(
            "Antigravity quota unavailable: no running Antigravity CLI or desktop language server responded, and Cloud Code API fallback failed."
        ))
    }
}

fn cloud_code_user_agent() -> String {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "amd64"
    } else {
        "unknown"
    };
    format!("antigravity/hub/2.9.1 {}/{}", os, arch)
}

pub(crate) fn antigravity_token_cache_path() -> std::path::PathBuf {
    if let Ok(override_path) = std::env::var("TOKENPULSE_ANTIGRAVITY_AUTH_PATH") {
        std::path::PathBuf::from(override_path)
    } else {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
        home.join(".gemini")
            .join("antigravity-cli")
            .join("antigravity-oauth-token")
    }
}

fn read_cached_id_token() -> Option<String> {
    let cache_path = antigravity_token_cache_path();
    let content = std::fs::read_to_string(cache_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("id_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn parse_project_id(val: &serde_json::Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    } else if let Some(obj) = val.as_object() {
        if let Some(id) = obj.get("id").and_then(|v| v.as_str()) {
            let trimmed = id.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[derive(Debug, Deserialize)]
struct LoadCodeAssistResponse {
    #[serde(default, rename = "cloudaicompanionProject")]
    cloudaicompanion_project: Option<serde_json::Value>,
    #[serde(default, rename = "paidTier")]
    paid_tier: Option<CloudCodeTier>,
    #[serde(default, rename = "currentTier")]
    current_tier: Option<CloudCodeTier>,
}

#[derive(Debug, Deserialize)]
struct CloudCodeTier {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgyCliResponse {
    #[serde(default)]
    command: Option<AgyCliCommand>,
}

#[derive(Debug, Deserialize)]
struct AgyCliCommand {
    #[serde(default)]
    data: Option<AgyCliData>,
}

#[derive(Debug, Deserialize)]
struct AgyCliData {
    #[serde(default)]
    groups: Vec<AgyCliGroup>,
}

#[derive(Debug, Deserialize)]
struct AgyCliGroup {
    #[serde(default)]
    name: String,
    #[serde(default)]
    buckets: Vec<AgyCliBucket>,
}

#[derive(Debug, Deserialize)]
struct AgyCliBucket {
    #[serde(default)]
    window: String,
    #[serde(default, rename = "remaining_fraction")]
    remaining_fraction: f64,
    #[serde(default, rename = "reset_time")]
    reset_time: Option<String>,
}

impl From<AgyCliData> for LsQuotaSummary {
    fn from(data: AgyCliData) -> Self {
        LsQuotaSummary {
            groups: data
                .groups
                .into_iter()
                .map(|g| LsQuotaGroup {
                    display_name: g.name,
                    buckets: g
                        .buckets
                        .into_iter()
                        .map(|b| LsQuotaBucket {
                            window: b.window,
                            remaining_fraction: b.remaining_fraction,
                            reset_time: b.reset_time,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

pub fn parse_agy_cli_output(json_str: &str) -> Result<QuotaSnapshot> {
    let resp: AgyCliResponse = serde_json::from_str(json_str)
        .map_err(|e| anyhow!("Failed to parse agy cli usage json: {}", e))?;
    let cli_data = resp
        .command
        .and_then(|c| c.data)
        .ok_or_else(|| anyhow!("No quota data found in agy usage output"))?;

    let summary = LsQuotaSummary::from(cli_data);
    let windows = windows_from_summary(&summary);
    if windows.is_empty() {
        return Err(anyhow!("No active quota windows found in agy usage output"));
    }

    Ok(QuotaSnapshot {
        provider: "antigravity".to_string(),
        plan: None,
        account: None,
        windows,
        credits: None,
        rate_limit_reset_credits: vec![],
        fetched_at: Utc::now(),
    })
}

pub fn build_cloud_code_quota_body(project_id: Option<&str>) -> serde_json::Value {
    if let Some(project) = project_id {
        serde_json::json!({
            "project": project
        })
    } else {
        serde_json::json!({})
    }
}

pub fn is_supported_agy_version(version_str: &str) -> bool {
    let trimmed = version_str.trim();
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() >= 2 {
        if let (Ok(major), Ok(minor)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) {
            return major > 1 || (major == 1 && minor >= 4);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_QUOTA_SUMMARY: &str = r#"{
        "response": {
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "description": "Models within this group: Gemini Flash, Gemini Pro",
                    "buckets": [
                        {
                            "bucketId": "gemini-weekly",
                            "displayName": "Weekly Limit",
                            "window": "weekly",
                            "remainingFraction": 0.9755835,
                            "resetTime": "2026-06-19T05:07:40Z"
                        },
                        {
                            "bucketId": "gemini-5h",
                            "displayName": "Five Hour Limit",
                            "window": "5h",
                            "remainingFraction": 0.8852908,
                            "resetTime": "2026-06-12T10:07:40Z"
                        }
                    ]
                },
                {
                    "displayName": "Claude and GPT models",
                    "description": "Models within this group: Claude Opus, Claude Sonnet, GPT-OSS",
                    "buckets": [
                        {
                            "bucketId": "3p-weekly",
                            "displayName": "Weekly Limit",
                            "window": "weekly",
                            "remainingFraction": 0.1584452,
                            "resetTime": "2026-06-12T09:51:22Z"
                        },
                        {
                            "bucketId": "3p-5h",
                            "displayName": "Five Hour Limit",
                            "window": "5h",
                            "remainingFraction": 1,
                            "resetTime": "2026-06-12T11:10:43Z"
                        }
                    ]
                }
            ]
        }
    }"#;

    #[test]
    fn parses_quota_summary_into_sorted_windows() {
        let parsed: LsQuotaSummaryResponse = serde_json::from_str(SAMPLE_QUOTA_SUMMARY).unwrap();
        let summary = parsed.response.expect("response present");
        let windows = windows_from_summary(&summary);

        let labels: Vec<&str> = windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Gemini (5h)", "Gemini (7d)", "Claude (5h)", "Claude (7d)",]
        );
    }

    #[test]
    fn maps_window_durations_and_used_percent() {
        let parsed: LsQuotaSummaryResponse = serde_json::from_str(SAMPLE_QUOTA_SUMMARY).unwrap();
        let windows = windows_from_summary(&parsed.response.unwrap());

        let gemini_5h = &windows[0];
        assert_eq!(gemini_5h.label, "Gemini (5h)");
        assert_eq!(gemini_5h.period_duration_ms, Some(FIVE_HOURS_MS));
        assert!((gemini_5h.used_percent - (1.0 - 0.8852908_f64) * 100.0).abs() < 1e-9);
        assert!(gemini_5h.resets_at.is_some());

        let gemini_weekly = &windows[1];
        assert_eq!(gemini_weekly.label, "Gemini (7d)");
        assert_eq!(gemini_weekly.period_duration_ms, Some(SEVEN_DAYS_MS));

        let third_party_weekly = &windows[3];
        assert_eq!(third_party_weekly.label, "Claude (7d)");
        assert!((third_party_weekly.used_percent - (1.0 - 0.1584452_f64) * 100.0).abs() < 1e-9);
    }

    #[test]
    fn clean_group_label_maps_known_groups() {
        assert_eq!(clean_group_label("Gemini Models"), "Gemini");
        assert_eq!(clean_group_label("Claude and GPT models"), "Claude");
        assert_eq!(clean_group_label("Experimental Models"), "Experimental");
    }

    #[test]
    fn window_descriptor_falls_back_for_unknown_windows() {
        assert_eq!(
            window_descriptor("5h"),
            ("5h".to_string(), 0, Some(FIVE_HOURS_MS))
        );
        assert_eq!(
            window_descriptor("weekly"),
            ("7d".to_string(), 1, Some(SEVEN_DAYS_MS))
        );
        assert_eq!(
            window_descriptor("monthly"),
            ("monthly".to_string(), 2, None)
        );
    }

    #[test]
    fn extracts_account_and_plan_from_user_status() {
        let parsed: LsUserStatusResponse = serde_json::from_str(
            r#"{
                "userStatus": {
                    "email": "user@example.com",
                    "planStatus": { "planInfo": { "planName": "Pro" } }
                }
            }"#,
        )
        .unwrap();
        let metadata = user_metadata_from_status(parsed);
        assert_eq!(metadata.account.as_deref(), Some("user@example.com"));
        assert_eq!(metadata.plan.as_deref(), Some("Pro"));

        let parsed_with_tier: LsUserStatusResponse = serde_json::from_str(
            r#"{
                "userStatus": {
                    "email": "user2@example.com",
                    "userTier": { "name": "Google AI Pro" },
                    "planStatus": { "planInfo": { "planName": "Pro" } }
                }
            }"#,
        )
        .unwrap();
        let metadata_tier = user_metadata_from_status(parsed_with_tier);
        assert_eq!(metadata_tier.account.as_deref(), Some("user2@example.com"));
        assert_eq!(metadata_tier.plan.as_deref(), Some("Google AI Pro"));
    }

    #[test]
    fn test_is_antigravity_token_expired() {
        let now = Utc::now();
        // Expired (1 hour ago)
        let past = (now - chrono::Duration::hours(1)).to_rfc3339();
        assert!(is_antigravity_token_expired(&past));

        // Near-expiry (within 5 minutes buffer)
        let near = (now + chrono::Duration::minutes(3)).to_rfc3339();
        assert!(is_antigravity_token_expired(&near));

        // Valid (1 hour in future)
        let future = (now + chrono::Duration::hours(1)).to_rfc3339();
        assert!(!is_antigravity_token_expired(&future));

        // Malformed string should be treated as expired
        assert!(is_antigravity_token_expired("not-a-date"));
    }

    #[test]
    fn test_ordered_connections_prioritizes_desktop_over_cli() {
        let conn_cli_1 = AntigravityConnection {
            pid: 100,
            port: 1001,
            csrf_token: None,
            scheme: "http".to_string(),
            fingerprint: "pid:100:http:1001".to_string(),
            runtime_kind: AntigravityRuntimeKind::Cli,
        };
        let conn_desktop = AntigravityConnection {
            pid: 200,
            port: 2001,
            csrf_token: Some("token".to_string()),
            scheme: "https".to_string(),
            fingerprint: "pid:200:https:2001".to_string(),
            runtime_kind: AntigravityRuntimeKind::Desktop,
        };
        let conn_cli_2 = AntigravityConnection {
            pid: 300,
            port: 3001,
            csrf_token: None,
            scheme: "http".to_string(),
            fingerprint: "pid:300:http:3001".to_string(),
            runtime_kind: AntigravityRuntimeKind::Cli,
        };

        let raw = vec![conn_cli_1, conn_desktop, conn_cli_2];
        let ordered = ordered_connections(&raw);

        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0].runtime_kind, AntigravityRuntimeKind::Desktop);
        assert_eq!(ordered[0].pid, 200);
        assert_eq!(ordered[1].runtime_kind, AntigravityRuntimeKind::Cli);
        assert_eq!(ordered[1].pid, 100);
        assert_eq!(ordered[2].runtime_kind, AntigravityRuntimeKind::Cli);
        assert_eq!(ordered[2].pid, 300);
    }

    #[test]
    fn test_parse_agy_cli_usage() {
        let json = r#"{
            "conversation_id": "",
            "status": "SUCCESS",
            "command": {
                "name": "usage",
                "data": {
                    "groups": [
                        {
                            "name": "Gemini Models",
                            "buckets": [
                                {
                                    "id": "gemini-weekly",
                                    "name": "Weekly Limit Remaining",
                                    "window": "weekly",
                                    "remaining_fraction": 0.137,
                                    "reset_time": "2026-10-24T04:16:10Z"
                                },
                                {
                                    "id": "gemini-5h",
                                    "name": "Five Hour Limit Remaining",
                                    "window": "5h",
                                    "remaining_fraction": 0.79,
                                    "reset_time": "2026-10-22T11:07:56Z"
                                }
                            ]
                        },
                        {
                            "name": "Claude and GPT models",
                            "buckets": [
                                {
                                    "id": "3p-weekly",
                                    "name": "Weekly Limit Remaining",
                                    "window": "weekly",
                                    "remaining_fraction": 0.358,
                                    "reset_time": "2026-10-28T17:53:10Z"
                                },
                                {
                                    "id": "3p-5h",
                                    "name": "Five Hour Limit Remaining",
                                    "window": "5h",
                                    "remaining_fraction": 0.736,
                                    "reset_time": "2026-10-22T13:39:31Z"
                                }
                            ]
                        }
                    ]
                }
            }
        }"#;

        let snapshot = parse_agy_cli_output(json).expect("should parse agy usage json");
        assert_eq!(snapshot.provider, "antigravity");
        assert_eq!(snapshot.windows.len(), 4);
        assert_eq!(snapshot.windows[0].label, "Gemini (5h)");
        assert!((snapshot.windows[0].used_percent - 21.0).abs() < 0.1);
        assert_eq!(snapshot.windows[1].label, "Gemini (7d)");
        assert!((snapshot.windows[1].used_percent - 86.3).abs() < 0.1);
        assert_eq!(snapshot.windows[2].label, "Claude (5h)");
        assert_eq!(snapshot.windows[3].label, "Claude (7d)");
    }

    #[test]
    fn test_cloud_code_quota_payload_with_project() {
        let project_id = "aicode-consumers";
        let body = build_cloud_code_quota_body(Some(project_id));
        assert_eq!(body["project"], "aicode-consumers");

        let default_body = build_cloud_code_quota_body(None);
        assert_eq!(default_body, serde_json::json!({}));

        // Test email extraction from dummy JWT id_token using crate::auth::decode_jwt_email
        use base64::Engine;
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(r#"{"email":"user@example.com"}"#);
        let dummy_jwt = format!("{}.{}.", header, payload);
        let email = crate::auth::decode_jwt_email(&dummy_jwt);
        assert_eq!(email.as_deref(), Some("user@example.com"));

        // Test project ID parsing (both string and object shapes)
        assert_eq!(
            parse_project_id(&serde_json::json!("project-123")).as_deref(),
            Some("project-123")
        );
        assert_eq!(
            parse_project_id(&serde_json::json!({"id": "project-456"})).as_deref(),
            Some("project-456")
        );
        assert_eq!(parse_project_id(&serde_json::json!({})), None);
    }

    #[test]
    fn test_is_supported_agy_version() {
        assert!(is_supported_agy_version("1.4.5"));
        assert!(is_supported_agy_version("1.4.0\n"));
        assert!(is_supported_agy_version("1.5.0"));
        assert!(is_supported_agy_version("2.0.0"));
        assert!(!is_supported_agy_version("1.3.9"));
        assert!(!is_supported_agy_version("0.9.1"));
        assert!(!is_supported_agy_version("invalid"));
    }

    #[test]
    fn test_read_cached_id_token() {
        let temp_dir = tempfile::tempdir().unwrap();
        let token_path = temp_dir.path().join("antigravity-oauth-token");
        std::fs::write(&token_path, r#"{"id_token": "mock-token-xyz"}"#).unwrap();

        std::env::set_var(
            "TOKENPULSE_ANTIGRAVITY_AUTH_PATH",
            token_path.to_str().unwrap(),
        );
        let token = read_cached_id_token();
        std::env::remove_var("TOKENPULSE_ANTIGRAVITY_AUTH_PATH");

        assert_eq!(token.as_deref(), Some("mock-token-xyz"));
    }
}
