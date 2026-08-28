use parking_lot::Mutex;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct RequestMetadata {
    pub started_at: Instant,
    pub method: String,
    pub path: String,
}

#[derive(Clone)]
pub struct ConnectionTracker {
    active: Arc<AtomicUsize>,
    requests_in_flight: Arc<Mutex<HashMap<String, RequestMetadata>>>,
}

impl ConnectionTracker {
    pub fn new() -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            requests_in_flight: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn track_request(&self, req_id: String, metadata: RequestMetadata) {
        self.active.fetch_add(1, Ordering::Relaxed);
        self.requests_in_flight.lock().insert(req_id, metadata);
    }

    pub fn complete_request(&self, req_id: &str) {
        self.active.fetch_sub(1, Ordering::Relaxed);
        self.requests_in_flight.lock().remove(req_id);
    }

    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    pub fn wait_for_drain(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            if self.active.load(Ordering::Relaxed) == 0 {
                return true;
            }
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

pub struct RequestGuard {
    req_id: String,
    tracker: ConnectionTracker,
}

impl RequestGuard {
    pub fn new(req_id: String, tracker: ConnectionTracker) -> Self {
        Self { req_id, tracker }
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.tracker.complete_request(&self.req_id);
    }
}
