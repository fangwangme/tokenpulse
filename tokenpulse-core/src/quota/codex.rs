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

    fn rate_window_from_window(position: WindowPosition, window: WindowInfo) -> RateWindow {
        let resets_at = if let Some(ts) = window.reset_at {
            Utc.timestamp_opt(ts, 0).single()
        } else if let Some(reset_after_seconds) = window.reset_after_seconds {
            Some(Utc::now() + chrono::Duration::seconds(reset_after_seconds))
        } else {
            None
        };

        RateWindow {
            label: window_label(position, window.limit_window_seconds),
            used_percent: window.used_percent.0,
            resets_at,
            period_duration_ms: window
                .limit_window_seconds
                .filter(|seconds| *seconds > 0)
                .map(|seconds| seconds * 1000),
        }
    }

    fn rate_windows(rate_limit: Option<RateLimit>) -> Vec<RateWindow> {
        let Some(rate_limit) = rate_limit else {
            return Vec::new();
        };
        [
            (WindowPosition::Primary, rate_limit.primary_window),
            (WindowPosition::Secondary, rate_limit.secondary_window),
        ]
        .into_iter()
        .filter_map(|(position, window)| {
            window.map(|window| Self::rate_window_from_window(position, window))
        })
        .collect()
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

#[derive(Clone, Copy)]
enum WindowPosition {
    Primary,
    Secondary,
}

fn window_label(position: WindowPosition, seconds: Option<i64>) -> String {
    match seconds {
        Some(18_000) => "Session (5h)".to_string(),
        Some(604_800) => "Weekly (7d)".to_string(),
        Some(seconds) if seconds > 0 => format!("Window ({})", format_window_duration(seconds)),
        _ => match position {
            WindowPosition::Primary => "Primary window".to_string(),
            WindowPosition::Secondary => "Secondary window".to_string(),
        },
    }
}

fn format_window_duration(seconds: i64) -> String {
    const DAY: i64 = 24 * 60 * 60;
    const HOUR: i64 = 60 * 60;
    const MINUTE: i64 = 60;
    if seconds % DAY == 0 {
        format!("{}d", seconds / DAY)
    } else if seconds % HOUR == 0 {
        format!("{}h", seconds / HOUR)
    } else if seconds % MINUTE == 0 {
        format!("{}m", seconds / MINUTE)
    } else {
        format!("{seconds}s")
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
        let mut candidate = self.auth.load_auth_candidate()?;

        if let Some(ref tokens) = candidate.credentials.tokens {
            if self.auth.is_token_expired(tokens) {
                debug!("Codex access token expired or near expiry; attempting proactive refresh");
                match self.auth.refresh_tokens(&candidate, &self.client).await {
                    Ok(updated) => {
                        candidate = updated;
                    }
                    Err(e) => {
                        debug!("Proactive Codex token refresh failed: {}", e);
                    }
                }
            }
        }

        let mut access_token = candidate
            .credentials
            .tokens
            .as_ref()
            .map(|t| t.access_token.clone())
            .ok_or_else(|| anyhow!("No tokens found in Codex credentials"))?;

        debug!("Fetching Codex quota with access token");

        let mut response = self
            .client
            .get(QUOTA_API_URL)
            .bearer_auth(&access_token)
            .send()
            .await?;

        debug!("Codex quota response status: {}", response.status());

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            debug!("Codex returned 401 Unauthorized; attempting reactive token refresh");
            match self.auth.refresh_tokens(&candidate, &self.client).await {
                Ok(updated) => {
                    candidate = updated;
                    if let Some(ref refreshed_tokens) = candidate.credentials.tokens {
                        access_token = refreshed_tokens.access_token.clone();
                        response = self
                            .client
                            .get(QUOTA_API_URL)
                            .bearer_auth(&access_token)
                            .send()
                            .await?;
                    }
                }
                Err(e) => {
                    debug!("Reactive Codex token refresh failed: {}", e);
                }
            }
        }

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

        let windows = Self::rate_windows(quota.rate_limit);

        let email = self.auth.load_email();

        let rate_limit_reset_credits = match self.fetch_reset_credits(&access_token).await {
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
        let window = CodexQuotaFetcher::rate_window_from_window(
            WindowPosition::Primary,
            WindowInfo {
                used_percent: FlexNumber(63.0),
                limit_window_seconds: Some(18_000),
                reset_after_seconds: Some(60),
                reset_at: Some(1_744_246_400),
            },
        );

        assert_eq!(window.label, "Session (5h)");
        assert_eq!(window.used_percent, 63.0);
        assert_eq!(window.period_duration_ms, Some(18_000_000));
        assert_eq!(
            window.resets_at,
            Utc.timestamp_opt(1_744_246_400, 0).single()
        );
    }

    fn windows_from_fixture(json: &str) -> Vec<RateWindow> {
        let response: CodexQuotaResponse = serde_json::from_str(json).unwrap();
        CodexQuotaFetcher::rate_windows(response.rate_limit)
    }

    #[test]
    fn maps_only_returned_five_hour_window() {
        let windows = windows_from_fixture(
            r#"{"rate_limit":{"primary_window":{"used_percent":12,"limit_window_seconds":18000},"secondary_window":null}}"#,
        );

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Session (5h)");
        assert_eq!(windows[0].period_duration_ms, Some(18_000_000));
    }

    #[test]
    fn maps_only_returned_weekly_primary_window_as_weekly() {
        let windows = windows_from_fixture(
            r#"{"rate_limit":{"primary_window":{"used_percent":34,"limit_window_seconds":604800},"secondary_window":null}}"#,
        );

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Weekly (7d)");
        assert_eq!(windows[0].period_duration_ms, Some(604_800_000));
    }

    #[test]
    fn maps_both_returned_windows_by_duration_not_position() {
        let windows = windows_from_fixture(
            r#"{"rate_limit":{"primary_window":{"used_percent":50,"limit_window_seconds":604800},"secondary_window":{"used_percent":25,"limit_window_seconds":18000}}}"#,
        );

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "Weekly (7d)");
        assert_eq!(windows[1].label, "Session (5h)");
    }

    #[test]
    fn maps_no_windows_to_empty_output() {
        let windows = windows_from_fixture(
            r#"{"rate_limit":{"primary_window":null,"secondary_window":null}}"#,
        );

        assert!(windows.is_empty());
    }

    #[test]
    fn maps_unknown_and_missing_durations_to_neutral_labels() {
        let windows = windows_from_fixture(
            r#"{"rate_limit":{"primary_window":{"used_percent":9,"limit_window_seconds":86400},"secondary_window":{"used_percent":3}}}"#,
        );

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "Window (1d)");
        assert_eq!(windows[0].period_duration_ms, Some(86_400_000));
        assert_eq!(windows[1].label, "Secondary window");
        assert_eq!(windows[1].period_duration_ms, None);
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
