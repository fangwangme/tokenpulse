use super::{ModelPricing, PricingRecord};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::HashMap;

const MODELSDEV_URL: &str = "https://models.dev/api.json";
const MODELSDEV_VERSION: &str = "models.dev-api-v1";
const TOKENS_PER_MILLION: f64 = 1_000_000.0;

#[derive(Debug, Deserialize)]
struct ModelsDevProvider {
    #[serde(default)]
    models: HashMap<String, ModelsDevModel>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevModel {
    id: String,
    cost: Option<ModelsDevCost>,
}

#[derive(Debug, Deserialize)]
struct ModelsDevCost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

pub fn fetch_sync() -> Result<HashMap<String, PricingRecord>> {
    let response = ureq::get(MODELSDEV_URL)
        .timeout(std::time::Duration::from_secs(30))
        .call()?;

    if response.status() >= 400 {
        anyhow::bail!("models.dev returned HTTP {}", response.status());
    }

    let providers: HashMap<String, ModelsDevProvider> = response.into_json()?;
    let mut entries = HashMap::new();

    for (provider, provider_data) in providers {
        for model in provider_data.models.into_values() {
            let Some(cost) = model.cost else {
                continue;
            };
            let Some(input_cost) = cost.input else {
                continue;
            };
            let Some(output_cost) = cost.output else {
                continue;
            };

            entries.insert(
                format!("{provider}/{}", model.id),
                PricingRecord::new(
                    ModelPricing::new(
                        input_cost / TOKENS_PER_MILLION,
                        output_cost / TOKENS_PER_MILLION,
                        cost.cache_read.map(|value| value / TOKENS_PER_MILLION),
                        cost.cache_write.map(|value| value / TOKENS_PER_MILLION),
                    ),
                    format!("models.dev:{provider}"),
                    MODELSDEV_VERSION,
                ),
            );
        }
    }

    if entries.is_empty() {
        return Err(anyhow!("models.dev returned no usable pricing rows"));
    }

    Ok(entries)
}
