use super::CredentialStatus;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info};

const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCredentials {
    #[serde(rename = "claudeAiOauth")]
    pub claude_ai_oauth: ClaudeOAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeOAuth {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
    #[serde(rename = "expiresAt", default)]
    pub expires_at: f64,
    #[serde(rename = "subscriptionType", default)]
    pub subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier", default)]
    pub rate_limit_tier: Option<String>,
}

pub struct ClaudeAuth {
    credentials_path: PathBuf,
}

impl ClaudeAuth {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        Self {
            credentials_path: home.join(".claude").join(".credentials.json"),
        }
    }

    pub fn save_credentials(&self, creds: &ClaudeCredentials) -> Result<()> {
        let json_data = serde_json::to_string(creds)?;

        if let Some(parent) = self.credentials_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(&self.credentials_path, &json_data)?;
        info!("Claude credentials saved to file");

        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = Command::new("security")
                .args(["find-generic-password", "-s", KEYCHAIN_SERVICE])
                .output()
            {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let mut account = None;
                    for line in stdout.lines() {
                        let trimmed = line.trim();
                        if trimmed.starts_with("\"acct\"<blob>=") {
                            if let Some(start) = trimmed.find('"') {
                                if let Some(end) = trimmed.rfind('"') {
                                    if start < end {
                                        account = Some(trimmed[start + 1..end].to_string());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    if let Some(account) = account {
                        let status = Command::new("security")
                            .args([
                                "add-generic-password",
                                "-s",
                                KEYCHAIN_SERVICE,
                                "-a",
                                &account,
                                "-w",
                                &json_data,
                                "-U",
                            ])
                            .status();
                        match status {
                            Ok(stat) if stat.success() => {
                                info!("Claude credentials updated in keychain");
                            }
                            other => {
                                debug!("Failed to update keychain: {:?}", other);
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub fn refresh_token(&self, creds: &mut ClaudeCredentials) -> Result<String> {
        info!("Refreshing Claude OAuth token");

        let client_id = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

        #[derive(Serialize)]
        struct RefreshRequest<'a> {
            grant_type: &'static str,
            refresh_token: &'a str,
            client_id: &'static str,
        }

        #[derive(Deserialize)]
        struct RefreshResponse {
            access_token: String,
            refresh_token: Option<String>,
            expires_in: Option<i64>,
        }

        let req_body = RefreshRequest {
            grant_type: "refresh_token",
            refresh_token: &creds.claude_ai_oauth.refresh_token,
            client_id,
        };

        let response: RefreshResponse = ureq::post("https://platform.claude.com/v1/oauth/token")
            .send_json(&req_body)
            .map_err(|e| anyhow!("Failed to refresh token request: {}", e))?
            .into_json()
            .map_err(|e| anyhow!("Failed to parse refresh token response: {}", e))?;

        if response.access_token.is_empty() {
            return Err(anyhow!("Received empty access token during refresh"));
        }

        creds.claude_ai_oauth.access_token = response.access_token.clone();

        if let Some(new_refresh) = response.refresh_token {
            if !new_refresh.is_empty() {
                creds.claude_ai_oauth.refresh_token = new_refresh;
            }
        }

        if let Some(expires_in) = response.expires_in {
            let now_ms = chrono::Utc::now().timestamp_millis();
            creds.claude_ai_oauth.expires_at = (now_ms + expires_in * 1000) as f64;
        }

        self.save_credentials(creds)?;

        Ok(creds.claude_ai_oauth.access_token.clone())
    }

    pub fn load_credentials(&self) -> Result<ClaudeCredentials> {
        debug!("Loading Claude credentials");

        if self.credentials_path.exists() {
            if let Ok(content) = fs::read_to_string(&self.credentials_path) {
                if let Ok(creds) = serde_json::from_str::<ClaudeCredentials>(&content) {
                    if !creds.claude_ai_oauth.access_token.is_empty() {
                        info!("Claude credentials loaded from file");
                        return Ok(creds);
                    }
                }
            }
        }

        self.load_from_keychain()
    }

    #[cfg(target_os = "macos")]
    fn load_from_keychain(&self) -> Result<ClaudeCredentials> {
        debug!("Trying to load Claude credentials from keychain");

        let output = Command::new("security")
            .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "Claude credentials not found in keychain: {}",
                stderr.trim()
            ));
        }

        let keychain_data = String::from_utf8_lossy(&output.stdout);
        let keychain_data = keychain_data.trim();

        let creds: ClaudeCredentials = serde_json::from_str(keychain_data)
            .map_err(|e| anyhow!("Failed to parse keychain JSON: {}", e))?;

        if creds.claude_ai_oauth.access_token.is_empty() {
            return Err(anyhow!("No access token in keychain credentials"));
        }

        info!("Claude credentials loaded from keychain");
        Ok(creds)
    }

    #[cfg(not(target_os = "macos"))]
    fn load_from_keychain(&self) -> Result<ClaudeCredentials> {
        Err(anyhow!("Keychain not supported on this platform"))
    }

    pub fn is_token_expired(&self, creds: &ClaudeCredentials) -> bool {
        if creds.claude_ai_oauth.expires_at == 0.0 {
            return false;
        }
        let now_ms = chrono::Utc::now().timestamp_millis();
        let buffer_ms = 300_000;
        creds.claude_ai_oauth.expires_at <= (now_ms + buffer_ms) as f64
    }

    pub fn detect() -> bool {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        let path = home.join(".claude").join(".credentials.json");
        if path.exists() {
            return true;
        }
        // Check keychain on macOS
        #[cfg(target_os = "macos")]
        {
            if let Ok(output) = Command::new("security")
                .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
                .output()
            {
                if output.status.success() {
                    return true;
                }
            }
        }
        false
    }

    pub fn credential_status(&self) -> CredentialStatus {
        match self.load_credentials() {
            Ok(creds) => {
                if self.is_token_expired(&creds) {
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
