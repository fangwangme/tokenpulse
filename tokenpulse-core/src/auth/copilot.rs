use super::CredentialStatus;
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::process::Command;
use tracing::debug;

pub struct CopilotAuth {
    hosts_path: PathBuf,
    apps_path: PathBuf,
}

impl CopilotAuth {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        let config_dir = home.join(".config").join("github-copilot");
        Self {
            hosts_path: config_dir.join("hosts.json"),
            apps_path: config_dir.join("apps.json"),
        }
    }

    pub fn load_token(&self) -> Result<String> {
        debug!("Loading GitHub Copilot token");

        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            if !token.is_empty() {
                debug!("Found GITHUB_TOKEN env var");
                return Ok(token);
            }
        }

        if let Some(token) = Self::load_from_gh_cli() {
            debug!("Found token via gh CLI");
            return Ok(token);
        }

        if let Some(token) = self.load_from_config_file() {
            debug!("Found token in config file");
            return Ok(token);
        }

        Err(anyhow!("GitHub Copilot credentials not found"))
    }

    fn load_from_gh_cli() -> Option<String> {
        let output = Command::new("gh").args(["auth", "token"]).output().ok()?;

        if !output.status.success() {
            return None;
        }

        let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
        if token.is_empty() {
            return None;
        }
        Some(token)
    }

    fn load_from_config_file(&self) -> Option<String> {
        for path in [&self.hosts_path, &self.apps_path] {
            if !path.exists() {
                continue;
            }
            debug!("Reading config file: {:?}", path);
            let content = std::fs::read_to_string(path).ok()?;
            let json: serde_json::Value = serde_json::from_str(&content).ok()?;

            // Format: {"github.com": {"oauth_token": "gho_xxx", ...}}
            if let Some(entry) = json.get("github.com") {
                if let Some(token) = entry.get("oauth_token").and_then(|v| v.as_str()) {
                    if !token.is_empty() {
                        return Some(token.to_string());
                    }
                }
            }
        }
        None
    }

    pub fn detect() -> bool {
        if std::env::var("GITHUB_TOKEN").map_or(false, |t| !t.is_empty()) {
            return true;
        }

        if Self::load_from_gh_cli().is_some() {
            return true;
        }

        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        let config_dir = home.join(".config").join("github-copilot");
        config_dir.join("hosts.json").exists() || config_dir.join("apps.json").exists()
    }

    pub fn credential_status(&self) -> CredentialStatus {
        match self.load_token() {
            Ok(_) => CredentialStatus::Valid,
            Err(_) => CredentialStatus::NotFound,
        }
    }

    pub fn credential_hint() -> String {
        if Self::detect() {
            if std::env::var("GITHUB_TOKEN").map_or(false, |t| !t.is_empty()) {
                "GITHUB_TOKEN env var".to_string()
            } else {
                "gh CLI or hosts.json".to_string()
            }
        } else {
            "not detected (run `gh auth login` or set GITHUB_TOKEN)".to_string()
        }
    }
}

impl Default for CopilotAuth {
    fn default() -> Self {
        Self::new()
    }
}
