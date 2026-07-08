use super::{litellm, modelsdev, openrouter, PricingCatalog, PricingRecord};
use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tracing::{debug, info, warn};

const CACHE_TTL_HOURS: i64 = 24;
const LAZY_REFRESH_MIN_INTERVAL_SECS: u64 = 60 * 60;

static HAS_REFRESHED_THIS_RUN: AtomicBool = AtomicBool::new(false);

pub struct PricingCache {
    cache_path: PathBuf,
    lazy_refresh_attempt_path: PathBuf,
}

impl PricingCache {
    pub fn has_refreshed_this_run() -> bool {
        HAS_REFRESHED_THIS_RUN.load(Ordering::Relaxed)
    }

    fn mark_refreshed_this_run() {
        HAS_REFRESHED_THIS_RUN.store(true, Ordering::Relaxed);
    }

    /// On-demand refresh for missing-model lookups. Attempted at most once
    /// per run, and skipped when the on-disk cache was fetched recently:
    /// a model absent from a fresh catalog is unlikely to appear upstream
    /// within the hour, so unknown or misparsed model ids cannot trigger
    /// repeated network fetches across reloads.
    pub fn lazy_refresh_sync(&self) -> Result<Option<PricingCatalog>> {
        Self::mark_refreshed_this_run();

        if self.lazy_refresh_attempted_recently() {
            debug!("Skipping on-demand pricing refresh; already attempted within the last hour");
            return Ok(None);
        }

        self.mark_lazy_refresh_attempt()?;
        self.fetch_and_cache_sync().map(Some)
    }

    pub fn clear_memory_cache() -> Result<()> {
        let mut cache = memory_cache()
            .lock()
            .map_err(|_| anyhow!("Pricing cache mutex poisoned"))?;
        cache.clear();
        Ok(())
    }

    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let data_dir = home.join(".local").join("share").join("tokenpulse");

        Self {
            cache_path: data_dir.join("pricing.json"),
            lazy_refresh_attempt_path: data_dir.join("pricing-refresh-attempt"),
        }
    }

    pub fn get_pricing_sync(&self) -> Result<PricingCatalog> {
        if let Some(cached) = self.load_memory_cached()? {
            debug!("Using in-memory pricing catalog");
            return Ok(cached.pricing);
        }

        if let Some(cached) = self.load_cached()? {
            debug!("Using cached pricing catalog");
            self.store_memory_cache(&cached)?;
            return Ok(cached.pricing);
        }

        self.fetch_and_cache_sync()
    }

    pub fn get_pricing_allow_stale_sync(&self) -> Result<PricingCatalog> {
        if let Some(cached) = self.load_memory_cached_any()? {
            debug!("Using in-memory pricing catalog");
            return Ok(cached.pricing);
        }

        if let Some(cached) = self.load_cached()? {
            debug!("Using cached pricing catalog");
            self.store_memory_cache(&cached)?;
            return Ok(cached.pricing);
        }

        if let Some(cached) = self.load_stale_cached()? {
            debug!("Using stale pricing catalog to avoid blocking usage startup");
            self.store_memory_cache(&cached)?;
            return Ok(cached.pricing);
        }

        self.fetch_and_cache_sync()
    }

    fn load_cached(&self) -> Result<Option<CachedPricing>> {
        if !self.cache_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&self.cache_path)?;
        let cached = match serde_json::from_str::<CachedPricing>(&content) {
            Ok(cached) => cached,
            Err(error) => {
                warn!(
                    "Ignoring unreadable pricing cache at {}: {}",
                    self.cache_path.display(),
                    error
                );
                return Ok(None);
            }
        };

        if cache_is_fresh(cached.fetched_at) {
            Ok(Some(cached))
        } else {
            debug!("Pricing catalog cache expired");
            Ok(None)
        }
    }

    fn load_stale_cached(&self) -> Result<Option<CachedPricing>> {
        if !self.cache_path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&self.cache_path)?;
        let cached = match serde_json::from_str::<CachedPricing>(&content) {
            Ok(cached) => cached,
            Err(error) => {
                warn!(
                    "Ignoring unreadable stale pricing cache at {}: {}",
                    self.cache_path.display(),
                    error
                );
                return Ok(None);
            }
        };

        Ok(Some(cached))
    }

    fn fetch_and_cache_sync(&self) -> Result<PricingCatalog> {
        info!("Refreshing pricing catalog from LiteLLM, OpenRouter, and models.dev");
        Self::mark_refreshed_this_run();

        let mut failures = Vec::new();
        let mut catalog = PricingCatalog::default();

        merge_source(
            &mut catalog,
            litellm::fetch_sync(),
            "litellm",
            &mut failures,
        );
        merge_source(
            &mut catalog,
            openrouter::fetch_sync(),
            "openrouter",
            &mut failures,
        );
        merge_source(
            &mut catalog,
            modelsdev::fetch_sync(),
            "models.dev",
            &mut failures,
        );

        if catalog.entries().is_empty() {
            if let Some(cached) = self.load_stale_cached()? {
                warn!(
                    "All live pricing sources failed ({}); using stale pricing cache",
                    failures.join(", ")
                );
                self.store_memory_cache(&cached)?;
                return Ok(cached.pricing);
            }

            let detail = if failures.is_empty() {
                "no pricing sources returned data".to_string()
            } else {
                failures.join(", ")
            };
            return Err(anyhow!("Failed to load pricing catalog: {detail}"));
        }

        if !failures.is_empty() {
            warn!(
                "Loaded pricing catalog with partial source coverage: {}",
                failures.join(", ")
            );
        }

        let cached = CachedPricing {
            pricing: catalog,
            fetched_at: Utc::now(),
        };

        if let Some(parent) = self.cache_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.cache_path, serde_json::to_string_pretty(&cached)?)?;
        self.store_memory_cache(&cached)?;

        Ok(cached.pricing)
    }

    fn load_memory_cached(&self) -> Result<Option<CachedPricing>> {
        let mut cache = memory_cache()
            .lock()
            .map_err(|_| anyhow!("Pricing cache mutex poisoned"))?;

        if let Some(mem_cached) = cache.get(&self.cache_path) {
            let mtime = fs::metadata(&self.cache_path)
                .and_then(|meta| meta.modified())
                .ok();
            if mem_cached.loaded_mtime == mtime && cache_is_fresh(mem_cached.cached.fetched_at) {
                return Ok(Some(mem_cached.cached.clone()));
            }
        }

        cache.remove(&self.cache_path);
        Ok(None)
    }

    fn load_memory_cached_any(&self) -> Result<Option<CachedPricing>> {
        let mut cache = memory_cache()
            .lock()
            .map_err(|_| anyhow!("Pricing cache mutex poisoned"))?;

        if let Some(mem_cached) = cache.get(&self.cache_path) {
            let mtime = fs::metadata(&self.cache_path)
                .and_then(|meta| meta.modified())
                .ok();
            if mem_cached.loaded_mtime == mtime {
                return Ok(Some(mem_cached.cached.clone()));
            }
        }

        cache.remove(&self.cache_path);
        Ok(None)
    }

    fn store_memory_cache(&self, cached: &CachedPricing) -> Result<()> {
        let mtime = fs::metadata(&self.cache_path)
            .and_then(|meta| meta.modified())
            .ok();

        memory_cache()
            .lock()
            .map_err(|_| anyhow!("Pricing cache mutex poisoned"))?
            .insert(
                self.cache_path.clone(),
                MemoryCachedPricing {
                    cached: cached.clone(),
                    loaded_mtime: mtime,
                },
            );
        Ok(())
    }

    fn lazy_refresh_attempted_recently(&self) -> bool {
        [&self.cache_path, &self.lazy_refresh_attempt_path]
            .iter()
            .filter_map(|path| fs::metadata(path).and_then(|meta| meta.modified()).ok())
            .filter_map(|mtime| mtime.elapsed().ok())
            .any(|age| age.as_secs() < LAZY_REFRESH_MIN_INTERVAL_SECS)
    }

    fn mark_lazy_refresh_attempt(&self) -> Result<()> {
        if let Some(parent) = self.lazy_refresh_attempt_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.lazy_refresh_attempt_path, Utc::now().to_rfc3339())?;
        Ok(())
    }
}

impl Default for PricingCache {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedPricing {
    pricing: PricingCatalog,
    fetched_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct MemoryCachedPricing {
    cached: CachedPricing,
    loaded_mtime: Option<std::time::SystemTime>,
}

fn memory_cache() -> &'static Mutex<std::collections::HashMap<PathBuf, MemoryCachedPricing>> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<PathBuf, MemoryCachedPricing>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn cache_is_fresh(fetched_at: DateTime<Utc>) -> bool {
    fetched_at + Duration::hours(CACHE_TTL_HOURS) > Utc::now()
}

fn merge_source(
    catalog: &mut PricingCatalog,
    source_result: Result<HashMap<String, PricingRecord>>,
    source_name: &str,
    failures: &mut Vec<String>,
) {
    match source_result {
        Ok(entries) => {
            for (key, record) in entries {
                catalog.insert_if_missing_or_unusable(key, record);
            }
        }
        Err(error) => failures.push(format!("{source_name}: {error}")),
    }
}
