use crate::{
    config::{ProxyConfig, RecommendStrategy},
    types::{Account, AccountIndex, StatusKind},
};
use chrono::{DateTime, Duration, Utc};
use parking_lot::{Mutex, RwLock};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CircuitReason {
    Unauthorized,
    Forbidden,
    RateLimited,
    Reauth,
}

#[derive(Clone, Debug)]
struct Circuit {
    reason: CircuitReason,
    until: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct RouteDecision {
    pub account_id: Uuid,
    pub reason: String,
    pub sticky: bool,
}

#[derive(Clone, Debug)]
pub struct RouteError {
    pub reason: String,
    pub retry_after_seconds: Option<u64>,
    pub earliest_recovery: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct Router {
    config: Arc<RwLock<ProxyConfig>>,
    bindings: Arc<Mutex<HashMap<String, Uuid>>>,
    circuits: Arc<Mutex<HashMap<Uuid, Circuit>>>,
    round_robin_cursor: Arc<Mutex<usize>>,
    paused: Arc<AtomicBool>,
    preferred: Arc<RwLock<Option<Uuid>>>,
}

impl Router {
    pub fn new(config: ProxyConfig) -> Self {
        Self {
            config: Arc::new(RwLock::new(config)),
            bindings: Arc::new(Mutex::new(HashMap::new())),
            circuits: Arc::new(Mutex::new(HashMap::new())),
            round_robin_cursor: Arc::new(Mutex::new(0)),
            paused: Arc::new(AtomicBool::new(false)),
            preferred: Arc::new(RwLock::new(None)),
        }
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }
    pub fn auto_switch_enabled(&self) -> bool {
        self.config.read().auto_switch
    }
    pub fn update_config(&self, config: ProxyConfig) {
        *self.config.write() = config;
    }
    pub fn prefer(&self, account_id: Uuid) {
        *self.preferred.write() = Some(account_id);
        self.bindings.lock().clear();
    }

    pub fn route(
        &self,
        accounts: &AccountIndex,
        sticky_key: &str,
    ) -> std::result::Result<RouteDecision, RouteError> {
        if self.is_paused() {
            return Err(RouteError {
                reason: "自动路由已暂停".into(),
                retry_after_seconds: Some(1),
                earliest_recovery: None,
            });
        }
        self.expire_circuits();
        let config = self.config.read().clone();

        if let Some(bound) = self.bindings.lock().get(sticky_key).copied() {
            if accounts
                .accounts
                .iter()
                .find(|a| a.id == bound)
                .is_some_and(|a| self.eligible(a, true, config.threshold))
            {
                return Ok(RouteDecision {
                    account_id: bound,
                    reason: "会话/进程粘性".into(),
                    sticky: true,
                });
            }
            if !config.auto_switch {
                let mut error = self.unavailable_error(accounts);
                error.reason = "当前粘性账户不可用，自动切换尚未启用".into();
                return Err(error);
            }
        }

        let mut eligible: Vec<&Account> = accounts
            .accounts
            .iter()
            .filter(|account| self.eligible(account, false, config.threshold))
            .collect();
        if eligible.is_empty() {
            return Err(self.unavailable_error(accounts));
        }
        eligible.sort_by_key(|account| account.id);

        if let Some(preferred) = *self.preferred.read()
            && let Some(account) = eligible.iter().find(|account| account.id == preferred)
        {
            self.bindings
                .lock()
                .insert(sticky_key.to_owned(), account.id);
            return Ok(RouteDecision {
                account_id: account.id,
                reason: "用户手动选择".into(),
                sticky: false,
            });
        }

        let selected = match config.strategy {
            RecommendStrategy::Smart => eligible
                .into_iter()
                .max_by(|a, b| smart_score(a).total_cmp(&smart_score(b))),
            RecommendStrategy::MaxRemaining => eligible
                .into_iter()
                .max_by(|a, b| remaining(a).total_cmp(&remaining(b))),
            RecommendStrategy::RoundRobin => {
                let mut cursor = self.round_robin_cursor.lock();
                let account = eligible[*cursor % eligible.len()];
                *cursor = (*cursor + 1) % eligible.len();
                Some(account)
            }
        }
        .expect("eligible list is non-empty");

        self.bindings
            .lock()
            .insert(sticky_key.to_owned(), selected.id);
        Ok(RouteDecision {
            account_id: selected.id,
            reason: format!("{:?} 策略", config.strategy),
            sticky: false,
        })
    }

    pub fn unbind(&self, sticky_key: &str) {
        self.bindings.lock().remove(sticky_key);
    }

    pub fn binding_for(&self, sticky_key: &str) -> Option<Uuid> {
        self.bindings.lock().get(sticky_key).copied()
    }

    pub fn binding_counts(&self) -> HashMap<Uuid, usize> {
        let mut counts = HashMap::new();
        for account_id in self.bindings.lock().values() {
            *counts.entry(*account_id).or_insert(0) += 1;
        }
        counts
    }

    pub fn open_circuit(
        &self,
        account_id: Uuid,
        reason: CircuitReason,
        until: Option<DateTime<Utc>>,
    ) {
        self.circuits
            .lock()
            .insert(account_id, Circuit { reason, until });
    }

    pub fn close_circuit(&self, account_id: Uuid) {
        self.circuits.lock().remove(&account_id);
    }

    pub fn circuit_reason(&self, account_id: Uuid) -> Option<CircuitReason> {
        self.expire_circuits();
        self.circuits
            .lock()
            .get(&account_id)
            .map(|c| c.reason.clone())
    }

    fn eligible(&self, account: &Account, existing_binding: bool, threshold: f64) -> bool {
        if !account.proxy_enabled
            || account.tenant_id != "local"
            || account.status.kind != StatusKind::Live
        {
            return false;
        }
        if self.circuits.lock().contains_key(&account.id) {
            return false;
        }
        let fresh = account
            .status
            .checked_at
            .is_some_and(|checked| Utc::now() - checked <= Duration::seconds(90));
        if !fresh {
            return false;
        }
        existing_binding
            || account
                .status
                .primary
                .as_ref()
                .is_some_and(|quota| quota.used_percent < threshold)
    }

    fn expire_circuits(&self) {
        let now = Utc::now();
        self.circuits
            .lock()
            .retain(|_, circuit| circuit.until.is_none_or(|until| until > now));
    }

    fn unavailable_error(&self, accounts: &AccountIndex) -> RouteError {
        let quota_recovery = accounts
            .accounts
            .iter()
            .filter(|a| a.proxy_enabled)
            .flat_map(|a| [a.status.primary.as_ref(), a.status.secondary.as_ref()])
            .flatten()
            .filter_map(|q| q.resets_at)
            .filter_map(|timestamp| DateTime::from_timestamp(timestamp, 0))
            .filter(|time| *time > Utc::now())
            .min();
        let circuit_recovery = self.circuits.lock().values().filter_map(|c| c.until).min();
        let earliest = [quota_recovery, circuit_recovery]
            .into_iter()
            .flatten()
            .min();
        let retry_after_seconds = Some(
            earliest
                .map(|time| (time - Utc::now()).num_seconds().max(1) as u64)
                .unwrap_or(30),
        );
        let reason = if !accounts.accounts.iter().any(|a| a.proxy_enabled) {
            "代理池为空；请先明确将账户加入代理池"
        } else {
            "没有认证有效、额度新鲜且未熔断的账户"
        };
        RouteError {
            reason: reason.into(),
            retry_after_seconds,
            earliest_recovery: earliest,
        }
    }
}

fn remaining(account: &Account) -> f64 {
    [
        account.status.primary.as_ref(),
        account.status.secondary.as_ref(),
    ]
    .into_iter()
    .flatten()
    .map(|q| 100.0 - q.used_percent)
    .fold(0.0, f64::max)
}

fn smart_score(account: &Account) -> f64 {
    let reset = [
        account.status.primary.as_ref(),
        account.status.secondary.as_ref(),
    ]
    .into_iter()
    .flatten()
    .filter_map(|q| q.resets_at)
    .min()
    .unwrap_or(i64::MAX);
    let reset_bonus = if reset - Utc::now().timestamp() <= 1800 {
        5.0
    } else {
        0.0
    };
    remaining(account) + reset_bonus
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CheckStatus, Quota};

    fn account(id: u128, used: f64, enabled: bool) -> Account {
        Account {
            id: Uuid::from_u128(id),
            label: id.to_string(),
            source: "test".into(),
            imported_at: Utc::now(),
            email: None,
            plan: None,
            account_id: None,
            status: CheckStatus {
                kind: StatusKind::Live,
                checked_at: Some(Utc::now()),
                detail: "ok".into(),
                primary: Some(Quota {
                    used_percent: used,
                    window_minutes: Some(300),
                    resets_at: None,
                }),
                secondary: None,
            },
            tenant_id: "local".into(),
            proxy_enabled: enabled,
        }
    }

    #[test]
    fn pool_is_opt_in_and_binding_survives_threshold() {
        let router = Router::new(ProxyConfig::default());
        let mut index = AccountIndex {
            accounts: vec![account(1, 20.0, false), account(2, 30.0, true)],
        };
        assert_eq!(
            router.route(&index, "p1").unwrap().account_id,
            Uuid::from_u128(2)
        );
        index.accounts[1]
            .status
            .primary
            .as_mut()
            .unwrap()
            .used_percent = 90.0;
        assert_eq!(
            router.route(&index, "p1").unwrap().account_id,
            Uuid::from_u128(2)
        );
        assert!(router.route(&index, "p2").is_err());
    }

    #[test]
    fn circuit_breaker_removes_sticky_binding() {
        let config = ProxyConfig {
            auto_switch: true,
            ..ProxyConfig::default()
        };
        let router = Router::new(config);
        let index = AccountIndex {
            accounts: vec![account(1, 20.0, true), account(2, 30.0, true)],
        };
        let first = router.route(&index, "p1").unwrap().account_id;
        router.open_circuit(first, CircuitReason::Forbidden, None);
        assert_ne!(router.route(&index, "p1").unwrap().account_id, first);
    }
}
