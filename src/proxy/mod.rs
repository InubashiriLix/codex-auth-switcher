mod server;
mod monitor;
mod recommender;
mod connection_tracker;
mod switcher;

pub use server::ProxyServer;
pub use monitor::TokenMonitor;
pub use recommender::Recommender;
pub use connection_tracker::ConnectionTracker;
pub use switcher::{AccountSwitcher, SwitchDecision, SwitchReason, SwitchRecord};

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Debug, Default)]
pub struct ProxyStats {
    pub total_requests: u64,
    pub failed_requests: u64,
    pub auto_switches: u64,
    pub last_switch: Option<DateTime<Utc>>,
    pub current_account: Option<Uuid>,
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
