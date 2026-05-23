pub mod claude;
pub mod codex;
pub mod copilot;

pub use claude::ClaudeAuth;
pub use codex::CodexAuth;
pub use copilot::CopilotAuth;

#[derive(Debug, Clone, PartialEq)]
pub enum CredentialStatus {
    Valid,
    Expired,
    NotFound,
}

#[derive(Debug, Clone)]
pub struct DetectedProvider {
    pub name: String,
    pub display_name: String,
    pub detected: bool,
    pub credential_hint: String,
}

pub fn detect_providers() -> Vec<DetectedProvider> {
    vec![
        DetectedProvider {
            name: "claude".to_string(),
            display_name: "Claude Code".to_string(),
            detected: ClaudeAuth::detect(),
            credential_hint: if ClaudeAuth::detect() {
                "~/.claude/.credentials.json found".to_string()
            } else {
                "not detected".to_string()
            },
        },
        DetectedProvider {
            name: "codex".to_string(),
            display_name: "Codex".to_string(),
            detected: CodexAuth::detect(),
            credential_hint: if CodexAuth::detect() {
                "~/.config/codex/auth.json found".to_string()
            } else {
                "not detected".to_string()
            },
        },
        DetectedProvider {
            name: "antigravity".to_string(),
            display_name: "Antigravity".to_string(),
            detected: {
                let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
                home.join("Library")
                    .join("Application Support")
                    .join("Antigravity")
                    .exists()
            },
            credential_hint: {
                let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
                if home
                    .join("Library")
                    .join("Application Support")
                    .join("Antigravity")
                    .exists()
                {
                    "installed".to_string()
                } else {
                    "not detected".to_string()
                }
            },
        },
        DetectedProvider {
            name: "copilot".to_string(),
            display_name: "GitHub Copilot".to_string(),
            detected: CopilotAuth::detect(),
            credential_hint: CopilotAuth::credential_hint(),
        },
    ]
}

pub fn decode_base64(input: &str) -> anyhow::Result<Vec<u8>> {
    let input = input.replace('-', "+").replace('_', "/");
    let padded_len = (input.len() + 3) / 4 * 4;
    let mut padded = input.to_string();
    while padded.len() < padded_len {
        padded.push('=');
    }

    use base64::Engine;
    let engine = base64::engine::general_purpose::STANDARD;
    engine
        .decode(&padded)
        .map_err(|e| anyhow::anyhow!("Base64 decode error: {}", e))
}

pub fn decode_jwt_email(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload_bytes = decode_base64(parts[1]).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    json.get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn decode_jwt_client_id(token: &str) -> Option<String> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload_bytes = decode_base64(parts[1]).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&payload_bytes).ok()?;
    json.get("azp")
        .or_else(|| json.get("aud"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
