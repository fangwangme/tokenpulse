use super::{ModelPricing, PricingRecord};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::HashMap;

const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/models";
const OPENROUTER_VERSION: &str = "openrouter-models-v1";

#[derive(Debug, Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModel {
    id: String,
    pricing: Option<OpenRouterPricing>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterPricing {
    prompt: Option<String>,
    completion: Option<String>,
    #[serde(default)]
    input_cache_read: Option<String>,
    #[serde(default)]
    input_cache_write: Option<String>,
}

pub fn fetch_sync() -> Result<HashMap<String, PricingRecord>> {
    let response = ureq::get(OPENROUTER_URL)
        .timeout(std::time::Duration::from_secs(30))
        .call()?;

    if response.status() >= 400 {
        anyhow::bail!("OpenRouter returned HTTP {}", response.status());
    }

    let payload: OpenRouterModelsResponse = response.into_json()?;
    let mut entries = HashMap::new();

    for model in payload.data {
        let Some(pricing) = model.pricing else {
            continue;
        };
        let Some(input_cost) = pricing.prompt.as_deref().and_then(parse_price) else {
            continue;
        };
        let Some(output_cost) = pricing.completion.as_deref().and_then(parse_price) else {
            continue;
        };

        let record = PricingRecord::new(
            ModelPricing::new(
                input_cost,
                output_cost,
                pricing.input_cache_read.as_deref().and_then(parse_price),
                pricing.input_cache_write.as_deref().and_then(parse_price),
            ),
            "openrouter",
            OPENROUTER_VERSION,
        );

        entries.insert(model.id.clone(), record.clone());
        entries.insert(format!("openrouter/{}", model.id), record);
    }

    if entries.is_empty() {
        return Err(anyhow!("OpenRouter returned no usable pricing rows"));
    }

    Ok(entries)
}

fn parse_price(value: &str) -> Option<f64> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|parsed| parsed.is_finite() && *parsed >= 0.0)
}
