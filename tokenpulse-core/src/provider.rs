use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub provider: String,
    pub plan: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    pub windows: Vec<RateWindow>,
    pub credits: Option<CreditInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rate_limit_reset_credits: Vec<RateLimitResetCredit>,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateWindow {
    pub label: String,
    /// Model family this window meters, when the provider splits quota by one.
    ///
    /// Antigravity bills Gemini and Claude separately, and Claude tracks Opus,
    /// Sonnet and Fable on their own weekly windows. Providers with a single
    /// pooled quota leave this empty. Kept out of `label` so history can group
    /// by family without re-parsing display text.
    #[serde(default)]
    pub model_family: Option<String>,
    pub used_percent: f64,
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub period_duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditInfo {
    pub used: f64,
    pub limit: Option<f64>,
    pub currency: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitResetCredit {
    pub id: String,
    #[serde(default)]
    pub reset_type: Option<String>,
    pub status: String,
    #[serde(default)]
    pub granted_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenBreakdown {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub reasoning: i64,
}

impl TokenBreakdown {
    pub fn total(&self) -> i64 {
        self.input + self.output + self.cache_read + self.cache_write + self.reasoning
    }

    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

impl Default for TokenBreakdown {
    fn default() -> Self {
        Self {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedMessage {
    pub client: String,
    #[serde(default)]
    pub client_detail: Option<String>,
    pub model_id: String,
    pub provider_id: String,
    pub session_id: String,
    pub message_key: String,
    pub timestamp: i64,
    pub date: String,
    pub tokens: TokenBreakdown,
    pub cost: f64,
    pub pricing_day: String,
    pub parser_version: String,
}

impl UnifiedMessage {
    pub fn new(
        client: impl Into<String>,
        model_id: impl Into<String>,
        provider_id: impl Into<String>,
        session_id: impl Into<String>,
        message_key: impl Into<String>,
        timestamp: i64,
        tokens: TokenBreakdown,
    ) -> Self {
        let date = local_date_string_from_timestamp(timestamp);

        Self {
            client: client.into(),
            client_detail: None,
            model_id: model_id.into(),
            provider_id: provider_id.into(),
            session_id: session_id.into(),
            message_key: message_key.into(),
            timestamp,
            date: date.clone(),
            tokens,
            cost: 0.0,
            pricing_day: date,
            parser_version: "v1".to_string(),
        }
    }

    pub fn with_client_detail(mut self, client_detail: impl Into<String>) -> Self {
        self.client_detail = Some(client_detail.into());
        self
    }

    pub fn with_cost(mut self, cost: f64) -> Self {
        self.cost = cost.max(0.0);
        self
    }

    pub fn with_parser_version(mut self, parser_version: impl Into<String>) -> Self {
        self.parser_version = parser_version.into();
        self
    }

    pub fn with_pricing_day(mut self, pricing_day: impl Into<String>) -> Self {
        self.pricing_day = pricing_day.into();
        self
    }

    pub fn total_tokens(&self) -> i64 {
        self.tokens.total()
    }
}

pub fn local_date_string_from_timestamp(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(timestamp)
        .map(|dt| dt.with_timezone(&Local).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncrementalIngestMode {
    UpsertMessages,
    ReplaceChangedSessions,
}

#[async_trait]
pub trait QuotaFetcher: Send + Sync {
    fn provider_name(&self) -> &str;
    fn provider_display_name(&self) -> &str;
    async fn fetch_quota(&self) -> Result<QuotaSnapshot>;
}

pub trait SessionParser: Send + Sync {
    fn provider_name(&self) -> &str;
    fn session_paths(&self) -> Vec<PathBuf>;
    fn parse_sessions(&self, since: Option<chrono::NaiveDate>) -> Result<Vec<UnifiedMessage>>;
    fn parser_version(&self) -> &str {
        "v1"
    }
    fn incremental_ingest_mode(&self) -> IncrementalIngestMode {
        IncrementalIngestMode::UpsertMessages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_breakdown_total() {
        let tokens = TokenBreakdown {
            input: 1000,
            output: 500,
            cache_read: 200,
            cache_write: 100,
            reasoning: 50,
        };
        assert_eq!(tokens.total(), 1850);
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_token_breakdown_empty() {
        let tokens = TokenBreakdown {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        };
        assert_eq!(tokens.total(), 0);
        assert!(tokens.is_empty());
    }
}
