mod cache;
pub mod litellm;
pub mod modelsdev;
pub mod openrouter;

pub use cache::PricingCache;

use crate::model_id::strip_date_suffix;
use crate::provider::TokenBreakdown;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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

    #[cfg(test)]
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

fn canonicalize_catalog_key(key: &str) -> String {
    if let Some((provider, model)) = key.split_once('/') {
        format!("{}/{}", provider, crate::model_id::pricing_key(model))
    } else {
        crate::model_id::pricing_key(key)
    }
}

impl PricingCatalog {
    pub fn new(entries: HashMap<String, PricingRecord>) -> Self {
        let mut canonical_entries = HashMap::new();
        for (key, record) in entries {
            let canonical_key = canonicalize_catalog_key(&key);
            canonical_entries.insert(canonical_key, record);
        }
        Self {
            entries: canonical_entries,
        }
    }

    pub fn entries(&self) -> &HashMap<String, PricingRecord> {
        &self.entries
    }

    // Sources are merged in explicit priority order. Keep the first usable
    // record for a key and only replace missing or zero-priced rows.
    pub fn insert_if_missing_or_unusable(&mut self, key: String, record: PricingRecord) {
        let canonical_key = canonicalize_catalog_key(&key);
        match self.entries.get(&canonical_key) {
            Some(existing) if pricing_record_is_usable(existing) => {}
            _ => {
                self.entries.insert(canonical_key, record);
            }
        }
    }

    pub fn lookup<'a>(
        &'a self,
        model_id: &str,
        provider_id: Option<&str>,
    ) -> Option<ResolvedPricing<'a>> {
        let key_id = crate::model_id::pricing_key(model_id);
        for candidate in pricing_lookup_candidates_with_provider(&key_id, provider_id) {
            // Candidates must be spelled like the stored keys, which went
            // through the same normalization on insert (`openrouter/google/X`
            // is stored as `openrouter/X`, `gemini-3.1-pro` as `gemini-3-1-pro`).
            let candidate = canonicalize_catalog_key(&candidate);
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

        if key_id.is_empty() || crate::model_id::is_pseudo(&key_id) {
            return None;
        }
        let preview = (!key_id.ends_with("-preview")).then(|| format!("{key_id}-preview"));
        self.lookup_under_any_provider(&key_id)
            .or_else(|| preview.and_then(|id| self.lookup_under_any_provider(&id)))
    }

    /// Last resort: the same model listed only under provider prefixes no
    /// candidate rule produces — `meta/muse-spark-1-3-contributor` plus a dozen
    /// resellers, with no unprefixed key. The highest-priority source wins,
    /// then the price most of its listings agree on, so one reseller's markup
    /// cannot set the price.
    fn lookup_under_any_provider(&self, key_id: &str) -> Option<ResolvedPricing<'_>> {
        let listings: Vec<(&String, &PricingRecord)> = self
            .entries
            .iter()
            .filter(|(key, record)| {
                key.split_once('/')
                    .is_some_and(|(_, model)| model == key_id)
                    && pricing_record_is_usable(record)
            })
            .collect();
        let best_rank = listings
            .iter()
            .map(|(_, record)| source_rank(&record.source))
            .min()?;

        let mut by_price: HashMap<(u64, u64), Vec<(&String, &PricingRecord)>> = HashMap::new();
        for (key, record) in listings {
            if source_rank(&record.source) == best_rank {
                let price = (
                    record.pricing.input_cost_per_token.to_bits(),
                    record.pricing.output_cost_per_token.to_bits(),
                );
                by_price.entry(price).or_default().push((key, record));
            }
        }
        let (key, record) = by_price
            .into_values()
            .map(|mut group| {
                group.sort_by_key(|(key, _)| *key);
                group
            })
            .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| b[0].0.cmp(a[0].0)))?
            .into_iter()
            .next()?;

        Some(ResolvedPricing {
            matched_key: key.as_str(),
            pricing: &record.pricing,
            source: &record.source,
            version: &record.version,
        })
    }
}

/// Source priority, matching the merge order: LiteLLM, then models.dev, then
/// OpenRouter.
fn source_rank(source: &str) -> u8 {
    if source == "litellm" {
        0
    } else if source.starts_with("models.dev") {
        1
    } else if source == "openrouter" {
        2
    } else {
        3
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

fn pricing_lookup_candidates_with_provider(
    model_id: &str,
    provider_id: Option<&str>,
) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    push_candidates_for_model(&mut candidates, &mut seen, model_id, provider_id);

    // Some models are only ever published under a `-preview` key, and sources
    // that name models from UI labels (Antigravity) never carry the suffix.
    let base = strip_quality_tier_suffix(model_id).unwrap_or_else(|| model_id.to_string());
    let preview = (!base.ends_with("-preview")).then(|| format!("{base}-preview"));
    if let Some(preview) = &preview {
        push_candidates_for_model(&mut candidates, &mut seen, preview, provider_id);
    }

    // Last resort only: Vertex AI keeps listing Gemini models the first-party
    // and router catalogs have already retired.
    for name in std::iter::once(base.as_str()).chain(preview.as_deref()) {
        if let Some(gemini_model) = canonicalize_gemini_model(name) {
            push_candidate(
                &mut candidates,
                &mut seen,
                format!("vertex_ai/{gemini_model}"),
            );
        }
    }

    candidates
}

fn push_candidates_for_model(
    candidates: &mut Vec<String>,
    seen: &mut HashSet<String>,
    model_id: &str,
    provider_id: Option<&str>,
) {
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

    for root in roots {
        push_lookup_candidates_for_base(candidates, seen, &root);
    }
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
    // `-tiered` is an Antigravity routing suffix; it must price as the base model.
    for suffix in ["-high", "-medium", "-low", "-thinking", "-tiered"] {
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

    if let Some(minimax_model) = canonicalize_minimax_model(&normalized) {
        push_candidate(candidates, seen, minimax_model.clone());
        push_candidate(candidates, seen, format!("minimax/{}", minimax_model));
        push_candidate(
            candidates,
            seen,
            format!("openrouter/minimax/{}", minimax_model),
        );
        push_candidate(candidates, seen, format!("minimaxai/{}", minimax_model));
    }

    if let Some(kimi_model) = canonicalize_kimi_model(&normalized) {
        push_candidate(candidates, seen, kimi_model.clone());
        push_candidate(candidates, seen, format!("moonshot/{}", kimi_model));
        push_candidate(candidates, seen, format!("moonshotai/{}", kimi_model));
        push_candidate(
            candidates,
            seen,
            format!("openrouter/moonshot/{}", kimi_model),
        );
    }

    if let Some(deepseek_model) = canonicalize_deepseek_model(&normalized) {
        push_candidate(candidates, seen, deepseek_model.clone());
        push_candidate(candidates, seen, format!("deepseek/{}", deepseek_model));
        push_candidate(candidates, seen, format!("deepseek-ai/{}", deepseek_model));
        push_candidate(
            candidates,
            seen,
            format!("openrouter/deepseek/{}", deepseek_model),
        );
    }

    if let Some(qwen_model) = canonicalize_qwen_model(&normalized) {
        push_candidate(candidates, seen, qwen_model.clone());
        push_candidate(candidates, seen, format!("qwen/{}", qwen_model));
        push_candidate(candidates, seen, format!("openrouter/qwen/{}", qwen_model));
    }

    if let Some(claude_model) = canonicalize_claude_model(&normalized) {
        let dot_version = normalize_claude_version(&claude_model);
        push_candidate(candidates, seen, claude_model.clone());
        push_candidate(candidates, seen, format!("anthropic/{}", claude_model));
        push_candidate(
            candidates,
            seen,
            format!("openrouter/anthropic/{}", claude_model),
        );

        if dot_version != claude_model {
            push_candidate(candidates, seen, dot_version.clone());
            push_candidate(candidates, seen, format!("anthropic/{}", dot_version));
            push_candidate(
                candidates,
                seen,
                format!("openrouter/anthropic/{}", dot_version),
            );
        }
    }

    if let Some(gemini_model) = canonicalize_gemini_model(&normalized) {
        let dot_version = normalize_claude_version(&gemini_model);
        for m in [&gemini_model, &dot_version] {
            if m.is_empty() {
                continue;
            }
            push_candidate(candidates, seen, m.clone());
            push_candidate(candidates, seen, format!("google/{}", m));
            push_candidate(candidates, seen, format!("gemini/{}", m));
            push_candidate(candidates, seen, format!("openrouter/google/{}", m));
        }
    }

    if let Some(gpt_model) = canonicalize_gpt_model(&normalized) {
        let dot_version = normalize_claude_version(&gpt_model);
        push_candidate(candidates, seen, gpt_model.clone());
        push_candidate(candidates, seen, format!("openai/{}", gpt_model));
        push_candidate(candidates, seen, format!("openrouter/openai/{}", gpt_model));

        if dot_version != gpt_model {
            push_candidate(candidates, seen, dot_version.clone());
            push_candidate(candidates, seen, format!("openai/{}", dot_version));
            push_candidate(
                candidates,
                seen,
                format!("openrouter/openai/{}", dot_version),
            );
        }
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

fn canonicalize_minimax_model(model_id: &str) -> Option<String> {
    let lower = model_id.trim().to_ascii_lowercase().replace('_', "-");
    let model = lower.rsplit('/').next().unwrap_or(lower.as_str());
    if let Some(rest) = model.strip_prefix("minimax-m") {
        if rest.chars().next().map_or(false, |c| c.is_ascii_digit()) {
            return Some(format!("minimax-m{}", rest));
        }
    }
    None
}

fn canonicalize_kimi_model(model_id: &str) -> Option<String> {
    let lower = model_id.trim().to_ascii_lowercase().replace('_', "-");
    let model = lower.rsplit('/').next().unwrap_or(lower.as_str());
    if let Some(rest) = model.strip_prefix("kimi-") {
        return Some(format!("kimi-{}", rest));
    }
    None
}

fn canonicalize_deepseek_model(model_id: &str) -> Option<String> {
    let lower = model_id.trim().to_ascii_lowercase().replace('_', "-");
    let model = lower.rsplit('/').next().unwrap_or(lower.as_str());
    if let Some(rest) = model.strip_prefix("deepseek-") {
        return Some(format!("deepseek-{}", rest));
    }
    None
}

fn canonicalize_qwen_model(model_id: &str) -> Option<String> {
    let lower = model_id.trim().to_ascii_lowercase().replace('_', "-");
    let model = lower.rsplit('/').next().unwrap_or(lower.as_str());
    if model.starts_with("qwen") {
        return Some(model.to_string());
    }
    None
}

fn canonicalize_claude_model(model_id: &str) -> Option<String> {
    let lower = model_id.trim().to_ascii_lowercase().replace('_', "-");
    let model = lower.rsplit('/').next().unwrap_or(lower.as_str());
    if model.starts_with("claude-") {
        return Some(model.to_string());
    }
    None
}

fn canonicalize_gemini_model(model_id: &str) -> Option<String> {
    let lower = model_id.trim().to_ascii_lowercase().replace('_', "-");
    let model = lower.rsplit('/').next().unwrap_or(lower.as_str());
    if model.starts_with("gemini-") {
        return Some(model.to_string());
    }
    None
}

fn canonicalize_gpt_model(model_id: &str) -> Option<String> {
    let lower = model_id.trim().to_ascii_lowercase().replace('_', "-");
    let model = lower.rsplit('/').next().unwrap_or(lower.as_str());
    if model.starts_with("gpt-") {
        return Some(model.to_string());
    }
    None
}

fn normalize_claude_version(model: &str) -> String {
    let bytes = model.as_bytes();
    if bytes.len() >= 3 {
        for i in (1..bytes.len() - 1).rev() {
            if bytes[i] == b'-' && bytes[i - 1].is_ascii_digit() && bytes[i + 1].is_ascii_digit() {
                let mut res = model.to_string();
                res.replace_range(i..i + 1, ".");
                return res;
            }
        }
    }
    model.to_string()
}

fn push_candidate(candidates: &mut Vec<String>, seen: &mut HashSet<String>, candidate: String) {
    if !candidate.is_empty() && seen.insert(candidate.clone()) {
        candidates.push(candidate);
    }
}

fn explicit_model_alias(model_id: &str) -> Option<&'static str> {
    match model_id {
        // Bare model names (often from -free stripping) → LiteLLM keys
        "grok-code" => Some("xai/grok-code-fast-1"),

        "nvidia/llama-3.3-nemotron-super-49b-v1.5" => {
            Some("deepinfra/nvidia/Llama-3.3-Nemotron-Super-49B-v1.5")
        }
        "nvidia/llama-3.1-nemotron-ultra-253b-v1" => {
            Some("nebius/nvidia/Llama-3.1-Nemotron-Ultra-253B-v1")
        }
        _ => None,
    }
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

    /// Looks `model_id` up the way usage ingest does: through a catalog built
    /// from `map`, keys normalized on insert exactly like a fetched one.
    fn lookup_model_pricing_with_provider(
        model_id: &str,
        provider_id: Option<&str>,
        map: &HashMap<String, ModelPricing>,
    ) -> Option<ModelPricing> {
        let catalog = PricingCatalog::new(
            map.iter()
                .map(|(key, pricing)| {
                    (
                        key.clone(),
                        PricingRecord::new(pricing.clone(), "test", "v1"),
                    )
                })
                .collect(),
        );
        catalog
            .lookup(model_id, provider_id)
            .map(|resolved| resolved.pricing.clone())
    }

    fn lookup_model_pricing(
        model_id: &str,
        map: &HashMap<String, ModelPricing>,
    ) -> Option<ModelPricing> {
        lookup_model_pricing_with_provider(model_id, None, map)
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

    fn catalog(entries: &[(&str, f64, f64)]) -> PricingCatalog {
        PricingCatalog::new(
            entries
                .iter()
                .map(|(key, input, output)| {
                    (
                        key.to_string(),
                        PricingRecord::new(make_pricing(*input, *output), "test", "v1"),
                    )
                })
                .collect(),
        )
    }

    fn matched_key(catalog: &PricingCatalog, model_id: &str) -> Option<String> {
        catalog
            .lookup(model_id, Some("google"))
            .map(|resolved| resolved.matched_key.to_string())
    }

    #[test]
    fn test_catalog_prices_label_ids_through_the_preview_retry() {
        // Antigravity names models from UI labels, which never say "preview".
        let catalog = catalog(&[("gemini-3-pro-preview", 0.000002, 0.000012)]);
        for model in [
            "antigravity-gemini-3-pro-high",
            "gemini-3-pro-high",
            "gemini-3-pro-preview-low",
        ] {
            assert_eq!(
                matched_key(&catalog, model).as_deref(),
                Some("gemini-3-pro-preview"),
                "{model}"
            );
        }
    }

    #[test]
    fn test_catalog_prefers_an_exact_key_over_its_preview() {
        let catalog = catalog(&[
            ("hy3", 0.000001, 0.000002),
            ("hy3-preview", 0.000003, 0.000004),
            ("gemini-3.8-flash", 0.00000075, 0.00000375),
            ("gemini-3.8-flash-preview", 0.000009, 0.000009),
        ]);
        let input = |model: &str| {
            catalog
                .lookup(model, None)
                .unwrap()
                .pricing
                .input_cost_per_token
        };
        // Both entries survive normalization with their own price.
        assert_eq!(input("hy3"), 0.000001);
        assert_eq!(input("hy3-preview"), 0.000003);
        assert_eq!(input("gemini-3-8-flash-medium"), 0.00000075);
    }

    #[test]
    fn test_catalog_uses_vertex_only_as_the_last_resort() {
        // Upstream keeps a retired preview only under router and Vertex keys.
        let both = catalog(&[
            ("vertex_ai/gemini-3-pro-preview", 0.000001, 0.000001),
            ("openrouter/google/gemini-3-pro-preview", 0.000002, 0.000012),
        ]);
        assert_eq!(
            matched_key(&both, "gemini-3-pro-high").as_deref(),
            Some("openrouter/gemini-3-pro-preview")
        );

        let vertex_only = catalog(&[("vertex_ai/gemini-3-pro-preview", 0.000002, 0.000012)]);
        assert_eq!(
            matched_key(&vertex_only, "gemini-3-pro-high").as_deref(),
            Some("vertex_ai/gemini-3-pro-preview")
        );
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
    fn test_lookup_model_pricing_uses_rule_based_minimax_m3_alias() {
        let mut map = HashMap::new();
        map.insert(
            "minimax/MiniMax-M3".to_string(),
            make_pricing(0.0000003, 0.0000012),
        );

        let result = lookup_model_pricing("minimax-m3", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().input_cost_per_token, 0.0000003);

        let result_free =
            lookup_model_pricing_with_provider("minimax-m3-free", Some("opencode"), &map);
        assert!(result_free.is_some());
        assert_eq!(result_free.unwrap().input_cost_per_token, 0.0000003);
    }

    #[test]
    fn test_lookup_model_pricing_uses_rule_based_deepseek_alias() {
        let mut map = HashMap::new();
        map.insert(
            "deepseek/deepseek-v4-flash".to_string(),
            make_pricing(0.0000001, 0.0000002),
        );

        let result = lookup_model_pricing("deepseek-ai/deepseek-v4-flash", &map);
        assert!(result.is_some());
        assert_eq!(result.unwrap().input_cost_per_token, 0.0000001);

        let result_bare = lookup_model_pricing("deepseek-v4-flash", &map);
        assert!(result_bare.is_some());
        assert_eq!(result_bare.unwrap().input_cost_per_token, 0.0000001);
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
    fn test_lookup_gemini_tiered_routing_suffix_uses_base_model_pricing() {
        let mut map = HashMap::new();
        map.insert(
            "gemini-3-6-flash".to_string(),
            make_pricing(0.000003, 0.000015),
        );

        assert_eq!(
            lookup_model_pricing("gemini-3-6-flash-tiered", &map)
                .expect("tiered routing suffix should resolve to the base model")
                .input_cost_per_token,
            0.000003
        );

        // Full catalog path with the raw Antigravity `responseModel` value.
        let mut entries = HashMap::new();
        entries.insert(
            "gemini-3-6-flash".to_string(),
            PricingRecord::new(
                make_pricing(0.000003, 0.000015),
                "litellm",
                "litellm-main-v1",
            ),
        );
        let catalog = PricingCatalog::new(entries);
        let resolved = catalog
            .lookup("gemini-3.6-flash-tiered", Some("antigravity"))
            .expect("tiered routing suffix should resolve to the base model");
        assert_eq!(resolved.matched_key, "gemini-3-6-flash");

        let tokens = TokenBreakdown {
            input: 1_000_000,
            output: 200_000,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        };
        assert!(
            calculate_cost(&tokens, resolved.pricing) > 0.0,
            "tiered Antigravity rows must not stay at zero cost"
        );
    }

    #[test]
    fn test_catalog_falls_back_to_the_same_model_under_any_provider() {
        let record = |input: f64, output: f64, source: &str| {
            PricingRecord::new(make_pricing(input, output), source, "v1")
        };
        let catalog = PricingCatalog::new(HashMap::from([
            // Free on OpenCode: unusable, so the paid price is wanted.
            (
                "opencode/muse-spark-1.3-contributor".to_string(),
                record(0.0, 0.0, "models.dev:opencode"),
            ),
            (
                "meta/muse-spark-1.3-contributor".to_string(),
                record(0.0000001, 0.0000002, "litellm"),
            ),
            (
                "openrouter/muse-spark-1.3-contributor".to_string(),
                record(0.0000001, 0.0000002, "litellm"),
            ),
            // A marked-up reseller in the same source loses to the majority.
            (
                "aihubmix/muse-spark-1.3-contributor".to_string(),
                record(0.00000011, 0.00000022, "litellm"),
            ),
            // A lower-priority source never wins over LiteLLM.
            (
                "kilo/muse-spark-1.3-contributor".to_string(),
                record(0.000005, 0.000005, "models.dev:kilo"),
            ),
            // A different model that merely shares a prefix is not matched.
            (
                "meta/muse-spark-1.3".to_string(),
                record(0.00000125, 0.00000425, "litellm"),
            ),
        ]));

        let resolved = catalog
            .lookup("muse-spark-1.3-contributor-free", Some("opencode"))
            .unwrap();
        assert_eq!(resolved.matched_key, "meta/muse-spark-1-3-contributor");
        assert_eq!(resolved.pricing.input_cost_per_token, 0.0000001);

        assert!(catalog.lookup("unknown", Some("opencode")).is_none());
    }

    #[test]
    fn test_lookup_gemini_3_1_uses_3_1_pricing_not_3_pro() {
        let wrong_version = catalog(&[("gemini-3-pro-preview", 0.000002, 0.000012)]);
        assert_eq!(matched_key(&wrong_version, "gemini-3.1-pro-high"), None);

        // LiteLLM spells the key with a dot; every spelling lands on it.
        let catalog = catalog(&[("gemini-3.1-pro-preview", 0.000003, 0.000015)]);
        for model in [
            "gemini-3.1-pro-high",
            "gemini-3-1-pro-high",
            "gemini_3_1_pro_high",
            "gemini-3.1-pro-preview-high",
        ] {
            assert_eq!(
                matched_key(&catalog, model).as_deref(),
                Some("gemini-3-1-pro-preview"),
                "{model}"
            );
        }
    }

    #[test]
    fn test_lookup_claude_4_5_thinking_variants_keep_pricing() {
        let catalog = catalog(&[("claude-opus-4-5", 0.000003, 0.000015)]);

        for model in [
            "antigravity-claude-opus-4-5-thinking",
            "claude-opus-4-5-thinking",
            "claude-opus-4-5-thinking-high",
            "claude-opus-4-5-thinking-medium",
        ] {
            assert!(
                catalog.lookup(model, Some("anthropic")).is_some(),
                "{model} should resolve to claude-opus-4-5 pricing"
            );
        }
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
    fn test_lookup_claude_future_version_rule() {
        let mut map = HashMap::new();
        map.insert(
            "openrouter/anthropic/claude-sonnet-4.7".to_string(),
            make_pricing(0.000003, 0.000015),
        );

        let result_dot = lookup_model_pricing("claude-sonnet-4.7", &map);
        assert!(
            result_dot.is_some(),
            "claude-sonnet-4.7 should resolve via rule"
        );

        let result_hyphen = lookup_model_pricing("claude-sonnet-4-7", &map);
        assert!(
            result_hyphen.is_some(),
            "claude-sonnet-4-7 should resolve via rule"
        );
    }

    #[test]
    fn test_lookup_gemini_future_version_rule() {
        let mut map = HashMap::new();
        map.insert(
            "google/gemini-3.2-pro".to_string(),
            make_pricing(0.000003, 0.000015),
        );

        let result_dot = lookup_model_pricing("gemini-3.2-pro", &map);
        assert!(
            result_dot.is_some(),
            "gemini-3.2-pro should resolve via rule"
        );

        let result_hyphen = lookup_model_pricing("gemini-3-2-pro", &map);
        assert!(
            result_hyphen.is_some(),
            "gemini-3-2-pro should resolve via rule"
        );
    }

    #[test]
    fn test_lookup_gpt_future_version_rule() {
        let mut map = HashMap::new();
        map.insert(
            "openai/gpt-5.5".to_string(),
            make_pricing(0.000003, 0.000015),
        );

        let result_dot = lookup_model_pricing("gpt-5.5", &map);
        assert!(result_dot.is_some(), "gpt-5.5 should resolve via rule");

        let result_hyphen = lookup_model_pricing("gpt-5-5", &map);
        assert!(result_hyphen.is_some(), "gpt-5-5 should resolve via rule");
    }

    #[test]
    fn test_lookup_claude_4_6_does_not_fall_back_to_4_5_pricing() {
        let mut map = HashMap::new();
        map.insert(
            "openrouter/anthropic/claude-opus-4.5".to_string(),
            make_pricing(0.000003, 0.000015),
        );
        map.insert(
            "openrouter/anthropic/claude-sonnet-4.5".to_string(),
            make_pricing(0.000003, 0.000015),
        );

        assert!(lookup_model_pricing("claude-opus-4-6", &map).is_none());
        assert!(lookup_model_pricing("claude-sonnet-4-6", &map).is_none());
    }

    #[test]
    fn test_lookup_claude_4_6_variants_use_4_6_pricing() {
        // Router keys as upstream spells them; the catalog stores them as
        // `openrouter/claude-*-4-6`, and lookups must land on that form.
        let catalog = catalog(&[
            ("openrouter/anthropic/claude-opus-4.6", 0.000004, 0.00002),
            ("openrouter/anthropic/claude-sonnet-4.6", 0.000003, 0.000015),
        ]);
        let input = |model: &str| {
            catalog
                .lookup(model, Some("anthropic"))
                .unwrap()
                .pricing
                .input_cost_per_token
        };

        assert_eq!(input("claude-opus-4-6"), 0.000004);
        assert_eq!(input("claude-opus-4-6-thinking"), 0.000004);
        assert_eq!(input("claude-sonnet-4-6"), 0.000003);
        assert_eq!(input("claude-sonnet-4-6-thinking"), 0.000003);
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
