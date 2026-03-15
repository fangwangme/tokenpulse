pub mod litellm;

pub use litellm::PricingCache;

use crate::provider::TokenBreakdown;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_input_token_cost: Option<f64>,
    pub cache_creation_input_token_cost: Option<f64>,
}

impl ModelPricing {
    pub fn new(
        input_cost_per_token: f64,
        output_cost_per_token: f64,
        cache_read_input_token_cost: Option<f64>,
        cache_creation_input_token_cost: Option<f64>,
    ) -> Self {
        Self {
            input_cost_per_token,
            output_cost_per_token,
            cache_read_input_token_cost,
            cache_creation_input_token_cost,
        }
    }

    pub fn simple(input_cost: f64, output_cost: f64) -> Self {
        Self {
            input_cost_per_token: input_cost,
            output_cost_per_token: output_cost,
            cache_read_input_token_cost: None,
            cache_creation_input_token_cost: None,
        }
    }
}

pub fn calculate_cost(tokens: &TokenBreakdown, pricing: &ModelPricing) -> f64 {
    let input = tokens.input as f64 * pricing.input_cost_per_token;
    let output = tokens.output as f64 * pricing.output_cost_per_token;

    let cache_read = tokens.cache_read as f64
        * pricing
            .cache_read_input_token_cost
            .unwrap_or_else(|| pricing.input_cost_per_token * 0.1);

    let cache_write = tokens.cache_write as f64
        * pricing
            .cache_creation_input_token_cost
            .unwrap_or_else(|| pricing.input_cost_per_token * 1.25);

    let reasoning = tokens.reasoning as f64 * pricing.output_cost_per_token;

    input + output + cache_read + cache_write + reasoning
}

pub fn lookup_model_pricing<'a>(
    model_id: &str,
    pricing_map: &'a std::collections::HashMap<String, ModelPricing>,
) -> Option<&'a ModelPricing> {
    // 1. Exact match
    if let Some(p) = pricing_map.get(model_id) {
        return Some(p);
    }

    // 2. Try with provider prefix
    if let Some(p) = pricing_map.get(&format!("anthropic/{}", model_id)) {
        return Some(p);
    }
    if let Some(p) = pricing_map.get(&format!("openai/{}", model_id)) {
        return Some(p);
    }

    // 3. Strip date suffix (e.g., claude-3-opus-20240229 -> claude-3-opus)
    let base_model = model_id
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches('-');
    
    if base_model != model_id {
        if let Some(p) = pricing_map.get(base_model) {
            return Some(p);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_pricing(input: f64, output: f64) -> ModelPricing {
        ModelPricing::simple(input, output)
    }

    fn make_pricing_full(
        input: f64,
        output: f64,
        cache_read: Option<f64>,
        cache_write: Option<f64>,
    ) -> ModelPricing {
        ModelPricing::new(input, output, cache_read, cache_write)
    }

    #[test]
    fn test_calculate_cost_basic() {
        let tokens = TokenBreakdown {
            input: 1000,
            output: 500,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        };

        let pricing = make_pricing(0.00001, 0.00003);
        let cost = calculate_cost(&tokens, &pricing);

        // 1000 * 0.00001 + 500 * 0.00003
        let expected = 1000.0 * 0.00001 + 500.0 * 0.00003;
        assert!((cost - expected).abs() < 0.0000001);
    }

    #[test]
    fn test_calculate_cost_with_cache() {
        let tokens = TokenBreakdown {
            input: 1000,
            output: 500,
            cache_read: 200,
            cache_write: 100,
            reasoning: 0,
        };

        let pricing = make_pricing_full(0.00001, 0.00003, Some(0.000001), Some(0.0000125));
        let cost = calculate_cost(&tokens, &pricing);

        // input + output + cache_read + cache_write
        let expected = 
            1000.0 * 0.00001 +    // input
            500.0 * 0.00003 +     // output
            200.0 * 0.000001 +    // cache_read
            100.0 * 0.0000125;    // cache_write

        assert!((cost - expected).abs() < 0.0000001, "Expected {}, got {}", expected, cost);
    }

    #[test]
    fn test_calculate_cost_with_reasoning() {
        let tokens = TokenBreakdown {
            input: 1000,
            output: 500,
            cache_read: 0,
            cache_write: 0,
            reasoning: 200,
        };

        let pricing = make_pricing(0.00001, 0.00003);
        let cost = calculate_cost(&tokens, &pricing);

        // reasoning uses output price
        let expected = 1000.0 * 0.00001 + 500.0 * 0.00003 + 200.0 * 0.00003;
        assert!((cost - expected).abs() < 0.0000001);
    }

    #[test]
    fn test_calculate_cost_cache_fallback() {
        let tokens = TokenBreakdown {
            input: 1000,
            output: 500,
            cache_read: 200,
            cache_write: 100,
            reasoning: 0,
        };

        let pricing = make_pricing(0.00001, 0.00003);
        let cost = calculate_cost(&tokens, &pricing);

        // cache_read defaults to 10% of input, cache_write defaults to 125% of input
        let expected = 
            1000.0 * 0.00001 +                    // input
            500.0 * 0.00003 +                     // output
            200.0 * 0.00001 * 0.1 +               // cache_read (10% of input)
            100.0 * 0.00001 * 1.25;               // cache_write (125% of input)

        assert!((cost - expected).abs() < 0.0000001, "Expected {}, got {}", expected, cost);
    }

    #[test]
    fn test_calculate_cost_empty_tokens() {
        let tokens = TokenBreakdown {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        };

        let pricing = make_pricing(0.00001, 0.00003);
        let cost = calculate_cost(&tokens, &pricing);

        assert_eq!(cost, 0.0);
    }

    #[test]
    fn test_lookup_model_pricing_exact() {
        let mut map = HashMap::new();
        map.insert("claude-3-opus".to_string(), make_pricing(0.000015, 0.000075));

        let result = lookup_model_pricing("claude-3-opus", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().input_cost_per_token, 0.000015);
    }

    #[test]
    fn test_lookup_model_pricing_not_found() {
        let map = HashMap::<String, ModelPricing>::new();
        let result = lookup_model_pricing("unknown-model", &map);
        assert!(result.is_none());
    }

    #[test]
    fn test_lookup_model_pricing_with_anthropic_prefix() {
        let mut map = HashMap::new();
        map.insert(
            "anthropic/claude-3-opus".to_string(),
            make_pricing(0.000015, 0.000075),
        );

        let result = lookup_model_pricing("claude-3-opus", &map);
        assert!(result.is_some());
    }

    #[test]
    fn test_lookup_model_pricing_with_openai_prefix() {
        let mut map = HashMap::new();
        map.insert("openai/gpt-4".to_string(), make_pricing(0.00003, 0.00006));

        let result = lookup_model_pricing("gpt-4", &map);
        assert!(result.is_some());
    }

    #[test]
    fn test_lookup_model_pricing_strip_date_suffix() {
        let mut map = HashMap::new();
        map.insert("claude-3-opus".to_string(), make_pricing(0.000015, 0.000075));

        let result = lookup_model_pricing("claude-3-opus-20240229", &map);
        assert!(result.is_some());
    }

    #[test]
    fn test_model_pricing_simple() {
        let pricing = ModelPricing::simple(0.00001, 0.00003);
        assert_eq!(pricing.input_cost_per_token, 0.00001);
        assert_eq!(pricing.output_cost_per_token, 0.00003);
        assert!(pricing.cache_read_input_token_cost.is_none());
        assert!(pricing.cache_creation_input_token_cost.is_none());
    }

    #[test]
    fn test_model_pricing_new() {
        let pricing = ModelPricing::new(0.00001, 0.00003, Some(0.000001), Some(0.0000125));
        assert_eq!(pricing.input_cost_per_token, 0.00001);
        assert_eq!(pricing.output_cost_per_token, 0.00003);
        assert_eq!(pricing.cache_read_input_token_cost, Some(0.000001));
        assert_eq!(pricing.cache_creation_input_token_cost, Some(0.0000125));
    }

    #[test]
    fn test_large_token_count() {
        let tokens = TokenBreakdown {
            input: 1_000_000,  // 1M tokens
            output: 500_000,
            cache_read: 100_000,
            cache_write: 50_000,
            reasoning: 0,
        };

        let pricing = make_pricing_full(0.000015, 0.000075, Some(0.0000015), Some(0.00001875));
        let cost = calculate_cost(&tokens, &pricing);

        // Should be a reasonable cost
        assert!(cost > 0.0);
        assert!(cost < 100.0); // Less than $100 for these tokens
    }
}
