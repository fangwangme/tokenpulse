use super::CredentialStatus;
use anyhow::{anyhow, Result};
use rusqlite::Connection;
use std::path::PathBuf;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct AntigravityCredentials {
    pub api_key: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

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

    pub fn load_credentials(&self) -> Result<AntigravityCredentials> {
        debug!("Loading Antigravity credentials from {:?}", self.db_path);

        if !self.db_path.exists() {
            return Err(anyhow!(
                "Antigravity database not found at {:?}",
                self.db_path
            ));
        }

        let api_key = self.load_api_key()?;
        let proto = self.load_proto_tokens()?;

        Ok(AntigravityCredentials {
            api_key,
            access_token: proto.as_ref().and_then(|p| p.access_token.clone()),
            refresh_token: proto.as_ref().and_then(|p| p.refresh_token.clone()),
        })
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
}

impl AntigravityAuth {
    pub fn detect() -> bool {
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
                if creds.access_token.is_some() || creds.api_key.is_some() {
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
                fields.insert(field_num, FieldValue::Varint(val));
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
    Varint(u64),
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
