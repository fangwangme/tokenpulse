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

    pub fn load_credentials(&self) -> Result<CodexCredentials> {
        debug!("Loading Codex credentials from {:?}", self.credentials_path);

        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        let paths = vec![
            self.credentials_path.clone(),
            home.join(".codex").join("auth.json"),
        ];

        for path in paths {
            if path.exists() {
                let content = fs::read_to_string(&path)?;
                let creds: CodexCredentials = serde_json::from_str(&content)?;
                if creds.tokens.is_some() || creds.openai_api_key.is_some() {
                    return Ok(creds);
                }
            }
        }

        Err(anyhow!("Codex credentials not found"))
    }

    pub fn detect() -> bool {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        let paths = vec![
            home.join(".config").join("codex").join("auth.json"),
            home.join(".codex").join("auth.json"),
        ];
        paths.iter().any(|p| p.exists())
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
