use crate::auth::codex::CodexAuth;
use crate::provider::{QuotaFetcher, QuotaSnapshot, RateLimitResetCredit, RateWindow};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tracing::debug;

const REQUEST_TIMEOUT_SECS: u64 = 20;

const QUOTA_API_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const RESET_CREDITS_API_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";

#[derive(Debug, Deserialize)]
struct CodexQuotaResponse {
    #[serde(default)]
    rate_limit: Option<RateLimit>,
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

#[derive(Debug, Clone)]
struct FlexNumber(f64);

#[derive(Debug, Deserialize)]
struct ResetCreditsResponse {
    #[serde(default)]
    credits: Vec<CodexResetCredit>,
}

#[derive(Debug, Deserialize)]
struct CodexResetCredit {
    id: String,
    #[serde(default)]
    reset_type: Option<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    granted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

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
            where
                E: de::Error,
            {
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

    fn rate_window_from_window(
        &self,
        label: &str,
        window: WindowInfo,
        fallback_window_seconds: i64,
    ) -> RateWindow {
        let resets_at = if let Some(ts) = window.reset_at {
            Utc.timestamp_opt(ts, 0).single()
        } else if let Some(reset_after_seconds) = window.reset_after_seconds {
            Some(Utc::now() + chrono::Duration::seconds(reset_after_seconds))
        } else {
            None
        };

        RateWindow {
            label: label.to_string(),
            used_percent: window.used_percent.0,
            resets_at,
            period_duration_ms: Some(
                window
                    .limit_window_seconds
                    .unwrap_or(fallback_window_seconds)
                    * 1000,
            ),
        }
    }

    async fn fetch_reset_credits(&self, access_token: &str) -> Result<Vec<RateLimitResetCredit>> {
        let response = self
            .client
            .get(RESET_CREDITS_API_URL)
            .bearer_auth(access_token)
            .send()
            .await?;

        debug!("Codex reset-credit response status: {}", response.status());

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Codex reset-credit API error {}: {}", status, text));
        }

        let response_text = response.text().await?;
        let parsed: ResetCreditsResponse = serde_json::from_str(&response_text)?;
        Ok(parsed
            .credits
            .into_iter()
            .filter(|credit| credit.status == "available")
            .map(|credit| RateLimitResetCredit {
                id: credit.id,
                reset_type: credit.reset_type,
                status: credit.status,
                granted_at: credit.granted_at,
                expires_at: credit.expires_at,
            })
            .collect())
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

        let tokens = creds
            .tokens
            .as_ref()
            .ok_or_else(|| anyhow!("No tokens found in Codex credentials"))?;

        debug!("Fetching Codex quota with access token");

        let response = self
            .client
            .get(QUOTA_API_URL)
            .bearer_auth(&tokens.access_token)
            .send()
            .await?;

        debug!("Codex quota response status: {}", response.status());

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!(
                "Codex session expired. Please run `codex` to refresh your session."
            ));
        }

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await?;
            return Err(anyhow!("Codex Quota API error {}: {}", status, text));
        }

        let response_text = response.text().await?;
        debug!(
            "Codex quota response body length: {} bytes",
            response_text.len()
        );

        let quota: CodexQuotaResponse = match serde_json::from_str(&response_text) {
            Ok(q) => q,
            Err(e) => {
                return Err(anyhow!(
                    "Failed to parse Codex quota response: {}. First 200 chars: {}",
                    e,
                    &response_text[..response_text.len().min(200)]
                ));
            }
        };
        debug!(
            "Codex quota parsed: plan={:?}, has_rate_limit={}",
            quota.plan_type.as_deref(),
            quota.rate_limit.is_some()
        );

        let mut windows = Vec::new();

        if let Some(rate_limit) = quota.rate_limit {
            if let Some(primary) = rate_limit.primary_window {
                windows.push(self.rate_window_from_window("Session (5h)", primary, 5 * 60 * 60));
            }

            if let Some(secondary) = rate_limit.secondary_window {
                windows.push(self.rate_window_from_window(
                    "Weekly (7d)",
                    secondary,
                    7 * 24 * 60 * 60,
                ));
            }
        }

        let email = self.auth.load_email();

        let rate_limit_reset_credits = match self.fetch_reset_credits(&tokens.access_token).await {
            Ok(credits) => credits,
            Err(err) => {
                debug!("Codex reset-credit fetch skipped: {}", err);
                Vec::new()
            }
        };

        Ok(QuotaSnapshot {
            provider: "codex".to_string(),
            plan: quota.plan_type,
            account: email,
            windows,
            credits: None,
            rate_limit_reset_credits,
            fetched_at: Utc::now(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flex_number_deserializes_numbers_and_strings() {
        let from_number: FlexNumber = serde_json::from_str("42.5").unwrap();
        let from_string: FlexNumber = serde_json::from_str(r#""17.25""#).unwrap();

        assert!((from_number.0 - 42.5).abs() < 0.001);
        assert!((from_string.0 - 17.25).abs() < 0.001);
    }

    #[test]
    fn rate_window_from_window_uses_reset_at_when_present() {
        let fetcher = CodexQuotaFetcher::new();
        let window = fetcher.rate_window_from_window(
            "Session (5h)",
            WindowInfo {
                used_percent: FlexNumber(63.0),
                limit_window_seconds: Some(18_000),
                reset_after_seconds: Some(60),
                reset_at: Some(1_744_246_400),
            },
            18_000,
        );

        assert_eq!(window.label, "Session (5h)");
        assert_eq!(window.used_percent, 63.0);
        assert_eq!(window.period_duration_ms, Some(18_000_000));
        assert_eq!(
            window.resets_at,
            Utc.timestamp_opt(1_744_246_400, 0).single()
        );
    }

    #[test]
    fn parses_reset_credit_expiry_list() {
        let response: ResetCreditsResponse = serde_json::from_str(
            r#"{
                "credits": [
                    {
                        "id": "RateLimitResetCredit_1",
                        "reset_type": "codex_rate_limits",
                        "status": "available",
                        "granted_at": "2026-06-18T00:37:13.295648Z",
                        "expires_at": "2026-07-18T00:37:13.295648Z"
                    }
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(response.credits.len(), 1);
        let credit = &response.credits[0];
        assert_eq!(credit.id, "RateLimitResetCredit_1");
        assert_eq!(credit.status, "available");
        assert_eq!(
            credit.expires_at,
            Some(
                DateTime::parse_from_rfc3339("2026-07-18T00:37:13.295648Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }
}
