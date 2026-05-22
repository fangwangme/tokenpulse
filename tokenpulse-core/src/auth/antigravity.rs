use super::CredentialStatus;
use anyhow::{anyhow, Result};
use rusqlite::Connection;
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct AntigravityCredentials {
    pub api_key: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub email: Option<String>,
    pub client_id: Option<String>,
}

pub struct AntigravityAuth {
    db_path: PathBuf,
}

impl AntigravityAuth {
    const CLI_KEYRING_SERVICE: &'static str = "gemini";
    const CLI_KEYRING_ACCOUNT: &'static str = "antigravity";
    const ANTIGRAVITY_CLIENT_ID: &'static str =
        "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
    const LEGACY_GEMINI_CLIENT_ID: &'static str =
        "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";

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

    pub fn load_email(&self) -> Option<String> {
        // 1. Attempt to read from state.vscdb first to match the loaded credentials
        if self.db_path.exists() {
            if let Ok(Some(email)) = self.load_desktop_email() {
                if !email.is_empty() {
                    return Some(email);
                }
            }
        }

        // 2. Attempt to read ~/.gemini/google_accounts.json
        if let Some(home) = dirs::home_dir() {
            let google_accounts_path = home.join(".gemini").join("google_accounts.json");
            if google_accounts_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&google_accounts_path) {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(active) = parsed.get("active").and_then(|v| v.as_str()) {
                            if !active.is_empty() {
                                return Some(active.to_string());
                            }
                        }
                    }
                }
            }

            // 3. Attempt to read ~/.gemini/oauth_creds.json
            let oauth_creds_path = home.join(".gemini").join("oauth_creds.json");
            if oauth_creds_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&oauth_creds_path) {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(id_token) = parsed.get("id_token").and_then(|v| v.as_str()) {
                            if let Some(email) = crate::auth::decode_jwt_email(id_token) {
                                if !email.is_empty() {
                                    return Some(email);
                                }
                            }
                        }
                    }
                }
            }
        }

        None
    }

    pub fn load_credentials(&self) -> Result<AntigravityCredentials> {
        debug!("Loading Antigravity credentials...");

        if let Ok(creds) = self.load_cli_keyring_credentials() {
            debug!("Successfully loaded AGY CLI credentials from OS keyring");
            return Ok(creds);
        }

        self.load_legacy_credentials()
    }

    pub fn load_legacy_credentials(&self) -> Result<AntigravityCredentials> {
        // 1. Try reading from state.vscdb first (highly preferred as it is updated by the editor and uses CLIENT_ID_1 with proper scopes)
        if self.db_path.exists() {
            if let Ok(proto) = self.load_proto_tokens() {
                if let Some(p) = proto {
                    if p.access_token.is_some() || p.refresh_token.is_some() {
                        let api_key = self.load_api_key().unwrap_or(None);
                        let email = self.load_desktop_email().unwrap_or(None);
                        debug!("Successfully loaded credentials from state.vscdb");
                        return Ok(AntigravityCredentials {
                            api_key,
                            access_token: p.access_token,
                            refresh_token: p.refresh_token,
                            email,
                            client_id: Some(Self::ANTIGRAVITY_CLIENT_ID.to_string()),
                        });
                    }
                }
            }
        }

        // 2. Fallback to ~/.gemini/oauth_creds.json
        if let Some(home) = dirs::home_dir() {
            let oauth_creds_path = home.join(".gemini").join("oauth_creds.json");
            if oauth_creds_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&oauth_creds_path) {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                        let access_token = parsed
                            .get("access_token")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        let refresh_token = parsed
                            .get("refresh_token")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        if access_token.is_some() || refresh_token.is_some() {
                            let email = self.load_email();
                            let mut client_id = None;
                            if let Some(id_token) = parsed.get("id_token").and_then(|v| v.as_str())
                            {
                                client_id = crate::auth::decode_jwt_client_id(id_token);
                            }
                            if client_id.is_none() {
                                client_id = Some(Self::LEGACY_GEMINI_CLIENT_ID.to_string());
                            }
                            debug!(
                                "Successfully loaded credentials from ~/.gemini/oauth_creds.json"
                            );
                            return Ok(AntigravityCredentials {
                                api_key: None,
                                access_token,
                                refresh_token,
                                email,
                                client_id,
                            });
                        }
                    }
                }
            }
        }

        Err(anyhow!(
            "No Antigravity credentials found in keyring, database, or oauth_creds.json"
        ))
    }

    pub fn load_cli_keyring_credentials(&self) -> Result<AntigravityCredentials> {
        let secret = read_cli_keyring_secret()?;
        let token = parse_agy_keyring_token(&secret)?;

        if token.access_token.is_none() && token.refresh_token.is_none() {
            return Err(anyhow!("AGY CLI keyring token is empty"));
        }

        Ok(AntigravityCredentials {
            api_key: None,
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            email: None,
            client_id: Some(Self::ANTIGRAVITY_CLIENT_ID.to_string()),
        })
    }

    pub fn has_cli_keyring_credentials() -> bool {
        cli_keyring_item_exists()
    }

    pub fn load_desktop_email(&self) -> Result<Option<String>> {
        let conn = Connection::open(&self.db_path)?;
        let mut stmt = conn
            .prepare("SELECT value FROM ItemTable WHERE key = 'antigravityAuthStatus' LIMIT 1")?;

        let result = stmt.query_row([], |row| {
            let value: String = row.get(0)?;
            Ok(value)
        });

        match result {
            Ok(json_str) => {
                let parsed: serde_json::Value = serde_json::from_str(&json_str)?;
                let email = parsed
                    .get("email")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Ok(email)
            }
            Err(_) => Ok(None),
        }
    }

    fn load_api_key(&self) -> Result<Option<String>> {
        let conn = Connection::open(&self.db_path)?;
        let mut stmt = conn
            .prepare("SELECT value FROM ItemTable WHERE key = 'antigravityAuthStatus' LIMIT 1")?;

        let result = stmt.query_row([], |row| {
            let value: String = row.get(0)?;
            Ok(value)
        });

        match result {
            Ok(json_str) => {
                let parsed: serde_json::Value = serde_json::from_str(&json_str)?;
                let api_key = parsed
                    .get("apiKey")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Ok(api_key)
            }
            Err(_) => Ok(None),
        }
    }

    fn load_proto_tokens(&self) -> Result<Option<ProtoTokens>> {
        let conn = Connection::open(&self.db_path)?;

        // Try querying the new key first
        let mut stmt = conn.prepare(
            "SELECT value FROM ItemTable WHERE key = 'antigravityUnifiedStateSync.oauthToken' LIMIT 1"
        )?;

        let result = stmt.query_row([], |row| {
            let value: String = row.get(0)?;
            Ok(value)
        });

        if let Ok(base64_str) = result {
            if let Ok(tokens) = self.parse_new_oauth_token(&base64_str) {
                return Ok(Some(tokens));
            }
        }

        // Fallback to the old key
        let mut stmt = conn.prepare(
            "SELECT value FROM ItemTable WHERE key = 'jetskiStateSync.agentManagerInitState' LIMIT 1"
        )?;

        let result = stmt.query_row([], |row| {
            let value: String = row.get(0)?;
            Ok(value)
        });

        match result {
            Ok(base64_str) => {
                let decoded = decode_base64(&base64_str)?;
                let tokens = parse_proto_tokens(&decoded)?;
                Ok(Some(tokens))
            }
            Err(_) => Ok(None),
        }
    }

    fn parse_new_oauth_token(&self, base64_str: &str) -> Result<ProtoTokens> {
        let outer = decode_base64(base64_str.trim())?;
        let outer_fields = read_fields_multi(&outer);

        for (field_num, val) in outer_fields {
            if field_num == 1 {
                if let FieldValue::Bytes(wrapper_bytes) = val {
                    let wrapper_fields = read_fields_multi(&wrapper_bytes);
                    let mut sentinel_matches = false;
                    let mut payload_bytes_opt = None;

                    for (wf_num, wf_val) in wrapper_fields {
                        if wf_num == 1 {
                            if let FieldValue::Bytes(sentinel_bytes) = wf_val {
                                if let Ok(sentinel_str) = String::from_utf8(sentinel_bytes) {
                                    if sentinel_str == "oauthTokenInfoSentinelKey" {
                                        sentinel_matches = true;
                                    }
                                }
                            }
                        } else if wf_num == 2 {
                            if let FieldValue::Bytes(pb) = wf_val {
                                payload_bytes_opt = Some(pb);
                            }
                        }
                    }

                    if sentinel_matches {
                        if let Some(payload_bytes) = payload_bytes_opt {
                            let payload_fields = read_fields_multi(&payload_bytes);
                            for (pf_num, pf_val) in payload_fields {
                                if pf_num == 1 {
                                    if let FieldValue::Bytes(inner_b64_bytes) = pf_val {
                                        if let Ok(inner_b64) = String::from_utf8(inner_b64_bytes) {
                                            let inner_b64_trimmed = inner_b64.trim();
                                            let inner_bytes = decode_base64(inner_b64_trimmed)?;
                                            let inner_fields = read_fields_multi(&inner_bytes);

                                            let mut access_token = None;
                                            let mut refresh_token = None;

                                            for (if_num, if_val) in inner_fields {
                                                if if_num == 1 {
                                                    if let FieldValue::Bytes(at_bytes) = if_val {
                                                        access_token = Some(
                                                            String::from_utf8_lossy(&at_bytes)
                                                                .to_string(),
                                                        );
                                                    }
                                                } else if if_num == 3 {
                                                    if let FieldValue::Bytes(rt_bytes) = if_val {
                                                        refresh_token = Some(
                                                            String::from_utf8_lossy(&rt_bytes)
                                                                .to_string(),
                                                        );
                                                    }
                                                }
                                            }

                                            return Ok(ProtoTokens {
                                                access_token,
                                                refresh_token,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Err(anyhow!("Failed to find or parse new OAuth token"))
    }
}

impl AntigravityAuth {
    pub fn detect() -> bool {
        if Self::has_cli_keyring_credentials() {
            return true;
        }

        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
        home.join("Library")
            .join("Application Support")
            .join("Antigravity")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb")
            .exists()
    }

    pub fn credential_status(&self) -> CredentialStatus {
        match self.load_credentials() {
            Ok(creds) => {
                if creds.access_token.is_some()
                    || creds.refresh_token.is_some()
                    || creds.api_key.is_some()
                {
                    CredentialStatus::Valid
                } else {
                    CredentialStatus::NotFound
                }
            }
            Err(_) => CredentialStatus::NotFound,
        }
    }
}

impl Default for AntigravityAuth {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
struct ProtoTokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AgyKeyringEnvelope {
    #[serde(default)]
    token: Option<AgyKeyringToken>,
}

#[derive(Debug, Deserialize)]
struct AgyKeyringToken {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

fn read_cli_keyring_secret() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                AntigravityAuth::CLI_KEYRING_SERVICE,
                "-a",
                AntigravityAuth::CLI_KEYRING_ACCOUNT,
                "-w",
            ])
            .output()?;

        if !output.status.success() {
            return Err(anyhow!("AGY CLI keyring credentials not found"));
        }

        let secret = String::from_utf8(output.stdout)?.trim().to_string();
        if secret.is_empty() {
            return Err(anyhow!("AGY CLI keyring secret is empty"));
        }
        Ok(secret)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(anyhow!(
            "AGY CLI keyring lookup is only implemented on macOS"
        ))
    }
}

fn cli_keyring_item_exists() -> bool {
    #[cfg(target_os = "macos")]
    {
        Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                AntigravityAuth::CLI_KEYRING_SERVICE,
                "-a",
                AntigravityAuth::CLI_KEYRING_ACCOUNT,
            ])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn parse_agy_keyring_token(secret: &str) -> Result<AgyKeyringToken> {
    let json_bytes = if let Some(payload) = secret.strip_prefix("go-keyring-base64:") {
        decode_base64(payload.trim())?
    } else {
        secret.as_bytes().to_vec()
    };

    let envelope: AgyKeyringEnvelope = serde_json::from_slice(&json_bytes)?;
    envelope
        .token
        .ok_or_else(|| anyhow!("AGY CLI keyring payload missing token"))
}

fn decode_base64(input: &str) -> Result<Vec<u8>> {
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
        .map_err(|e| anyhow!("Base64 decode error: {}", e))
}

fn read_varint(data: &[u8], pos: usize) -> Option<(u64, usize)> {
    let mut v: u64 = 0;
    let mut shift = 0;
    let mut pos = pos;

    while pos < data.len() {
        let b = data[pos];
        pos += 1;
        v |= ((b & 0x7F) as u64) << shift;
        if (b & 0x80) == 0 {
            return Some((v, pos));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

fn read_fields(data: &[u8]) -> std::collections::HashMap<u64, FieldValue> {
    let mut fields = std::collections::HashMap::new();
    let mut pos = 0;

    while pos < data.len() {
        let tag = match read_varint(data, pos) {
            Some((t, p)) => {
                pos = p;
                t
            }
            None => break,
        };

        let field_num = tag >> 3;
        let wire_type = tag & 7;

        match wire_type {
            0 => {
                let val = match read_varint(data, pos) {
                    Some((v, p)) => {
                        pos = p;
                        v
                    }
                    None => break,
                };
                let _ = val;
                fields.insert(field_num, FieldValue::Varint(()));
            }
            2 => {
                let len = match read_varint(data, pos) {
                    Some((l, p)) => {
                        pos = p;
                        l as usize
                    }
                    None => break,
                };
                if pos + len > data.len() {
                    break;
                }
                fields.insert(field_num, FieldValue::Bytes(data[pos..pos + len].to_vec()));
                pos += len;
            }
            _ => break,
        }
    }

    fields
}

#[derive(Debug, Clone)]
enum FieldValue {
    Varint(()),
    Bytes(Vec<u8>),
}

fn parse_proto_tokens(data: &[u8]) -> Result<ProtoTokens> {
    let outer = read_fields(data);

    let field_6 = match outer.get(&6) {
        Some(FieldValue::Bytes(b)) => b,
        _ => {
            return Ok(ProtoTokens {
                access_token: None,
                refresh_token: None,
            })
        }
    };

    let inner = read_fields(field_6);

    let access_token = match inner.get(&1) {
        Some(FieldValue::Bytes(b)) => Some(String::from_utf8_lossy(b).to_string()),
        _ => None,
    };

    let refresh_token = match inner.get(&3) {
        Some(FieldValue::Bytes(b)) => Some(String::from_utf8_lossy(b).to_string()),
        _ => None,
    };

    Ok(ProtoTokens {
        access_token,
        refresh_token,
    })
}

fn read_fields_multi(data: &[u8]) -> Vec<(u64, FieldValue)> {
    let mut fields = Vec::new();
    let mut pos = 0;

    while pos < data.len() {
        let tag = match read_varint(data, pos) {
            Some((t, p)) => {
                pos = p;
                t
            }
            None => break,
        };

        let field_num = tag >> 3;
        let wire_type = tag & 7;

        match wire_type {
            0 => {
                let val = match read_varint(data, pos) {
                    Some((v, p)) => {
                        pos = p;
                        v
                    }
                    None => break,
                };
                let _ = val;
                fields.push((field_num, FieldValue::Varint(())));
            }
            2 => {
                let len = match read_varint(data, pos) {
                    Some((l, p)) => {
                        pos = p;
                        l as usize
                    }
                    None => break,
                };
                if pos + len > data.len() {
                    break;
                }
                fields.push((field_num, FieldValue::Bytes(data[pos..pos + len].to_vec())));
                pos += len;
            }
            _ => break,
        }
    }

    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn parse_agy_keyring_token_decodes_go_keyring_base64_payload() {
        let json = r#"{"token":{"access_token":"access-123","refresh_token":"refresh-456","token_type":"Bearer","expiry":"2026-05-22T22:15:28+08:00"},"auth_method":"consumer"}"#;
        let payload = base64::engine::general_purpose::STANDARD.encode(json);
        let secret = format!("go-keyring-base64:{payload}");

        let token = parse_agy_keyring_token(&secret).unwrap();

        assert_eq!(token.access_token.as_deref(), Some("access-123"));
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-456"));
    }

    #[test]
    fn parse_agy_keyring_token_accepts_raw_json_payload() {
        let token = parse_agy_keyring_token(
            r#"{"token":{"access_token":"access-raw","refresh_token":"refresh-raw"}}"#,
        )
        .unwrap();

        assert_eq!(token.access_token.as_deref(), Some("access-raw"));
        assert_eq!(token.refresh_token.as_deref(), Some("refresh-raw"));
    }
}
