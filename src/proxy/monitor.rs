use super::Router;
use crate::{
    account::probe,
    config::{Config, ProxyConfig},
    types::{AccountIndex, StatusKind},
};
use parking_lot::RwLock;
use std::{sync::Arc, time::Duration};
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
            check_interval: Duration::from_secs(30), // 每30秒检查一次
            router: None,
        }
    }

    pub fn with_router(mut self, router: Router) -> Self {
        self.router = Some(router);
        self
    }

    pub async fn start_monitoring(self: Arc<Self>) {
        let mut interval = time::interval(self.check_interval);

        loop {
            interval.tick().await;

            // Probe every explicitly opted-in account. A stale quota probe is
            // never allowed to enter the routing pool.
            let account_data: Vec<_> = self
                .accounts
                .read()
                .accounts
                .iter()
                .filter(|account| account.proxy_enabled)
                .cloned()
                .collect();
            for mut account in account_data {
                let id = account.id;
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
                    if updated_account.status.kind == StatusKind::Live
                        && let Some(router) = &self.router
                    {
                        router.close_circuit(id);
                    }
                    let mut accounts = self.accounts.write();
                    if let Some(acc) = accounts.accounts.iter_mut().find(|a| a.id == id) {
                        *acc = updated_account;
                    }
                }
            }
        }
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
