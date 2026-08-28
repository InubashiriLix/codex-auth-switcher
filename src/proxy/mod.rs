mod auth;
mod connection_tracker;
mod monitor;
mod recommender;
mod routing;
mod server;
mod switcher;

pub use auth::{RefreshOutcome, TokenRefresher};
pub use connection_tracker::{ConnectionTracker, InFlightRequest};
pub use monitor::TokenMonitor;
pub use recommender::Recommender;
pub use routing::{CircuitReason, RouteDecision, RouteError, Router};
pub use server::ProxyServer;
pub use switcher::{AccountSwitcher, SwitchDecision, SwitchReason, SwitchRecord};

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::task::JoinHandle;
use uuid::Uuid;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProxyStats {
    pub total_requests: u64,
    pub failed_requests: u64,
    pub auto_switches: u64,
    pub last_switch: Option<DateTime<Utc>>,
    pub current_account: Option<Uuid>,
    pub upstream_responses: u64,
    pub retries: u64,
    pub http_401: u64,
    pub http_403: u64,
    pub http_429: u64,
    pub http_5xx: u64,
    pub partial_failures: u64,
    pub response_bytes: u64,
    pub last_ttfb_ms: Option<u64>,
}

pub type SharedProxyStats = Arc<RwLock<ProxyStats>>;

#[derive(Clone)]
pub struct ProxyState {
    pub running: Arc<std::sync::atomic::AtomicBool>,
    pub stats: SharedProxyStats,
}

impl ProxyState {
    pub fn new() -> Self {
        Self {
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(ProxyStats::default())),
        }
    }
}

impl Default for ProxyState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    #[default]
    Stopped,
    Starting,
    Running,
    Paused,
    Draining,
    Blocked,
    Error,
}

/// Restartable data-plane supervisor shared by the daemon control API and TUI.
#[derive(Clone)]
pub struct ProxyRuntime {
    server: Arc<ProxyServer>,
    proxy_state: ProxyState,
    state: Arc<RwLock<RuntimeState>>,
    last_error: Arc<RwLock<Option<String>>>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl ProxyRuntime {
    pub fn new(server: Arc<ProxyServer>, proxy_state: ProxyState) -> Self {
        Self {
            server,
            proxy_state,
            state: Arc::new(RwLock::new(RuntimeState::Stopped)),
            last_error: Arc::new(RwLock::new(None)),
            task: Arc::new(Mutex::new(None)),
        }
    }

    pub fn state(&self) -> RuntimeState {
        self.state.read().clone()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.read().clone()
    }

    pub fn mark_blocked(&self, reason: impl Into<String>) {
        *self.last_error.write() = Some(reason.into());
        *self.state.write() = RuntimeState::Blocked;
        self.proxy_state
            .running
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn start(&self) -> bool {
        if matches!(
            self.state(),
            RuntimeState::Starting | RuntimeState::Running | RuntimeState::Paused
        ) {
            return false;
        }
        self.server.router().resume();
        self.server.start_accepting();
        *self.last_error.write() = None;
        *self.state.write() = RuntimeState::Starting;
        let runtime = self.clone();
        let server = self.server.clone();
        let handle = tokio::spawn(async move {
            runtime
                .proxy_state
                .running
                .store(true, std::sync::atomic::Ordering::Relaxed);
            *runtime.state.write() = RuntimeState::Running;
            if let Err(error) = server.serve().await {
                *runtime.last_error.write() = Some(crate::storage::sanitize(&error.to_string()));
                *runtime.state.write() = RuntimeState::Error;
            } else if runtime.state() != RuntimeState::Draining {
                *runtime.state.write() = RuntimeState::Stopped;
            }
            runtime
                .proxy_state
                .running
                .store(false, std::sync::atomic::Ordering::Relaxed);
        });
        *self.task.lock() = Some(handle);
        true
    }

    pub fn pause(&self) {
        self.server.router().pause();
        if self.state() == RuntimeState::Running {
            *self.state.write() = RuntimeState::Paused;
        }
    }

    pub fn resume(&self) {
        self.server.router().resume();
        if self.state() == RuntimeState::Paused {
            *self.state.write() = RuntimeState::Running;
        }
    }

    pub async fn stop(&self, timeout: Duration) -> bool {
        if matches!(self.state(), RuntimeState::Stopped | RuntimeState::Blocked) {
            *self.state.write() = RuntimeState::Stopped;
            return true;
        }
        *self.state.write() = RuntimeState::Draining;
        self.server.stop_accepting().await;
        let tracker = self.server.connection_tracker().clone();
        let drained = tokio::task::spawn_blocking(move || tracker.wait_for_drain(timeout))
            .await
            .unwrap_or(false);
        let handle = self.task.lock().take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
        *self.state.write() = RuntimeState::Stopped;
        self.proxy_state
            .running
            .store(false, std::sync::atomic::Ordering::Relaxed);
        drained
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;
    use crate::{config::Config, types::AccountIndex};
    use parking_lot::RwLock;

    #[tokio::test]
    async fn data_plane_can_stop_and_start_again() {
        let mut config = Config::defaults();
        config.proxy.listen_addr = "127.0.0.1:0".into();
        let accounts = Arc::new(RwLock::new(AccountIndex::default()));
        let current = Arc::new(RwLock::new(None));
        let state = ProxyState::new();
        let server = Arc::new(ProxyServer::new(
            config.clone(),
            config.proxy.clone(),
            accounts,
            current,
            state.stats.clone(),
        ));
        let runtime = ProxyRuntime::new(server, state);

        assert!(runtime.start());
        wait_for_state(&runtime, RuntimeState::Running).await;
        assert!(runtime.stop(Duration::from_secs(1)).await);
        assert_eq!(runtime.state(), RuntimeState::Stopped);

        assert!(runtime.start());
        wait_for_state(&runtime, RuntimeState::Running).await;
        assert!(runtime.stop(Duration::from_secs(1)).await);
        assert_eq!(runtime.state(), RuntimeState::Stopped);
    }

    async fn wait_for_state(runtime: &ProxyRuntime, expected: RuntimeState) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.state() != expected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
}
