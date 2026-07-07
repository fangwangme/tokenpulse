use crate::auth::claude::ClaudeAuth;
use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow};
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
    limits: Vec<UsageLimit>,
}

#[derive(Debug, Deserialize)]
struct WindowUsage {
    #[serde(default)]
    utilization: f64,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageLimit {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    percent: f64,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(default)]
    scope: Option<UsageLimitScope>,
}

#[derive(Debug, Deserialize)]
struct UsageLimitScope {
    #[serde(default)]
    model: Option<UsageLimitModel>,
}

#[derive(Debug, Deserialize)]
struct UsageLimitModel {
    #[serde(default)]
    display_name: Option<String>,
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
        let mut creds = self.auth.load_credentials()?;
        let mut token = creds.claude_ai_oauth.access_token.clone();

        if self.auth.is_token_expired(&creds) {
            debug!("Claude token is expired, attempting to refresh");
            match self.auth.refresh_token(&mut creds) {
                Ok(new_token) => {
                    token = new_token;
                }
                Err(e) => {
                    debug!(
                        "Failed to refresh Claude token: {}. Falling back to existing token.",
                        e
                    );
                }
            }
        }

        let mut response = self
            .client
            .get(QUOTA_API_URL)
            .bearer_auth(&token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("Accept", "application/json")
            .send()
            .await?;

        let mut status = response.status();

        if status == reqwest::StatusCode::UNAUTHORIZED {
            debug!("Claude quota fetch returned 401 Unauthorized, attempting to refresh token");
            match self.auth.refresh_token(&mut creds) {
                Ok(new_token) => {
                    token = new_token;
                    response = self
                        .client
                        .get(QUOTA_API_URL)
                        .bearer_auth(&token)
                        .header("anthropic-beta", "oauth-2025-04-20")
                        .header("Accept", "application/json")
                        .send()
                        .await?;
                    status = response.status();
                }
                Err(e) => {
                    return Err(anyhow!(
                        "Claude session expired and token refresh failed: {}",
                        e
                    ));
                }
            }
        }

        let body = response.text().await?;
        debug!(
            "Claude quota response status: {}, {} bytes",
            status,
            body.len()
        );

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

        for limit in quota.limits {
            let is_fable = limit
                .scope
                .as_ref()
                .and_then(|s| s.model.as_ref())
                .and_then(|m| m.display_name.as_deref())
                .map(|name| name == "Fable")
                .unwrap_or(false);
            let is_weekly = limit.kind.as_deref() == Some("weekly_scoped");
            if is_fable && is_weekly {
                windows.push(RateWindow {
                    label: "Fable (7d)".to_string(),
                    used_percent: limit.percent,
                    resets_at: limit.resets_at.and_then(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|d| d.with_timezone(&Utc))
                    }),
                    period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
                });
            }
        }

        Ok(QuotaSnapshot {
            provider: "claude".to_string(),
            plan: Some("Pro".to_string()),
            account: None,
            windows,
            // Claude Code credit usage is intentionally not surfaced.
            credits: None,
            rate_limit_reset_credits: vec![],
            fetched_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_quota_response_deserializes_usage_windows() {
        let quota: ClaudeQuotaResponse = serde_json::from_str(
            r#"{
                "five_hour":{"utilization":42.5,"resets_at":"2026-04-10T10:00:00Z"},
                "seven_day":{"utilization":18.0,"resets_at":"2026-04-14T00:00:00Z"}
            }"#,
        )
        .unwrap();

        assert_eq!(quota.five_hour.unwrap().utilization, 42.5);
        assert_eq!(quota.seven_day.unwrap().utilization, 18.0);
    }

    #[test]
    fn claude_quota_response_parses_fable_limit() {
        let quota: ClaudeQuotaResponse = serde_json::from_str(
            r#"{
                "five_hour":null,
                "seven_day":null,
                "seven_day_sonnet":null,
                "seven_day_opus":null,
                "limits":[
                    {"kind":"session","group":"session","percent":24,"resets_at":null,"scope":null},
                    {"kind":"weekly_all","group":"weekly","percent":73,"resets_at":null,"scope":null},
                    {"kind":"weekly_scoped","group":"weekly","percent":76,"resets_at":"2026-07-04T14:00:00Z","scope":{"model":{"id":null,"display_name":"Fable"}}}
                ]
            }"#,
        )
        .unwrap();

        let fable = quota
            .limits
            .into_iter()
            .find(|l| {
                l.kind.as_deref() == Some("weekly_scoped")
                    && l.scope
                        .as_ref()
                        .and_then(|s| s.model.as_ref())
                        .and_then(|m| m.display_name.as_deref())
                        == Some("Fable")
            })
            .expect("fable limit present");

        assert_eq!(fable.percent, 76.0);
        assert_eq!(fable.resets_at.as_deref(), Some("2026-07-04T14:00:00Z"));
    }
}
