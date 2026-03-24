# Pricing Module - Detailed Design

## Overview

Fetch model pricing from LiteLLM, cache locally, calculate cost per message.

## Architecture

```
pricing/
├── mod.rs          # ModelPricing struct, calculate_cost(), lookup logic
└── litellm.rs      # fetch from GitHub, disk cache with TTL
```

## Pricing Data Source

**Primary:** LiteLLM model pricing JSON
```
GET https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
```

**Cache:** `~/.cache/tokenpulse/pricing.json`
**TTL:** 24 hours. On network failure, use stale cache.

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
4. Fallback: warn and use $0 cost
