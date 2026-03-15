use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexCredentials {
    pub tokens: CodexTokens,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodexTokens {
    pub access_token: String,
    pub refresh_token: String,
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
                return Ok(creds);
            }
        }

        Err(anyhow!("Codex credentials not found"))
    }

    pub fn save_credentials(&self, creds: &CodexCredentials) -> Result<()> {
        let content = serde_json::to_string_pretty(creds)?;
        fs::write(&self.credentials_path, content)?;
        Ok(())
    }
}

impl Default for CodexAuth {
    fn default() -> Self {
        Self::new()
    }
}
