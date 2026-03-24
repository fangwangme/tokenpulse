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
    pub expiry_date: Option<f64>,
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

    pub fn save_credentials(&self, creds: &GeminiCredentials) -> Result<()> {
        let content = serde_json::to_string_pretty(creds)?;
        fs::write(&self.credentials_path, content)?;
        Ok(())
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
                let now = chrono::Utc::now().timestamp() as f64;
                let buffer = 300.0;
                // expiry could be in seconds or milliseconds
                let expiry_secs = if expiry > 10_000_000_000.0 {
                    expiry / 1000.0
                } else {
                    expiry
                };
                expiry_secs <= now + buffer
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
    #[serde(default)]
    pub security: Option<GeminiSecuritySettings>,
}

impl GeminiSettings {
    pub fn selected_auth_type(&self) -> Option<&str> {
        self.auth_type.as_deref().or_else(|| {
            self.security
                .as_ref()
                .and_then(|security| security.auth.as_ref())
                .and_then(|auth| auth.selected_type.as_deref())
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeminiSecuritySettings {
    #[serde(default)]
    pub auth: Option<GeminiSecurityAuthSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeminiSecurityAuthSettings {
    #[serde(default, rename = "selectedType")]
    pub selected_type: Option<String>,
}
