use super::CredentialStatus;
use std::path::PathBuf;

pub struct AntigravityAuth {
    db_path: PathBuf,
}

impl AntigravityAuth {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        Self {
            db_path: home
                .join("Library")
                .join("Application Support")
                .join("Antigravity")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb"),
        }
    }

    pub fn detect() -> bool {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        home.join("Library")
            .join("Application Support")
            .join("Antigravity")
            .exists()
    }

    pub fn credential_status(&self) -> CredentialStatus {
        if Self::detect() {
            CredentialStatus::Valid
        } else {
            CredentialStatus::NotFound
        }
    }
}

impl Default for AntigravityAuth {
    fn default() -> Self {
        Self::new()
    }
}
