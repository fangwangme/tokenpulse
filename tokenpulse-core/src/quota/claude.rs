use crate::auth::claude::ClaudeAuth;
use crate::provider::{CreditInfo, QuotaFetcher, QuotaSnapshot, RateWindow};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tracing::debug;

const REQUEST_TIMEOUT_SECS: u64 = 20;

const QUOTA_API_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Debug, Deserialize)]
struct ClaudeQuotaResponse {
    #[serde(default)]
    five_hour: Option<WindowUsage>,
    #[serde(default)]
    seven_day: Option<WindowUsage>,
    #[serde(default)]
    seven_day_sonnet: Option<WindowUsage>,
    #[serde(default)]
    seven_day_opus: Option<WindowUsage>,
    #[serde(default)]
    extra_usage: Option<ExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct WindowUsage {
    #[serde(default)]
    utilization: f64,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExtraUsage {
    #[serde(default)]
    is_enabled: bool,
    #[serde(default)]
    monthly_limit: Option<f64>,
    #[serde(default)]
    used_credits: Option<f64>,
}

pub struct ClaudeQuotaFetcher {
    client: Client,
    auth: ClaudeAuth,
}

impl ClaudeQuotaFetcher {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            auth: ClaudeAuth::new(),
        }
    }
}

impl Default for ClaudeQuotaFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QuotaFetcher for ClaudeQuotaFetcher {
    fn provider_name(&self) -> &str {
        "claude"
    }

    fn provider_display_name(&self) -> &str {
        "Claude Code"
    }

    async fn fetch_quota(&self) -> Result<QuotaSnapshot> {
        let creds = self.auth.load_credentials()?;

        if self.auth.is_token_expired(&creds) {
            return Err(anyhow!(
                "Claude token expired. Please run `claude` to refresh your session."
            ));
        }

        let response = self
            .client
            .get(QUOTA_API_URL)
            .bearer_auth(&creds.claude_ai_oauth.access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("Accept", "application/json")
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;
        debug!(
            "Claude quota response status: {}, {} bytes",
            status,
            body.len()
        );

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!(
                "Claude session expired. Please run `claude` to refresh your session."
            ));
        }

        if !status.is_success() {
            return Err(anyhow!("Quota API error {}: {}", status, body));
        }

        let quota: ClaudeQuotaResponse = serde_json::from_str(&body).map_err(|e| {
            anyhow!(
                "Failed to parse Claude quota response: {}. Body: {}",
                e,
                &body[..body.len().min(200)]
            )
        })?;
        let mut windows = Vec::new();

        if let Some(five_hour) = quota.five_hour {
            windows.push(RateWindow {
                label: "Session (5h)".to_string(),
                used_percent: five_hour.utilization,
                resets_at: five_hour.resets_at.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
                period_duration_ms: Some(5 * 60 * 60 * 1000),
            });
        }

        if let Some(seven_day) = quota.seven_day {
            windows.push(RateWindow {
                label: "Weekly (7d)".to_string(),
                used_percent: seven_day.utilization,
                resets_at: seven_day.resets_at.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
                period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
            });
        }

        if let Some(sonnet) = quota.seven_day_sonnet {
            windows.push(RateWindow {
                label: "Sonnet (7d)".to_string(),
                used_percent: sonnet.utilization,
                resets_at: sonnet.resets_at.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
                period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
            });
        }

        if let Some(opus) = quota.seven_day_opus {
            windows.push(RateWindow {
                label: "Opus (7d)".to_string(),
                used_percent: opus.utilization,
                resets_at: opus.resets_at.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
                period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
            });
        }

        let credits = quota.extra_usage.and_then(|e| {
            if e.is_enabled {
                Some(CreditInfo {
                    used: e.used_credits.unwrap_or(0.0),
                    limit: e.monthly_limit,
                    currency: "USD".to_string(),
                })
            } else {
                None
            }
        });

        Ok(QuotaSnapshot {
            provider: "claude".to_string(),
            plan: Some("Pro".to_string()),
            windows,
            credits,
            fetched_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_quota_response_deserializes_usage_windows_and_credits() {
        let quota: ClaudeQuotaResponse = serde_json::from_str(
            r#"{
                "five_hour":{"utilization":42.5,"resets_at":"2026-04-10T10:00:00Z"},
                "seven_day":{"utilization":18.0,"resets_at":"2026-04-14T00:00:00Z"},
                "extra_usage":{"is_enabled":true,"monthly_limit":100.0,"used_credits":12.5}
            }"#,
        )
        .unwrap();

        assert_eq!(quota.five_hour.unwrap().utilization, 42.5);
        assert_eq!(quota.seven_day.unwrap().utilization, 18.0);
        let extra = quota.extra_usage.unwrap();
        assert!(extra.is_enabled);
        assert_eq!(extra.monthly_limit, Some(100.0));
        assert_eq!(extra.used_credits, Some(12.5));
    }
}
