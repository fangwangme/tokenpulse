mod cache;
pub mod litellm;
pub mod modelsdev;
pub mod openrouter;

pub use cache::PricingCache;

use crate::model_id::strip_date_suffix;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingRecord {
    pub pricing: ModelPricing,
    pub source: String,
    pub version: String,
}

impl PricingRecord {
    pub fn new(
        pricing: ModelPricing,
        source: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        Self {
            pricing,
            source: source.into(),
            version: version.into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PricingCatalog {
    entries: HashMap<String, PricingRecord>,
}

impl PricingCatalog {
    pub fn new(entries: HashMap<String, PricingRecord>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &HashMap<String, PricingRecord> {
        &self.entries
    }

    // Sources are merged in explicit priority order. Keep the first usable
    // record for a key and only replace missing or zero-priced rows.
    pub fn insert_if_missing_or_unusable(&mut self, key: String, record: PricingRecord) {
        match self.entries.get(&key) {
            Some(existing) if pricing_record_is_usable(existing) => {}
            _ => {
                self.entries.insert(key, record);
            }
        }
    }

    pub fn lookup<'a>(
        &'a self,
        model_id: &str,
        provider_id: Option<&str>,
    ) -> Option<ResolvedPricing<'a>> {
        for candidate in pricing_lookup_candidates_with_provider(model_id, provider_id) {
            let Some((key, record)) = self
                .entries
                .get_key_value(&candidate)
                .or_else(|| find_case_insensitive_key_value(&candidate, &self.entries))
            else {
                continue;
            };

            if !pricing_record_is_usable(record) {
                continue;
            }

            return Some(ResolvedPricing {
                matched_key: key.as_str(),
                pricing: &record.pricing,
                source: &record.source,
                version: &record.version,
            });
        }

        None
    }
}

pub struct ResolvedPricing<'a> {
    pub matched_key: &'a str,
    pub pricing: &'a ModelPricing,
    pub source: &'a str,
    pub version: &'a str,
}

fn pricing_record_is_usable(record: &PricingRecord) -> bool {
    record.pricing.input_cost_per_token > 0.0
        || record.pricing.output_cost_per_token > 0.0
        || record
            .pricing
            .cache_read_input_token_cost
            .is_some_and(|value| value > 0.0)
        || record
            .pricing
            .cache_creation_input_token_cost
            .is_some_and(|value| value > 0.0)
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
    lookup_model_pricing_with_provider(model_id, None, pricing_map)
}

pub fn lookup_model_pricing_with_provider<'a>(
    model_id: &str,
    provider_id: Option<&str>,
    pricing_map: &'a HashMap<String, ModelPricing>,
) -> Option<&'a ModelPricing> {
    let matched_key = lookup_pricing_key_with_provider(model_id, provider_id, pricing_map)?;
    pricing_map.get(matched_key)
}

fn pricing_lookup_candidates_with_provider(
    model_id: &str,
    provider_id: Option<&str>,
) -> Vec<String> {
    let mut roots = Vec::new();
    let mut root_seen = HashSet::new();

    for provider_hint in provider_hint_candidates(provider_id) {
        push_candidate(
            &mut roots,
            &mut root_seen,
            format!("{provider_hint}/{model_id}"),
        );
    }

    push_candidate(&mut roots, &mut root_seen, model_id.to_string());

    let mut idx = 0usize;
    while idx < roots.len() {
        let root = roots[idx].clone();
        for suffix in strip_left_segment_suffixes(&root) {
            push_candidate(&mut roots, &mut root_seen, suffix);
        }
        idx += 1;
    }

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    for root in roots {
        push_lookup_candidates_for_base(&mut candidates, &mut seen, &root);
    }

    candidates
}

fn push_lookup_candidates_for_base(
    candidates: &mut Vec<String>,
    seen: &mut HashSet<String>,
    model_id: &str,
) {
    if model_id.trim().is_empty() {
        return;
    }

    push_candidate(candidates, seen, model_id.to_string());

    if let Some(alias) = explicit_model_alias(model_id) {
        push_candidate(candidates, seen, alias.to_string());
    }

    push_generalized_candidates(candidates, seen, model_id);

    if let Some(base) = strip_quality_tier_suffix(model_id) {
        push_candidate(candidates, seen, base.clone());
        if let Some(alias) = explicit_model_alias(&base) {
            push_candidate(candidates, seen, alias.to_string());
        }
        push_generalized_candidates(candidates, seen, &base);
    }

    // Strip "-free" suffix (e.g. "kimi-k2.5-free" → "kimi-k2.5")
    if let Some(base) = model_id.strip_suffix("-free") {
        push_candidate(candidates, seen, base.to_string());
        if let Some(alias) = explicit_model_alias(base) {
            push_candidate(candidates, seen, alias.to_string());
        }
        push_generalized_candidates(candidates, seen, base);
    }

    if !model_id.contains('/') && !model_id.contains('.') {
        push_candidate(candidates, seen, format!("anthropic/{}", model_id));
        push_candidate(candidates, seen, format!("openai/{}", model_id));
    }

    if let Some(stripped) = strip_date_suffix(model_id) {
        push_candidate(candidates, seen, stripped.clone());

        if let Some(alias) = explicit_model_alias(&stripped) {
            push_candidate(candidates, seen, alias.to_string());
        }
        push_generalized_candidates(candidates, seen, &stripped);
    }

    // Generic: strip provider prefixes from three-segment identifiers
    // e.g. "nvidia/moonshotai/kimi-k2.6" → "moonshotai/kimi-k2.6"
    if let Some(rest) = strip_three_segment_prefix(model_id) {
        push_candidate(candidates, seen, rest.to_string());
        if let Some(alias) = explicit_model_alias(rest) {
            push_candidate(candidates, seen, alias.to_string());
        }
        push_generalized_candidates(candidates, seen, rest);
    }

    if model_id.contains('/') {
        push_candidate(candidates, seen, model_id.replacen('/', ".", 1));
        push_candidate(candidates, seen, model_id.replace('/', "."));
    }
}

fn strip_left_segment_suffixes(model_id: &str) -> Vec<String> {
    let segments: Vec<&str> = model_id
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    let mut suffixes = Vec::new();

    for start in 1..segments.len() {
        suffixes.push(segments[start..].join("/"));
    }

    suffixes
}

fn provider_hint_candidates(provider_id: Option<&str>) -> Vec<String> {
    let Some(provider_id) = provider_id
        .map(str::trim)
        .filter(|provider| !provider.is_empty())
        .filter(|provider| {
            !provider.eq_ignore_ascii_case("unknown") && !provider.eq_ignore_ascii_case("other")
        })
    else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    push_candidate(&mut candidates, &mut seen, provider_id.to_string());

    if provider_id.contains('_') {
        push_candidate(&mut candidates, &mut seen, provider_id.replace('_', "-"));
    }
    if provider_id.contains('-') {
        push_candidate(&mut candidates, &mut seen, provider_id.replace('-', "_"));
    }

    match provider_id {
        "nvidia" => {
            push_candidate(&mut candidates, &mut seen, "nvidia_nim".to_string());
            push_candidate(&mut candidates, &mut seen, "nvidia-nim".to_string());
        }
        "nvidia_nim" | "nvidia-nim" => {
            push_candidate(&mut candidates, &mut seen, "nvidia".to_string());
        }
        _ => {}
    }

    candidates
}

/// Strip the first segment of a three-segment `/` delimited identifier.
/// e.g. "nvidia/moonshotai/kimi-k2.6" → Some("moonshotai/kimi-k2.6")
fn strip_three_segment_prefix(model_id: &str) -> Option<&str> {
    let (_, after_first) = model_id.split_once('/')?;
    let (_, after_second) = after_first.split_once('/')?;
    // There must be exactly three segments, no more
    after_second
        .contains('/')
        .then_some(())
        .map_or(Some(after_first), |_| None)
}

fn strip_quality_tier_suffix(model_id: &str) -> Option<String> {
    let normalized = model_id.trim().replace('_', "-");
    for suffix in ["-high", "-medium", "-low"] {
        if normalized.to_ascii_lowercase().ends_with(suffix) {
            let end = normalized.len() - suffix.len();
            return Some(normalized[..end].to_string());
        }
    }
    None
}

fn push_generalized_candidates(
    candidates: &mut Vec<String>,
    seen: &mut HashSet<String>,
    model_id: &str,
) {
    let normalized = model_id.trim().replace('_', "-");
    if normalized.is_empty() {
        return;
    }

    if let Some(glm_model) = canonicalize_glm_model(&normalized) {
        push_candidate(candidates, seen, glm_model.clone());
        push_candidate(candidates, seen, format!("zai/{}", glm_model));
        push_candidate(candidates, seen, format!("zai.{}", glm_model));
        push_candidate(candidates, seen, format!("z-ai/{}", glm_model));
        push_candidate(candidates, seen, format!("openrouter/z-ai/{}", glm_model));
    }

    let lower = normalized.to_ascii_lowercase();
    if lower.contains("z-ai/") {
        push_candidate(candidates, seen, lower.replace("z-ai/", "zai/"));
    }
    if lower.contains("z.ai/") {
        push_candidate(candidates, seen, lower.replace("z.ai/", "zai/"));
    }

    if let Some(rest) = lower
        .strip_prefix("z-ai/")
        .or_else(|| lower.strip_prefix("z.ai/"))
    {
        if let Some(glm_model) = canonicalize_glm_model(rest) {
            push_candidate(candidates, seen, format!("zai/{}", glm_model));
            push_candidate(candidates, seen, format!("zai.{}", glm_model));
            push_candidate(candidates, seen, format!("z-ai/{}", glm_model));
            push_candidate(candidates, seen, format!("openrouter/z-ai/{}", glm_model));
        }
    }
}

fn canonicalize_glm_model(model_id: &str) -> Option<String> {
    let lower = model_id.trim().to_ascii_lowercase().replace('_', "-");
    let model = lower.rsplit('/').next().unwrap_or(lower.as_str());
    let rest = model.strip_prefix("glm")?;
    let rest = rest.trim_start_matches(['-', '.']);
    if rest.is_empty() {
        return None;
    }
    Some(format!("glm-{}", rest))
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
        | "antigravity-gemini-3-pro-low"
        | "gemini-3-pro-high"
        | "gemini-3-pro-low"
        | "gemini-3.1-pro-high"
        | "gemini-3-1-pro"
        | "gemini-3-pro"
        | "gemini-3.1-pro" => Some("gemini-3-pro-preview"),

        "antigravity-gemini-3-flash-a" | "gemini-3-flash-a" => Some("gemini-3.5-flash"),

        "antigravity-gemini-3-flash" | "gemini-3-flash" | "gemini-3-flash-c" => {
            Some("gemini-3-flash-preview")
        }

        "antigravity-claude-opus-4-5-thinking"
        | "antigravity-claude-opus-4-5-thinking-high"
        | "antigravity-claude-opus-4-5-thinking-medium" => Some("claude-opus-4-5"),

        "antigravity-claude-opus-4-6-thinking" | "claude-opus-4-6" => Some("claude-opus-4-5"),

        "claude-opus-4.6" => Some("openrouter/anthropic/claude-opus-4.6"),
        "claude-opus-4.5" => Some("openrouter/anthropic/claude-opus-4.5"),

        "claude-sonnet-4.6" | "claude-sonnet-4-6" => Some("claude-sonnet-4-5"),
        "claude-sonnet-4.5" => Some("openrouter/anthropic/claude-sonnet-4.5"),
        "claude-haiku-4.5" => Some("openrouter/anthropic/claude-haiku-4.5"),
        "claude-haiku-4.6" | "claude-haiku-4-6" => Some("openrouter/anthropic/claude-haiku-4.5"),

        // Antigravity Placeholders → canonical models (Strictly from tokscale)
        "MODEL_PLACEHOLDER_M26" | "model-placeholder-m26" => Some("claude-opus-4-5"),

        "MODEL_PLACEHOLDER_M35" | "model-placeholder-m35" => Some("claude-sonnet-4-5"),

        "MODEL_PLACEHOLDER_M36"
        | "model-placeholder-m36"
        | "MODEL_PLACEHOLDER_M37"
        | "model-placeholder-m37" => Some("gemini-3-pro-preview"),

        "MODEL_PLACEHOLDER_M47" | "model-placeholder-m47" => Some("gemini-3-flash-preview"),

        "model_openai_gpt_oss_120b_medium" | "model-openai-gpt-oss-120b-medium" => {
            Some("openrouter/openai/gpt-oss-120b-medium")
        }

        // Bare model names (often from -free stripping) → LiteLLM keys
        "kimi-k2.5" => Some("moonshot/kimi-k2.5"),
        "minimax-m2.5" => Some("minimax/MiniMax-M2.5"),
        "minimax-m2.1" => Some("minimax/MiniMax-M2.1"),
        "grok-code" => Some("xai/grok-code-fast-1"),
        "deepseek-v4-flash" => Some("deepseek/deepseek-v4-flash"),
        "deepseek-v4-pro" => Some("deepseek/deepseek-v4-pro"),

        // Provider-prefixed aliases
        "moonshotai/kimi-k2.5" => Some("moonshot/kimi-k2.5"),
        "moonshotai/kimi-k2.6" => Some("moonshot/kimi-k2.6"),
        "minimaxai/minimax-m2.1" => Some("minimax/MiniMax-M2.1"),
        "minimaxai/minimax-m2.5" => Some("minimax/MiniMax-M2.5"),
        "qwen/qwen3.5-397b-a17b" => Some("openrouter/qwen/qwen3.5-397b-a17b"),
        "deepseek-ai/deepseek-v3.2" => Some("deepseek/deepseek-v3.2"),
        "deepseek-ai/deepseek-v4-flash" => Some("deepseek/deepseek-v4-flash"),
        "deepseek-ai/deepseek-v4-pro" => Some("deepseek/deepseek-v4-pro"),
        "nvidia/llama-3.3-nemotron-super-49b-v1.5" => {
            Some("deepinfra/nvidia/Llama-3.3-Nemotron-Super-49B-v1.5")
        }
        "nvidia/llama-3.1-nemotron-ultra-253b-v1" => {
            Some("nebius/nvidia/Llama-3.1-Nemotron-Ultra-253B-v1")
        }
        _ => None,
    }
}

fn lookup_pricing_key_with_provider<'a, T>(
    model_id: &str,
    provider_id: Option<&str>,
    pricing_map: &'a HashMap<String, T>,
) -> Option<&'a str> {
    let candidates = pricing_lookup_candidates_with_provider(model_id, provider_id);

    for candidate in &candidates {
        if let Some((key, _)) = pricing_map.get_key_value(candidate) {
            return Some(key.as_str());
        }

        if let Some(key) = find_case_insensitive_key_generic(candidate, pricing_map) {
            return Some(key);
        }
    }

    None
}

fn find_case_insensitive_key_generic<'a, T>(
    candidate: &str,
    pricing_map: &'a HashMap<String, T>,
) -> Option<&'a str> {
    pricing_map
        .keys()
        .filter(|key| key.eq_ignore_ascii_case(candidate))
        .min_by_key(|key| key.len())
        .map(String::as_str)
}

fn find_case_insensitive_key_value<'a, T>(
    candidate: &str,
    pricing_map: &'a HashMap<String, T>,
) -> Option<(&'a String, &'a T)> {
    let key = pricing_map
        .keys()
        .filter(|key| key.eq_ignore_ascii_case(candidate))
        .min_by_key(|key| key.len())?;
    pricing_map.get_key_value(key)
}

pub fn lookup_model_pricing_or_warn<'a>(
    model_id: &str,
    pricing_map: &'a HashMap<String, ModelPricing>,
) -> Option<&'a ModelPricing> {
    lookup_model_pricing_or_warn_with_provider(model_id, None, pricing_map)
}

pub fn lookup_model_pricing_or_warn_with_provider<'a>(
    model_id: &str,
    provider_id: Option<&str>,
    pricing_map: &'a HashMap<String, ModelPricing>,
) -> Option<&'a ModelPricing> {
    let pricing = lookup_model_pricing_with_provider(model_id, provider_id, pricing_map);

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
    fn test_lookup_model_pricing_does_not_strip_model_version_suffix() {
        let mut map = HashMap::new();
        map.insert(
            "claude-opus-4".to_string(),
            make_pricing(0.000015, 0.000075),
        );

        let result = lookup_model_pricing("claude-opus-4-5", &map);
        assert!(result.is_none());
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
    fn test_lookup_model_pricing_uses_glm_5_1_alias() {
        let mut map = HashMap::new();
        map.insert(
            "zai/glm-5.1".to_string(),
            make_pricing(0.0000014, 0.0000044),
        );

        let result = lookup_model_pricing("z-ai/glm5.1", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().output_cost_per_token, 0.0000044);
    }

    #[test]
    fn test_lookup_model_pricing_uses_openrouter_glm_5_1_alias() {
        let mut map = HashMap::new();
        map.insert(
            "z-ai/glm-5.1".to_string(),
            make_pricing(0.00000105, 0.0000035),
        );

        let result = lookup_model_pricing("glm5.1", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().output_cost_per_token, 0.0000035);
    }

    #[test]
    fn test_lookup_model_pricing_uses_prefixed_openrouter_glm_5_1_alias() {
        let mut map = HashMap::new();
        map.insert(
            "openrouter/z-ai/glm-5.1".to_string(),
            make_pricing(0.00000105, 0.0000035),
        );

        let result = lookup_model_pricing("z-ai/glm5.1", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().input_cost_per_token, 0.00000105);
    }

    #[test]
    fn test_lookup_model_pricing_strips_quality_tier_suffixes() {
        let mut map = HashMap::new();
        map.insert(
            "zai/glm-5.1".to_string(),
            make_pricing(0.0000014, 0.0000044),
        );
        map.insert(
            "gemini-3-pro-preview".to_string(),
            make_pricing(0.000002, 0.000012),
        );

        let glm = lookup_model_pricing("z-ai/glm-5.1-low", &map);
        assert!(glm.is_some());
        assert_eq!(glm.unwrap().output_cost_per_token, 0.0000044);

        let gemini = lookup_model_pricing("gemini-3-pro-high", &map);
        assert!(gemini.is_some());
        assert_eq!(gemini.unwrap().output_cost_per_token, 0.000012);
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
    fn test_lookup_claude_dot_version_alias() {
        let mut map = HashMap::new();
        map.insert(
            "openrouter/anthropic/claude-opus-4.6".to_string(),
            make_pricing(0.000003, 0.000015),
        );

        let result = lookup_model_pricing("claude-opus-4.6", &map);
        assert!(result.is_some(), "claude-opus-4.6 should resolve via alias");
    }

    #[test]
    fn test_lookup_strips_three_segment_prefix() {
        let mut map = HashMap::new();
        map.insert(
            "moonshot/kimi-k2.6".to_string(),
            make_pricing(0.0000009, 0.000004),
        );

        let result = lookup_model_pricing("nvidia/moonshotai/kimi-k2.6", &map);
        assert!(
            result.is_some(),
            "nvidia/moonshotai/kimi-k2.6 should resolve via three-segment prefix stripping"
        );
        assert_eq!(result.unwrap().input_cost_per_token, 0.0000009);
    }

    #[test]
    fn test_lookup_model_pricing_uses_provider_hint_prefix() {
        let mut map = HashMap::new();
        map.insert(
            "nvidia/deepseek-ai/deepseek-v4-pro".to_string(),
            make_pricing(0.00000174, 0.00000348),
        );

        let result =
            lookup_model_pricing_with_provider("deepseek-ai/deepseek-v4-pro", Some("nvidia"), &map);

        assert!(result.is_some());
        assert_eq!(result.unwrap().output_cost_per_token, 0.00000348);
    }

    #[test]
    fn test_lookup_model_pricing_uses_deepseek_v4_alias() {
        let mut map = HashMap::new();
        map.insert(
            "deepseek/deepseek-v4-flash".to_string(),
            make_pricing(0.00000014, 0.00000028),
        );

        let result = lookup_model_pricing("deepseek-ai/deepseek-v4-flash", &map);

        assert!(result.is_some());
        assert_eq!(result.unwrap().input_cost_per_token, 0.00000014);
    }

    #[test]
    fn test_lookup_model_pricing_strips_arbitrary_left_route_prefixes() {
        let mut map = HashMap::new();
        map.insert(
            "deepseek-ai/deepseek-v4-pro".to_string(),
            make_pricing(0.00000174, 0.00000348),
        );

        let result = lookup_model_pricing("anthropic/nvidia_nim/deepseek-ai/deepseek-v4-pro", &map);

        assert!(result.is_some());
        assert_eq!(result.unwrap().input_cost_per_token, 0.00000174);
    }

    #[test]
    fn test_strip_three_segment_prefix_function() {
        assert_eq!(
            strip_three_segment_prefix("nvidia/moonshotai/kimi-k2.6"),
            Some("moonshotai/kimi-k2.6")
        );
        assert_eq!(strip_three_segment_prefix("foo/bar/baz"), Some("bar/baz"));
        // Two segments – not touched
        assert_eq!(strip_three_segment_prefix("moonshotai/kimi-k2.6"), None);
        // Four segments – not touched
        assert_eq!(strip_three_segment_prefix("a/b/c/d"), None);
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

    #[test]
    fn test_provider_specific_zero_price_free_model_falls_back_to_paid_model() {
        let mut entries = HashMap::new();
        entries.insert(
            "opencode/deepseek-v4-flash-free".to_string(),
            PricingRecord::new(
                make_pricing(0.0, 0.0),
                "models.dev:opencode",
                "models.dev-api-v1",
            ),
        );
        entries.insert(
            "deepseek-v4-flash".to_string(),
            PricingRecord::new(
                make_pricing(0.00000014, 0.00000028),
                "models.dev:deepseek",
                "models.dev-api-v1",
            ),
        );

        let catalog = PricingCatalog::new(entries);
        let resolved = catalog
            .lookup("deepseek-v4-flash-free", Some("opencode"))
            .unwrap();

        assert_eq!(resolved.matched_key, "deepseek-v4-flash");
        assert_eq!(resolved.pricing.input_cost_per_token, 0.00000014);
        assert_eq!(resolved.source, "models.dev:deepseek");
    }

    #[test]
    fn pricing_catalog_replaces_zero_price_with_lower_priority_paid_record() {
        let mut catalog = PricingCatalog::default();
        catalog.insert_if_missing_or_unusable(
            "github-copilot/gpt-5.4".to_string(),
            PricingRecord::new(make_pricing(0.0, 0.0), "litellm", "litellm-main-v1"),
        );
        catalog.insert_if_missing_or_unusable(
            "github-copilot/gpt-5.4".to_string(),
            PricingRecord::new(
                make_pricing(0.0000025, 0.000015),
                "models.dev:opencode",
                "models.dev-api-v1",
            ),
        );

        let resolved = catalog.lookup("gpt-5.4", Some("github-copilot")).unwrap();
        assert_eq!(resolved.pricing.input_cost_per_token, 0.0000025);
        assert_eq!(resolved.source, "models.dev:opencode");
    }
}
