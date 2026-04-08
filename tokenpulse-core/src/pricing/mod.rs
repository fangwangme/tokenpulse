pub mod litellm;

pub use litellm::PricingCache;

use crate::provider::TokenBreakdown;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use tracing::warn;

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
    pricing_map: &'a HashMap<String, ModelPricing>,
) -> Option<&'a ModelPricing> {
    let candidates = pricing_lookup_candidates(model_id);

    for candidate in &candidates {
        if let Some(pricing) = pricing_map.get(candidate) {
            return Some(pricing);
        }

        if let Some(key) = find_case_insensitive_key(candidate, pricing_map) {
            return pricing_map.get(key);
        }
    }

    None
}

fn pricing_lookup_candidates(model_id: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    push_candidate(&mut candidates, &mut seen, model_id.to_string());

    if let Some(alias) = explicit_model_alias(model_id) {
        push_candidate(&mut candidates, &mut seen, alias.to_string());
    }

    // Strip "-free" suffix (e.g. "kimi-k2.5-free" → "kimi-k2.5")
    if let Some(base) = model_id.strip_suffix("-free") {
        push_candidate(&mut candidates, &mut seen, base.to_string());
        if let Some(alias) = explicit_model_alias(base) {
            push_candidate(&mut candidates, &mut seen, alias.to_string());
        }
    }

    if !model_id.contains('/') && !model_id.contains('.') {
        push_candidate(
            &mut candidates,
            &mut seen,
            format!("anthropic/{}", model_id),
        );
        push_candidate(&mut candidates, &mut seen, format!("openai/{}", model_id));
    }

    if let Some(stripped) = strip_date_suffix(model_id) {
        push_candidate(&mut candidates, &mut seen, stripped.clone());

        if let Some(alias) = explicit_model_alias(&stripped) {
            push_candidate(&mut candidates, &mut seen, alias.to_string());
        }
    }

    if model_id.contains('/') {
        push_candidate(&mut candidates, &mut seen, model_id.replacen('/', ".", 1));
        push_candidate(&mut candidates, &mut seen, model_id.replace('/', "."));
    }

    candidates
}

fn push_candidate(candidates: &mut Vec<String>, seen: &mut HashSet<String>, candidate: String) {
    if !candidate.is_empty() && seen.insert(candidate.clone()) {
        candidates.push(candidate);
    }
}

fn explicit_model_alias(model_id: &str) -> Option<&'static str> {
    match model_id {
        // Antigravity variants → canonical models
        "antigravity-gemini-3-pro"
        | "antigravity-gemini-3-pro-high"
        | "antigravity-gemini-3-pro-low" => Some("gemini-3-pro-preview"),
        "antigravity-gemini-3-flash" => Some("gemini-3-flash-preview"),
        "antigravity-claude-opus-4-5-thinking"
        | "antigravity-claude-opus-4-5-thinking-high"
        | "antigravity-claude-opus-4-5-thinking-medium" => Some("claude-opus-4-5"),
        "antigravity-claude-opus-4-6-thinking" => Some("claude-opus-4-6"),

        // Gemini quality tier aliases
        "gemini-3-pro-high" | "gemini-3-pro-low" => Some("gemini-3-pro-preview"),

        // Bare model names (often from -free stripping) → LiteLLM keys
        "kimi-k2.5" => Some("moonshot/kimi-k2.5"),
        "minimax-m2.5" => Some("minimax/MiniMax-M2.5"),
        "minimax-m2.1" => Some("minimax/MiniMax-M2.1"),
        "glm-4.7" => Some("zai/glm-4.7"),
        "glm-5" => Some("zai/glm-5"),
        "grok-code" => Some("xai/grok-code-fast-1"),

        // Provider-prefixed aliases
        "moonshotai/kimi-k2.5" => Some("moonshot/kimi-k2.5"),
        "minimaxai/minimax-m2.1" => Some("minimax/MiniMax-M2.1"),
        "minimaxai/minimax-m2.5" => Some("minimax/MiniMax-M2.5"),
        "z-ai/glm5" => Some("zai/glm-5"),
        "z-ai/glm4.7" | "z-ai/glm-4.7" => Some("zai/glm-4.7"),
        "qwen/qwen3.5-397b-a17b" => Some("openrouter/qwen/qwen3.5-397b-a17b"),
        "deepseek-ai/deepseek-v3.2" => Some("deepseek/deepseek-v3.2"),
        "nvidia/llama-3.3-nemotron-super-49b-v1.5" => {
            Some("deepinfra/nvidia/Llama-3.3-Nemotron-Super-49B-v1.5")
        }
        "nvidia/llama-3.1-nemotron-ultra-253b-v1" => {
            Some("nebius/nvidia/Llama-3.1-Nemotron-Ultra-253B-v1")
        }
        _ => None,
    }
}

fn strip_date_suffix(model_id: &str) -> Option<String> {
    let base_model = model_id
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end_matches('-');

    (base_model != model_id).then(|| base_model.to_string())
}

fn find_case_insensitive_key<'a>(
    candidate: &str,
    pricing_map: &'a HashMap<String, ModelPricing>,
) -> Option<&'a str> {
    pricing_map
        .keys()
        .filter(|key| key.eq_ignore_ascii_case(candidate))
        .min_by_key(|key| key.len())
        .map(String::as_str)
}

pub fn lookup_model_pricing_or_warn<'a>(
    model_id: &str,
    pricing_map: &'a HashMap<String, ModelPricing>,
) -> Option<&'a ModelPricing> {
    let pricing = lookup_model_pricing(model_id, pricing_map);

    if pricing.is_none() && should_warn_for_missing_model(model_id) {
        warn!("No pricing found for model: {}", model_id);
    }

    pricing
}

fn should_warn_for_missing_model(model_id: &str) -> bool {
    static WARNED_MODELS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

    WARNED_MODELS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|mut warned_models| warned_models.insert(model_id.to_string()))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let expected = 1000.0 * 0.00001 +    // input
            500.0 * 0.00003 +     // output
            200.0 * 0.000001 +    // cache_read
            100.0 * 0.0000125; // cache_write

        assert!(
            (cost - expected).abs() < 0.0000001,
            "Expected {}, got {}",
            expected,
            cost
        );
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
        let expected = 1000.0 * 0.00001 +                    // input
            500.0 * 0.00003 +                     // output
            200.0 * 0.00001 * 0.1 +               // cache_read (10% of input)
            100.0 * 0.00001 * 1.25; // cache_write (125% of input)

        assert!(
            (cost - expected).abs() < 0.0000001,
            "Expected {}, got {}",
            expected,
            cost
        );
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
        map.insert(
            "claude-3-opus".to_string(),
            make_pricing(0.000015, 0.000075),
        );

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
    fn test_missing_model_warning_is_deduplicated_per_model() {
        assert!(should_warn_for_missing_model("missing-model-a"));
        assert!(!should_warn_for_missing_model("missing-model-a"));
        assert!(should_warn_for_missing_model("missing-model-b"));
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
        map.insert(
            "claude-3-opus".to_string(),
            make_pricing(0.000015, 0.000075),
        );

        let result = lookup_model_pricing("claude-3-opus-20240229", &map);
        assert!(result.is_some());
    }

    #[test]
    fn test_lookup_model_pricing_uses_explicit_antigravity_alias() {
        let mut map = HashMap::new();
        map.insert(
            "gemini-3-pro-preview".to_string(),
            make_pricing(0.000002, 0.000012),
        );

        let result = lookup_model_pricing("antigravity-gemini-3-pro-high", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().input_cost_per_token, 0.000002);
    }

    #[test]
    fn test_lookup_model_pricing_uses_explicit_moonshot_alias() {
        let mut map = HashMap::new();
        map.insert(
            "moonshot/kimi-k2.5".to_string(),
            make_pricing(0.0000006, 0.000003),
        );

        let result = lookup_model_pricing("moonshotai/kimi-k2.5", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().output_cost_per_token, 0.000003);
    }

    #[test]
    fn test_lookup_model_pricing_uses_explicit_qwen_alias() {
        let mut map = HashMap::new();
        map.insert(
            "openrouter/qwen/qwen3.5-397b-a17b".to_string(),
            make_pricing(0.0000006, 0.0000036),
        );

        let result = lookup_model_pricing("qwen/qwen3.5-397b-a17b", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().output_cost_per_token, 0.0000036);
    }

    #[test]
    fn test_lookup_model_pricing_uses_explicit_minimax_alias() {
        let mut map = HashMap::new();
        map.insert(
            "minimax/MiniMax-M2.1".to_string(),
            make_pricing(0.0000003, 0.0000012),
        );

        let result = lookup_model_pricing("minimaxai/minimax-m2.1", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().input_cost_per_token, 0.0000003);
    }

    #[test]
    fn test_lookup_model_pricing_uses_explicit_glm_alias() {
        let mut map = HashMap::new();
        map.insert("zai/glm-5".to_string(), make_pricing(0.0000005, 0.000002));

        let result = lookup_model_pricing("z-ai/glm5", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().output_cost_per_token, 0.000002);
    }

    #[test]
    fn test_lookup_model_pricing_matches_case_insensitive_exact_key() {
        let mut map = HashMap::new();
        map.insert(
            "deepinfra/nvidia/Llama-3.3-Nemotron-Super-49B-v1.5".to_string(),
            make_pricing(0.0000001, 0.0000004),
        );

        let result =
            lookup_model_pricing("deepinfra/nvidia/llama-3.3-nemotron-super-49b-v1.5", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().output_cost_per_token, 0.0000004);
    }

    #[test]
    fn test_lookup_strips_free_suffix_kimi() {
        let mut map = HashMap::new();
        map.insert(
            "moonshot/kimi-k2.5".to_string(),
            make_pricing(0.0000006, 0.000003),
        );

        let result = lookup_model_pricing("kimi-k2.5-free", &map);
        assert!(
            result.is_some(),
            "kimi-k2.5-free should resolve via -free stripping + alias"
        );
        assert_eq!(result.unwrap().output_cost_per_token, 0.000003);
    }

    #[test]
    fn test_lookup_strips_free_suffix_minimax() {
        let mut map = HashMap::new();
        map.insert(
            "minimax/MiniMax-M2.5".to_string(),
            make_pricing(0.0000003, 0.0000012),
        );

        let result = lookup_model_pricing("minimax-m2.5-free", &map);
        assert!(
            result.is_some(),
            "minimax-m2.5-free should resolve via -free stripping + alias"
        );
    }

    #[test]
    fn test_lookup_strips_free_suffix_glm() {
        let mut map = HashMap::new();
        map.insert("zai/glm-4.7".to_string(), make_pricing(0.0000005, 0.000002));

        let result = lookup_model_pricing("glm-4.7-free", &map);
        assert!(
            result.is_some(),
            "glm-4.7-free should resolve via -free stripping + alias"
        );
    }

    #[test]
    fn test_lookup_grok_code_alias() {
        let mut map = HashMap::new();
        map.insert(
            "xai/grok-code-fast-1".to_string(),
            make_pricing(0.000003, 0.000015),
        );

        let result = lookup_model_pricing("grok-code", &map);
        assert!(result.is_some(), "grok-code should resolve via alias");
    }

    #[test]
    fn test_lookup_gemini_quality_tier_alias() {
        let mut map = HashMap::new();
        map.insert(
            "gemini-3-pro-preview".to_string(),
            make_pricing(0.000002, 0.000012),
        );

        let result = lookup_model_pricing("gemini-3-pro-high", &map);
        assert!(
            result.is_some(),
            "gemini-3-pro-high should resolve via alias"
        );
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
            input: 1_000_000, // 1M tokens
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
