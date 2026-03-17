use super::CredentialStatus;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiCredentials {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub id_token: Option<String>,
    pub expiry_date: Option<i64>,
}

pub struct GeminiAuth {
    credentials_path: PathBuf,
    settings_path: PathBuf,
}

impl GeminiAuth {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        Self {
            credentials_path: home.join(".gemini").join("oauth_creds.json"),
            settings_path: home.join(".gemini").join("settings.json"),
        }
    }

    pub fn load_credentials(&self) -> Result<GeminiCredentials> {
        debug!(
            "Loading Gemini credentials from {:?}",
            self.credentials_path
        );

        if !self.credentials_path.exists() {
            return Err(anyhow!(
                "Gemini credentials not found at {:?}",
                self.credentials_path
            ));
        }

        let content = fs::read_to_string(&self.credentials_path)?;
        let creds: GeminiCredentials = serde_json::from_str(&content)?;

        if creds.access_token.is_none() && creds.refresh_token.is_none() {
            return Err(anyhow!("No valid tokens in Gemini credentials"));
        }

        Ok(creds)
    }

    pub fn load_settings(&self) -> Result<GeminiSettings> {
        if !self.settings_path.exists() {
            return Ok(GeminiSettings::default());
        }

        let content = fs::read_to_string(&self.settings_path)?;
        let settings: GeminiSettings = serde_json::from_str(&content).unwrap_or_default();
        Ok(settings)
    }

    pub fn detect() -> bool {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        home.join(".gemini").join("oauth_creds.json").exists()
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

    pub fn is_token_expired(&self, creds: &GeminiCredentials) -> bool {
        match creds.expiry_date {
            Some(expiry) => {
                let now = chrono::Utc::now().timestamp();
                let buffer = 300;
                let expiry_ms = if expiry > 10_000_000_000 {
                    expiry
                } else {
                    expiry * 1000
                };
                expiry_ms / 1000 <= now + buffer
            }
            None => true,
        }
    }
}

impl Default for GeminiAuth {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeminiSettings {
    #[serde(rename = "authType")]
    pub auth_type: Option<String>,
}
