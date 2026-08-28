use crate::{
    config::RecommendStrategy,
    types::{Account, StatusKind},
};
use chrono::Utc;
use parking_lot::Mutex;
use std::sync::Arc;
use uuid::Uuid;

pub struct Recommender {
    strategy: RecommendStrategy,
    last_used: Arc<Mutex<Vec<Uuid>>>,
}

impl Recommender {
    pub fn new(strategy: RecommendStrategy) -> Self {
        Self {
            strategy,
            last_used: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn recommend(&self, accounts: &[Account], threshold: f64) -> Option<Uuid> {
        let eligible: Vec<Account> = accounts
            .iter()
            .filter(|account| account.proxy_enabled && account.status.kind == StatusKind::Live)
            .filter(|account| {
                [
                    account.status.primary.as_ref(),
                    account.status.secondary.as_ref(),
                ]
                .into_iter()
                .flatten()
                .any(|quota| quota.used_percent < threshold)
            })
            .cloned()
            .collect();
        match self.strategy {
            RecommendStrategy::Smart => self.smart_recommend(&eligible, threshold),
            RecommendStrategy::MaxRemaining => self.max_remaining(&eligible),
            RecommendStrategy::RoundRobin => self.round_robin(&eligible),
        }
    }

    fn smart_recommend(&self, accounts: &[Account], threshold: f64) -> Option<Uuid> {
        accounts
            .iter()
            .filter(|a| a.status.kind == StatusKind::Live)
            .filter(|a| {
                a.status
                    .primary
                    .as_ref()
                    .map(|q| q.used_percent < threshold)
                    .unwrap_or(false)
            })
            .max_by_key(|a| {
                // 综合评分：剩余百分比 + 即将重置的加分
                let remaining = a
                    .status
                    .primary
                    .as_ref()
                    .into_iter()
                    .chain(a.status.secondary.as_ref())
                    .map(|q| 100.0 - q.used_percent)
                    .fold(0.0, f64::max);

                let reset_bonus = a
                    .status
                    .primary
                    .as_ref()
                    .and_then(|q| q.resets_at)
                    .map(|t| {
                        let now = Utc::now().timestamp();
                        let minutes_until_reset = (t - now) / 60;
                        // 如果30分钟内重置，给予20分的加成
                        if minutes_until_reset < 30 && minutes_until_reset > 0 {
                            20.0
                        } else {
                            0.0
                        }
                    })
                    .unwrap_or(0.0);

                ((remaining + reset_bonus) * 100.0) as i64
            })
            .map(|a| a.id)
    }

    fn max_remaining(&self, accounts: &[Account]) -> Option<Uuid> {
        accounts
            .iter()
            .filter(|a| a.status.kind == StatusKind::Live)
            .max_by_key(|a| {
                a.status
                    .primary
                    .as_ref()
                    .into_iter()
                    .chain(a.status.secondary.as_ref())
                    .map(|q| ((100.0 - q.used_percent) * 100.0) as i64)
                    .max()
                    .unwrap_or(0)
            })
            .map(|a| a.id)
    }

    fn round_robin(&self, accounts: &[Account]) -> Option<Uuid> {
        let available: Vec<_> = accounts
            .iter()
            .filter(|a| a.status.kind == StatusKind::Live)
            .collect();

        if available.is_empty() {
            return None;
        }

        let mut last_used = self.last_used.lock();

        // 找到上次使用的账户
        if let Some(last_id) = last_used.last()
            && let Some(pos) = available.iter().position(|a| &a.id == last_id)
        {
            // 返回下一个
            let next_idx = (pos + 1) % available.len();
            let next_id = available[next_idx].id;
            last_used.push(next_id);
            // 只保留最近10个记录
            if last_used.len() > 10 {
                last_used.remove(0);
            }
            return Some(next_id);
        }

        // 如果没有历史记录，返回第一个
        let first_id = available[0].id;
        last_used.push(first_id);
        Some(first_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CheckStatus, Quota};
    use chrono::Utc;

    fn make_test_account(id: u128, used_percent: f64) -> Account {
        Account {
            id: Uuid::from_u128(id),
            label: format!("Account {}", id),
            source: "test".into(),
            imported_at: Utc::now(),
            email: None,
            plan: None,
            account_id: None,
            tenant_id: "local".into(),
            proxy_enabled: true,
            status: CheckStatus {
                kind: StatusKind::Live,
                checked_at: Some(Utc::now()),
                detail: "Test".into(),
                primary: Some(Quota {
                    used_percent,
                    window_minutes: Some(60),
                    resets_at: None,
                }),
                secondary: None,
            },
        }
    }

    #[test]
    fn test_max_remaining() {
        let recommender = Recommender::new(RecommendStrategy::MaxRemaining);
        let accounts = vec![
            make_test_account(1, 90.0),
            make_test_account(2, 20.0),
            make_test_account(3, 50.0),
        ];

        let recommended = recommender.recommend(&accounts, 85.0);
        assert_eq!(recommended, Some(Uuid::from_u128(2)));
    }

    #[test]
    fn test_round_robin() {
        let recommender = Recommender::new(RecommendStrategy::RoundRobin);
        let accounts = vec![
            make_test_account(1, 50.0),
            make_test_account(2, 50.0),
            make_test_account(3, 50.0),
        ];

        let first = recommender.recommend(&accounts, 85.0);
        let second = recommender.recommend(&accounts, 85.0);
        let third = recommender.recommend(&accounts, 85.0);

        assert_eq!(first, Some(Uuid::from_u128(1)));
        assert_eq!(second, Some(Uuid::from_u128(2)));
        assert_eq!(third, Some(Uuid::from_u128(3)));
    }
}
