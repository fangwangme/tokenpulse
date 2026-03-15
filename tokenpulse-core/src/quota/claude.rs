use crate::auth::claude::ClaudeAuth;
use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow, CreditInfo};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info, warn};

const QUOTA_API_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TOKEN_REFRESH_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

#[derive(Debug, Deserialize)]
struct ClaudeQuotaResponse {
    five_hour: Option<WindowUsage>,
    seven_day: Option<WindowUsage>,
    seven_day_sonnet: Option<WindowUsage>,
    seven_day_opus: Option<WindowUsage>,
    extra_usage: Option<ExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct WindowUsage {
    utilization: f64,
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExtraUsage {
    used: f64,
    limit: f64,
}

pub struct ClaudeQuotaFetcher {
    client: Client,
    auth: ClaudeAuth,
}

impl ClaudeQuotaFetcher {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            auth: ClaudeAuth::new(),
        }
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<(String, String, i64)> {
        info!("Refreshing Claude token");
        
        let response = self
            .client
            .post(TOKEN_REFRESH_URL)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLIENT_ID),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let text = response.text().await?;
            return Err(anyhow!("Token refresh failed: {}", text));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            refresh_token: String,
            expires_in: i64,
        }

        let token: TokenResponse = response.json().await?;
        let expires_at = Utc::now().timestamp() + token.expires_in;

        Ok((token.access_token, token.refresh_token, expires_at))
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

        if self.auth.is_token_expired(&creds) {
            info!("Token expired, refreshing...");
            let (access_token, refresh_token, expires_at) = 
                self.refresh_token(&creds.claude_ai_oauth.refresh_token).await?;
            
            creds.claude_ai_oauth.access_token = access_token;
            creds.claude_ai_oauth.refresh_token = refresh_token;
            creds.claude_ai_oauth.expires_at = expires_at;
            
            self.auth.save_credentials(&creds)?;
        }

        let response = self
            .client
            .get(QUOTA_API_URL)
            .bearer_auth(&creds.claude_ai_oauth.access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await?;
            return Err(anyhow!("Quota API error {}: {}", status, text));
        }

        let quota: ClaudeQuotaResponse = response.json().await?;
        debug!("Quota response: {:?}", quota);

        let mut windows = Vec::new();

        if let Some(five_hour) = quota.five_hour {
            windows.push(RateWindow {
                label: "Session (5h)".to_string(),
                used_percent: five_hour.utilization * 100.0,
                resets_at: five_hour.resets_at.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc))),
            });
        }

        if let Some(seven_day) = quota.seven_day {
            windows.push(RateWindow {
                label: "Weekly (7d)".to_string(),
                used_percent: seven_day.utilization * 100.0,
                resets_at: seven_day.resets_at.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc))),
            });
        }

        if let Some(sonnet) = quota.seven_day_sonnet {
            windows.push(RateWindow {
                label: "Sonnet".to_string(),
                used_percent: sonnet.utilization * 100.0,
                resets_at: sonnet.resets_at.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc))),
            });
        }

        if let Some(opus) = quota.seven_day_opus {
            windows.push(RateWindow {
                label: "Opus".to_string(),
                used_percent: opus.utilization * 100.0,
                resets_at: opus.resets_at.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&Utc))),
            });
        }

        let credits = quota.extra_usage.map(|e| CreditInfo {
            used: e.used,
            limit: Some(e.limit),
            currency: "USD".to_string(),
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
