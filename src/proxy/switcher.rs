use crate::{
    config::ProxyConfig,
    daemon::DaemonState,
    error::*,
    types::{Account, StatusKind},
};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::{
    collections::VecDeque,
    sync::Arc,
    time::Instant,
};
use tracing::{info, warn};
use uuid::Uuid;

use super::Recommender;

#[derive(Clone, Debug)]
pub struct SwitchRecord {
    pub timestamp: DateTime<Utc>,
    pub from_account: Uuid,
    pub to_account: Uuid,
    pub reason: SwitchReason,
    pub success: bool,
}

#[derive(Clone, Debug)]
pub enum SwitchReason {
    TokenExhausted,
    Manual,
    AccountUnavailable,
    ConfigReload,
}

pub enum SwitchDecision {
    Switch { target: Uuid, reason: String },
    Wait { reason: String },
    NoAction,
}

pub struct AccountSwitcher {
    config: ProxyConfig,
    recommender: Recommender,
    last_switch: Arc<RwLock<Option<Instant>>>,
    switch_history: Arc<RwLock<VecDeque<SwitchRecord>>>,
}

impl AccountSwitcher {
    pub fn new(config: ProxyConfig, recommender: Recommender) -> Self {
        Self {
            config,
            recommender,
            last_switch: Arc::new(RwLock::new(None)),
            switch_history: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    /// 检查是否需要切换并返回决策
    pub fn check_and_switch(&self, state: &DaemonState) -> Result<SwitchDecision> {
        // 1. 检查冷却期
        if let Some(last) = *self.last_switch.read() {
            let cooldown = std::time::Duration::from_secs(self.config.cooldown_seconds);
            if last.elapsed() < cooldown {
                return Ok(SwitchDecision::Wait {
                    reason: format!("冷却中，剩余 {:?}", cooldown - last.elapsed()),
                });
            }
        }

        // 2. 获取当前账户（立即释放锁）
        let current_id = {
            let guard = state.current_account.read();
            *guard
        };

        if current_id.is_none() {
            return Ok(SwitchDecision::NoAction);
        }

        let current_id = current_id.unwrap();

        // 3. 检查当前账户状态（立即释放锁）
        let (current_used_percent, current_status_kind) = {
            let accounts = state.accounts.read();
            let current = accounts.accounts.iter().find(|a| a.id == current_id);

            if current.is_none() {
                drop(accounts);
                // 当前账户不存在，获取可用账户列表
                let accounts = state.accounts.read();
                let accounts_clone = accounts.accounts.clone();
                drop(accounts);
                return self.recommend_switch(&accounts_clone, current_id, "当前账户不存在");
            }

            let current = current.unwrap();
            let used_percent = current.status.primary.as_ref().map(|q| q.used_percent);
            let status_kind = current.status.kind.clone();
            (used_percent, status_kind)
        };

        // 4. 检查使用率
        if let Some(percent) = current_used_percent {
            if percent >= self.config.threshold {
                let accounts = state.accounts.read();
                let accounts_clone = accounts.accounts.clone();
                drop(accounts);
                return self.recommend_switch(
                    &accounts_clone,
                    current_id,
                    &format!("使用率达到 {:.1}%", percent),
                );
            }
        }

        // 5. 检查账户状态
        if current_status_kind != StatusKind::Live {
            let accounts = state.accounts.read();
            let accounts_clone = accounts.accounts.clone();
            drop(accounts);
            return self.recommend_switch(
                &accounts_clone,
                current_id,
                &format!("账户状态: {:?}", current_status_kind),
            );
        }

        Ok(SwitchDecision::NoAction)
    }

    fn recommend_switch(
        &self,
        accounts: &[Account],
        current_id: Uuid,
        reason: &str,
    ) -> Result<SwitchDecision> {
        // 推荐下一个账户（排除当前账户）
        let available: Vec<_> = accounts.iter().filter(|a| a.id != current_id).cloned().collect();

        if let Some(target_id) = self.recommender.recommend(&available, self.config.threshold) {
            Ok(SwitchDecision::Switch {
                target: target_id,
                reason: reason.to_string(),
            })
        } else {
            Ok(SwitchDecision::Wait {
                reason: "没有可用的账户".into(),
            })
        }
    }

    /// 执行切换
    pub async fn execute_switch(&self, state: &mut DaemonState, target: Uuid) -> Result<()> {
        let old_id = *state.current_account.read();

        info!("开始切换账户: {:?} → {}", old_id, target);

        // 1. 验证目标账户（立即释放锁）
        let target_status = {
            let accounts = state.accounts.read();
            let target_account = accounts
                .accounts
                .iter()
                .find(|a| a.id == target)
                .ok_or_else(|| AppError::Message("目标账户不存在".into()))?;

            let status = target_account.status.kind.clone();
            status
        };

        if target_status != StatusKind::Live {
            return Err(AppError::Message(format!("目标账户不可用: {:?}", target_status)));
        }

        // 2. 等待活跃请求完成
        let cooldown = std::time::Duration::from_secs(self.config.cooldown_seconds);
        info!("等待活跃请求完成...");
        let drained = state
            .proxy_server
            .connection_tracker()
            .wait_for_drain(cooldown);

        if !drained {
            warn!(
                "部分请求未完成，活跃连接数: {}",
                state.proxy_server.connection_tracker().active_count()
            );
        }

        // 3. 执行切换
        state.switch_account(target).await?;

        // 4. 更新记录
        *self.last_switch.write() = Some(Instant::now());

        if let Some(from) = old_id {
            self.record_switch(SwitchRecord {
                timestamp: Utc::now(),
                from_account: from,
                to_account: target,
                reason: SwitchReason::TokenExhausted,
                success: true,
            });
        }

        info!("账户切换成功");
        Ok(())
    }

    /// 记录切换历史
    pub fn record_switch(&self, record: SwitchRecord) {
        let mut history = self.switch_history.write();
        history.push_back(record);
        if history.len() > 100 {
            history.pop_front();
        }
    }

    /// 获取切换历史
    pub fn get_history(&self) -> Vec<SwitchRecord> {
        self.switch_history.read().iter().cloned().collect()
    }

    /// 回滚到上一个账户
    pub async fn rollback(&self, state: &mut DaemonState) -> Result<()> {
        let history = self.switch_history.read();
        if let Some(last) = history.back() {
            let from = last.from_account;
            drop(history);
            info!("回滚到上一个账户: {}", from);
            self.execute_switch(state, from).await?;
        } else {
            warn!("没有可回滚的历史记录");
        }
        Ok(())
    }
}
