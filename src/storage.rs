//! Metadata-only SQLite persistence. Request/response bodies and credentials
//! are deliberately absent from this schema.

use crate::error::{AppError, Result};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestSummary {
    pub id: String,
    pub tenant_id: String,
    pub device_id: String,
    pub client_instance_id: String,
    pub session_key: Option<String>,
    pub started_at: DateTime<Utc>,
    pub method: String,
    pub path: String,
    pub status: Option<u16>,
    pub stage: String,
    pub duration_ms: Option<u64>,
    pub ttfb_ms: Option<u64>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub account_id: Option<Uuid>,
    pub route_reason: String,
    pub retries: u32,
    pub partial_failure: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub id: String,
    pub occurred_at: DateTime<Utc>,
    pub tenant_id: String,
    pub device_id: String,
    pub client_instance_id: Option<String>,
    pub kind: String,
    pub account_id: Option<Uuid>,
    /// Must already be sanitized; storage also redacts common secret markers.
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct RetentionPolicy {
    pub days: i64,
    pub max_requests: usize,
    pub max_events: usize,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            days: 7,
            max_requests: 50_000,
            max_events: 10_000,
        }
    }
}

pub struct MetadataStore {
    connection: Mutex<Connection>,
    retention: RetentionPolicy,
}

impl MetadataStore {
    pub fn open(path: &Path, retention: RetentionPolicy) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let created = !parent.exists();
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            if created {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
        }
        let connection = Connection::open(path).map_err(db_error)?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(db_error)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(db_error)?;
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS request_summaries (
                id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, device_id TEXT NOT NULL,
                client_instance_id TEXT NOT NULL, session_key TEXT, started_at TEXT NOT NULL,
                method TEXT NOT NULL, path TEXT NOT NULL, status INTEGER, stage TEXT NOT NULL,
                duration_ms INTEGER, ttfb_ms INTEGER, request_bytes INTEGER NOT NULL,
                response_bytes INTEGER NOT NULL, account_id TEXT, route_reason TEXT NOT NULL,
                retries INTEGER NOT NULL, partial_failure INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS requests_started_at ON request_summaries(started_at DESC);
             CREATE TABLE IF NOT EXISTS runtime_events (
                id TEXT PRIMARY KEY, occurred_at TEXT NOT NULL, tenant_id TEXT NOT NULL,
                device_id TEXT NOT NULL, client_instance_id TEXT, kind TEXT NOT NULL,
                account_id TEXT, detail TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS events_occurred_at ON runtime_events(occurred_at DESC);",
            )
            .map_err(db_error)?;
        #[cfg(unix)]
        if path.exists() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        let store = Self {
            connection: Mutex::new(connection),
            retention,
        };
        store.cleanup()?;
        Ok(store)
    }

    pub fn record_request(&self, summary: &RequestSummary) -> Result<()> {
        self.connection.lock().execute(
            "INSERT OR REPLACE INTO request_summaries VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
            params![summary.id, summary.tenant_id, summary.device_id, summary.client_instance_id,
                summary.session_key, summary.started_at.to_rfc3339(), summary.method, summary.path,
                summary.status, summary.stage, summary.duration_ms, summary.ttfb_ms,
                summary.request_bytes, summary.response_bytes,
                summary.account_id.map(|id| id.to_string()), summary.route_reason,
                summary.retries, summary.partial_failure]
        ).map_err(db_error)?;
        Ok(())
    }

    pub fn record_event(&self, event: &RuntimeEvent) -> Result<()> {
        self.connection
            .lock()
            .execute(
                "INSERT OR REPLACE INTO runtime_events VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    event.id,
                    event.occurred_at.to_rfc3339(),
                    event.tenant_id,
                    event.device_id,
                    event.client_instance_id,
                    event.kind,
                    event.account_id.map(|id| id.to_string()),
                    sanitize(&event.detail)
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn cleanup(&self) -> Result<()> {
        let cutoff = (Utc::now() - Duration::days(self.retention.days)).to_rfc3339();
        let connection = self.connection.lock();
        connection
            .execute(
                "DELETE FROM request_summaries WHERE started_at < ?1",
                [&cutoff],
            )
            .map_err(db_error)?;
        connection
            .execute(
                "DELETE FROM runtime_events WHERE occurred_at < ?1",
                [&cutoff],
            )
            .map_err(db_error)?;
        connection.execute(
            "DELETE FROM request_summaries WHERE id IN (SELECT id FROM request_summaries ORDER BY started_at DESC LIMIT -1 OFFSET ?1)",
            [self.retention.max_requests as i64],
        ).map_err(db_error)?;
        connection.execute(
            "DELETE FROM runtime_events WHERE id IN (SELECT id FROM runtime_events ORDER BY occurred_at DESC LIMIT -1 OFFSET ?1)",
            [self.retention.max_events as i64],
        ).map_err(db_error)?;
        Ok(())
    }

    pub fn counts(&self) -> Result<(u64, u64)> {
        let connection = self.connection.lock();
        let requests = connection
            .query_row("SELECT COUNT(*) FROM request_summaries", [], |row| {
                row.get(0)
            })
            .map_err(db_error)?;
        let events = connection
            .query_row("SELECT COUNT(*) FROM runtime_events", [], |row| row.get(0))
            .map_err(db_error)?;
        Ok((requests, events))
    }

    pub fn recent_events(&self, limit: usize, offset: usize) -> Result<Vec<RuntimeEvent>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT id,occurred_at,tenant_id,device_id,client_instance_id,kind,account_id,detail
             FROM runtime_events ORDER BY occurred_at DESC LIMIT ?1 OFFSET ?2",
        ).map_err(db_error)?;
        let rows = statement
            .query_map(params![limit.min(500) as i64, offset as i64], |row| {
                let occurred: String = row.get(1)?;
                let account: Option<String> = row.get(6)?;
                Ok(RuntimeEvent {
                    id: row.get(0)?,
                    occurred_at: DateTime::parse_from_rfc3339(&occurred)
                        .map(|time| time.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    tenant_id: row.get(2)?,
                    device_id: row.get(3)?,
                    client_instance_id: row.get(4)?,
                    kind: row.get(5)?,
                    account_id: account.and_then(|id| Uuid::parse_str(&id).ok()),
                    detail: row.get(7)?,
                })
            })
            .map_err(db_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)
    }
}

pub fn sanitize(message: &str) -> String {
    let lowered = message.to_ascii_lowercase();
    if [
        "authorization",
        "bearer ",
        "access_token",
        "refresh_token",
        "id_token",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
    {
        "[敏感信息已隐藏]".into()
    } else {
        message.chars().take(1024).collect()
    }
}

fn db_error(error: rusqlite::Error) -> AppError {
    AppError::Message(format!("SQLite 元数据存储错误：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_never_keeps_secret_bearing_event_detail() {
        assert_eq!(sanitize("Authorization: Bearer secret"), "[敏感信息已隐藏]");
    }

    #[test]
    fn capacity_cleanup_is_enforced() {
        let path =
            std::env::temp_dir().join(format!("codex-switcher-store-{}.sqlite", Uuid::new_v4()));
        let store = MetadataStore::open(
            &path,
            RetentionPolicy {
                days: 7,
                max_requests: 1,
                max_events: 1,
            },
        )
        .unwrap();
        for i in 0..2 {
            store
                .record_event(&RuntimeEvent {
                    id: i.to_string(),
                    occurred_at: Utc::now(),
                    tenant_id: "local".into(),
                    device_id: "test".into(),
                    client_instance_id: None,
                    kind: "test".into(),
                    account_id: None,
                    detail: "safe".into(),
                })
                .unwrap();
        }
        store.cleanup().unwrap();
        assert_eq!(store.counts().unwrap().1, 1);
    }
}
