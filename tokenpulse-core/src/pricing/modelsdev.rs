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
            let Some(input_cost) = cost.input.and_then(price_per_token) else {
                continue;
            };
            let Some(output_cost) = cost.output.and_then(price_per_token) else {
                continue;
            };

            entries.insert(
                format!("{provider}/{}", model.id),
                PricingRecord::new(
                    ModelPricing::new(
                        input_cost,
                        output_cost,
                        cost.cache_read.and_then(price_per_token),
                        cost.cache_write.and_then(price_per_token),
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

fn price_per_token(value: f64) -> Option<f64> {
    value
        .is_finite()
        .then_some(value)
        .filter(|price| *price >= 0.0)
        .map(|price| price / TOKENS_PER_MILLION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_per_token_accepts_non_negative_finite_values() {
        assert_eq!(price_per_token(1.4), Some(0.0000014));
        assert_eq!(price_per_token(0.0), Some(0.0));
    }

    #[test]
    fn price_per_token_rejects_negative_and_non_finite_values() {
        assert_eq!(price_per_token(-1.0), None);
        assert_eq!(price_per_token(f64::NAN), None);
        assert_eq!(price_per_token(f64::INFINITY), None);
    }
}
