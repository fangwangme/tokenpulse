use tokenpulse_core::{
    config::AccountDisplay,
    quota::{AntigravityQuotaFetcher, ClaudeQuotaFetcher, CodexQuotaFetcher},
    QuotaFetcher,
};

// ---------------------------------------------------------------------------
// Quota Provider Registry
//
// All supported quota providers are registered here. To add a new provider:
// 1. Add a QuotaProviderEntry below
// 2. Implement QuotaFetcher in tokenpulse-core/src/quota/
//
// This is the only list. The Settings rows, the config keys `config enable`
// accepts, and the ids quota resolution honours are all derived from it, so a
// provider is never half-registered.
// ---------------------------------------------------------------------------

struct QuotaProviderEntry {
    /// Internal identifier used in config, CLI flags, and cache keys.
    id: &'static str,
    /// Human-readable name shown in UI headers and error messages.
    display_name: &'static str,
    /// Factory function to create the fetcher. A fetcher that has to do extra
    /// work to find the account email skips it unless the setting shows it.
    make_fetcher: fn(AccountDisplay) -> Box<dyn QuotaFetcher>,
}

const QUOTA_PROVIDERS: &[QuotaProviderEntry] = &[
    QuotaProviderEntry {
        id: "claude",
        display_name: "CLAUDE CODE",
        make_fetcher: |account_display| {
            Box::new(ClaudeQuotaFetcher::new().with_account_email(account_display.shows_account()))
        },
    },
    QuotaProviderEntry {
        id: "codex",
        display_name: "CODEX",
        make_fetcher: |_| Box::new(CodexQuotaFetcher::new()),
    },
    QuotaProviderEntry {
        id: "antigravity",
        display_name: "ANTIGRAVITY",
        make_fetcher: |_| Box::new(AntigravityQuotaFetcher::new()),
    },
];

/// Look up the display name for a quota provider.
pub fn quota_display_name(provider_id: &str) -> &'static str {
    QUOTA_PROVIDERS
        .iter()
        .find(|e| e.id == provider_id)
        .map(|e| e.display_name)
        .unwrap_or("UNKNOWN")
}

/// Ids of every registered quota provider, in registry order.
///
/// This is the single source of truth for "which providers are configurable as
/// quota providers": the Settings rows are generated from it, and config keys
/// are intersected with it, so a provider without a fetcher never has to be
/// listed a second time — and can never be configured by accident.
pub fn quota_provider_ids() -> Vec<&'static str> {
    QUOTA_PROVIDERS.iter().map(|e| e.id).collect()
}

/// Whether `provider_id` has a registered quota fetcher.
pub fn is_quota_provider(provider_id: &str) -> bool {
    QUOTA_PROVIDERS.iter().any(|e| e.id == provider_id)
}

/// Build QuotaFetcher instances for the requested providers.
///
/// When `provider` is Some, only that single provider is built (if known).
/// When `provider` is None, all enabled providers are built.
pub fn build_quota_fetchers(
    enabled_providers: &[String],
    account_display: AccountDisplay,
) -> Vec<Box<dyn QuotaFetcher>> {
    QUOTA_PROVIDERS
        .iter()
        .filter(|e| enabled_providers.contains(&e.id.to_string()))
        .map(|e| (e.make_fetcher)(account_display))
        .collect()
}

/// A plan name as the quota views show it: `plus` → `Plus`, `max` → `Max`.
/// Providers report it in whatever case their API uses.
pub fn display_plan(plan: &str) -> String {
    let mut chars = plan.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Provider ids in the same order `build_quota_fetchers` returns fetchers.
///
/// `fetch_all` yields bare `Result`s, so a failure carries no provider id;
/// zipping against this list is what lets a failed poll be attributed.
pub fn quota_fetcher_ids(enabled_providers: &[String]) -> Vec<&'static str> {
    QUOTA_PROVIDERS
        .iter()
        .filter(|e| enabled_providers.contains(&e.id.to_string()))
        .map(|e| e.id)
        .collect()
}
