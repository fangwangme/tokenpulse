use super::{ModelPricing, PricingRecord};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::warn;

const LITELLM_PRICING_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const LITELLM_VERSION: &str = "litellm-main-v1";

#[derive(Debug, Deserialize, Serialize)]
struct LiteLLMPricing {
    input_cost_per_token: Option<f64>,
    output_cost_per_token: Option<f64>,
    cache_read_input_token_cost: Option<f64>,
    cache_creation_input_token_cost: Option<f64>,
}

pub fn fetch_sync() -> Result<HashMap<String, PricingRecord>> {
    let response = ureq::get(LITELLM_PRICING_URL)
        .timeout(std::time::Duration::from_secs(30))
        .call()?;

    if response.status() >= 400 {
        anyhow::bail!("LiteLLM returned HTTP {}", response.status());
    }

    let payload: HashMap<String, LiteLLMPricing> = response.into_json()?;
    let entries: HashMap<String, PricingRecord> = payload
        .into_iter()
        .filter_map(|(key, pricing)| {
            let input_cost = pricing.input_cost_per_token?;
            let output_cost = pricing.output_cost_per_token?;

            Some((
                key,
                PricingRecord::new(
                    ModelPricing::new(
                        input_cost,
                        output_cost,
                        pricing.cache_read_input_token_cost,
                        pricing.cache_creation_input_token_cost,
                    ),
                    "litellm",
                    LITELLM_VERSION,
                ),
            ))
        })
        .collect();

    if entries.is_empty() {
        warn!("LiteLLM returned no usable pricing rows");
    }

    Ok(entries)
}
