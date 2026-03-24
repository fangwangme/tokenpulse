pub mod antigravity;
pub mod cache;
pub mod claude;
pub mod codex;
pub mod gemini;

pub use antigravity::AntigravityQuotaFetcher;
pub use cache::{CachedQuotaSnapshot, QuotaCacheStore};
pub use claude::ClaudeQuotaFetcher;
pub use codex::CodexQuotaFetcher;
pub use gemini::GeminiQuotaFetcher;

use crate::{QuotaFetcher, QuotaSnapshot};
use anyhow::Result;
use futures::future::join_all;

pub async fn fetch_all(providers: Vec<Box<dyn QuotaFetcher>>) -> Vec<Result<QuotaSnapshot>> {
    let futures: Vec<_> = providers.iter().map(|p| p.fetch_quota()).collect();
    join_all(futures).await
}
