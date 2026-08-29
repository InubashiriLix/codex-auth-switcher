//! Metadata-only SQLite persistence. Request/response bodies and credentials
//! are deliberately absent from this schema.

use crate::{
    error::{AppError, Result},
    i18n::LocalizedMessage,
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
}
