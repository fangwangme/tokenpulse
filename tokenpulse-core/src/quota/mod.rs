pub mod antigravity;
pub mod cache;
pub mod claude;
pub mod codex;
pub mod copilot;

pub use antigravity::AntigravityQuotaFetcher;
pub use cache::{CachedQuotaSnapshot, QuotaCacheStore};
pub use claude::ClaudeQuotaFetcher;
pub use codex::CodexQuotaFetcher;
pub use copilot::CopilotQuotaFetcher;

use crate::{QuotaFetcher, QuotaSnapshot};
use anyhow::{anyhow, Result};

/// Fetch quota for every provider concurrently, spawning one Tokio task per
/// provider so a slow or blocking provider cannot stall the others. Results
/// preserve the input provider order.
pub async fn fetch_all(providers: Vec<Box<dyn QuotaFetcher>>) -> Vec<Result<QuotaSnapshot>> {
    let handles: Vec<_> = providers
        .into_iter()
        .map(|provider| tokio::spawn(async move { provider.fetch_quota().await }))
        .collect();

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        results.push(match handle.await {
            Ok(result) => result,
            Err(join_err) => Err(anyhow!("Quota fetch task panicked: {}", join_err)),
        });
    }
    results
}
