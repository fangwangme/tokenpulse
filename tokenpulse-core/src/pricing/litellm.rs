use super::{ModelPricing, calculate_cost, lookup_model_pricing};
use crate::provider::UnifiedMessage;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{debug, info, warn};

const LITELLM_PRICING_URL: &str = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";
const CACHE_TTL_HOURS: i64 = 24;

#[derive(Debug, Deserialize)]
struct LiteLLMPricing {
    input_cost_per_token: Option<f64>,
    output_cost_per_token: Option<f64>,
    cache_read_input_token_cost: Option<f64>,
    cache_creation_input_token_cost: Option<f64>,
}

pub struct PricingCache {
    cache_path: PathBuf,
}

impl PricingCache {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        let cache_dir = home.join(".cache").join("tokenpulse");

        Self {
            cache_path: cache_dir.join("pricing.json"),
        }
    }

    pub fn get_pricing_sync(&self) -> Result<HashMap<String, ModelPricing>> {
        if let Some(cached) = self.load_cached()? {
            debug!("Using cached pricing data");
            return Ok(cached);
        }

        self.fetch_and_cache_sync()
    }

    pub async fn get_pricing(&self) -> Result<HashMap<String, ModelPricing>> {
        self.get_pricing_sync()
    }

    fn load_cached(&self) -> Result<Option<HashMap<String, ModelPricing>>> {
        if !self.cache_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&self.cache_path)?;
        let cached: CachedPricing = serde_json::from_str(&content)?;

        let now = Utc::now();
        if cached.fetched_at + Duration::hours(CACHE_TTL_HOURS) > now {
            Ok(Some(cached.pricing))
        } else {
            debug!("Cache expired");
            Ok(None)
        }
    }

    fn fetch_and_cache_sync(&self) -> Result<HashMap<String, ModelPricing>> {
        info!("Fetching pricing data from LiteLLM");

        let response = ureq::get(LITELLM_PRICING_URL)
            .timeout(std::time::Duration::from_secs(30))
            .call()?;

        if response.status() >= 400 {
            warn!("Failed to fetch pricing, using stale cache if available");
            if let Some(cached) = self.load_cached()? {
                return Ok(cached);
            }
            anyhow::bail!("Failed to fetch pricing data: {}", response.status());
        }

        let litellm_data: HashMap<String, LiteLLMPricing> = response.into_json()?;

        let pricing: HashMap<String, ModelPricing> = litellm_data
            .into_iter()
            .filter_map(|(k, v)| {
                let input = v.input_cost_per_token?;
                let output = v.output_cost_per_token?;
                Some((
                    k,
                    ModelPricing {
                        input_cost_per_token: input,
                        output_cost_per_token: output,
                        cache_read_input_token_cost: v.cache_read_input_token_cost,
                        cache_creation_input_token_cost: v.cache_creation_input_token_cost,
                    },
                ))
            })
            .collect();

        let cached = CachedPricing {
            pricing: pricing.clone(),
            fetched_at: Utc::now(),
        };

        if let Some(parent) = self.cache_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.cache_path, serde_json::to_string_pretty(&cached)?)?;

        Ok(pricing)
    }
}

impl Default for PricingCache {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedPricing {
    pricing: HashMap<String, ModelPricing>,
    fetched_at: DateTime<Utc>,
}

pub fn calculate_message_cost(message: &UnifiedMessage, pricing: &HashMap<String, ModelPricing>) -> f64 {
    match lookup_model_pricing(&message.model_id, pricing) {
        Some(p) => calculate_cost(&message.tokens, p),
        None => {
            warn!("No pricing found for model: {}", message.model_id);
            0.0
        }
    }
}
