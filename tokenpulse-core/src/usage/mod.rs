pub mod claude;
pub mod codex;
pub mod opencode;
pub mod pi;
pub mod scanner;

pub use claude::ClaudeSessionParser;
pub use codex::CodexSessionParser;
pub use opencode::OpenCodeSessionParser;
pub use pi::PiSessionParser;

use crate::provider::UnifiedMessage;
use std::collections::{BTreeMap, HashMap};

pub fn group_by_date(messages: &[UnifiedMessage]) -> BTreeMap<String, Vec<&UnifiedMessage>> {
    let mut grouped: BTreeMap<String, Vec<&UnifiedMessage>> = BTreeMap::new();
    for msg in messages {
        grouped.entry(msg.date.clone()).or_default().push(msg);
    }
    grouped
}

pub fn group_by_provider(messages: &[UnifiedMessage]) -> HashMap<String, Vec<&UnifiedMessage>> {
    let mut grouped: HashMap<String, Vec<&UnifiedMessage>> = HashMap::new();
    for msg in messages {
        grouped.entry(msg.client.clone()).or_default().push(msg);
    }
    grouped
}

pub fn group_by_model(messages: &[UnifiedMessage]) -> HashMap<String, Vec<&UnifiedMessage>> {
    let mut grouped: HashMap<String, Vec<&UnifiedMessage>> = HashMap::new();
    for msg in messages {
        grouped.entry(msg.model_id.clone()).or_default().push(msg);
    }
    grouped
}

#[derive(Debug, Clone)]
pub struct DailySummary {
    pub date: String,
    pub total_cost: f64,
    pub total_tokens: i64,
    pub by_provider: HashMap<String, f64>,
}

#[derive(Debug, Clone)]
pub struct ProviderSummary {
    pub provider: String,
    pub cost: f64,
    pub tokens: i64,
    pub percent: f64,
}

#[derive(Debug, Clone)]
pub struct ModelSummary {
    pub model: String,
    pub provider: String,
    pub cost: f64,
    pub tokens: i64,
    pub percent: f64,
}

#[derive(Debug, Clone)]
pub struct UsageSummary {
    pub total_cost: f64,
    pub total_tokens: i64,
    pub active_days: usize,
    pub avg_daily_cost: f64,
    pub max_daily_cost: f64,
    pub by_provider: Vec<ProviderSummary>,
    pub by_model: Vec<ModelSummary>,
}

pub fn compute_usage_summary(messages: &[UnifiedMessage]) -> UsageSummary {
    if messages.is_empty() {
        return UsageSummary {
            total_cost: 0.0,
            total_tokens: 0,
            active_days: 0,
            avg_daily_cost: 0.0,
            max_daily_cost: 0.0,
            by_provider: vec![],
            by_model: vec![],
        };
    }

    let by_date = group_by_date(messages);
    let by_provider = group_by_provider(messages);
    let by_model = group_by_model(messages);

    let total_cost: f64 = messages.iter().map(|m| m.cost).sum();
    let total_tokens: i64 = messages.iter().map(|m| m.tokens.total()).sum();
    let active_days = by_date.len();

    let daily_costs: Vec<f64> = by_date
        .values()
        .map(|msgs| msgs.iter().map(|m| m.cost).sum())
        .collect();

    let avg_daily_cost = total_cost / active_days as f64;
    let max_daily_cost = daily_costs.iter().cloned().fold(0.0, f64::max);

    let mut provider_summaries: Vec<ProviderSummary> = by_provider
        .iter()
        .map(|(provider, msgs)| {
            let cost = msgs.iter().map(|m| m.cost).sum();
            let tokens = msgs.iter().map(|m| m.tokens.total()).sum();
            ProviderSummary {
                provider: provider.clone(),
                cost,
                tokens,
                percent: if total_cost > 0.0 {
                    cost / total_cost * 100.0
                } else {
                    0.0
                },
            }
        })
        .collect();
    provider_summaries.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap());

    let mut model_summaries: Vec<ModelSummary> = by_model
        .iter()
        .map(|(model, msgs)| {
            let cost = msgs.iter().map(|m| m.cost).sum();
            let tokens = msgs.iter().map(|m| m.tokens.total()).sum();
            let provider = msgs.first().map(|m| m.client.clone()).unwrap_or_default();
            ModelSummary {
                model: model.clone(),
                provider,
                cost,
                tokens,
                percent: if total_cost > 0.0 {
                    cost / total_cost * 100.0
                } else {
                    0.0
                },
            }
        })
        .collect();
    model_summaries.sort_by(|a, b| b.cost.partial_cmp(&a.cost).unwrap());

    UsageSummary {
        total_cost,
        total_tokens,
        active_days,
        avg_daily_cost,
        max_daily_cost,
        by_provider: provider_summaries,
        by_model: model_summaries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TokenBreakdown;

    fn make_message(client: &str, model: &str, date: &str, cost: f64) -> UnifiedMessage {
        UnifiedMessage {
            client: client.to_string(),
            model_id: model.to_string(),
            provider_id: "test".to_string(),
            session_id: "test-session".to_string(),
            timestamp: 0,
            date: date.to_string(),
            tokens: TokenBreakdown {
                input: 100,
                output: 50,
                cache_read: 0,
                cache_write: 0,
                reasoning: 0,
            },
            cost,
        }
    }

    #[test]
    fn test_group_by_date_single_date() {
        let messages = vec![
            make_message("claude", "claude-3", "2024-01-01", 0.01),
            make_message("claude", "claude-3", "2024-01-01", 0.02),
        ];

        let grouped = group_by_date(&messages);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped.get("2024-01-01").map(|v| v.len()), Some(2));
    }

    #[test]
    fn test_group_by_date_multiple_dates() {
        let messages = vec![
            make_message("claude", "claude-3", "2024-01-01", 0.01),
            make_message("claude", "claude-3", "2024-01-02", 0.02),
            make_message("claude", "claude-3", "2024-01-03", 0.03),
        ];

        let grouped = group_by_date(&messages);
        assert_eq!(grouped.len(), 3);
        assert_eq!(grouped.get("2024-01-01").map(|v| v.len()), Some(1));
        assert_eq!(grouped.get("2024-01-02").map(|v| v.len()), Some(1));
        assert_eq!(grouped.get("2024-01-03").map(|v| v.len()), Some(1));
    }

    #[test]
    fn test_group_by_date_sorted() {
        let messages = vec![
            make_message("claude", "claude-3", "2024-01-03", 0.01),
            make_message("claude", "claude-3", "2024-01-01", 0.02),
            make_message("claude", "claude-3", "2024-01-02", 0.03),
        ];

        let grouped = group_by_date(&messages);
        let dates: Vec<_> = grouped.keys().collect();
        assert_eq!(dates, vec!["2024-01-01", "2024-01-02", "2024-01-03"]);
    }

    #[test]
    fn test_group_by_provider_single() {
        let messages = vec![
            make_message("claude", "claude-3", "2024-01-01", 0.01),
            make_message("claude", "claude-3", "2024-01-02", 0.02),
        ];

        let grouped = group_by_provider(&messages);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped.get("claude").map(|v| v.len()), Some(2));
    }

    #[test]
    fn test_group_by_provider_multiple() {
        let messages = vec![
            make_message("claude", "claude-3", "2024-01-01", 0.01),
            make_message("codex", "gpt-4", "2024-01-01", 0.02),
            make_message("opencode", "claude-3", "2024-01-01", 0.03),
        ];

        let grouped = group_by_provider(&messages);
        assert_eq!(grouped.len(), 3);
    }

    #[test]
    fn test_group_by_model() {
        let messages = vec![
            make_message("claude", "claude-3-opus", "2024-01-01", 0.01),
            make_message("claude", "claude-3-sonnet", "2024-01-01", 0.02),
            make_message("claude", "claude-3-opus", "2024-01-02", 0.03),
        ];

        let grouped = group_by_model(&messages);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped.get("claude-3-opus").map(|v| v.len()), Some(2));
        assert_eq!(grouped.get("claude-3-sonnet").map(|v| v.len()), Some(1));
    }

    #[test]
    fn test_compute_usage_summary_empty() {
        let messages: Vec<UnifiedMessage> = vec![];
        let summary = compute_usage_summary(&messages);

        assert_eq!(summary.total_cost, 0.0);
        assert_eq!(summary.total_tokens, 0);
        assert_eq!(summary.active_days, 0);
        assert_eq!(summary.avg_daily_cost, 0.0);
        assert_eq!(summary.max_daily_cost, 0.0);
        assert!(summary.by_provider.is_empty());
        assert!(summary.by_model.is_empty());
    }

    #[test]
    fn test_compute_usage_summary_single_message() {
        let messages = vec![make_message("claude", "claude-3", "2024-01-01", 0.01)];
        let summary = compute_usage_summary(&messages);

        assert!((summary.total_cost - 0.01).abs() < 0.001);
        assert_eq!(summary.total_tokens, 150); // 100 + 50
        assert_eq!(summary.active_days, 1);
        assert!((summary.avg_daily_cost - 0.01).abs() < 0.001);
        assert!((summary.max_daily_cost - 0.01).abs() < 0.001);
        assert_eq!(summary.by_provider.len(), 1);
        assert_eq!(summary.by_model.len(), 1);
    }

    #[test]
    fn test_compute_usage_summary_multiple_messages() {
        let messages = vec![
            make_message("claude", "claude-3-opus", "2024-01-01", 0.03),
            make_message("claude", "claude-3-sonnet", "2024-01-01", 0.02),
            make_message("codex", "gpt-4", "2024-01-02", 0.05),
            make_message("claude", "claude-3-opus", "2024-01-03", 0.04),
        ];
        let summary = compute_usage_summary(&messages);

        assert!((summary.total_cost - 0.14).abs() < 0.001);
        assert_eq!(summary.total_tokens, 600); // 4 * 150
        assert_eq!(summary.active_days, 3);
        assert!((summary.avg_daily_cost - 0.14 / 3.0).abs() < 0.001);
        assert!((summary.max_daily_cost - 0.05).abs() < 0.001);
        assert_eq!(summary.by_provider.len(), 2);
        assert_eq!(summary.by_model.len(), 3);
    }

    #[test]
    fn test_compute_usage_summary_provider_percent() {
        let messages = vec![
            make_message("claude", "claude-3", "2024-01-01", 0.60),
            make_message("codex", "gpt-4", "2024-01-01", 0.40),
        ];
        let summary = compute_usage_summary(&messages);

        let claude_summary = summary
            .by_provider
            .iter()
            .find(|p| p.provider == "claude")
            .unwrap();
        let codex_summary = summary
            .by_provider
            .iter()
            .find(|p| p.provider == "codex")
            .unwrap();

        assert!((claude_summary.percent - 60.0).abs() < 0.001);
        assert!((codex_summary.percent - 40.0).abs() < 0.001);
    }

    #[test]
    fn test_compute_usage_summary_sorted_by_cost() {
        let messages = vec![
            make_message("claude", "claude-3", "2024-01-01", 0.01),
            make_message("codex", "gpt-4", "2024-01-01", 0.05),
            make_message("opencode", "claude-3", "2024-01-01", 0.03),
        ];
        let summary = compute_usage_summary(&messages);

        // Should be sorted by cost descending
        assert_eq!(summary.by_provider[0].provider, "codex");
        assert_eq!(summary.by_provider[1].provider, "opencode");
        assert_eq!(summary.by_provider[2].provider, "claude");
    }

    #[test]
    fn test_compute_usage_summary_model_percent() {
        let messages = vec![
            make_message("claude", "claude-3-opus", "2024-01-01", 0.25),
            make_message("claude", "claude-3-sonnet", "2024-01-01", 0.75),
        ];
        let summary = compute_usage_summary(&messages);

        let opus = summary
            .by_model
            .iter()
            .find(|m| m.model == "claude-3-opus")
            .unwrap();
        let sonnet = summary
            .by_model
            .iter()
            .find(|m| m.model == "claude-3-sonnet")
            .unwrap();

        assert!((opus.percent - 25.0).abs() < 0.001);
        assert!((sonnet.percent - 75.0).abs() < 0.001);
    }
}
