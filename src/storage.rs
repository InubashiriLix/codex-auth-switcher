//! Metadata-only SQLite persistence. Request/response bodies and credentials
//! are deliberately absent from this schema.

use crate::{
    error::{AppError, Result},
    i18n::LocalizedMessage,
    types::{Account, AccountIndex, CheckStatus, Quota, StatusKind},
};
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
    #[serde(default)]
    pub route_message: Option<LocalizedMessage>,
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
    #[serde(default)]
    pub message: Option<LocalizedMessage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountEvent {
    pub id: String,
    pub account_id: Option<Uuid>,
    pub occurred_at: DateTime<Utc>,
    pub kind: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MetricBucket {
    pub started_at: DateTime<Utc>,
    pub requests: u64,
    pub failures: u64,
    pub average_ttfb_ms: Option<u64>,
    #[serde(default)]
    pub ttfb_p95_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MetricsWindow {
    pub window_seconds: u64,
    pub bucket_seconds: u64,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub rps: f64,
    pub success_rate: f64,
    pub ttfb_p50_ms: Option<u64>,
    pub ttfb_p95_ms: Option<u64>,
    pub ttfb_p99_ms: Option<u64>,
    pub duration_p50_ms: Option<u64>,
    pub duration_p95_ms: Option<u64>,
    pub duration_p99_ms: Option<u64>,
    pub buckets: Vec<MetricBucket>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredCircuit {
    pub account_id: Uuid,
    pub reason: String,
    pub until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBinding {
    pub sticky_key: String,
    pub account_id: Uuid,
    pub expires_at: DateTime<Utc>,
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
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS account_meta (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    revision INTEGER NOT NULL DEFAULT 0
                 );
                 INSERT OR IGNORE INTO account_meta(singleton, revision) VALUES (1, 0);
                 CREATE TABLE IF NOT EXISTS accounts (
                    id TEXT PRIMARY KEY, label TEXT NOT NULL, source TEXT NOT NULL,
                    imported_at TEXT NOT NULL, email TEXT, plan TEXT, upstream_account_id TEXT,
                    tenant_id TEXT NOT NULL, proxy_enabled INTEGER NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1, priority INTEGER NOT NULL DEFAULT 100,
                    concurrency_limit INTEGER NOT NULL DEFAULT 0, revision INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE UNIQUE INDEX IF NOT EXISTS accounts_upstream_id
                    ON accounts(upstream_account_id) WHERE upstream_account_id IS NOT NULL;
                 CREATE TABLE IF NOT EXISTS quota_windows (
                    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                    kind TEXT NOT NULL, used_percent REAL NOT NULL,
                    window_minutes INTEGER, resets_at INTEGER,
                    PRIMARY KEY(account_id, kind)
                 );
                 CREATE TABLE IF NOT EXISTS account_runtime (
                    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
                    status TEXT NOT NULL, checked_at TEXT, detail TEXT NOT NULL,
                    failure_count INTEGER NOT NULL DEFAULT 0,
                    circuit_reason TEXT, circuit_until TEXT,
                    last_success_at TEXT, last_failure_at TEXT, last_refresh_at TEXT
                 );
                 CREATE TABLE IF NOT EXISTS session_bindings (
                    sticky_key TEXT PRIMARY KEY,
                    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                    updated_at TEXT NOT NULL, expires_at TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS bindings_expires_at ON session_bindings(expires_at);
                 CREATE TABLE IF NOT EXISTS account_events (
                    id TEXT PRIMARY KEY, account_id TEXT REFERENCES accounts(id) ON DELETE SET NULL,
                    occurred_at TEXT NOT NULL, kind TEXT NOT NULL, detail TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS account_events_time ON account_events(occurred_at DESC);",
            )
            .map_err(db_error)?;
        ensure_column(
            &connection,
            "request_summaries",
            "route_message_key",
            "TEXT",
        )?;
        ensure_column(
            &connection,
            "request_summaries",
            "route_message_args",
            "TEXT",
        )?;
        ensure_column(&connection, "runtime_events", "message_key", "TEXT")?;
        ensure_column(&connection, "runtime_events", "message_args", "TEXT")?;
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
            "INSERT OR REPLACE INTO request_summaries
             (id,tenant_id,device_id,client_instance_id,session_key,started_at,method,path,status,stage,duration_ms,ttfb_ms,request_bytes,response_bytes,account_id,route_reason,retries,partial_failure,route_message_key,route_message_args)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            params![summary.id, summary.tenant_id, summary.device_id, summary.client_instance_id,
                summary.session_key, summary.started_at.to_rfc3339(), summary.method, summary.path,
                summary.status, summary.stage, summary.duration_ms, summary.ttfb_ms,
                summary.request_bytes, summary.response_bytes,
                summary.account_id.map(|id| id.to_string()), summary.route_reason,
                summary.retries, summary.partial_failure,
                summary.route_message.as_ref().map(|message| message.key.as_str()),
                summary.route_message.as_ref().and_then(|message| serde_json::to_string(&message.args).ok())]
        ).map_err(db_error)?;
        Ok(())
    }

    pub fn record_event(&self, event: &RuntimeEvent) -> Result<()> {
        self.connection
            .lock()
            .execute(
                "INSERT OR REPLACE INTO runtime_events
                 (id,occurred_at,tenant_id,device_id,client_instance_id,kind,account_id,detail,message_key,message_args)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    event.id,
                    event.occurred_at.to_rfc3339(),
                    event.tenant_id,
                    event.device_id,
                    event.client_instance_id,
                    event.kind,
                    event.account_id.map(|id| id.to_string()),
                    sanitize(&event.detail),
                    event.message.as_ref().map(|message| message.key.as_str()),
                    event.message.as_ref().and_then(|message| serde_json::to_string(&message.args).ok()).map(|args| sanitize(&args))
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn record_account_event(
        &self,
        account_id: Option<Uuid>,
        kind: &str,
        detail: &str,
    ) -> Result<()> {
        self.connection
            .lock()
            .execute(
                "INSERT INTO account_events(id,account_id,occurred_at,kind,detail)
                 VALUES (?1,?2,?3,?4,?5)",
                params![
                    Uuid::new_v4().to_string(),
                    account_id.map(|id| id.to_string()),
                    Utc::now().to_rfc3339(),
                    kind,
                    sanitize(detail),
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
        connection
            .execute(
                "DELETE FROM account_events WHERE occurred_at < ?1",
                [&cutoff],
            )
            .map_err(db_error)?;
        connection
            .execute(
                "DELETE FROM account_events WHERE id IN (SELECT id FROM account_events ORDER BY occurred_at DESC LIMIT -1 OFFSET ?1)",
                [self.retention.max_events as i64],
            )
            .map_err(db_error)?;
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

    /// Import the legacy TOML index once, then treat SQLite as the canonical
    /// account metadata store. Missing legacy rows are added so interrupted
    /// migrations remain recoverable and idempotent.
    pub fn reconcile_accounts(&self, legacy: &AccountIndex) -> Result<(AccountIndex, u64)> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction().map_err(db_error)?;
        for account in &legacy.accounts {
            insert_account_if_missing(&transaction, account)?;
        }
        if !legacy.accounts.is_empty() {
            transaction
                .execute(
                    "UPDATE account_meta SET revision = MAX(revision, 1) WHERE singleton = 1",
                    [],
                )
                .map_err(db_error)?;
        }
        transaction.commit().map_err(db_error)?;
        drop(connection);
        Ok((self.load_accounts()?, self.accounts_revision()?))
    }

    pub fn replace_accounts(&self, index: &AccountIndex) -> Result<u64> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction().map_err(db_error)?;
        for account in &index.accounts {
            upsert_account(&transaction, account)?;
        }
        let retained = index
            .accounts
            .iter()
            .map(|account| account.id.to_string())
            .collect::<Vec<_>>();
        {
            let mut statement = transaction
                .prepare("SELECT id FROM accounts")
                .map_err(db_error)?;
            let existing = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(db_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_error)?;
            for id in existing {
                if !retained.iter().any(|candidate| candidate == &id) {
                    transaction
                        .execute("DELETE FROM accounts WHERE id = ?1", [&id])
                        .map_err(db_error)?;
                }
            }
        }
        transaction
            .execute(
                "UPDATE account_meta SET revision = revision + 1 WHERE singleton = 1",
                [],
            )
            .map_err(db_error)?;
        let revision = transaction
            .query_row(
                "SELECT revision FROM account_meta WHERE singleton = 1",
                [],
                |row| row.get::<_, u64>(0),
            )
            .map_err(db_error)?;
        transaction.commit().map_err(db_error)?;
        Ok(revision)
    }

    pub fn load_accounts(&self) -> Result<AccountIndex> {
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare(
                "SELECT a.id,a.label,a.source,a.imported_at,a.email,a.plan,a.upstream_account_id,
                        a.tenant_id,a.proxy_enabled,a.enabled,a.priority,a.concurrency_limit,a.revision,
                        r.status,r.checked_at,r.detail
                 FROM accounts a LEFT JOIN account_runtime r ON r.account_id = a.id
                 ORDER BY a.priority ASC, a.imported_at ASC, a.id ASC",
            )
            .map_err(db_error)?;
        let mut accounts = statement
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let imported_at: String = row.get(3)?;
                let status: Option<String> = row.get(13)?;
                let checked_at: Option<String> = row.get(14)?;
                Ok(Account {
                    id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::nil()),
                    label: row.get(1)?,
                    source: row.get(2)?,
                    imported_at: parse_time(&imported_at),
                    email: row.get(4)?,
                    plan: row.get(5)?,
                    account_id: row.get(6)?,
                    tenant_id: row.get(7)?,
                    proxy_enabled: row.get(8)?,
                    enabled: row.get(9)?,
                    priority: row.get(10)?,
                    concurrency_limit: row.get(11)?,
                    revision: row.get(12)?,
                    status: CheckStatus {
                        kind: status
                            .as_deref()
                            .map(status_kind)
                            .unwrap_or(StatusKind::Unknown),
                        checked_at: checked_at.as_deref().map(parse_time),
                        detail: row
                            .get::<_, Option<String>>(15)?
                            .unwrap_or_else(|| "尚未检测".into()),
                        primary: None,
                        secondary: None,
                    },
                })
            })
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        let mut quota_statement = connection
            .prepare(
                "SELECT kind,used_percent,window_minutes,resets_at FROM quota_windows WHERE account_id = ?1",
            )
            .map_err(db_error)?;
        for account in &mut accounts {
            let rows = quota_statement
                .query_map([account.id.to_string()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        Quota {
                            used_percent: row.get(1)?,
                            window_minutes: row.get(2)?,
                            resets_at: row.get(3)?,
                        },
                    ))
                })
                .map_err(db_error)?;
            for row in rows {
                let (kind, quota) = row.map_err(db_error)?;
                if kind == "primary" {
                    account.status.primary = Some(quota);
                } else if kind == "secondary" {
                    account.status.secondary = Some(quota);
                }
            }
        }
        Ok(AccountIndex { accounts })
    }

    pub fn accounts_revision(&self) -> Result<u64> {
        self.connection
            .lock()
            .query_row(
                "SELECT revision FROM account_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .map_err(db_error)
    }

    pub fn save_circuit(
        &self,
        account_id: Uuid,
        reason: &str,
        until: Option<DateTime<Utc>>,
    ) -> Result<()> {
        self.connection
            .lock()
            .execute(
                "UPDATE account_runtime SET circuit_reason=?2,circuit_until=?3,
                    failure_count=failure_count+1,last_failure_at=?4 WHERE account_id=?1",
                params![
                    account_id.to_string(),
                    reason,
                    until.map(|time| time.to_rfc3339()),
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn clear_circuit(&self, account_id: Uuid) -> Result<()> {
        self.connection
            .lock()
            .execute(
                "UPDATE account_runtime SET circuit_reason=NULL,circuit_until=NULL,
                    failure_count=0,last_success_at=?2 WHERE account_id=?1",
                params![account_id.to_string(), Utc::now().to_rfc3339()],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn load_circuits(&self) -> Result<Vec<StoredCircuit>> {
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare(
                "SELECT account_id,circuit_reason,circuit_until FROM account_runtime
                 WHERE circuit_reason IS NOT NULL",
            )
            .map_err(db_error)?;
        statement
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let until: Option<String> = row.get(2)?;
                Ok(StoredCircuit {
                    account_id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::nil()),
                    reason: row.get(1)?,
                    until: until.as_deref().map(parse_time),
                })
            })
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)
    }

    pub fn save_binding(&self, sticky_key: &str, account_id: Uuid, ttl: Duration) -> Result<()> {
        let now = Utc::now();
        self.connection
            .lock()
            .execute(
                "INSERT INTO session_bindings(sticky_key,account_id,updated_at,expires_at)
                 VALUES (?1,?2,?3,?4) ON CONFLICT(sticky_key) DO UPDATE SET
                 account_id=excluded.account_id,updated_at=excluded.updated_at,expires_at=excluded.expires_at",
                params![sticky_key, account_id.to_string(), now.to_rfc3339(), (now + ttl).to_rfc3339()],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn delete_binding(&self, sticky_key: &str) -> Result<()> {
        self.connection
            .lock()
            .execute(
                "DELETE FROM session_bindings WHERE sticky_key=?1",
                [sticky_key],
            )
            .map_err(db_error)?;
        Ok(())
    }

    pub fn load_bindings(&self) -> Result<Vec<StoredBinding>> {
        let now = Utc::now().to_rfc3339();
        let connection = self.connection.lock();
        connection
            .execute(
                "DELETE FROM session_bindings WHERE expires_at <= ?1",
                [&now],
            )
            .map_err(db_error)?;
        let mut statement = connection
            .prepare(
                "SELECT sticky_key,account_id,expires_at FROM session_bindings
                 WHERE sticky_key LIKE 'session:%' ORDER BY updated_at DESC",
            )
            .map_err(db_error)?;
        statement
            .query_map([], |row| {
                let id: String = row.get(1)?;
                let expires: String = row.get(2)?;
                Ok(StoredBinding {
                    sticky_key: row.get(0)?,
                    account_id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::nil()),
                    expires_at: parse_time(&expires),
                })
            })
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)
    }

    pub fn recent_events(&self, limit: usize, offset: usize) -> Result<Vec<RuntimeEvent>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT id,occurred_at,tenant_id,device_id,client_instance_id,kind,account_id,detail,message_key,message_args
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
                    message: localized_message(row.get(8)?, row.get(9)?),
                })
            })
            .map_err(db_error)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)
    }

    pub fn recent_account_events(
        &self,
        account_id: Option<Uuid>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<AccountEvent>> {
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare(
                "SELECT id,account_id,occurred_at,kind,detail FROM account_events ORDER BY occurred_at DESC LIMIT ?1 OFFSET ?2",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map(params![limit.min(500) as i64, offset as i64], |row| {
                let occurred: String = row.get(2)?;
                let account: Option<String> = row.get(1)?;
                Ok(AccountEvent {
                    id: row.get(0)?,
                    account_id: account.and_then(|id| Uuid::parse_str(&id).ok()),
                    occurred_at: DateTime::parse_from_rfc3339(&occurred)
                        .map(|time| time.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    kind: row.get(3)?,
                    detail: row.get(4)?,
                })
            })
            .map_err(db_error)?;
        Ok(rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?
            .into_iter()
            .filter(|event| account_id.is_none_or(|id| event.account_id == Some(id)))
            .collect())
    }

    pub fn recent_requests(
        &self,
        limit: usize,
        offset: usize,
        account_id: Option<Uuid>,
        client_instance_id: Option<&str>,
        status: Option<u16>,
    ) -> Result<Vec<RequestSummary>> {
        let connection = self.connection.lock();
        let mut statement = connection
            .prepare(
                "SELECT id,tenant_id,device_id,client_instance_id,session_key,started_at,
                        method,path,status,stage,duration_ms,ttfb_ms,request_bytes,response_bytes,
                        account_id,route_reason,retries,partial_failure,route_message_key,route_message_args
                 FROM request_summaries ORDER BY started_at DESC LIMIT 50000",
            )
            .map_err(db_error)?;
        let rows = statement
            .query_map([], request_from_row)
            .map_err(db_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(db_error)?;
        Ok(rows
            .into_iter()
            .filter(|request| account_id.is_none_or(|id| request.account_id == Some(id)))
            .filter(|request| client_instance_id.is_none_or(|id| request.client_instance_id == id))
            .filter(|request| status.is_none_or(|status| request.status == Some(status)))
            .skip(offset)
            .take(limit.min(50_000))
            .collect())
    }

    pub fn metrics(&self, window_seconds: u64, bucket_seconds: u64) -> Result<MetricsWindow> {
        let window_seconds = window_seconds.clamp(10, 86_400);
        let bucket_seconds = bucket_seconds.clamp(1, window_seconds);
        // Anchor bucket boundaries to wall-clock bucket edges. A fresh query
        // therefore updates the current bucket in place instead of shifting
        // every chart column a little to the left on each TUI refresh.
        let now = Utc::now();
        let bucket_count = window_seconds.div_ceil(bucket_seconds) as usize;
        let current_bucket =
            now.timestamp().div_euclid(bucket_seconds as i64) * bucket_seconds as i64;
        let cutoff = chrono::DateTime::from_timestamp(
            current_bucket - (bucket_count.saturating_sub(1) as i64 * bucket_seconds as i64),
            0,
        )
        .unwrap_or(now);
        let requests = {
            let connection = self.connection.lock();
            let mut statement = connection
                .prepare(
                    "SELECT id,tenant_id,device_id,client_instance_id,session_key,started_at,
                            method,path,status,stage,duration_ms,ttfb_ms,request_bytes,response_bytes,
                            account_id,route_reason,retries,partial_failure,route_message_key,route_message_args
                     FROM request_summaries WHERE started_at >= ?1 ORDER BY started_at DESC",
                )
                .map_err(db_error)?;
            statement
                .query_map([cutoff.to_rfc3339()], request_from_row)
                .map_err(db_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_error)?
        };
        let total_requests = requests.len() as u64;
        let successful_requests = requests
            .iter()
            .filter(|request| {
                request
                    .status
                    .is_some_and(|status| (200..400).contains(&status))
            })
            .count() as u64;
        let mut ttfb = requests
            .iter()
            .filter_map(|request| request.ttfb_ms)
            .collect::<Vec<_>>();
        let mut duration = requests
            .iter()
            .filter_map(|request| request.duration_ms)
            .collect::<Vec<_>>();
        ttfb.sort_unstable();
        duration.sort_unstable();

        let mut buckets = (0..bucket_count)
            .map(|index| MetricBucket {
                started_at: cutoff + Duration::seconds((index as u64 * bucket_seconds) as i64),
                ..Default::default()
            })
            .collect::<Vec<_>>();
        let mut ttfb_totals = vec![0u64; bucket_count];
        let mut ttfb_counts = vec![0u64; bucket_count];
        let mut bucket_ttfb = vec![Vec::<u64>::new(); bucket_count];
        for request in &requests {
            let elapsed = request
                .started_at
                .signed_duration_since(cutoff)
                .num_seconds()
                .max(0) as u64;
            let index =
                (elapsed / bucket_seconds).min(bucket_count.saturating_sub(1) as u64) as usize;
            buckets[index].requests += 1;
            if !request
                .status
                .is_some_and(|status| (200..400).contains(&status))
            {
                buckets[index].failures += 1;
            }
            if let Some(value) = request.ttfb_ms {
                ttfb_totals[index] += value;
                ttfb_counts[index] += 1;
                bucket_ttfb[index].push(value);
            }
        }
        for (index, bucket) in buckets.iter_mut().enumerate() {
            bucket.average_ttfb_ms =
                (ttfb_counts[index] > 0).then(|| ttfb_totals[index] / ttfb_counts[index]);
            bucket_ttfb[index].sort_unstable();
            bucket.ttfb_p95_ms = percentile(&bucket_ttfb[index], 95);
        }
        Ok(MetricsWindow {
            window_seconds,
            bucket_seconds,
            total_requests,
            successful_requests,
            rps: total_requests as f64 / window_seconds as f64,
            success_rate: if total_requests == 0 {
                100.0
            } else {
                successful_requests as f64 * 100.0 / total_requests as f64
            },
            ttfb_p50_ms: percentile(&ttfb, 50),
            ttfb_p95_ms: percentile(&ttfb, 95),
            ttfb_p99_ms: percentile(&ttfb, 99),
            duration_p50_ms: percentile(&duration, 50),
            duration_p95_ms: percentile(&duration, 95),
            duration_p99_ms: percentile(&duration, 99),
            buckets,
        })
    }
}

fn request_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RequestSummary> {
    let started_at: String = row.get(5)?;
    let account_id: Option<String> = row.get(14)?;
    Ok(RequestSummary {
        id: row.get(0)?,
        tenant_id: row.get(1)?,
        device_id: row.get(2)?,
        client_instance_id: row.get(3)?,
        session_key: row.get(4)?,
        started_at: DateTime::parse_from_rfc3339(&started_at)
            .map(|time| time.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        method: row.get(6)?,
        path: row.get(7)?,
        status: row.get(8)?,
        stage: row.get(9)?,
        duration_ms: row.get(10)?,
        ttfb_ms: row.get(11)?,
        request_bytes: row.get(12)?,
        response_bytes: row.get(13)?,
        account_id: account_id.and_then(|id| Uuid::parse_str(&id).ok()),
        route_reason: row.get(15)?,
        route_message: localized_message(row.get(18)?, row.get(19)?),
        retries: row.get(16)?,
        partial_failure: row.get(17)?,
    })
}

fn insert_account_if_missing(
    transaction: &rusqlite::Transaction<'_>,
    account: &Account,
) -> Result<()> {
    let exists = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=?1)",
            [account.id.to_string()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(db_error)?;
    if !exists {
        upsert_account(transaction, account)?;
    }
    Ok(())
}

fn upsert_account(transaction: &rusqlite::Transaction<'_>, account: &Account) -> Result<()> {
    transaction
        .execute(
            "INSERT INTO accounts
             (id,label,source,imported_at,email,plan,upstream_account_id,tenant_id,proxy_enabled,
              enabled,priority,concurrency_limit,revision)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(id) DO UPDATE SET label=excluded.label,source=excluded.source,
              imported_at=excluded.imported_at,email=excluded.email,plan=excluded.plan,
              upstream_account_id=excluded.upstream_account_id,tenant_id=excluded.tenant_id,
              proxy_enabled=excluded.proxy_enabled,enabled=excluded.enabled,priority=excluded.priority,
              concurrency_limit=excluded.concurrency_limit,revision=excluded.revision",
            params![
                account.id.to_string(),
                account.label,
                account.source,
                account.imported_at.to_rfc3339(),
                account.email,
                account.plan,
                account.account_id,
                account.tenant_id,
                account.proxy_enabled,
                account.enabled,
                account.priority,
                account.concurrency_limit,
                account.revision,
            ],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "INSERT INTO account_runtime(account_id,status,checked_at,detail)
             VALUES (?1,?2,?3,?4) ON CONFLICT(account_id) DO UPDATE SET
             status=excluded.status,checked_at=excluded.checked_at,detail=excluded.detail",
            params![
                account.id.to_string(),
                status_kind_name(&account.status.kind),
                account.status.checked_at.map(|time| time.to_rfc3339()),
                sanitize(&account.status.detail),
            ],
        )
        .map_err(db_error)?;
    transaction
        .execute(
            "DELETE FROM quota_windows WHERE account_id=?1",
            [account.id.to_string()],
        )
        .map_err(db_error)?;
    for (kind, quota) in [
        ("primary", account.status.primary.as_ref()),
        ("secondary", account.status.secondary.as_ref()),
    ] {
        if let Some(quota) = quota {
            transaction
                .execute(
                    "INSERT INTO quota_windows(account_id,kind,used_percent,window_minutes,resets_at)
                     VALUES (?1,?2,?3,?4,?5)",
                    params![
                        account.id.to_string(),
                        kind,
                        quota.used_percent,
                        quota.window_minutes,
                        quota.resets_at,
                    ],
                )
                .map_err(db_error)?;
        }
    }
    Ok(())
}

fn status_kind_name(kind: &StatusKind) -> &'static str {
    match kind {
        StatusKind::Live => "healthy",
        StatusKind::Exhausted => "quota_exhausted",
        StatusKind::Reauth => "reauth_required",
        StatusKind::AccessDenied => "access_denied",
        StatusKind::RateLimited => "rate_limited",
        StatusKind::TemporaryFailure => "temporary_failure",
        StatusKind::Disabled => "disabled",
        StatusKind::Invalid => "invalid",
        StatusKind::Unknown => "unknown",
    }
}

fn status_kind(value: &str) -> StatusKind {
    match value {
        "healthy" => StatusKind::Live,
        "quota_exhausted" => StatusKind::Exhausted,
        "reauth_required" => StatusKind::Reauth,
        "access_denied" => StatusKind::AccessDenied,
        "rate_limited" => StatusKind::RateLimited,
        "temporary_failure" => StatusKind::TemporaryFailure,
        "disabled" => StatusKind::Disabled,
        "invalid" => StatusKind::Invalid,
        _ => StatusKind::Unknown,
    }
}

fn parse_time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn ensure_column(connection: &Connection, table: &str, column: &str, kind: &str) -> Result<()> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(db_error)?;
    let exists = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(db_error)?
        .filter_map(std::result::Result::ok)
        .any(|name| name == column);
    if !exists {
        connection
            .execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {kind}"),
                [],
            )
            .map_err(db_error)?;
    }
    Ok(())
}

fn localized_message(key: Option<String>, args: Option<String>) -> Option<LocalizedMessage> {
    key.map(|key| LocalizedMessage {
        key,
        args: args
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default(),
    })
}

fn percentile(values: &[u64], percentile: usize) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let index = ((values.len() - 1) * percentile).div_ceil(100);
    values.get(index).copied()
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

    fn account(id: Uuid) -> Account {
        Account {
            id,
            label: "account".into(),
            source: "test".into(),
            imported_at: Utc::now(),
            email: Some("account@example.test".into()),
            plan: Some("team".into()),
            account_id: Some("upstream-account".into()),
            status: CheckStatus {
                kind: StatusKind::Live,
                checked_at: Some(Utc::now()),
                detail: "healthy".into(),
                primary: Some(Quota {
                    used_percent: 20.0,
                    window_minutes: Some(300),
                    resets_at: Some(Utc::now().timestamp() + 300),
                }),
                secondary: None,
            },
            tenant_id: "local".into(),
            proxy_enabled: true,
            enabled: true,
            priority: 20,
            concurrency_limit: 2,
            revision: 7,
        }
    }

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
                    message: None,
                })
                .unwrap();
        }
        store.cleanup().unwrap();
        assert_eq!(store.counts().unwrap().1, 1);
    }

    #[test]
    fn metrics_include_percentiles_and_time_buckets() {
        let path =
            std::env::temp_dir().join(format!("codex-switcher-metrics-{}.sqlite", Uuid::new_v4()));
        let store = MetadataStore::open(&path, RetentionPolicy::default()).unwrap();
        for (index, status, ttfb) in [(0, 200, 10), (1, 500, 90), (2, 200, 50)] {
            store
                .record_request(&RequestSummary {
                    id: index.to_string(),
                    tenant_id: "local".into(),
                    device_id: "test".into(),
                    client_instance_id: "instance".into(),
                    session_key: None,
                    started_at: Utc::now(),
                    method: "POST".into(),
                    path: "/responses".into(),
                    status: Some(status),
                    stage: "completed".into(),
                    duration_ms: Some(ttfb + 20),
                    ttfb_ms: Some(ttfb),
                    request_bytes: 0,
                    response_bytes: 0,
                    account_id: None,
                    route_reason: "test".into(),
                    route_message: None,
                    retries: 0,
                    partial_failure: status >= 500,
                })
                .unwrap();
        }
        let metrics = store.metrics(300, 10).unwrap();
        assert_eq!(metrics.total_requests, 3);
        assert_eq!(metrics.successful_requests, 2);
        assert_eq!(metrics.ttfb_p50_ms, Some(50));
        assert_eq!(metrics.ttfb_p95_ms, Some(90));
        assert_eq!(
            metrics.buckets.iter().find_map(|bucket| bucket.ttfb_p95_ms),
            Some(90)
        );
        assert_eq!(
            metrics
                .buckets
                .iter()
                .map(|bucket| bucket.requests)
                .sum::<u64>(),
            3
        );
    }

    #[test]
    fn legacy_database_gains_localized_message_columns_without_losing_rows() {
        let path = std::env::temp_dir().join(format!(
            "codex-switcher-legacy-store-{}.sqlite",
            Uuid::new_v4()
        ));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE request_summaries (
                id TEXT PRIMARY KEY, tenant_id TEXT NOT NULL, device_id TEXT NOT NULL,
                client_instance_id TEXT NOT NULL, session_key TEXT, started_at TEXT NOT NULL,
                method TEXT NOT NULL, path TEXT NOT NULL, status INTEGER, stage TEXT NOT NULL,
                duration_ms INTEGER, ttfb_ms INTEGER, request_bytes INTEGER NOT NULL,
                response_bytes INTEGER NOT NULL, account_id TEXT, route_reason TEXT NOT NULL,
                retries INTEGER NOT NULL, partial_failure INTEGER NOT NULL);
             CREATE TABLE runtime_events (
                id TEXT PRIMARY KEY, occurred_at TEXT NOT NULL, tenant_id TEXT NOT NULL,
                device_id TEXT NOT NULL, client_instance_id TEXT, kind TEXT NOT NULL,
                account_id TEXT, detail TEXT NOT NULL);",
            )
            .unwrap();
        drop(connection);

        let store = MetadataStore::open(&path, RetentionPolicy::default()).unwrap();
        store
            .record_event(&RuntimeEvent {
                id: "localized".into(),
                occurred_at: Utc::now(),
                tenant_id: "local".into(),
                device_id: "test".into(),
                client_instance_id: None,
                kind: "daemon_started".into(),
                account_id: None,
                detail: "legacy fallback".into(),
                message: Some(LocalizedMessage::new("event-daemon-started")),
            })
            .unwrap();
        let events = store.recent_events(10, 0).unwrap();
        assert_eq!(
            events[0].message.as_ref().unwrap().key,
            "event-daemon-started"
        );
    }

    #[test]
    fn account_migration_round_trips_runtime_and_revision() {
        let path = std::env::temp_dir().join(format!(
            "codex-switcher-accounts-store-{}.sqlite",
            Uuid::new_v4()
        ));
        let store = MetadataStore::open(&path, RetentionPolicy::default()).unwrap();
        let id = Uuid::new_v4();
        let legacy = AccountIndex {
            accounts: vec![account(id)],
        };
        let (loaded, initial_revision) = store.reconcile_accounts(&legacy).unwrap();
        assert_eq!(initial_revision, 1);
        assert_eq!(loaded.accounts[0].priority, 20);
        assert_eq!(loaded.accounts[0].concurrency_limit, 2);
        assert_eq!(
            loaded.accounts[0]
                .status
                .primary
                .as_ref()
                .unwrap()
                .used_percent,
            20.0
        );

        let revision = store.replace_accounts(&loaded).unwrap();
        assert_eq!(revision, 2);
        store
            .save_circuit(id, "rate_limited", Some(Utc::now() + Duration::minutes(5)))
            .unwrap();
        store
            .save_binding("session:test", id, Duration::hours(1))
            .unwrap();
        assert_eq!(store.load_circuits().unwrap()[0].account_id, id);
        assert_eq!(store.load_bindings().unwrap()[0].account_id, id);
    }
}
