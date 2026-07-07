use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow};
use crate::usage::antigravity::{
    detect_antigravity_connections, AntigravityConnection, AntigravityRuntimeKind,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntigravityCredentials {
    pub token: AntigravityToken,
    #[serde(default)]
    pub auth_method: Option<String>,
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

    fn load_antigravity_creds(&self) -> Result<AntigravityCredentials> {
        // 1. Try to read from cache
        let cache_path =
            if let Ok(override_path) = std::env::var("TOKENPULSE_ANTIGRAVITY_AUTH_PATH") {
                std::path::PathBuf::from(override_path)
            } else {
                let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
                home.join(".gemini")
                    .join("antigravity-cli")
                    .join("antigravity-oauth-token")
            };
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

                    // If keychain token is not expired, we can return it
                    if !is_antigravity_token_expired(&keychain_creds.token.expiry) {
                        return Ok(keychain_creds);
                    }

                    // If keychain is expired, but we have a refresh token, we refresh it
                    if let Some(ref refresh) = keychain_creds.token.refresh_token {
                        return self.refresh_antigravity_token(&keychain_creds, refresh);
                    }
                }
            }
        }

        // If we couldn't get a valid token from keychain but have a cached token with refresh token:
        if let Some(ref creds) = cached_creds {
            if let Some(ref refresh) = creds.token.refresh_token {
                return self.refresh_antigravity_token(creds, refresh);
            }
        }

        Err(anyhow!(
            "No valid Antigravity credentials found or refresh token missing"
        ))
    }

    fn refresh_antigravity_token(
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

        let response: RefreshResponse = ureq::post("https://oauth2.googleapis.com/token")
            .send_form(&[
                ("client_id", client_id),
                ("client_secret", &client_secret),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .map_err(|e| anyhow!("Google OAuth token refresh failed: {}", e))?
            .into_json()
            .map_err(|e| anyhow!("Failed to parse Google OAuth token refresh response: {}", e))?;

        if response.access_token.is_empty() {
            return Err(anyhow!("Received empty access token from Google OAuth"));
        }

        let expires_in = response.expires_in.unwrap_or(3599);
        let expiry_dt = Utc::now() + chrono::Duration::seconds(expires_in);
        let expiry_str = expiry_dt.to_rfc3339_opts(chrono::SecondsFormat::Micros, true);

        let mut new_creds = original_creds.clone();
        new_creds.token.access_token = response.access_token;
        new_creds.token.expiry = expiry_str;
        if let Some(new_refresh) = response.refresh_token {
            if !new_refresh.is_empty() {
                new_creds.token.refresh_token = Some(new_refresh);
            }
        }

        // Save to cache
        let cache_path =
            if let Ok(override_path) = std::env::var("TOKENPULSE_ANTIGRAVITY_AUTH_PATH") {
                std::path::PathBuf::from(override_path)
            } else {
                let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
                home.join(".gemini")
                    .join("antigravity-cli")
                    .join("antigravity-oauth-token")
            };
        if let Some(parent) = cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json_data = serde_json::to_string(&new_creds)?;
        std::fs::write(&cache_path, &json_data)?;
        info!("Saved refreshed Antigravity token to cache");

        Ok(new_creds)
    }

    async fn fetch_quota_via_cloud_code(&self) -> Result<QuotaSnapshot> {
        let creds = self.load_antigravity_creds()?;

        let api_url = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
        let response = self
            .ls_client
            .post(api_url)
            .bearer_auth(&creds.token.access_token)
            .header("User-Agent", "antigravity")
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .send()
            .await?;

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

        Ok(QuotaSnapshot {
            provider: "antigravity".to_string(),
            plan: Some("Pro".to_string()),
            account: None,
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
        .and_then(|u| u.plan_status)
        .and_then(|p| p.plan_info)
        .and_then(|i| i.plan_name)
        .filter(|name| !name.trim().is_empty());
    UserMetadata { account, plan }
}

/// Order detected connections so the CLI server is tried first, then Desktop,
/// then any other runtime kinds, preserving discovery order within each group.
fn ordered_connections(connections: &[AntigravityConnection]) -> Vec<&AntigravityConnection> {
    let mut ordered: Vec<&AntigravityConnection> = Vec::with_capacity(connections.len());
    for kind in [AntigravityRuntimeKind::Cli, AntigravityRuntimeKind::Desktop] {
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
        let connections = tokio::task::spawn_blocking(detect_antigravity_connections)
            .await
            .unwrap_or_else(|e| Err(anyhow!("Spawn blocking failed: {}", e)))
            .unwrap_or_else(|e| {
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

        debug!("No running Language Server processes succeeded. Attempting direct Cloud Code API fallback.");
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
    }
}
