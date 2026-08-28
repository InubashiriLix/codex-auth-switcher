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
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
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
