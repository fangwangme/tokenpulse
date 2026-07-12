use crate::auth::claude::{
    ClaudeAuth, ClaudeCredentialCandidate, ClaudeRefreshFailureKind, ClaudeRefreshOutcome,
};
use crate::provider::{QuotaFetcher, QuotaSnapshot, RateWindow};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;
use tracing::debug;

const REQUEST_TIMEOUT_SECS: u64 = 20;

const QUOTA_API_URL: &str = "https://api.anthropic.com/api/oauth/usage";

#[derive(Debug, Deserialize)]
struct ClaudeQuotaResponse {
    #[serde(default)]
    five_hour: Option<WindowUsage>,
    #[serde(default)]
    seven_day: Option<WindowUsage>,
    #[serde(default)]
    seven_day_sonnet: Option<WindowUsage>,
    #[serde(default)]
    seven_day_opus: Option<WindowUsage>,
    #[serde(default)]
    limits: Vec<UsageLimit>,
}

#[derive(Debug, Deserialize)]
struct WindowUsage {
    #[serde(default)]
    utilization: f64,
    #[serde(default)]
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageLimit {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    percent: f64,
    #[serde(default)]
    resets_at: Option<String>,
    #[serde(default)]
    scope: Option<UsageLimitScope>,
}

#[derive(Debug, Deserialize)]
struct UsageLimitScope {
    #[serde(default)]
    model: Option<UsageLimitModel>,
}

#[derive(Debug, Deserialize)]
struct UsageLimitModel {
    #[serde(default)]
    display_name: Option<String>,
}

pub struct ClaudeQuotaFetcher {
    client: Client,
    auth: ClaudeAuth,
    quota_api_url: String,
}

impl ClaudeQuotaFetcher {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            auth: ClaudeAuth::new(),
            quota_api_url: QUOTA_API_URL.to_string(),
        }
    }

    #[cfg(test)]
    fn with_auth_and_url(auth: ClaudeAuth, quota_api_url: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
                .build()
                .unwrap(),
            auth,
            quota_api_url,
        }
    }

    fn refresh_candidate_once(
        &self,
        candidate: ClaudeCredentialCandidate,
        expected_candidates: &[ClaudeCredentialCandidate],
        attempted_refresh_tokens: &mut HashSet<String>,
    ) -> Result<ClaudeCredentialCandidate, CandidateFailure> {
        let refresh_token = &candidate.credentials.claude_ai_oauth.refresh_token;
        if refresh_token.is_empty() {
            return Err(CandidateFailure::Credential(
                "Claude credential has no refresh token".to_string(),
            ));
        }
        if !attempted_refresh_tokens.insert(refresh_token.clone()) {
            return Err(CandidateFailure::Credential(
                "Claude refresh token was already attempted".to_string(),
            ));
        }

        match self.auth.refresh_candidate(&candidate, expected_candidates) {
            Ok(ClaudeRefreshOutcome::Updated(updated)) => Ok(updated),
            Ok(ClaudeRefreshOutcome::Reloaded(Some(reloaded))) => Ok(reloaded),
            Ok(ClaudeRefreshOutcome::Reloaded(None)) => Err(CandidateFailure::Credential(
                "Claude credential changed and no replacement was found".to_string(),
            )),
            Err(error) if error.kind == ClaudeRefreshFailureKind::Credential => {
                Err(CandidateFailure::Credential(error.to_string()))
            }
            Err(error) => Err(CandidateFailure::Other(anyhow!(error))),
        }
    }

    async fn fetch_candidate(
        &self,
        mut candidate: ClaudeCredentialCandidate,
        expected_candidates: &[ClaudeCredentialCandidate],
        attempted_refresh_tokens: &mut HashSet<String>,
    ) -> Result<QuotaSnapshot, CandidateFailure> {
        let mut refreshed = false;
        if self.auth.is_token_expired(&candidate.credentials) {
            debug!(
                "Claude credential from {} is expired; attempting one refresh",
                candidate.source
            );
            candidate = self.refresh_candidate_once(
                candidate,
                expected_candidates,
                attempted_refresh_tokens,
            )?;
            refreshed = true;
        }

        let mut response = self
            .client
            .get(&self.quota_api_url)
            .bearer_auth(&candidate.credentials.claude_ai_oauth.access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|error| CandidateFailure::Other(error.into()))?;

        if is_credential_rejection(response.status()) {
            if refreshed {
                return Err(CandidateFailure::Credential(format!(
                    "Claude access token from {} was rejected after refresh",
                    candidate.source
                )));
            }
            debug!(
                "Claude access token from {} was rejected; attempting one refresh",
                candidate.source
            );
            candidate = self.refresh_candidate_once(
                candidate,
                expected_candidates,
                attempted_refresh_tokens,
            )?;
            response = self
                .client
                .get(&self.quota_api_url)
                .bearer_auth(&candidate.credentials.claude_ai_oauth.access_token)
                .header("anthropic-beta", "oauth-2025-04-20")
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|error| CandidateFailure::Other(error.into()))?;
            if is_credential_rejection(response.status()) {
                return Err(CandidateFailure::Credential(format!(
                    "Claude access token from {} was rejected after refresh",
                    candidate.source
                )));
            }
        }

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| CandidateFailure::Other(error.into()))?;
        debug!(
            "Claude quota response status: {}, {} bytes",
            status,
            body.len()
        );
        if !status.is_success() {
            return Err(CandidateFailure::Other(anyhow!(
                "Claude quota API returned HTTP {}",
                status
            )));
        }

        quota_snapshot_from_body(&body).map_err(CandidateFailure::Other)
    }
}

enum CandidateFailure {
    Credential(String),
    Other(anyhow::Error),
}

fn is_credential_rejection(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    )
}

impl Default for ClaudeQuotaFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QuotaFetcher for ClaudeQuotaFetcher {
    fn provider_name(&self) -> &str {
        "claude"
    }

    fn provider_display_name(&self) -> &str {
        "Claude Code"
    }

    async fn fetch_quota(&self) -> Result<QuotaSnapshot> {
        let candidates = self.auth.load_credentials_candidates()?;
        if candidates.is_empty() {
            return Err(anyhow!("Claude credentials not found"));
        }
        let mut attempted_refresh_tokens = HashSet::new();
        let mut last_credential_error = None;

        for candidate in candidates.iter().cloned() {
            debug!("Trying Claude credential from {}", candidate.source);
            match self
                .fetch_candidate(candidate, &candidates, &mut attempted_refresh_tokens)
                .await
            {
                Ok(snapshot) => return Ok(snapshot),
                Err(CandidateFailure::Credential(error)) => {
                    debug!("Claude credential candidate failed: {error}");
                    last_credential_error = Some(error);
                }
                Err(CandidateFailure::Other(error)) => return Err(error),
            }
        }

        Err(anyhow!(
            "Claude credentials were rejected{}",
            last_credential_error
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        ))
    }
}

fn quota_snapshot_from_body(body: &str) -> Result<QuotaSnapshot> {
    let quota: ClaudeQuotaResponse = serde_json::from_str(body)
        .map_err(|error| anyhow!("Failed to parse Claude quota response: {error}"))?;
    let mut windows = Vec::new();

    if let Some(five_hour) = quota.five_hour {
        windows.push(RateWindow {
            label: "Session (5h)".to_string(),
            used_percent: five_hour.utilization,
            resets_at: five_hour.resets_at.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            period_duration_ms: Some(5 * 60 * 60 * 1000),
        });
    }

    if let Some(seven_day) = quota.seven_day {
        windows.push(RateWindow {
            label: "Weekly (7d)".to_string(),
            used_percent: seven_day.utilization,
            resets_at: seven_day.resets_at.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
        });
    }

    if let Some(sonnet) = quota.seven_day_sonnet {
        windows.push(RateWindow {
            label: "Sonnet (7d)".to_string(),
            used_percent: sonnet.utilization,
            resets_at: sonnet.resets_at.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
        });
    }

    if let Some(opus) = quota.seven_day_opus {
        windows.push(RateWindow {
            label: "Opus (7d)".to_string(),
            used_percent: opus.utilization,
            resets_at: opus.resets_at.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            }),
            period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
        });
    }

    for limit in quota.limits {
        let is_fable = limit
            .scope
            .as_ref()
            .and_then(|s| s.model.as_ref())
            .and_then(|m| m.display_name.as_deref())
            .map(|name| name == "Fable")
            .unwrap_or(false);
        let is_weekly = limit.kind.as_deref() == Some("weekly_scoped");
        if is_fable && is_weekly {
            windows.push(RateWindow {
                label: "Fable (7d)".to_string(),
                used_percent: limit.percent,
                resets_at: limit.resets_at.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
                period_duration_ms: Some(7 * 24 * 60 * 60 * 1000),
            });
        }
    }

    Ok(QuotaSnapshot {
        provider: "claude".to_string(),
        plan: Some("Pro".to_string()),
        account: None,
        windows,
        // Claude Code credit usage is intentionally not surfaced.
        credits: None,
        rate_limit_reset_credits: vec![],
        fetched_at: Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::claude::{
        ClaudeCredentialSource, ClaudeCredentialStore, ClaudeCredentials, ClaudeOAuth,
        ClaudeRefreshError, ClaudeTokenRefresher, RotatedClaudeTokens,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn credentials(access: &str, refresh: &str, expires_at: f64) -> ClaudeCredentials {
        ClaudeCredentials {
            claude_ai_oauth: ClaudeOAuth {
                access_token: access.to_string(),
                refresh_token: refresh.to_string(),
                expires_at,
                subscription_type: None,
                rate_limit_tier: None,
            },
        }
    }

    struct CandidateStore {
        candidates: Vec<ClaudeCredentialCandidate>,
    }

    impl ClaudeCredentialStore for CandidateStore {
        fn load_candidates(&self) -> Result<Vec<ClaudeCredentialCandidate>> {
            Ok(self.candidates.clone())
        }

        fn save_source(
            &self,
            _source: &ClaudeCredentialSource,
            _credentials: &ClaudeCredentials,
        ) -> Result<()> {
            panic!("invalid_grant must not save credentials")
        }
    }

    struct InvalidGrantRefresher {
        attempted: Mutex<Vec<String>>,
    }

    impl ClaudeTokenRefresher for InvalidGrantRefresher {
        fn refresh(
            &self,
            credentials: &ClaudeCredentials,
        ) -> Result<RotatedClaudeTokens, ClaudeRefreshError> {
            self.attempted
                .lock()
                .unwrap()
                .push(credentials.claude_ai_oauth.refresh_token.clone());
            Err(ClaudeRefreshError::credential(
                "Claude token refresh rejected the credential (HTTP 400)",
            ))
        }
    }

    fn spawn_quota_server(expected_token: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(request.contains(&format!(
                "authorization: bearer {}",
                expected_token.to_ascii_lowercase()
            )));
            let body = r#"{"five_hour":{"utilization":42.0,"resets_at":null}}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        format!("http://{address}/api/oauth/usage")
    }

    #[test]
    fn claude_quota_response_deserializes_usage_windows() {
        let quota: ClaudeQuotaResponse = serde_json::from_str(
            r#"{
                "five_hour":{"utilization":42.5,"resets_at":"2026-04-10T10:00:00Z"},
                "seven_day":{"utilization":18.0,"resets_at":"2026-04-14T00:00:00Z"}
            }"#,
        )
        .unwrap();

        assert_eq!(quota.five_hour.unwrap().utilization, 42.5);
        assert_eq!(quota.seven_day.unwrap().utilization, 18.0);
    }

    #[test]
    fn claude_quota_response_parses_fable_limit() {
        let quota: ClaudeQuotaResponse = serde_json::from_str(
            r#"{
                "five_hour":null,
                "seven_day":null,
                "seven_day_sonnet":null,
                "seven_day_opus":null,
                "limits":[
                    {"kind":"session","group":"session","percent":24,"resets_at":null,"scope":null},
                    {"kind":"weekly_all","group":"weekly","percent":73,"resets_at":null,"scope":null},
                    {"kind":"weekly_scoped","group":"weekly","percent":76,"resets_at":"2026-07-04T14:00:00Z","scope":{"model":{"id":null,"display_name":"Fable"}}}
                ]
            }"#,
        )
        .unwrap();

        let fable = quota
            .limits
            .into_iter()
            .find(|l| {
                l.kind.as_deref() == Some("weekly_scoped")
                    && l.scope
                        .as_ref()
                        .and_then(|s| s.model.as_ref())
                        .and_then(|m| m.display_name.as_deref())
                        == Some("Fable")
            })
            .expect("fable limit present");

        assert_eq!(fable.percent, 76.0);
        assert_eq!(fable.resets_at.as_deref(), Some("2026-07-04T14:00:00Z"));
    }

    #[tokio::test]
    async fn invalid_grant_falls_through_to_valid_candidate() {
        let store = Arc::new(CandidateStore {
            candidates: vec![
                ClaudeCredentialCandidate {
                    source: ClaudeCredentialSource::CurrentUserKeychain {
                        account: "alice".to_string(),
                    },
                    credentials: credentials("stale-access", "failed-refresh", 1.0),
                },
                ClaudeCredentialCandidate {
                    source: ClaudeCredentialSource::File,
                    credentials: credentials("valid-access", "valid-refresh", 0.0),
                },
            ],
        });
        let refresher = Arc::new(InvalidGrantRefresher {
            attempted: Mutex::new(Vec::new()),
        });
        let auth = ClaudeAuth::with_components(store, refresher.clone());
        let fetcher =
            ClaudeQuotaFetcher::with_auth_and_url(auth, spawn_quota_server("valid-access"));

        let snapshot = fetcher.fetch_quota().await.unwrap();

        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].label, "Session (5h)");
        assert_eq!(
            refresher.attempted.lock().unwrap().as_slice(),
            ["failed-refresh"]
        );
    }

    #[test]
    fn same_failed_refresh_token_is_attempted_only_once() {
        let first = ClaudeCredentialCandidate {
            source: ClaudeCredentialSource::CurrentUserKeychain {
                account: "alice".to_string(),
            },
            credentials: credentials("first-access", "shared-refresh", 1.0),
        };
        let second = ClaudeCredentialCandidate {
            source: ClaudeCredentialSource::File,
            credentials: credentials("second-access", "shared-refresh", 1.0),
        };
        let expected_candidates = vec![first.clone(), second.clone()];
        let store = Arc::new(CandidateStore {
            candidates: expected_candidates.clone(),
        });
        let refresher = Arc::new(InvalidGrantRefresher {
            attempted: Mutex::new(Vec::new()),
        });
        let auth = ClaudeAuth::with_components(store, refresher.clone());
        let fetcher =
            ClaudeQuotaFetcher::with_auth_and_url(auth, "http://127.0.0.1:1/unused".to_string());
        let mut attempted = HashSet::new();

        assert!(matches!(
            fetcher.refresh_candidate_once(first, &expected_candidates, &mut attempted),
            Err(CandidateFailure::Credential(_))
        ));
        assert!(matches!(
            fetcher.refresh_candidate_once(second, &expected_candidates, &mut attempted),
            Err(CandidateFailure::Credential(_))
        ));
        assert_eq!(
            refresher.attempted.lock().unwrap().as_slice(),
            ["shared-refresh"]
        );
    }
}
