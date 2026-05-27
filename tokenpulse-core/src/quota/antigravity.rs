use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow};
use crate::usage::antigravity::{
    detect_antigravity_connections, AntigravityConnection, AntigravityRuntimeKind,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info};

const LS_PROBE_TIMEOUT_SECS: u64 = 3;

// Connect-RPC service
const LS_SERVICE: &str = "exa.language_server_pb.LanguageServerService";

// ── LS RPC response types ──

#[derive(Debug, Deserialize)]
struct LsUserStatusResponse {
    #[serde(default, rename = "cascadeModelConfigData")]
    cascade_model_config_data: Option<CascadeModelConfigData>,
    #[serde(default, rename = "userStatus")]
    user_status: Option<UserStatus>,
    #[serde(default, rename = "clientModelConfigs")]
    client_model_configs: Vec<ClientModelConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct CascadeModelConfigData {
    #[serde(default, rename = "clientModelConfigs")]
    client_model_configs: Vec<ClientModelConfig>,
}

#[derive(Debug, Clone, Deserialize)]
struct ClientModelConfig {
    #[serde(default)]
    label: Option<String>,
    #[serde(default, rename = "modelOrAlias")]
    model_or_alias: Option<ModelOrAlias>,
    #[serde(default, rename = "quotaInfo")]
    quota_info: Option<LsQuotaInfo>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModelOrAlias {
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct LsQuotaInfo {
    #[serde(default, rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(default, rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserStatus {
    #[serde(default)]
    email: Option<String>,
    #[serde(default, rename = "planStatus")]
    plan_status: Option<PlanStatus>,
    #[serde(default, rename = "cascadeModelConfigData")]
    cascade_model_config_data: Option<CascadeModelConfigData>,
}

#[derive(Debug, Deserialize)]
struct PlanStatus {
    #[serde(default, rename = "planInfo")]
    plan_info: Option<PlanInfo>,
}

#[derive(Debug, Deserialize)]
struct PlanInfo {
    #[serde(default, rename = "planName")]
    plan_name: Option<String>,
}

// ── Unified pool data ──

#[derive(Debug, Clone)]
struct PoolQuota {
    remaining_fraction: f64,
    reset_time: Option<String>,
    period_duration_ms: i64,
}

struct PoolData {
    pools: HashMap<String, PoolQuota>,
    plan: Option<String>,
    account: Option<String>,
}

// ── Main fetcher ──

pub struct AntigravityQuotaFetcher {
    ls_client: Client,
}

impl AntigravityQuotaFetcher {
    const FIVE_HOURS_MS: i64 = 5 * 60 * 60 * 1000;
    const SEVEN_DAYS_MS: i64 = 7 * 24 * 60 * 60 * 1000;

    pub fn new() -> Self {
        let ls_client = Client::builder()
            .timeout(Duration::from_secs(LS_PROBE_TIMEOUT_SECS))
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { ls_client }
    }

    // ── Language Server fallback ──

    async fn probe_ls(&self, connection: &AntigravityConnection) -> Option<PoolData> {
        debug!(
            "Using {} Antigravity LS at {}:{}",
            connection.runtime_kind.as_str(),
            connection.scheme,
            connection.port
        );

        let url = format!(
            "{}://127.0.0.1:{}/{}/GetUserStatus",
            connection.scheme, connection.port, LS_SERVICE
        );

        let metadata = serde_json::json!({
            "ideName": "antigravity",
            "extensionName": "antigravity",
            "ideVersion": "unknown",
            "locale": "en",
        });

        let body = serde_json::json!({ "metadata": metadata });

        let mut request = self
            .ls_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .json(&body);
        if let Some(csrf_token) = connection.csrf_token.as_deref() {
            request = request.header("x-codeium-csrf-token", csrf_token);
        }
        let response = request.send().await.ok()?;

        if !response.status().is_success() {
            debug!("GetUserStatus failed: {}", response.status());
            return self.probe_ls_model_configs(connection).await;
        }

        let data: LsUserStatusResponse = response.json().await.ok()?;
        self.parse_ls_response(data)
    }

    async fn probe_ls_model_configs(&self, connection: &AntigravityConnection) -> Option<PoolData> {
        let url = format!(
            "{}://127.0.0.1:{}/{}/GetCommandModelConfigs",
            connection.scheme, connection.port, LS_SERVICE
        );

        let body = serde_json::json!({
            "metadata": {
                "ideName": "antigravity",
                "extensionName": "antigravity",
                "ideVersion": "unknown",
                "locale": "en",
            }
        });

        let mut request = self
            .ls_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Connect-Protocol-Version", "1")
            .json(&body);
        if let Some(csrf_token) = connection.csrf_token.as_deref() {
            request = request.header("x-codeium-csrf-token", csrf_token);
        }
        let response = request.send().await.ok()?;

        if !response.status().is_success() {
            debug!("GetCommandModelConfigs failed: {}", response.status());
            return None;
        }

        let data: LsUserStatusResponse = response.json().await.ok()?;
        self.parse_ls_response(data)
    }

    async fn probe_ls_connections(
        &self,
        connections: &[AntigravityConnection],
        runtime_kind: AntigravityRuntimeKind,
    ) -> Option<PoolData> {
        for connection in connections
            .iter()
            .filter(|conn| conn.runtime_kind == runtime_kind)
        {
            if let Some(pool_data) = self.probe_ls(connection).await {
                info!(
                    "Antigravity quota fetched via {} Language Server (pid={})",
                    connection.runtime_kind.as_str(),
                    connection.pid
                );
                return Some(pool_data);
            }
        }
        None
    }

    async fn probe_remaining_ls_connections(
        &self,
        connections: &[AntigravityConnection],
    ) -> Option<PoolData> {
        for connection in connections.iter().filter(|conn| {
            conn.runtime_kind != AntigravityRuntimeKind::Cli
                && conn.runtime_kind != AntigravityRuntimeKind::Desktop
        }) {
            if let Some(pool_data) = self.probe_ls(connection).await {
                info!(
                    "Antigravity quota fetched via {} Language Server (pid={})",
                    connection.runtime_kind.as_str(),
                    connection.pid
                );
                return Some(pool_data);
            }
        }
        None
    }

    fn parse_ls_response(&self, data: LsUserStatusResponse) -> Option<PoolData> {
        let account = data
            .user_status
            .as_ref()
            .and_then(|u| u.email.clone())
            .filter(|email| !email.trim().is_empty());

        let configs = data
            .user_status
            .as_ref()
            .and_then(|u| u.cascade_model_config_data.as_ref())
            .map(|c| c.client_model_configs.clone())
            .or_else(|| {
                data.cascade_model_config_data
                    .as_ref()
                    .map(|c| c.client_model_configs.clone())
            })
            .unwrap_or_else(|| data.client_model_configs.clone());
        if configs.is_empty() {
            return None;
        }

        let plan = data
            .user_status
            .and_then(|u| u.plan_status)
            .and_then(|p| p.plan_info)
            .and_then(|i| i.plan_name);

        let mut pools: HashMap<String, PoolQuota> = HashMap::new();

        for config in &configs {
            let label = config.label.as_deref().unwrap_or("");
            let model = config
                .model_or_alias
                .as_ref()
                .and_then(|m| m.model.as_deref())
                .unwrap_or("");

            // Skip if no quota info
            let quota = match &config.quota_info {
                Some(q) => q,
                None => continue,
            };

            let frac = quota.remaining_fraction.unwrap_or(1.0);
            let reset_time = quota.reset_time.clone();

            // Determine pool from label or model name
            let pool_name = self.pool_label_from_ls(label, model);
            let period_duration_ms =
                self.infer_period_duration_ms(reset_time.as_deref(), &pool_name);

            let entry = pools.entry(pool_name).or_insert(PoolQuota {
                remaining_fraction: frac,
                reset_time: reset_time.clone(),
                period_duration_ms,
            });
            if frac < entry.remaining_fraction {
                *entry = PoolQuota {
                    remaining_fraction: frac,
                    reset_time,
                    period_duration_ms,
                };
            }
        }

        if pools.is_empty() {
            return None;
        }

        Some(PoolData {
            pools,
            plan,
            account,
        })
    }

    fn pool_label_from_ls(&self, label: &str, model: &str) -> String {
        let combined = format!("{} {}", label, model).to_lowercase();
        if combined.contains("gemini") && combined.contains("pro") {
            "Gemini Pro".to_string()
        } else if combined.contains("gemini") && combined.contains("flash") {
            "Gemini Flash".to_string()
        } else {
            "Claude".to_string()
        }
    }

    fn pool_period_duration_ms(&self, pool_label: &str) -> i64 {
        if pool_label.to_lowercase().contains("flash") {
            Self::FIVE_HOURS_MS
        } else {
            Self::SEVEN_DAYS_MS
        }
    }

    fn infer_period_duration_ms(&self, reset_time: Option<&str>, pool_label: &str) -> i64 {
        let Some(reset_time) = reset_time else {
            return self.pool_period_duration_ms(pool_label);
        };

        let Some(reset_at) = DateTime::parse_from_rfc3339(reset_time)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
        else {
            return self.pool_period_duration_ms(pool_label);
        };

        if reset_at
            .signed_duration_since(Utc::now())
            .num_milliseconds()
            <= Self::FIVE_HOURS_MS
        {
            Self::FIVE_HOURS_MS
        } else {
            Self::SEVEN_DAYS_MS
        }
    }

    // Common: convert pool data to QuotaSnapshot
    fn pools_to_snapshot(&self, pool_data: PoolData) -> QuotaSnapshot {
        let mut windows = Vec::new();

        let mut sorted_pools: Vec<_> = pool_data.pools.into_iter().collect();
        sorted_pools.sort_by(|a, b| {
            let key = |name: &str| -> &str {
                if name.contains("Pro") {
                    "0"
                } else if name.contains("Flash") {
                    "1"
                } else {
                    "2"
                }
            };
            key(&a.0).cmp(key(&b.0))
        });

        for (pool, quota) in sorted_pools {
            let used = ((1.0 - quota.remaining_fraction.clamp(0.0, 1.0)) * 100.0).round();
            let resets_at = quota.reset_time.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            });

            windows.push(RateWindow {
                label: pool,
                used_percent: used,
                resets_at,
                period_duration_ms: Some(quota.period_duration_ms),
            });
        }

        if windows.is_empty() {
            windows.push(RateWindow {
                label: "Usage".to_string(),
                used_percent: 0.0,
                resets_at: None,
                period_duration_ms: None,
            });
        }

        QuotaSnapshot {
            provider: "antigravity".to_string(),
            plan: pool_data.plan,
            account: pool_data.account,
            windows,
            credits: None,
            fetched_at: Utc::now(),
        }
    }
}

impl Default for AntigravityQuotaFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QuotaFetcher for AntigravityQuotaFetcher {
    fn provider_name(&self) -> &str {
        "antigravity"
    }

    fn provider_display_name(&self) -> &str {
        "Antigravity"
    }

    async fn fetch_quota(&self) -> Result<QuotaSnapshot> {
        let connections = tokio::task::spawn_blocking(detect_antigravity_connections)
            .await
            .unwrap_or_else(|e| Err(anyhow!("Spawn blocking failed: {}", e)))
            .unwrap_or_else(|e| {
                debug!("Antigravity language server discovery failed: {}", e);
                Vec::new()
            });

        if let Some(pool_data) = self
            .probe_ls_connections(&connections, AntigravityRuntimeKind::Cli)
            .await
        {
            return Ok(self.pools_to_snapshot(pool_data));
        }

        if let Some(pool_data) = self
            .probe_ls_connections(&connections, AntigravityRuntimeKind::Desktop)
            .await
        {
            return Ok(self.pools_to_snapshot(pool_data));
        }

        if let Some(pool_data) = self.probe_remaining_ls_connections(&connections).await {
            return Ok(self.pools_to_snapshot(pool_data));
        }

        Err(anyhow!(
            "Antigravity quota unavailable: no running Antigravity CLI or desktop language server responded."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_period_duration_matches_pool_type() {
        let fetcher = AntigravityQuotaFetcher::new();

        assert_eq!(
            fetcher.pool_period_duration_ms("Gemini Flash"),
            AntigravityQuotaFetcher::FIVE_HOURS_MS
        );
        assert_eq!(
            fetcher.pool_period_duration_ms("Gemini Pro"),
            AntigravityQuotaFetcher::SEVEN_DAYS_MS
        );
        assert_eq!(
            fetcher.pool_period_duration_ms("Claude"),
            AntigravityQuotaFetcher::SEVEN_DAYS_MS
        );
    }

    #[test]
    fn infer_period_duration_uses_reset_time_before_pool_default() {
        let fetcher = AntigravityQuotaFetcher::new();
        let short_reset = (Utc::now() + chrono::Duration::hours(4)).to_rfc3339();
        let long_reset = (Utc::now() + chrono::Duration::hours(6)).to_rfc3339();

        assert_eq!(
            fetcher.infer_period_duration_ms(Some(&short_reset), "Claude"),
            AntigravityQuotaFetcher::FIVE_HOURS_MS
        );
        assert_eq!(
            fetcher.infer_period_duration_ms(Some(&long_reset), "Gemini Flash"),
            AntigravityQuotaFetcher::SEVEN_DAYS_MS
        );
        assert_eq!(
            fetcher.infer_period_duration_ms(None, "Gemini Flash"),
            AntigravityQuotaFetcher::FIVE_HOURS_MS
        );
    }

    #[test]
    fn pools_to_snapshot_preserves_per_pool_periods() {
        let fetcher = AntigravityQuotaFetcher::new();
        let snapshot = fetcher.pools_to_snapshot(PoolData {
            pools: HashMap::from([
                (
                    "Gemini Flash".to_string(),
                    PoolQuota {
                        remaining_fraction: 0.9,
                        reset_time: Some("2026-03-18T00:00:00Z".to_string()),
                        period_duration_ms: AntigravityQuotaFetcher::FIVE_HOURS_MS,
                    },
                ),
                (
                    "Claude".to_string(),
                    PoolQuota {
                        remaining_fraction: 0.2,
                        reset_time: Some("2026-03-24T00:00:00Z".to_string()),
                        period_duration_ms: AntigravityQuotaFetcher::SEVEN_DAYS_MS,
                    },
                ),
            ]),
            plan: Some("test".to_string()),
            account: Some("user@example.com".to_string()),
        });

        assert_eq!(snapshot.account.as_deref(), Some("user@example.com"));

        let flash = snapshot
            .windows
            .iter()
            .find(|window| window.label == "Gemini Flash")
            .unwrap();
        let claude = snapshot
            .windows
            .iter()
            .find(|window| window.label == "Claude")
            .unwrap();

        assert_eq!(
            flash.period_duration_ms,
            Some(AntigravityQuotaFetcher::FIVE_HOURS_MS)
        );
        assert_eq!(
            claude.period_duration_ms,
            Some(AntigravityQuotaFetcher::SEVEN_DAYS_MS)
        );
    }
}
