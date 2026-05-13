# Pricing Module - Detailed Design

## Overview

Build a merged pricing catalog from aggregator sources, cache it locally, and calculate cost from daily captured pricing snapshots.

## Architecture

```text
pricing/
├── cache.rs        # merged catalog cache and source priority
├── mod.rs          # ModelPricing, lookup logic, catalog types
├── litellm.rs      # LiteLLM source
├── modelsdev.rs    # models.dev source
└── openrouter.rs   # OpenRouter source
```

## Pricing Data Source

Source priority:

1. LiteLLM
2. models.dev
3. OpenRouter

Endpoints:

```text
https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
https://models.dev/api.json
https://openrouter.ai/api/v1/models
```

**Cache:** `~/.cache/tokenpulse/pricing.json`
**TTL:** 24 hours. If all live fetches fail, TokenPulse uses the stale merged cache.

## Data Model

```rust
pub struct ModelPricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_input_token_cost: Option<f64>,
    pub cache_creation_input_token_cost: Option<f64>,
}
```

## Cost Calculation

```rust
pub fn calculate_cost(tokens: &TokenBreakdown, pricing: &ModelPricing) -> f64 {
    let input = tokens.input as f64 * pricing.input_cost_per_token;
    let output = tokens.output as f64 * pricing.output_cost_per_token;
    let cache_read = tokens.cache_read as f64
        * pricing.cache_read_input_token_cost.unwrap_or(pricing.input_cost_per_token * 0.1);
    let cache_write = tokens.cache_write as f64
        * pricing.cache_creation_input_token_cost.unwrap_or(pricing.input_cost_per_token * 1.25);
    let reasoning = tokens.reasoning as f64 * pricing.output_cost_per_token;

    input + output + cache_read + cache_write + reasoning
}
```

## Model ID Lookup Strategy

1. Exact match: `"claude-opus-4"` → found
2. With provider prefix: `"anthropic/claude-opus-4"` → found
3. Strip date suffix: `"claude-opus-4-20260315"` → try `"claude-opus-4"`
4. Exact provider-qualified matches win over normalized fallback candidates.
5. Fallback source order per key: LiteLLM -> models.dev -> OpenRouter.
6. If still missing: warn and use $0 cost.
