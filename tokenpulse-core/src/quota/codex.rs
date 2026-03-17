use crate::auth::codex::CodexAuth;
use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow, CreditInfo};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tracing::debug;

const REQUEST_TIMEOUT_SECS: u64 = 20;

const QUOTA_API_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

#[derive(Debug, Deserialize)]
struct CodexQuotaResponse {
    #[serde(default)]
    rate_limit: Option<RateLimit>,
    #[serde(default)]
    credits: Option<Credits>,
    #[serde(default)]
    plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RateLimit {
    #[serde(default)]
    primary_window: Option<WindowInfo>,
    #[serde(default)]
    secondary_window: Option<WindowInfo>,
}

#[derive(Debug, Deserialize)]
struct WindowInfo {
    #[serde(default)]
    used_percent: FlexNumber,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Credits {
    #[serde(default)]
    balance: FlexNumber,
}

#[derive(Debug, Clone)]
struct FlexNumber(f64);

impl Default for FlexNumber {
    fn default() -> Self {
        FlexNumber(0.0)
    }
}

impl<'de> Deserialize<'de> for FlexNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, Visitor};
        struct FlexNumberVisitor;
        impl<'de> Visitor<'de> for FlexNumberVisitor {
            type Value = FlexNumber;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a number or string")
            }
            fn visit_f64<E>(self, v: f64) -> Result<FlexNumber, E> {
                Ok(FlexNumber(v))
            }
            fn visit_i64<E>(self, v: i64) -> Result<FlexNumber, E> {
                Ok(FlexNumber(v as f64))
            }
            fn visit_u64<E>(self, v: u64) -> Result<FlexNumber, E> {
                Ok(FlexNumber(v as f64))
            }
            fn visit_str<E>(self, v: &str) -> Result<FlexNumber, E>
            where E: de::Error {
                v.parse::<f64>().map(FlexNumber).map_err(de::Error::custom)
            }
        }
        deserializer.deserialize_any(FlexNumberVisitor)
    }
}

pub struct CodexQuotaFetcher {
    client: Client,
    auth: CodexAuth,
}

impl CodexQuotaFetcher {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            auth: CodexAuth::new(),
        }
    }
}

impl Default for CodexQuotaFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QuotaFetcher for CodexQuotaFetcher {
    fn provider_name(&self) -> &str {
        "codex"
    }

    fn provider_display_name(&self) -> &str {
        "Codex"
    }

    async fn fetch_quota(&self) -> Result<QuotaSnapshot> {
        let creds = self.auth.load_credentials()?;

        let tokens = creds.tokens.as_ref().ok_or_else(|| anyhow!("No tokens found in Codex credentials"))?;

        debug!("Fetching Codex quota with access token");

        let response = self
            .client
            .get(QUOTA_API_URL)
            .bearer_auth(&tokens.access_token)
            .send()
            .await?;

        debug!("Codex quota response status: {}", response.status());

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!("Codex session expired. Please run `codex` to refresh your session."));
        }

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await?;
            return Err(anyhow!("Codex Quota API error {}: {}", status, text));
        }

        let response_text = response.text().await?;
        debug!("Codex quota response body length: {} bytes", response_text.len());

        let quota: CodexQuotaResponse = match serde_json::from_str(&response_text) {
            Ok(q) => q,
            Err(e) => {
                return Err(anyhow!("Failed to parse Codex quota response: {}. First 200 chars: {}", e, &response_text[..response_text.len().min(200)]));
            }
        };
        debug!("Quota response: {:?}", quota);

        let mut windows = Vec::new();

        if let Some(rate_limit) = quota.rate_limit {
            if let Some(primary) = rate_limit.primary_window {
                let resets_at = primary.reset_at.map(|ts| {
                    chrono::TimeZone::timestamp(&Utc, ts, 0)
                });
                windows.push(RateWindow {
                    label: "Session (5h)".to_string(),
                    used_percent: primary.used_percent.0,
                    resets_at,
                    period_duration_ms: Some(5 * 60 * 60 * 1000),
                });
            }

            if let Some(secondary) = rate_limit.secondary_window {
                let resets_at = secondary.reset_at.map(|ts| {
                    chrono::TimeZone::timestamp(&Utc, ts, 0)
                });
                windows.push(RateWindow {
                    label: "Weekly (7d)".to_string(),
                    used_percent: secondary.used_percent.0,
                    resets_at,
                    period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
                });
            }
        }

        let credits = quota.credits.map(|c| CreditInfo {
            used: c.balance.0,
            limit: None,
            currency: "USD".to_string(),
        });

        Ok(QuotaSnapshot {
            provider: "codex".to_string(),
            plan: quota.plan_type,
            windows,
            credits,
            fetched_at: Utc::now(),
        })
    }
}
