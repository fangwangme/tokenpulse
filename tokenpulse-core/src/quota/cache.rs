use crate::provider::QuotaSnapshot;
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const QUOTA_CACHE_TTL_MINUTES: i64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedQuotaSnapshot {
    pub provider: String,
    pub observed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub fetched_at: DateTime<Utc>,
    pub snapshot: QuotaSnapshot,
}

pub struct QuotaCacheStore {
    db_path: PathBuf,
}

impl QuotaCacheStore {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let db_path = home
            .join(".local")
            .join("share")
            .join("tokenpulse")
            .join("tokenpulse.db");

        Self { db_path }
    }

    pub fn load_valid(
        &self,
        provider: &str,
        now: DateTime<Utc>,
    ) -> Result<Option<CachedQuotaSnapshot>> {
        let cached = self.load_cached_from_path(&self.db_path, provider)?;
        Ok(cached.filter(|entry| entry.expires_at > now))
    }

    pub fn save(
        &self,
        provider: &str,
        observed_at: DateTime<Utc>,
        snapshot: &QuotaSnapshot,
    ) -> Result<()> {
        let conn = self.open()?;
        let expires_at = observed_at + Duration::minutes(QUOTA_CACHE_TTL_MINUTES);
        let entry = CachedQuotaSnapshot {
            provider: provider.to_string(),
            observed_at,
            expires_at,
            fetched_at: snapshot.fetched_at,
            snapshot: snapshot.clone(),
        };

        let snapshot_json = serde_json::to_string(&entry.snapshot)?;
        conn.execute(
            "
            INSERT INTO quota_cache (
                provider,
                observed_at,
                expires_at,
                fetched_at,
                snapshot_json
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(provider) DO UPDATE SET
                observed_at = excluded.observed_at,
                expires_at = excluded.expires_at,
                fetched_at = excluded.fetched_at,
                snapshot_json = excluded.snapshot_json
            ",
            params![
                entry.provider,
                entry.observed_at.to_rfc3339(),
                entry.expires_at.to_rfc3339(),
                entry.fetched_at.to_rfc3339(),
                snapshot_json
            ],
        )?;

        Ok(())
    }

    fn open(&self) -> Result<Connection> {
        self.open_at_path(&self.db_path)
    }

    fn open_at_path(&self, path: &PathBuf) -> Result<Connection> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS quota_cache (
                provider TEXT PRIMARY KEY,
                observed_at TEXT NOT NULL,
                expires_at TEXT NOT NULL,
                fetched_at TEXT NOT NULL,
                snapshot_json TEXT NOT NULL
            );
            ",
        )?;
        Ok(conn)
    }

    fn load_cached_from_path(
        &self,
        path: &PathBuf,
        provider: &str,
    ) -> Result<Option<CachedQuotaSnapshot>> {
        if !path.exists() {
            return Ok(None);
        }

        let conn = self.open_at_path(path)?;
        self.load_cached(&conn, provider)
    }

    fn load_cached(
        &self,
        conn: &Connection,
        provider: &str,
    ) -> Result<Option<CachedQuotaSnapshot>> {
        let row = conn
            .query_row(
                "
                SELECT observed_at, expires_at, fetched_at, snapshot_json
                FROM quota_cache
                WHERE provider = ?1
                LIMIT 1
                ",
                params![provider],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;

        let Some((observed_at, expires_at, fetched_at, snapshot_json)) = row else {
            return Ok(None);
        };

        let snapshot: QuotaSnapshot = serde_json::from_str(&snapshot_json)?;

        Ok(Some(CachedQuotaSnapshot {
            provider: provider.to_string(),
            observed_at: DateTime::parse_from_rfc3339(&observed_at)?.with_timezone(&Utc),
            expires_at: DateTime::parse_from_rfc3339(&expires_at)?.with_timezone(&Utc),
            fetched_at: DateTime::parse_from_rfc3339(&fetched_at)?.with_timezone(&Utc),
            snapshot,
        }))
    }
}

impl Default for QuotaCacheStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::RateWindow;

    fn sample_snapshot(provider: &str) -> QuotaSnapshot {
        QuotaSnapshot {
            provider: provider.to_string(),
            plan: Some("Pro".to_string()),
            windows: vec![RateWindow {
                label: "Session".to_string(),
                used_percent: 25.0,
                resets_at: None,
                period_duration_ms: Some(5 * 60 * 60 * 1000),
            }],
            credits: None,
            fetched_at: Utc::now(),
        }
    }

    #[test]
    fn cache_store_loads_valid_entry() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = QuotaCacheStore {
            db_path: temp_dir.path().join("quota.db"),
        };
        let observed_at = Utc::now();

        store
            .save("gemini", observed_at, &sample_snapshot("gemini"))
            .unwrap();

        let cached = store.load_valid("gemini", observed_at).unwrap();
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().provider, "gemini");
    }

    #[test]
    fn cache_store_skips_expired_entry() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = QuotaCacheStore {
            db_path: temp_dir.path().join("quota.db"),
        };
        let observed_at = Utc::now() - Duration::minutes(10);

        store
            .save("claude", observed_at, &sample_snapshot("claude"))
            .unwrap();

        let cached = store.load_valid("claude", Utc::now()).unwrap();
        assert!(cached.is_none());
    }

    #[test]
    fn cache_store_overwrites_existing_provider_entry() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = QuotaCacheStore {
            db_path: temp_dir.path().join("quota.db"),
        };
        let observed_at = Utc::now();
        let mut first = sample_snapshot("gemini");
        first.plan = Some("Free".to_string());
        let mut second = sample_snapshot("gemini");
        second.plan = Some("Paid".to_string());

        store.save("gemini", observed_at, &first).unwrap();
        store
            .save("gemini", observed_at + Duration::minutes(1), &second)
            .unwrap();

        let cached = store
            .load_valid("gemini", observed_at + Duration::minutes(1))
            .unwrap()
            .unwrap();
        assert_eq!(cached.snapshot.plan.as_deref(), Some("Paid"));
    }

    #[test]
    fn cache_store_uses_local_share_database_path() {
        let store = QuotaCacheStore::new();
        let path = store.db_path.to_string_lossy();
        assert!(path.ends_with(".local/share/tokenpulse/tokenpulse.db"));
    }
}
