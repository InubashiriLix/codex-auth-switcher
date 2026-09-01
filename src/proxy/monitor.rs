use super::Router;
use crate::{
    account::{probe, save_index},
    config::{Config, ProxyConfig},
    paths::Paths,
    storage::MetadataStore,
    types::{AccountIndex, StatusKind},
};
use parking_lot::{Mutex, RwLock};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time;
use tracing::info;
use uuid::Uuid;

pub struct TokenMonitor {
    config: Config,
    accounts: Arc<RwLock<AccountIndex>>,
    current_account: Arc<RwLock<Option<Uuid>>>,
    threshold: f64,
    check_interval: Duration,
    router: Option<Router>,
    store: Option<Arc<MetadataStore>>,
    accounts_revision: Option<Arc<RwLock<u64>>>,
    paths: Option<Paths>,
    probe_schedule: Arc<Mutex<HashMap<Uuid, ProbeSchedule>>>,
}

#[derive(Clone, Debug)]
struct ProbeSchedule {
    next_due: Instant,
    failures: u8,
}

impl TokenMonitor {
    pub fn new(
        config: Config,
        accounts: Arc<RwLock<AccountIndex>>,
        current_account: Arc<RwLock<Option<Uuid>>>,
        proxy_config: &ProxyConfig,
    ) -> Self {
        Self {
            config,
            accounts,
            current_account,
            threshold: proxy_config.threshold,
            // A short scheduler tick lets active accounts stay fresh without
            // blindly probing every pool member on each pass.
            check_interval: Duration::from_secs(2),
            router: None,
            store: None,
            accounts_revision: None,
            paths: None,
            probe_schedule: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn with_router(mut self, router: Router) -> Self {
        self.router = Some(router);
        self
    }

    pub fn with_store(
        mut self,
        store: Arc<MetadataStore>,
        accounts_revision: Arc<RwLock<u64>>,
        paths: Paths,
    ) -> Self {
        self.store = Some(store);
        self.accounts_revision = Some(accounts_revision);
        self.paths = Some(paths);
        self
    }

    pub async fn start_monitoring(self: Arc<Self>) {
        let mut interval = time::interval(self.check_interval);

        loop {
            interval.tick().await;

            // Probe only due, explicitly opted-in accounts. A stale quota
            // probe is never allowed to enter the routing pool.
            let account_data: Vec<_> = self
                .accounts
                .read()
                .accounts
                .iter()
                .filter(|account| account.enabled && account.proxy_enabled)
                .cloned()
                .collect();
            for mut account in account_data {
                let id = account.id;
                if !self.probe_due(&account) {
                    continue;
                }
                let config = self.config.clone();
                let threshold = self.threshold;

                // 在后台线程中探测
                let handle = tokio::task::spawn_blocking(move || {
                    probe(&config, &mut account);

                    // 检查是否需要切换
                    let should_recommend = if let Some(quota) = &account.status.primary {
                        quota.used_percent >= threshold
                    } else {
                        account.status.kind != StatusKind::Live
                    };

                    if should_recommend {
                        info!(
                            "Account {} usage: {:.1}% (threshold: {:.1}%)",
                            account.label,
                            account
                                .status
                                .primary
                                .as_ref()
                                .map(|q| q.used_percent)
                                .unwrap_or(100.0),
                            threshold
                        );
                    }

                    account
                });

                // 等待探测完成并更新账户状态
                if let Ok(updated_account) = handle.await {
                    self.schedule_next(&updated_account);
                    let prior_kind = self
                        .accounts
                        .read()
                        .accounts
                        .iter()
                        .find(|account| account.id == id)
                        .map(|account| account.status.kind.clone());
                    if updated_account.status.kind == StatusKind::Live
                        && let Some(router) = &self.router
                    {
                        router.close_circuit(id);
                    }
                    let committed =
                        merge_probe_result(&mut self.accounts.write(), id, updated_account);
                    if let (Some(store), Some(index)) = (&self.store, committed)
                        && let Ok(revision) = store.replace_accounts(&index)
                    {
                        // Keep the legacy TOML mirror current too. The hot
                        // reloader treats it as an external-edit input.
                        if let Some(paths) = &self.paths
                            && let Err(error) = save_index(paths, &index)
                        {
                            tracing::warn!(%error, "failed to mirror probe result to account index");
                        }
                        if let Some(shared) = &self.accounts_revision {
                            *shared.write() = revision;
                        }
                        if prior_kind.as_ref()
                            != index
                                .accounts
                                .iter()
                                .find(|account| account.id == id)
                                .map(|account| &account.status.kind)
                        {
                            let account = index.accounts.iter().find(|account| account.id == id);
                            let detail = account
                                .map(|account| {
                                    format!(
                                        "探测状态：{:?}；{}",
                                        account.status.kind, account.status.detail
                                    )
                                })
                                .unwrap_or_else(|| "探测状态已更新".into());
                            let _ = store.record_account_event(
                                Some(id),
                                "probe_state_changed",
                                &detail,
                            );
                        }
                    }
                }
            }
        }
    }

    fn probe_due(&self, account: &crate::types::Account) -> bool {
        let now = Instant::now();
        let mut schedules = self.probe_schedule.lock();
        let schedule = schedules.entry(account.id).or_insert(ProbeSchedule {
            next_due: now,
            failures: 0,
        });
        now >= schedule.next_due
    }

    fn schedule_next(&self, account: &crate::types::Account) {
        let now = Instant::now();
        let mut schedules = self.probe_schedule.lock();
        let schedule = schedules.entry(account.id).or_insert(ProbeSchedule {
            next_due: now,
            failures: 0,
        });
        let healthy = account.status.kind == StatusKind::Live;
        schedule.failures = if healthy {
            0
        } else {
            schedule.failures.saturating_add(1).min(6)
        };
        let active = *self.current_account.read() == Some(account.id);
        let near_threshold = account
            .status
            .primary
            .as_ref()
            .is_some_and(|quota| quota.used_percent >= self.threshold - 5.0);
        let seconds = if healthy && (active || near_threshold) {
            5
        } else if healthy {
            15
        } else {
            (5u64.saturating_mul(1u64 << schedule.failures)).min(300)
        };
        // Keep concurrent installs from probing in lock-step while preserving
        // high real-time freshness for healthy accounts.
        let jitter = rand::random_range(0..=seconds.saturating_div(5));
        schedule.next_due = now + Duration::from_secs(seconds + jitter);
    }

    pub fn check_current_usage(&self) -> Option<f64> {
        let current_id = *self.current_account.read();
        let accounts = self.accounts.read();

        current_id.and_then(|id| {
            accounts
                .accounts
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| a.status.primary.as_ref())
                .map(|q| q.used_percent)
        })
    }

    pub fn needs_switch(&self) -> bool {
        self.check_current_usage()
            .map(|usage| usage >= self.threshold)
            .unwrap_or(false)
    }
}

/// A probe is only authoritative for identity and quota fields. Scheduling
/// can change while its network request is in flight, so it stays with the
/// newest local account revision.
fn merge_probe_result(
    index: &mut AccountIndex,
    id: Uuid,
    updated: crate::types::Account,
) -> Option<AccountIndex> {
    let account = index.accounts.iter_mut().find(|account| account.id == id)?;
    account.email = updated.email;
    account.plan = updated.plan;
    account.status = updated.status;
    account.revision = account.revision.saturating_add(1);
    Some(index.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Account, CheckStatus};
    use chrono::Utc;

    #[test]
    fn late_probe_does_not_restore_pool_membership() {
        let id = Uuid::new_v4();
        let mut index = AccountIndex {
            accounts: vec![Account {
                id,
                label: "local".into(),
                source: "test".into(),
                imported_at: Utc::now(),
                email: Some("before@example.test".into()),
                plan: None,
                account_id: None,
                status: CheckStatus::default(),
                tenant_id: "local".into(),
                proxy_enabled: false,
                enabled: false,
                priority: 3,
                concurrency_limit: 4,
                revision: 8,
            }],
        };
        let mut probe_result = index.accounts[0].clone();
        probe_result.email = Some("after@example.test".into());
        probe_result.proxy_enabled = true;
        probe_result.enabled = true;
        probe_result.priority = 100;
        probe_result.concurrency_limit = 0;
        probe_result.status.kind = StatusKind::Live;

        merge_probe_result(&mut index, id, probe_result);
        let account = &index.accounts[0];
        assert_eq!(account.email.as_deref(), Some("after@example.test"));
        assert_eq!(account.status.kind, StatusKind::Live);
        assert!(!account.proxy_enabled);
        assert!(!account.enabled);
        assert_eq!(account.priority, 3);
        assert_eq!(account.concurrency_limit, 4);
        assert_eq!(account.revision, 9);
    }
}
