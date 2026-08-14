use super::CredentialStatus;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexCredentials {
    #[serde(default)]
    pub tokens: Option<CodexTokens>,
    #[serde(default, rename = "OPENAI_API_KEY")]
    pub openai_api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexTokens {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CodexAuthCandidate {
    pub path: PathBuf,
    pub credentials: CodexCredentials,
}

pub struct CodexAuth {
    credentials_path: PathBuf,
}

impl CodexAuth {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        let path = home.join(".config").join("codex").join("auth.json");
        Self {
            credentials_path: path,
        }
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self {
            credentials_path: path,
        }
    }

    pub fn auth_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        if let Ok(codex_home) = std::env::var("CODEX_HOME") {
            let codex_home = codex_home.trim();
            if !codex_home.is_empty() {
                paths.push(PathBuf::from(codex_home).join("auth.json"));
            }
        }
        paths.push(self.credentials_path.clone());
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        let alt_path = home.join(".codex").join("auth.json");
        if !paths.contains(&alt_path) {
            paths.push(alt_path);
        }
        paths
    }

    pub fn load_auth_candidate(&self) -> Result<CodexAuthCandidate> {
        debug!("Loading Codex credentials candidate");

        for path in self.auth_paths() {
            if path.exists() {
                match fs::read_to_string(&path) {
                    Ok(content) => match serde_json::from_str::<CodexCredentials>(&content) {
                        Ok(creds) => {
                            if creds.tokens.is_some() || creds.openai_api_key.is_some() {
                                return Ok(CodexAuthCandidate {
                                    path,
                                    credentials: creds,
                                });
                            }
                        }
                        Err(e) => {
                            debug!("Codex credentials JSON malformed at {:?}: {}", path, e);
                        }
                    },
                    Err(e) => {
                        debug!("Failed to read Codex credentials at {:?}: {}", path, e);
                    }
                }
            }
        }

        Err(anyhow!("Codex credentials not found"))
    }

    pub fn load_credentials(&self) -> Result<CodexCredentials> {
        self.load_auth_candidate().map(|c| c.credentials)
    }

    pub fn is_token_expired(&self, tokens: &CodexTokens) -> bool {
        if let Some(exp) = crate::auth::decode_jwt_exp(&tokens.access_token) {
            let now = chrono::Utc::now().timestamp();
            // 5 minutes (300 seconds) refresh buffer, matching openusage
            return exp <= now + 300;
        }
        false
    }

    pub async fn refresh_tokens(
        &self,
        candidate: &CodexAuthCandidate,
        client: &reqwest::Client,
    ) -> Result<CodexAuthCandidate> {
        let tokens = candidate
            .credentials
            .tokens
            .as_ref()
            .ok_or_else(|| anyhow!("No tokens found in Codex credentials for refresh"))?;

        let refresh_token = tokens.refresh_token.trim();
        if refresh_token.is_empty() {
            return Err(anyhow!("Codex refresh token is empty"));
        }

        const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
        const CODEX_REFRESH_URL: &str = "https://auth.openai.com/oauth/token";

        #[derive(Deserialize)]
        struct RefreshResponse {
            access_token: String,
            #[serde(default)]
            refresh_token: Option<String>,
            #[serde(default)]
            id_token: Option<String>,
        }

        let params = [
            ("grant_type", "refresh_token"),
            ("client_id", CODEX_CLIENT_ID),
            ("refresh_token", refresh_token),
        ];

        debug!("Refreshing Codex OAuth token at {}", CODEX_REFRESH_URL);
        let res = client
            .post(CODEX_REFRESH_URL)
            .form(&params)
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| anyhow!("Failed to send Codex refresh request: {}", e))?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Codex token refresh failed with HTTP {}: {}",
                status,
                body
            ));
        }

        let refresh_resp: RefreshResponse = res
            .json()
            .await
            .map_err(|e| anyhow!("Failed to parse Codex token refresh response: {}", e))?;

        if refresh_resp.access_token.trim().is_empty() {
            return Err(anyhow!("Received empty access token from Codex refresh"));
        }

        let mut updated = candidate.clone();
        if let Some(ref mut creds_tokens) = updated.credentials.tokens {
            creds_tokens.access_token = refresh_resp.access_token;
            if let Some(new_rt) = refresh_resp.refresh_token {
                if !new_rt.trim().is_empty() {
                    creds_tokens.refresh_token = new_rt;
                }
            }
            if let Some(new_id) = refresh_resp.id_token {
                if !new_id.trim().is_empty() {
                    creds_tokens.id_token = Some(new_id);
                }
            }
        }

        // Persist updated credentials back to candidate.path
        match serde_json::to_string_pretty(&updated.credentials) {
            Ok(json) => {
                if let Some(parent) = updated.path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if let Err(e) = fs::write(&updated.path, json) {
                    tracing::warn!(
                        "Failed to write refreshed Codex credentials to {:?}: {}",
                        updated.path,
                        e
                    );
                } else {
                    debug!(
                        "Successfully persisted refreshed Codex credentials to {:?}",
                        updated.path
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Failed to serialize refreshed Codex credentials: {}", e);
            }
        }

        Ok(updated)
    }

    pub fn load_email(&self) -> Option<String> {
        if let Ok(creds) = self.load_credentials() {
            if let Some(tokens) = creds.tokens {
                if let Some(id_token) = tokens.id_token {
                    if let Some(email) = crate::auth::decode_jwt_email(&id_token) {
                        return Some(email);
                    }
                }
                // Fallback to access_token JWT
                if let Some(email) = crate::auth::decode_jwt_email(&tokens.access_token) {
                    return Some(email);
                }
            }
        }
        None
    }

    pub fn detect() -> bool {
        let auth = Self::new();
        auth.auth_paths().iter().any(|p| p.exists())
    }

    pub fn credential_status(&self) -> CredentialStatus {
        match self.load_credentials() {
            Ok(_) => CredentialStatus::Valid,
            Err(_) => CredentialStatus::NotFound,
        }
    }
}

impl Default for CodexAuth {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_codex_auth_multi_path_fallback() {
        let temp_dir = tempdir().unwrap();
        let invalid_path = temp_dir.path().join("invalid_auth.json");
        let valid_path = temp_dir.path().join("valid_auth.json");

        // Write malformed JSON to first path
        fs::write(&invalid_path, "{ malformed json").unwrap();

        // Write valid credentials to second path
        let valid_creds = CodexCredentials {
            tokens: Some(CodexTokens {
                access_token: "access-123".to_string(),
                refresh_token: "refresh-123".to_string(),
                id_token: None,
                account_id: None,
            }),
            openai_api_key: None,
        };
        fs::write(
            &valid_path,
            serde_json::to_string_pretty(&valid_creds).unwrap(),
        )
        .unwrap();

        let mut auth = CodexAuth::with_path(invalid_path);
        // Fallback should find the second file when auth_paths contains it
        auth.credentials_path = valid_path.clone();
        let loaded = auth.load_credentials().unwrap();
        assert_eq!(
            loaded.tokens.unwrap().access_token,
            "access-123".to_string()
        );
    }

    fn make_test_jwt(exp: i64) -> String {
        use base64::Engine;
        let payload = format!(r#"{{"exp":{},"email":"test@example.com"}}"#, exp);
        let b64 = base64::engine::general_purpose::STANDARD.encode(payload.as_bytes());
        format!("header.{}.signature", b64)
    }

    #[test]
    fn test_codex_is_token_expired() {
        let auth = CodexAuth::new();
        let now = chrono::Utc::now().timestamp();

        // Expired token (exp in past)
        let expired_token = make_test_jwt(now - 100);
        let tokens = CodexTokens {
            access_token: expired_token,
            refresh_token: "rt".to_string(),
            id_token: None,
            account_id: None,
        };
        assert!(auth.is_token_expired(&tokens));

        // Near-expiry token (exp in 200s, within 300s buffer)
        let near_expiry_token = make_test_jwt(now + 200);
        let tokens_near = CodexTokens {
            access_token: near_expiry_token,
            refresh_token: "rt".to_string(),
            id_token: None,
            account_id: None,
        };
        assert!(auth.is_token_expired(&tokens_near));

        // Fresh token (exp in 10000s)
        let fresh_token = make_test_jwt(now + 10000);
        let tokens_fresh = CodexTokens {
            access_token: fresh_token,
            refresh_token: "rt".to_string(),
            id_token: None,
            account_id: None,
        };
        assert!(!auth.is_token_expired(&tokens_fresh));
    }
}
