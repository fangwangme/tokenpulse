use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeCredentials {
    pub claude_ai_oauth: ClaudeOAuth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeOAuth {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
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

    pub fn with_path(path: PathBuf) -> Self {
        Self {
            credentials_path: path,
        }
    }

    pub fn load_credentials(&self) -> Result<ClaudeCredentials> {
        debug!(
            "Loading Claude credentials from {:?}",
            self.credentials_path
        );

        if !self.credentials_path.exists() {
            return Err(anyhow!(
                "Claude credentials not found at {:?}",
                self.credentials_path
            ));
        }

        let content = fs::read_to_string(&self.credentials_path)?;
        let creds: ClaudeCredentials = serde_json::from_str(&content)?;

        Ok(creds)
    }

    pub fn is_token_expired(&self, creds: &ClaudeCredentials) -> bool {
        let now = chrono::Utc::now().timestamp();
        let buffer = 300;
        creds.claude_ai_oauth.expires_at <= now + buffer
    }

    pub fn save_credentials(&self, creds: &ClaudeCredentials) -> Result<()> {
        let content = serde_json::to_string_pretty(creds)?;
        fs::write(&self.credentials_path, content)?;
        Ok(())
    }
}

impl Default for ClaudeAuth {
    fn default() -> Self {
        Self::new()
    }
}
