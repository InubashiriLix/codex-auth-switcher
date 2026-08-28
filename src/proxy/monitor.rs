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
        }
    }

    pub async fn start_monitoring(self: Arc<Self>) {
        let mut interval = time::interval(self.check_interval);

        loop {
            interval.tick().await;

            // 先获取current_id，然后释放锁
            let current_id = *self.current_account.read();

            if let Some(id) = current_id {
                // 克隆账户数据（在锁外）
                let account_data = {
                    let accounts = self.accounts.read();
                    accounts.accounts.iter().find(|a| a.id == id).cloned()
                };

                if let Some(mut account) = account_data {
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
                        let mut accounts = self.accounts.write();
                        if let Some(acc) = accounts.accounts.iter_mut().find(|a| a.id == id) {
                            *acc = updated_account;
                        }
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
