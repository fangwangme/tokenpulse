use crate::auth::gemini::GeminiAuth;
use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const REQUEST_TIMEOUT_SECS: u64 = 20;

const LOAD_CODE_ASSIST_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
const PROJECTS_URL: &str = "https://cloudresourcemanager.googleapis.com/v1/projects";
const TOKEN_REFRESH_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug, Deserialize)]
struct LoadCodeAssistResponse {
    #[serde(default)]
    tier: Option<String>,
    #[serde(default)]
    user_tier: Option<String>,
    #[serde(default)]
    subscription_tier: Option<String>,
    #[serde(default)]
    current_tier: Option<CurrentTier>,
    #[serde(default, rename = "cloudaicompanionProject")]
    cloudaicompanion_project: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct CurrentTier {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug)]
struct GeminiCodeAssistStatus {
    plan: Option<String>,
    project_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaResponse {
    #[serde(default, alias = "quotaBuckets")]
    buckets: Option<serde_json::Value>,
    #[serde(default, alias = "quota_buckets")]
    quota_buckets: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct TokenRefreshResponse {
    access_token: String,
    #[serde(default)]
    expires_in: Option<f64>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Debug)]
struct OAuthClientCredentials {
    client_id: String,
    client_secret: String,
}

pub struct GeminiQuotaFetcher {
    client: Client,
    auth: GeminiAuth,
}

impl GeminiQuotaFetcher {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            auth: GeminiAuth::new(),
        }
    }

    fn gemini_binary_path(&self) -> Option<PathBuf> {
        let path_var = env::var_os("PATH")?;
        for dir in env::split_paths(&path_var) {
            let candidate = dir.join("gemini");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }

    fn resolve_oauth_config_path(&self) -> Option<PathBuf> {
        let gemini_path = self.gemini_binary_path()?;
        let real_path = fs::canonicalize(&gemini_path).ok().unwrap_or(gemini_path);
        let bin_dir = real_path.parent()?;
        let base_dir = bin_dir.parent()?;
        let oauth_file = Path::new("dist/src/code_assist/oauth2.js");
        let candidates = [
            base_dir
                .join("lib/node_modules/@google/gemini-cli/node_modules/@google/gemini-cli-core")
                .join(oauth_file),
            base_dir
                .join("node_modules/@google/gemini-cli-core")
                .join(oauth_file),
            base_dir.join("../gemini-cli-core").join(oauth_file),
            base_dir
                .join("share/gemini-cli/node_modules/@google/gemini-cli-core")
                .join(oauth_file),
        ];

        candidates.into_iter().find(|path| path.is_file())
    }

    fn parse_js_constant(content: &str, constant: &str) -> Option<String> {
        let needle = format!("{constant} = ");
        let start = content.find(&needle)? + needle.len();
        let rest = &content[start..];
        let quote = rest.chars().next()?;
        if quote != '\'' && quote != '"' {
            return None;
        }
        let value_start = 1;
        let value_end = rest[value_start..].find(quote)? + value_start;
        Some(rest[value_start..value_end].to_string())
    }

    fn extract_oauth_client_credentials(&self) -> Result<OAuthClientCredentials> {
        let oauth_path = self
            .resolve_oauth_config_path()
            .ok_or_else(|| anyhow!("Could not locate Gemini CLI OAuth configuration"))?;
        let content = fs::read_to_string(oauth_path)?;
        let client_id = Self::parse_js_constant(&content, "OAUTH_CLIENT_ID")
            .ok_or_else(|| anyhow!("Could not parse Gemini CLI OAuth client id"))?;
        let client_secret = Self::parse_js_constant(&content, "OAUTH_CLIENT_SECRET")
            .ok_or_else(|| anyhow!("Could not parse Gemini CLI OAuth client secret"))?;
        Ok(OAuthClientCredentials {
            client_id,
            client_secret,
        })
    }

    async fn refresh_access_token(
        &self,
        refresh_token: &str,
        creds: &mut crate::auth::gemini::GeminiCredentials,
    ) -> Result<String> {
        let oauth_creds = self.extract_oauth_client_credentials()?;
        let response = self
            .client
            .post(TOKEN_REFRESH_URL)
            .form(&[
                ("client_id", oauth_creds.client_id.as_str()),
                ("client_secret", oauth_creds.client_secret.as_str()),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Gemini token refresh failed: {}",
                response.status()
            ));
        }

        let refreshed: TokenRefreshResponse = response.json().await?;
        creds.access_token = Some(refreshed.access_token.clone());
        if let Some(id_token) = refreshed.id_token {
            creds.id_token = Some(id_token);
        }
        if let Some(expires_in) = refreshed.expires_in {
            creds.expiry_date =
                Some((Utc::now().timestamp_millis() as f64) + (expires_in * 1000.0));
        }
        self.auth.save_credentials(creds)?;
        Ok(refreshed.access_token)
    }

    async fn access_token(&self) -> Result<String> {
        let mut creds = self.auth.load_credentials()?;

        if !self.auth.is_token_expired(&creds) {
            return creds.access_token.ok_or_else(|| {
                anyhow!("No Gemini access token found. Please run `gemini` to login.")
            });
        }

        let refresh_token = creds
            .refresh_token
            .clone()
            .ok_or_else(|| anyhow!("Gemini token expired and no refresh token is available. Please run `gemini` to refresh your session."))?;

        self.refresh_access_token(&refresh_token, &mut creds).await
    }

    async fn fetch_load_code_assist(&self, access_token: &str) -> Result<GeminiCodeAssistStatus> {
        let response = self
            .client
            .post(LOAD_CODE_ASSIST_URL)
            .bearer_auth(access_token)
            .json(&serde_json::json!({
                "metadata": {
                    "ideType": "GEMINI_CLI",
                    "pluginType": "GEMINI"
                }
            }))
            .send()
            .await?;

        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            return Err(anyhow!(
                "Gemini session expired. Please run `gemini` to refresh your session."
            ));
        }

        if !response.status().is_success() {
            return Err(anyhow!("LoadCodeAssist failed: {}", response.status()));
        }

        let data: LoadCodeAssistResponse = response.json().await?;
        let tier = data
            .current_tier
            .and_then(|t| t.id)
            .or(data.tier)
            .or(data.user_tier)
            .or(data.subscription_tier);

        let plan = match tier.as_deref().map(|s| s.to_lowercase()) {
            Some(s) if s.contains("standard") => Some("Paid".to_string()),
            Some(s) if s.contains("legacy") => Some("Legacy".to_string()),
            Some(s) if s.contains("free") => Some("Free".to_string()),
            _ => None,
        };

        let project_id = match data.cloudaicompanion_project {
            Some(serde_json::Value::String(project)) => Some(project),
            Some(serde_json::Value::Object(obj)) => obj
                .get("id")
                .or_else(|| obj.get("projectId"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            _ => None,
        };

        Ok(GeminiCodeAssistStatus { plan, project_id })
    }

    async fn discover_project_id(&self, access_token: &str) -> Result<Option<String>> {
        let response = self
            .client
            .get(PROJECTS_URL)
            .bearer_auth(access_token)
            .send()
            .await?;

        if !response.status().is_success() {
            return Ok(None);
        }

        let value: serde_json::Value = response.json().await?;
        let projects = value
            .get("projects")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        for project in projects {
            let Some(project_id) = project.get("projectId").and_then(|v| v.as_str()) else {
                continue;
            };

            if project_id.starts_with("gen-lang-client") {
                return Ok(Some(project_id.to_string()));
            }

            if project
                .get("labels")
                .and_then(|v| v.as_object())
                .map(|labels| labels.contains_key("generative-language"))
                .unwrap_or(false)
            {
                return Ok(Some(project_id.to_string()));
            }
        }

        Ok(None)
    }

    async fn fetch_quota_api(
        &self,
        access_token: &str,
        project_id: Option<&str>,
    ) -> Result<QuotaResponse> {
        let body = if let Some(pid) = project_id {
            serde_json::json!({ "project": pid })
        } else {
            serde_json::json!({})
        };

        let response = self
            .client
            .post(QUOTA_URL)
            .bearer_auth(access_token)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Quota API error: {}", response.status()));
        }

        let quota: QuotaResponse = response.json().await?;
        Ok(quota)
    }

    fn extract_quota_buckets(&self, value: &serde_json::Value) -> Vec<QuotaBucket> {
        let mut buckets = Vec::new();
        self.collect_buckets(value, &mut buckets);
        buckets
    }

    fn collect_buckets(&self, value: &serde_json::Value, out: &mut Vec<QuotaBucket>) {
        match value {
            serde_json::Value::Array(arr) => {
                for item in arr {
                    self.collect_buckets(item, out);
                }
            }
            serde_json::Value::Object(obj) => {
                if let Some(frac) = obj.get("remainingFraction").and_then(|v| v.as_f64()) {
                    let model_id = obj
                        .get("modelId")
                        .or_else(|| obj.get("model_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let reset_time = obj
                        .get("resetTime")
                        .or_else(|| obj.get("reset_time"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    out.push(QuotaBucket {
                        model_id,
                        remaining_fraction: frac,
                        reset_time,
                    });
                }
                for v in obj.values() {
                    self.collect_buckets(v, out);
                }
            }
            _ => {}
        }
    }

    fn rate_window_from_bucket(&self, label: &str, bucket: &QuotaBucket) -> RateWindow {
        let used = ((1.0 - bucket.remaining_fraction.clamp(0.0, 1.0)) * 100.0).round();
        RateWindow {
            label: label.to_string(),
            used_percent: used,
            resets_at: bucket.reset_time.as_ref().and_then(|s| {
                DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            period_duration_ms: Some(24 * 60 * 60 * 1000),
        }
    }

    fn classify_model(model_id: &str) -> Option<(GeminiCategory, Vec<u32>, bool)> {
        let lower = model_id.to_lowercase();
        let category = if lower.contains("flash-lite") {
            GeminiCategory::FlashLite
        } else if lower.contains("flash") {
            GeminiCategory::Flash
        } else if lower.contains("pro") {
            GeminiCategory::Pro
        } else {
            return None;
        };

        let version = lower
            .strip_prefix("gemini-")
            .and_then(|rest| rest.split('-').next())
            .map(|version_text| {
                version_text
                    .split('.')
                    .filter_map(|part| part.parse::<u32>().ok())
                    .collect::<Vec<_>>()
            })
            .filter(|parts| !parts.is_empty())
            .unwrap_or_default();

        let is_preview = lower.contains("preview");

        Some((category, version, is_preview))
    }

    fn category_label(category: GeminiCategory, version: &[u32], is_preview: bool) -> String {
        let mut label = category.label().to_string();
        if !version.is_empty() {
            label.push_str(" (");
            label.push_str(
                &version
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("."),
            );
            if is_preview {
                label.push_str(" preview");
            }
            label.push(')');
        }
        label
    }
}

#[derive(Debug)]
struct QuotaBucket {
    model_id: String,
    remaining_fraction: f64,
    reset_time: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GeminiCategory {
    Pro,
    Flash,
    FlashLite,
}

impl GeminiCategory {
    fn label(self) -> &'static str {
        match self {
            GeminiCategory::Pro => "Pro",
            GeminiCategory::Flash => "Flash",
            GeminiCategory::FlashLite => "Flash Lite",
        }
    }
}

#[derive(Debug)]
struct SelectedBucket {
    bucket: QuotaBucket,
    version: Vec<u32>,
    is_preview: bool,
}

impl Default for GeminiQuotaFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QuotaFetcher for GeminiQuotaFetcher {
    fn provider_name(&self) -> &str {
        "gemini"
    }

    fn provider_display_name(&self) -> &str {
        "Gemini"
    }

    async fn fetch_quota(&self) -> Result<QuotaSnapshot> {
        let settings = self.auth.load_settings()?;
        if let Some(auth_type) = settings.selected_auth_type() {
            let auth_lower = auth_type.to_lowercase();
            if auth_lower == "api-key" {
                return Err(anyhow!("Gemini API key auth not supported yet"));
            }
            if auth_lower == "vertex-ai" {
                return Err(anyhow!("Gemini Vertex AI auth not supported yet"));
            }
        }

        let access_token = self.access_token().await?;
        let code_assist_status = self.fetch_load_code_assist(&access_token).await?;
        let project_id = match code_assist_status.project_id.as_deref() {
            Some(project_id) => Some(project_id.to_string()),
            None => self.discover_project_id(&access_token).await?,
        };

        let quota_resp = self
            .fetch_quota_api(&access_token, project_id.as_deref())
            .await?;
        let bucket_value = quota_resp
            .buckets
            .or(quota_resp.quota_buckets)
            .unwrap_or(serde_json::Value::Null);
        let buckets = self.extract_quota_buckets(&bucket_value);

        let mut model_buckets: BTreeMap<String, QuotaBucket> = BTreeMap::new();
        for bucket in buckets {
            let key = bucket.model_id.to_lowercase();
            match model_buckets.get(&key) {
                Some(existing) if existing.remaining_fraction <= bucket.remaining_fraction => {}
                _ => {
                    model_buckets.insert(key, bucket);
                }
            }
        }

        let mut selected_by_category: BTreeMap<GeminiCategory, SelectedBucket> = BTreeMap::new();
        for bucket in model_buckets.into_values() {
            let Some((category, version, is_preview)) = Self::classify_model(&bucket.model_id)
            else {
                continue;
            };

            let replace = match selected_by_category.get(&category) {
                None => true,
                Some(existing) if version > existing.version => true,
                Some(existing)
                    if version == existing.version && existing.is_preview && !is_preview =>
                {
                    true
                }
                Some(existing)
                    if version == existing.version
                        && existing.is_preview == is_preview
                        && bucket.remaining_fraction < existing.bucket.remaining_fraction =>
                {
                    true
                }
                _ => false,
            };

            if replace {
                selected_by_category.insert(
                    category,
                    SelectedBucket {
                        bucket,
                        version,
                        is_preview,
                    },
                );
            }
        }

        let mut windows = Vec::new();
        for category in [
            GeminiCategory::Pro,
            GeminiCategory::Flash,
            GeminiCategory::FlashLite,
        ] {
            if let Some(selected) = selected_by_category.get(&category) {
                windows.push(self.rate_window_from_bucket(
                    &Self::category_label(category, &selected.version, selected.is_preview),
                    &selected.bucket,
                ));
            }
        }

        if windows.is_empty() {
            windows.push(RateWindow {
                label: "Usage".to_string(),
                used_percent: 0.0,
                resets_at: None,
                period_duration_ms: None,
            });
        }

        Ok(QuotaSnapshot {
            provider: "gemini".to_string(),
            plan: code_assist_status.plan,
            windows,
            credits: None,
            fetched_at: Utc::now(),
        })
    }
}
