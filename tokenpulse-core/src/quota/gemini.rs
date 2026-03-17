use crate::auth::gemini::GeminiAuth;
use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tracing::debug;

const REQUEST_TIMEOUT_SECS: u64 = 20;

const LOAD_CODE_ASSIST_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const QUOTA_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";

#[derive(Debug, Deserialize)]
struct LoadCodeAssistResponse {
    #[serde(default)]
    tier: Option<String>,
    #[serde(default)]
    user_tier: Option<String>,
    #[serde(default)]
    subscription_tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuotaResponse {
    #[serde(default)]
    quota_buckets: Option<serde_json::Value>,
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

    async fn fetch_load_code_assist(&self, access_token: &str) -> Result<(Option<String>, String)> {
        let response = self
            .client
            .post(LOAD_CODE_ASSIST_URL)
            .bearer_auth(access_token)
            .json(&serde_json::json!({
                "metadata": {
                    "ideType": "IDE_UNSPECIFIED",
                    "platform": "PLATFORM_UNSPECIFIED",
                    "pluginType": "GEMINI",
                    "duetProject": "default"
                }
            }))
            .send()
            .await?;

        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            return Err(anyhow!("Gemini session expired. Please run `gemini` to refresh your session."));
        }

        if !response.status().is_success() {
            return Err(anyhow!("LoadCodeAssist failed: {}", response.status()));
        }

        let data: LoadCodeAssistResponse = response.json().await?;
        let tier = data.tier.or(data.user_tier).or(data.subscription_tier);

        let plan = match tier.as_deref().map(|s| s.to_lowercase()) {
            Some(s) if s.contains("standard") => Some("Paid".to_string()),
            Some(s) if s.contains("legacy") => Some("Legacy".to_string()),
            Some(s) if s.contains("free") => Some("Free".to_string()),
            _ => None,
        };

        Ok((plan, access_token.to_string()))
    }

    async fn fetch_quota_api(&self, access_token: &str, project_id: Option<&str>) -> Result<QuotaResponse> {
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
}

#[derive(Debug)]
struct QuotaBucket {
    model_id: String,
    remaining_fraction: f64,
    reset_time: Option<String>,
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
        if let Some(auth_type) = settings.auth_type.as_ref() {
            let auth_lower = auth_type.to_lowercase();
            if auth_lower == "api-key" {
                return Err(anyhow!("Gemini API key auth not supported yet"));
            }
            if auth_lower == "vertex-ai" {
                return Err(anyhow!("Gemini Vertex AI auth not supported yet"));
            }
        }

        let creds = self.auth.load_credentials()?;

        if self.auth.is_token_expired(&creds) {
            return Err(anyhow!("Gemini token expired. Please run `gemini` to refresh your session."));
        }

        let access_token = match creds.access_token {
            Some(t) => t,
            None => {
                return Err(anyhow!("No Gemini access token found. Please run `gemini` to login."));
            }
        };

        let (plan, token) = self.fetch_load_code_assist(&access_token).await?;

        let quota_resp = self.fetch_quota_api(&token, None).await?;
        let buckets = self.extract_quota_buckets(&quota_resp.quota_buckets.unwrap_or(serde_json::Value::Null));

        let mut pro_bucket: Option<&QuotaBucket> = None;
        let mut flash_bucket: Option<&QuotaBucket> = None;

        for bucket in &buckets {
            let lower = bucket.model_id.to_lowercase();
            if lower.contains("gemini") && lower.contains("pro") {
                if pro_bucket.is_none() || bucket.remaining_fraction < pro_bucket.unwrap().remaining_fraction {
                    pro_bucket = Some(bucket);
                }
            } else if lower.contains("gemini") && lower.contains("flash") {
                if flash_bucket.is_none() || bucket.remaining_fraction < flash_bucket.unwrap().remaining_fraction {
                    flash_bucket = Some(bucket);
                }
            }
        }

        let mut windows = Vec::new();

        if let Some(pro) = pro_bucket {
            let used = ((1.0 - pro.remaining_fraction.clamp(0.0, 1.0)) * 100.0).round();
            windows.push(RateWindow {
                label: "Pro".to_string(),
                used_percent: used,
                resets_at: pro.reset_time.as_ref().and_then(|s| {
                    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
                }),
                period_duration_ms: Some(5 * 60 * 60 * 1000),
            });
        }

        if let Some(flash) = flash_bucket {
            let used = ((1.0 - flash.remaining_fraction.clamp(0.0, 1.0)) * 100.0).round();
            windows.push(RateWindow {
                label: "Flash".to_string(),
                used_percent: used,
                resets_at: flash.reset_time.as_ref().and_then(|s| {
                    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
                }),
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

        Ok(QuotaSnapshot {
            provider: "gemini".to_string(),
            plan,
            windows,
            credits: None,
            fetched_at: Utc::now(),
        })
    }
}
