use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    pub provider: String,
    pub plan: Option<String>,
    pub windows: Vec<RateWindow>,
    pub credits: Option<CreditInfo>,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateWindow {
    pub label: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedMessage {
    pub client: String,
    pub model_id: String,
    pub provider_id: String,
    pub session_id: String,
    pub timestamp: i64,
    pub date: String,
    pub tokens: TokenBreakdown,
    pub cost: f64,
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

    #[test]
    fn test_rate_window_creation() {
        let window = RateWindow {
            label: "Session (5h)".to_string(),
            used_percent: 42.5,
            resets_at: None,
            period_duration_ms: Some(5 * 60 * 60 * 1000),
        };

        assert_eq!(window.label, "Session (5h)");
        assert!((window.used_percent - 42.5).abs() < 0.001);
        assert!(window.resets_at.is_none());
        assert_eq!(window.period_duration_ms, Some(5 * 60 * 60 * 1000));
    }

    #[test]
    fn test_credit_info_limited() {
        let credits = CreditInfo {
            used: 12.40,
            limit: Some(100.0),
            currency: "USD".to_string(),
        };

        assert_eq!(credits.used, 12.40);
        assert_eq!(credits.limit, Some(100.0));
        assert!(credits.limit.is_some());
    }

    #[test]
    fn test_credit_info_unlimited() {
        let credits = CreditInfo {
            used: 45.20,
            limit: None,
            currency: "USD".to_string(),
        };

        assert_eq!(credits.used, 45.20);
        assert!(credits.limit.is_none());
    }

    #[test]
    fn test_quota_snapshot_creation() {
        let now = Utc::now();
        let snapshot = QuotaSnapshot {
            provider: "claude".to_string(),
            plan: Some("Pro".to_string()),
            windows: vec![
                RateWindow {
                    label: "Session".to_string(),
                    used_percent: 50.0,
                    resets_at: None,
                    period_duration_ms: Some(5 * 60 * 60 * 1000),
                },
                RateWindow {
                    label: "Weekly".to_string(),
                    used_percent: 25.0,
                    resets_at: None,
                    period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
                },
            ],
            credits: Some(CreditInfo {
                used: 10.0,
                limit: Some(100.0),
                currency: "USD".to_string(),
            }),
            fetched_at: now,
        };

        assert_eq!(snapshot.provider, "claude");
        assert_eq!(snapshot.plan, Some("Pro".to_string()));
        assert_eq!(snapshot.windows.len(), 2);
        assert!(snapshot.credits.is_some());
    }

    #[test]
    fn test_unified_message_creation() {
        let msg = UnifiedMessage {
            client: "claude".to_string(),
            model_id: "claude-opus-4".to_string(),
            provider_id: "anthropic".to_string(),
            session_id: "session-123".to_string(),
            timestamp: 1700000000000,
            date: "2024-01-15".to_string(),
            tokens: TokenBreakdown {
                input: 1000,
                output: 500,
                cache_read: 100,
                cache_write: 50,
                reasoning: 0,
            },
            cost: 0.05,
        };

        assert_eq!(msg.client, "claude");
        assert_eq!(msg.model_id, "claude-opus-4");
        assert_eq!(msg.provider_id, "anthropic");
        assert_eq!(msg.session_id, "session-123");
        assert_eq!(msg.timestamp, 1700000000000);
        assert_eq!(msg.date, "2024-01-15");
        assert_eq!(msg.tokens.total(), 1650);
        assert!((msg.cost - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_unified_message_different_providers() {
        let claude_msg = UnifiedMessage {
            client: "claude".to_string(),
            model_id: "claude-3-opus".to_string(),
            provider_id: "anthropic".to_string(),
            session_id: "s1".to_string(),
            timestamp: 1000,
            date: "2024-01-01".to_string(),
            tokens: TokenBreakdown {
                input: 100,
                output: 50,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 0.01,
        };

        let codex_msg = UnifiedMessage {
            client: "codex".to_string(),
            model_id: "gpt-4".to_string(),
            provider_id: "openai".to_string(),
            session_id: "s2".to_string(),
            timestamp: 2000,
            date: "2024-01-02".to_string(),
            tokens: TokenBreakdown {
                input: 200,
                output: 100,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 0.02,
        };

        assert_ne!(claude_msg.client, codex_msg.client);
        assert_ne!(claude_msg.provider_id, codex_msg.provider_id);
    }

    #[test]
    fn test_quota_snapshot_serialization() {
        let snapshot = QuotaSnapshot {
            provider: "claude".to_string(),
            plan: Some("Pro".to_string()),
            windows: vec![],
            credits: None,
            fetched_at: Utc::now(),
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("claude"));
        
        let deserialized: QuotaSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.provider, "claude");
    }

    #[test]
    fn test_unified_message_serialization() {
        let msg = UnifiedMessage {
            client: "claude".to_string(),
            model_id: "claude-3".to_string(),
            provider_id: "anthropic".to_string(),
            session_id: "s1".to_string(),
            timestamp: 1000,
            date: "2024-01-01".to_string(),
            tokens: TokenBreakdown {
                input: 100,
                output: 50,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost: 0.01,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("claude"));
        
        let deserialized: UnifiedMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.client, "claude");
        assert_eq!(deserialized.tokens.total(), 150);
    }
}
