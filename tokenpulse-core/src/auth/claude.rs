use super::CredentialStatus;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::Arc;
use tracing::{debug, info};

#[cfg(any(target_os = "macos", test))]
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
#[cfg(any(target_os = "macos", test))]
const SECURITY_BIN: &str = "/usr/bin/security";
#[cfg(target_os = "macos")]
const ID_BIN: &str = "/usr/bin/id";
const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_CODE_SCOPES: &str = "user:profile user:inference";

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeCredentials {
    #[serde(rename = "claudeAiOauth")]
    pub claude_ai_oauth: ClaudeOAuth,
}

impl fmt::Debug for ClaudeCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeCredentials")
            .field("claude_ai_oauth", &self.claude_ai_oauth)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct ClaudeOAuth {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "refreshToken", default)]
    pub refresh_token: String,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: f64,
    #[serde(rename = "subscriptionType", default)]
    pub subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier", default)]
    pub rate_limit_tier: Option<String>,
}

impl fmt::Debug for ClaudeOAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeOAuth")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("subscription_type", &self.subscription_type)
            .field("rate_limit_tier", &self.rate_limit_tier)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ClaudeCredentialSource {
    CurrentUserKeychain { account: String },
    LegacyKeychain,
    File,
}

impl fmt::Debug for ClaudeCredentialSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CurrentUserKeychain { .. } => "CurrentUserKeychain",
            Self::LegacyKeychain => "LegacyKeychain",
            Self::File => "File",
        })
    }
}

impl fmt::Display for ClaudeCredentialSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentUserKeychain { .. } => f.write_str("current-user keychain"),
            Self::LegacyKeychain => f.write_str("legacy keychain"),
            Self::File => f.write_str("credentials file"),
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct ClaudeCredentialCandidate {
    pub(crate) source: ClaudeCredentialSource,
    pub(crate) credentials: ClaudeCredentials,
}

impl fmt::Debug for ClaudeCredentialCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaudeCredentialCandidate")
            .field("source", &self.source)
            .field("credentials", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeRefreshFailureKind {
    Credential,
    Other,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct ClaudeRefreshError {
    pub(crate) kind: ClaudeRefreshFailureKind,
    message: String,
}

impl ClaudeRefreshError {
    pub(crate) fn credential(message: impl Into<String>) -> Self {
        Self {
            kind: ClaudeRefreshFailureKind::Credential,
            message: message.into(),
        }
    }

    fn other(message: impl Into<String>) -> Self {
        Self {
            kind: ClaudeRefreshFailureKind::Other,
            message: message.into(),
        }
    }
}

pub(crate) enum ClaudeRefreshOutcome {
    Updated(ClaudeCredentialCandidate),
    Reloaded(Option<ClaudeCredentialCandidate>),
}

pub(crate) struct RotatedClaudeTokens {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_in: Option<i64>,
}

impl fmt::Debug for RotatedClaudeTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RotatedClaudeTokens")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

pub(crate) trait ClaudeCredentialStore: Send + Sync {
    fn load_candidates(&self) -> Result<Vec<ClaudeCredentialCandidate>>;
    fn save_source(
        &self,
        source: &ClaudeCredentialSource,
        credentials: &ClaudeCredentials,
    ) -> Result<()>;
}

pub(crate) trait ClaudeTokenRefresher: Send + Sync {
    fn refresh(
        &self,
        credentials: &ClaudeCredentials,
    ) -> Result<RotatedClaudeTokens, ClaudeRefreshError>;
}

struct SystemClaudeCredentialStore {
    credentials_path: PathBuf,
}

#[cfg(any(target_os = "macos", test))]
fn keychain_account(source: &ClaudeCredentialSource) -> Option<Option<&str>> {
    match source {
        ClaudeCredentialSource::CurrentUserKeychain { account } => Some(Some(account)),
        ClaudeCredentialSource::LegacyKeychain => Some(None),
        ClaudeCredentialSource::File => None,
    }
}

#[cfg(any(target_os = "macos", test))]
fn keychain_read_args(account: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "find-generic-password".to_string(),
        "-s".to_string(),
        KEYCHAIN_SERVICE.to_string(),
    ];
    if let Some(account) = account {
        args.extend(["-a".to_string(), account.to_string()]);
    }
    args.push("-w".to_string());
    args
}

#[cfg(any(target_os = "macos", test))]
fn keychain_write_args(account: Option<&str>, json: &str) -> Vec<String> {
    let mut args = vec![
        "add-generic-password".to_string(),
        "-s".to_string(),
        KEYCHAIN_SERVICE.to_string(),
    ];
    if let Some(account) = account {
        args.extend(["-a".to_string(), account.to_string()]);
    }
    args.extend(["-U".to_string(), "-w".to_string(), json.to_string()]);
    args
}

impl SystemClaudeCredentialStore {
    fn new(credentials_path: PathBuf) -> Self {
        Self { credentials_path }
    }

    fn load_file(&self) -> Option<ClaudeCredentials> {
        let content = fs::read_to_string(&self.credentials_path).ok()?;
        parse_credentials(&content, "credentials file")
    }

    #[cfg(target_os = "macos")]
    fn current_user_account() -> Option<String> {
        let output = Command::new(ID_BIN).arg("-un").output().ok()?;
        if !output.status.success() {
            return None;
        }
        let account = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!account.is_empty()).then_some(account)
    }

    #[cfg(target_os = "macos")]
    fn load_keychain(&self, account: Option<&str>) -> Option<ClaudeCredentials> {
        let output = Command::new(SECURITY_BIN)
            .args(keychain_read_args(account))
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let content = String::from_utf8_lossy(&output.stdout);
        parse_credentials(content.trim(), "keychain")
    }

    #[cfg(target_os = "macos")]
    fn save_keychain(&self, account: Option<&str>, credentials: &ClaudeCredentials) -> Result<()> {
        let json = serde_json::to_string(credentials)?;
        let status = Command::new(SECURITY_BIN)
            .args(keychain_write_args(account, &json))
            .status()?;
        if !status.success() {
            return Err(anyhow!("Failed to update Claude credentials in keychain"));
        }
        Ok(())
    }
}

impl ClaudeCredentialStore for SystemClaudeCredentialStore {
    fn load_candidates(&self) -> Result<Vec<ClaudeCredentialCandidate>> {
        #[cfg(target_os = "macos")]
        {
            let account = Self::current_user_account();
            let current_user = account.as_deref().and_then(|name| {
                self.load_keychain(Some(name))
                    .map(|credentials| ClaudeCredentialCandidate {
                        source: ClaudeCredentialSource::CurrentUserKeychain {
                            account: name.to_string(),
                        },
                        credentials,
                    })
            });
            let legacy = self
                .load_keychain(None)
                .map(|credentials| ClaudeCredentialCandidate {
                    source: ClaudeCredentialSource::LegacyKeychain,
                    credentials,
                });
            let file = self
                .load_file()
                .map(|credentials| ClaudeCredentialCandidate {
                    source: ClaudeCredentialSource::File,
                    credentials,
                });
            return Ok(ordered_unique_candidates([current_user, legacy, file]));
        }

        #[cfg(not(target_os = "macos"))]
        {
            Ok(self
                .load_file()
                .map(|credentials| ClaudeCredentialCandidate {
                    source: ClaudeCredentialSource::File,
                    credentials,
                })
                .into_iter()
                .collect())
        }
    }

    fn save_source(
        &self,
        source: &ClaudeCredentialSource,
        credentials: &ClaudeCredentials,
    ) -> Result<()> {
        match source {
            ClaudeCredentialSource::File => {
                if let Some(parent) = self.credentials_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&self.credentials_path, serde_json::to_string(credentials)?)?;
                Ok(())
            }
            #[cfg(target_os = "macos")]
            source @ (ClaudeCredentialSource::CurrentUserKeychain { .. }
            | ClaudeCredentialSource::LegacyKeychain) => self.save_keychain(
                keychain_account(source).expect("keychain source"),
                credentials,
            ),
            #[cfg(not(target_os = "macos"))]
            _ => Err(anyhow!("Keychain is not supported on this platform")),
        }
    }
}

struct UreqClaudeTokenRefresher;

impl ClaudeTokenRefresher for UreqClaudeTokenRefresher {
    fn refresh(
        &self,
        credentials: &ClaudeCredentials,
    ) -> Result<RotatedClaudeTokens, ClaudeRefreshError> {
        if credentials.claude_ai_oauth.refresh_token.is_empty() {
            return Err(ClaudeRefreshError::credential(
                "Claude credential has no refresh token",
            ));
        }

        #[derive(Serialize)]
        struct RefreshRequest<'a> {
            grant_type: &'static str,
            refresh_token: &'a str,
            client_id: &'static str,
            scope: &'static str,
        }

        #[derive(Deserialize)]
        struct RefreshResponse {
            access_token: String,
            refresh_token: Option<String>,
            expires_in: Option<i64>,
        }

        let response = ureq::post(CLAUDE_TOKEN_URL)
            .send_json(RefreshRequest {
                grant_type: "refresh_token",
                refresh_token: &credentials.claude_ai_oauth.refresh_token,
                client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
                scope: CLAUDE_CODE_SCOPES,
            })
            .map_err(classify_refresh_transport_error)?;

        let response: RefreshResponse = response.into_json().map_err(|_| {
            ClaudeRefreshError::other("Failed to parse Claude token refresh response")
        })?;
        if response.access_token.is_empty() {
            return Err(ClaudeRefreshError::other(
                "Claude token refresh returned an empty access token",
            ));
        }
        Ok(RotatedClaudeTokens {
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            expires_in: response.expires_in,
        })
    }
}

fn classify_refresh_transport_error(error: ureq::Error) -> ClaudeRefreshError {
    match error {
        ureq::Error::Status(status, response) => {
            let invalid_grant = response
                .into_json::<serde_json::Value>()
                .ok()
                .map(|body| response_is_invalid_grant(&body))
                .unwrap_or(false);
            if invalid_grant || matches!(status, 401 | 403) {
                ClaudeRefreshError::credential(format!(
                    "Claude token refresh rejected the credential (HTTP {status})"
                ))
            } else {
                ClaudeRefreshError::other(format!("Claude token refresh failed with HTTP {status}"))
            }
        }
        ureq::Error::Transport(_) => {
            ClaudeRefreshError::other("Claude token refresh request failed")
        }
    }
}

fn response_is_invalid_grant(body: &serde_json::Value) -> bool {
    let error = body.get("error");
    error.and_then(serde_json::Value::as_str) == Some("invalid_grant")
        || error
            .and_then(|value| value.get("type"))
            .and_then(serde_json::Value::as_str)
            == Some("invalid_grant")
        || body.get("type").and_then(serde_json::Value::as_str) == Some("invalid_grant")
        || body
            .get("error_description")
            .and_then(serde_json::Value::as_str)
            .map(|description| description.contains("invalid_grant"))
            .unwrap_or(false)
}

fn parse_credentials(content: &str, source: &str) -> Option<ClaudeCredentials> {
    match serde_json::from_str::<ClaudeCredentials>(content) {
        Ok(credentials) if !credentials.claude_ai_oauth.access_token.is_empty() => {
            Some(credentials)
        }
        Ok(_) => {
            debug!("Claude {source} credential has no access token");
            None
        }
        Err(_) => {
            debug!("Claude {source} credential is not valid JSON");
            None
        }
    }
}

fn ordered_unique_candidates<const N: usize>(
    candidates: [Option<ClaudeCredentialCandidate>; N],
) -> Vec<ClaudeCredentialCandidate> {
    let mut ordered = Vec::new();
    for candidate in candidates.into_iter().flatten() {
        if !ordered.iter().any(|existing: &ClaudeCredentialCandidate| {
            existing.credentials == candidate.credentials
        }) {
            ordered.push(candidate);
        }
    }
    ordered
}

pub struct ClaudeAuth {
    store: Arc<dyn ClaudeCredentialStore>,
    token_refresher: Arc<dyn ClaudeTokenRefresher>,
}

impl ClaudeAuth {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        Self {
            store: Arc::new(SystemClaudeCredentialStore::new(
                home.join(".claude").join(".credentials.json"),
            )),
            token_refresher: Arc::new(UreqClaudeTokenRefresher),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_components(
        store: Arc<dyn ClaudeCredentialStore>,
        token_refresher: Arc<dyn ClaudeTokenRefresher>,
    ) -> Self {
        Self {
            store,
            token_refresher,
        }
    }

    pub(crate) fn load_credentials_candidates(&self) -> Result<Vec<ClaudeCredentialCandidate>> {
        let candidates = self.store.load_candidates()?;
        for candidate in &candidates {
            debug!(
                "Found Claude credential candidate from {}",
                candidate.source
            );
        }
        Ok(candidates)
    }

    pub(crate) fn refresh_candidate(
        &self,
        candidate: &ClaudeCredentialCandidate,
        expected_candidates: &[ClaudeCredentialCandidate],
    ) -> Result<ClaudeRefreshOutcome, ClaudeRefreshError> {
        info!("Refreshing Claude OAuth token from {}", candidate.source);
        let rotated = self.token_refresher.refresh(&candidate.credentials)?;
        let mut credentials = candidate.credentials.clone();
        credentials.claude_ai_oauth.access_token = rotated.access_token;
        if let Some(refresh_token) = rotated.refresh_token.filter(|token| !token.is_empty()) {
            credentials.claude_ai_oauth.refresh_token = refresh_token;
        }
        if let Some(expires_in) = rotated.expires_in {
            credentials.claude_ai_oauth.expires_at =
                (chrono::Utc::now().timestamp_millis() + expires_in * 1000) as f64;
        }

        let latest_candidates = self
            .store
            .load_candidates()
            .map_err(|_| ClaudeRefreshError::other("Failed to re-read Claude credentials"))?;
        if latest_candidates != expected_candidates {
            info!(
                "Claude credential changed during refresh; discarding rotation from {}",
                candidate.source
            );
            let replacement = latest_candidates.into_iter().next();
            return Ok(ClaudeRefreshOutcome::Reloaded(replacement));
        }

        self.store
            .save_source(&candidate.source, &credentials)
            .map_err(|_| ClaudeRefreshError::other("Failed to save rotated Claude credential"))?;
        info!("Claude credential rotation saved to {}", candidate.source);
        Ok(ClaudeRefreshOutcome::Updated(ClaudeCredentialCandidate {
            source: candidate.source.clone(),
            credentials,
        }))
    }

    pub(crate) fn is_token_expired(&self, credentials: &ClaudeCredentials) -> bool {
        if credentials.claude_ai_oauth.expires_at == 0.0 {
            return false;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        credentials.claude_ai_oauth.expires_at <= (now_ms + 300_000) as f64
    }

    pub fn detect() -> bool {
        Self::new()
            .load_credentials_candidates()
            .map(|candidates| !candidates.is_empty())
            .unwrap_or(false)
    }

    pub fn credential_hint(&self) -> Option<String> {
        self.load_credentials_candidates()
            .ok()?
            .first()
            .map(|candidate| candidate.source.to_string())
    }

    pub fn credential_status(&self) -> CredentialStatus {
        match self.load_credentials_candidates() {
            Ok(candidates) if candidates.is_empty() => CredentialStatus::NotFound,
            Ok(candidates) => {
                if candidates
                    .iter()
                    .all(|candidate| self.is_token_expired(&candidate.credentials))
                {
                    CredentialStatus::Expired
                } else {
                    CredentialStatus::Valid
                }
            }
            Err(_) => CredentialStatus::NotFound,
        }
    }
}

impl Default for ClaudeAuth {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn credentials(access: &str, refresh: &str) -> ClaudeCredentials {
        ClaudeCredentials {
            claude_ai_oauth: ClaudeOAuth {
                access_token: access.to_string(),
                refresh_token: refresh.to_string(),
                expires_at: 0.0,
                subscription_type: None,
                rate_limit_tier: None,
            },
        }
    }

    #[test]
    fn current_user_keychain_wins_over_legacy_and_file_candidates() {
        let candidates = ordered_unique_candidates([
            Some(ClaudeCredentialCandidate {
                source: ClaudeCredentialSource::CurrentUserKeychain {
                    account: "alice".to_string(),
                },
                credentials: credentials("current", "current-refresh"),
            }),
            Some(ClaudeCredentialCandidate {
                source: ClaudeCredentialSource::LegacyKeychain,
                credentials: credentials("legacy", "legacy-refresh"),
            }),
            Some(ClaudeCredentialCandidate {
                source: ClaudeCredentialSource::File,
                credentials: credentials("stale-file", "file-refresh"),
            }),
        ]);

        assert!(matches!(
            candidates[0].source,
            ClaudeCredentialSource::CurrentUserKeychain { .. }
        ));
        assert_eq!(
            candidates[0].credentials.claude_ai_oauth.access_token,
            "current"
        );
        assert!(matches!(
            candidates[1].source,
            ClaudeCredentialSource::LegacyKeychain
        ));
        assert!(matches!(candidates[2].source, ClaudeCredentialSource::File));
    }

    #[test]
    fn system_store_keeps_current_user_and_legacy_keychain_commands_distinct() {
        let current_user = ClaudeCredentialSource::CurrentUserKeychain {
            account: "alice".to_string(),
        };
        let legacy = ClaudeCredentialSource::LegacyKeychain;
        let current_account = keychain_account(&current_user).expect("keychain source");
        let legacy_account = keychain_account(&legacy).expect("keychain source");

        assert_eq!(SECURITY_BIN, "/usr/bin/security");
        assert_eq!(current_account, Some("alice"));
        assert_eq!(legacy_account, None);
        assert_eq!(
            keychain_read_args(current_account),
            [
                "find-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                "alice",
                "-w",
            ]
        );
        assert_eq!(
            keychain_read_args(legacy_account),
            ["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"]
        );
        assert_eq!(
            keychain_write_args(current_account, "credential-json"),
            [
                "add-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                "alice",
                "-U",
                "-w",
                "credential-json",
            ]
        );
        assert_eq!(
            keychain_write_args(legacy_account, "credential-json"),
            [
                "add-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-U",
                "-w",
                "credential-json",
            ]
        );
    }

    struct MockStore {
        candidates: Mutex<Vec<ClaudeCredentialCandidate>>,
        saved: Mutex<Vec<(ClaudeCredentialSource, ClaudeCredentials)>>,
    }

    impl ClaudeCredentialStore for MockStore {
        fn load_candidates(&self) -> Result<Vec<ClaudeCredentialCandidate>> {
            Ok(self.candidates.lock().unwrap().clone())
        }

        fn save_source(
            &self,
            source: &ClaudeCredentialSource,
            credentials: &ClaudeCredentials,
        ) -> Result<()> {
            self.saved
                .lock()
                .unwrap()
                .push((source.clone(), credentials.clone()));
            Ok(())
        }
    }

    struct SuccessfulRefresher;

    impl ClaudeTokenRefresher for SuccessfulRefresher {
        fn refresh(
            &self,
            _credentials: &ClaudeCredentials,
        ) -> Result<RotatedClaudeTokens, ClaudeRefreshError> {
            Ok(RotatedClaudeTokens {
                access_token: "rotated-access".to_string(),
                refresh_token: Some("rotated-refresh".to_string()),
                expires_in: Some(3600),
            })
        }
    }

    #[test]
    fn rotation_writes_only_to_the_candidate_source() {
        let candidate = ClaudeCredentialCandidate {
            source: ClaudeCredentialSource::LegacyKeychain,
            credentials: credentials("old-access", "old-refresh"),
        };
        let store = Arc::new(MockStore {
            candidates: Mutex::new(vec![candidate.clone()]),
            saved: Mutex::new(Vec::new()),
        });
        let auth = ClaudeAuth::with_components(store.clone(), Arc::new(SuccessfulRefresher));

        let outcome = auth
            .refresh_candidate(&candidate, std::slice::from_ref(&candidate))
            .unwrap();

        assert!(matches!(outcome, ClaudeRefreshOutcome::Updated(_)));
        let saved = store.saved.lock().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].0, ClaudeCredentialSource::LegacyKeychain);
        assert_eq!(saved[0].1.claude_ai_oauth.access_token, "rotated-access");
    }

    #[test]
    fn concurrent_relogin_discards_in_flight_rotation() {
        let old_candidate = ClaudeCredentialCandidate {
            source: ClaudeCredentialSource::CurrentUserKeychain {
                account: "alice".to_string(),
            },
            credentials: credentials("old-access", "old-refresh"),
        };
        let new_candidate = ClaudeCredentialCandidate {
            source: old_candidate.source.clone(),
            credentials: credentials("new-login-access", "new-login-refresh"),
        };
        let store = Arc::new(MockStore {
            candidates: Mutex::new(vec![new_candidate.clone()]),
            saved: Mutex::new(Vec::new()),
        });
        let auth = ClaudeAuth::with_components(store.clone(), Arc::new(SuccessfulRefresher));

        let outcome = auth
            .refresh_candidate(&old_candidate, std::slice::from_ref(&old_candidate))
            .unwrap();

        match outcome {
            ClaudeRefreshOutcome::Reloaded(Some(candidate)) => {
                assert_eq!(candidate.credentials, new_candidate.credentials);
            }
            _ => panic!("expected reloaded concurrent login"),
        }
        assert!(store.saved.lock().unwrap().is_empty());
    }

    #[test]
    fn credential_debug_output_redacts_secret_values() {
        let candidate = ClaudeCredentialCandidate {
            source: ClaudeCredentialSource::File,
            credentials: credentials("secret-access", "secret-refresh"),
        };
        let rotation = RotatedClaudeTokens {
            access_token: "rotated-secret-access".to_string(),
            refresh_token: Some("rotated-secret-refresh".to_string()),
            expires_in: Some(3600),
        };
        let diagnostics = format!("{candidate:?} {:?} {rotation:?}", candidate.credentials);

        assert!(!diagnostics.contains("secret-access"));
        assert!(!diagnostics.contains("secret-refresh"));
        assert!(!diagnostics.contains("rotated-secret-access"));
        assert!(!diagnostics.contains("rotated-secret-refresh"));
        assert!(diagnostics.contains("[REDACTED]"));
    }

    #[test]
    fn refresh_request_preserves_profile_scope() {
        assert!(CLAUDE_CODE_SCOPES
            .split_whitespace()
            .any(|scope| scope == "user:profile"));
    }

    #[test]
    fn invalid_grant_response_shapes_are_classified_as_credentials() {
        for body in [
            serde_json::json!({"error": "invalid_grant"}),
            serde_json::json!({"error": {"type": "invalid_grant"}}),
            serde_json::json!({"type": "invalid_grant"}),
        ] {
            assert!(response_is_invalid_grant(&body));
        }
    }
}
