use tokenpulse_core::{
    quota::{AntigravityQuotaFetcher, ClaudeQuotaFetcher, CodexQuotaFetcher, CopilotQuotaFetcher},
    QuotaFetcher,
};

// ---------------------------------------------------------------------------
// Quota Provider Registry
//
// All supported quota providers are registered here. To add a new provider:
// 1. Add a QuotaProviderEntry below
// 2. Implement QuotaFetcher in tokenpulse-core/src/quota/
// ---------------------------------------------------------------------------

struct QuotaProviderEntry {
    /// Internal identifier used in config, CLI flags, and cache keys.
    id: &'static str,
    /// Human-readable name shown in UI headers and error messages.
    display_name: &'static str,
    /// Project URL shown in the "no providers" help message.
    url: &'static str,
    /// Factory function to create the fetcher.
    make_fetcher: fn() -> Box<dyn QuotaFetcher>,
}

const QUOTA_PROVIDERS: &[QuotaProviderEntry] = &[
    QuotaProviderEntry {
        id: "claude",
        display_name: "CLAUDE CODE",
        url: "https://docs.anthropic.com/en/docs/claude-code",
        make_fetcher: || Box::new(ClaudeQuotaFetcher::new()),
    },
    QuotaProviderEntry {
        id: "codex",
        display_name: "CODEX",
        url: "https://github.com/openai/codex",
        make_fetcher: || Box::new(CodexQuotaFetcher::new()),
    },
    QuotaProviderEntry {
        id: "copilot",
        display_name: "GITHUB COPILOT",
        url: "https://github.com/features/copilot",
        make_fetcher: || Box::new(CopilotQuotaFetcher::new()),
    },
    QuotaProviderEntry {
        id: "antigravity",
        display_name: "ANTIGRAVITY",
        url: "https://antigravity.com",
        make_fetcher: || Box::new(AntigravityQuotaFetcher::new()),
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

/// Provider metadata exposed for the TUI settings tab.
#[allow(dead_code)]
pub struct QuotaProviderInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub url: &'static str,
}

/// Return metadata for all supported quota providers (for TUI rendering).
pub fn quota_provider_info_list() -> Vec<QuotaProviderInfo> {
    QUOTA_PROVIDERS
        .iter()
        .map(|e| QuotaProviderInfo {
            id: e.id,
            display_name: e.display_name,
            url: e.url,
        })
        .collect()
}

/// Build QuotaFetcher instances for the requested providers.
///
/// When `provider` is Some, only that single provider is built (if known).
/// When `provider` is None, all enabled providers are built.
pub fn build_quota_fetchers(
    provider: Option<&str>,
    enabled_providers: &[String],
) -> Vec<Box<dyn QuotaFetcher>> {
    match provider {
        Some(name) => QUOTA_PROVIDERS
            .iter()
            .filter(|e| e.id == name)
            .map(|e| (e.make_fetcher)())
            .collect(),
        None => QUOTA_PROVIDERS
            .iter()
            .filter(|e| enabled_providers.contains(&e.id.to_string()))
            .map(|e| (e.make_fetcher)())
            .collect(),
    }
}
