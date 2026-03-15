use crate::auth::codex::CodexAuth;
use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow, CreditInfo};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, info};

const QUOTA_API_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const TOKEN_REFRESH_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Debug, Deserialize)]
struct CodexQuotaResponse {
    rate_limit: Option<RateLimit>,
    credits: Option<Credits>,
    plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RateLimit {
    primary_window: Option<WindowInfo>,
    secondary_window: Option<WindowInfo>,
}

#[derive(Debug, Deserialize)]
struct WindowInfo {
    used_percent: f64,
}

#[derive(Debug, Deserialize)]
struct Credits {
    balance: f64,
}

pub struct CodexQuotaFetcher {
    client: Client,
    auth: CodexAuth,
}

impl CodexQuotaFetcher {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            auth: CodexAuth::new(),
        }
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<(String, String)> {
        info!("Refreshing Codex token");
        
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
        }

        let token: TokenResponse = response.json().await?;
        Ok((token.access_token, token.refresh_token))
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

        let response = self
            .client
            .get(QUOTA_API_URL)
            .bearer_auth(&creds.tokens.access_token)
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            info!("Token unauthorized, refreshing...");
            let (access_token, refresh_token) = 
                self.refresh_token(&creds.tokens.refresh_token).await?;
            
            let mut new_creds = creds.clone();
            new_creds.tokens.access_token = access_token;
            new_creds.tokens.refresh_token = refresh_token;
            self.auth.save_credentials(&new_creds)?;

            return self.fetch_quota().await;
        }

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await?;
            return Err(anyhow!("Quota API error {}: {}", status, text));
        }

        let quota: CodexQuotaResponse = response.json().await?;
        debug!("Quota response: {:?}", quota);

        let mut windows = Vec::new();

        if let Some(rate_limit) = quota.rate_limit {
            if let Some(primary) = rate_limit.primary_window {
                windows.push(RateWindow {
                    label: "Session (5h)".to_string(),
                    used_percent: primary.used_percent,
                    resets_at: None,
                });
            }

            if let Some(secondary) = rate_limit.secondary_window {
                windows.push(RateWindow {
                    label: "Weekly (7d)".to_string(),
                    used_percent: secondary.used_percent,
                    resets_at: None,
                });
            }
        }

        let credits = quota.credits.map(|c| CreditInfo {
            used: c.balance,
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
