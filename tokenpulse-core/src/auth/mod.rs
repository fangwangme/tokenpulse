pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod gemini;

pub use antigravity::AntigravityAuth;
pub use claude::ClaudeAuth;
pub use codex::CodexAuth;
pub use copilot::CopilotAuth;
pub use gemini::GeminiAuth;

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
            name: "gemini".to_string(),
            display_name: "Gemini".to_string(),
            detected: GeminiAuth::detect(),
            credential_hint: if GeminiAuth::detect() {
                "~/.gemini/oauth_creds.json found".to_string()
            } else {
                "not detected".to_string()
            },
        },
        DetectedProvider {
            name: "antigravity".to_string(),
            display_name: "Antigravity".to_string(),
            detected: AntigravityAuth::detect(),
            credential_hint: if AntigravityAuth::detect() {
                "state.vscdb found".to_string()
            } else {
                "not detected".to_string()
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
