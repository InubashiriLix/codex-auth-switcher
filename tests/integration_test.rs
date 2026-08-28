use codex_switcher::{
    config::{Config, ProxyConfig, RecommendStrategy},
    proxy::Recommender,
    types::{Account, CheckStatus, StatusKind, Quota},
};
use chrono::Utc;
use uuid::Uuid;

fn create_test_account(label: &str, used_percent: f64, status_kind: StatusKind) -> Account {
    Account {
        id: Uuid::new_v4(),
        label: label.to_string(),
        source: "test".to_string(),
        imported_at: Utc::now(),
        email: Some(format!("{}@test.com", label)),
        plan: Some("test".to_string()),
        account_id: None,
        status: CheckStatus {
            kind: status_kind,
            detail: "test".to_string(),
            checked_at: Some(Utc::now()),
            primary: Some(Quota {
                used_percent,
                window_minutes: Some(60),
                resets_at: Some(Utc::now().timestamp() + 3600),
            }),
            secondary: None,
        },
    }
}

#[test]
fn test_recommender_smart_strategy() {
    let recommender = Recommender::new(RecommendStrategy::Smart);

    let accounts = vec![
        create_test_account("account1", 90.0, StatusKind::Live),
        create_test_account("account2", 50.0, StatusKind::Live),
        create_test_account("account3", 95.0, StatusKind::Exhausted),
    ];

    let result = recommender.recommend(&accounts, 85.0);
    assert!(result.is_some());

    // 应该推荐account2（使用率最低且可用）
    let recommended = accounts.iter().find(|a| Some(a.id) == result).unwrap();
    assert_eq!(recommended.label, "account2");
}

#[test]
fn test_recommender_max_remaining_strategy() {
    let recommender = Recommender::new(RecommendStrategy::MaxRemaining);

    let accounts = vec![
        create_test_account("account1", 80.0, StatusKind::Live),
        create_test_account("account2", 30.0, StatusKind::Live),
        create_test_account("account3", 60.0, StatusKind::Live),
    ];

    let result = recommender.recommend(&accounts, 85.0);
    assert!(result.is_some());

    let recommended = accounts.iter().find(|a| Some(a.id) == result).unwrap();
    assert_eq!(recommended.label, "account2");
}

#[test]
fn test_recommender_filters_unavailable_accounts() {
    let recommender = Recommender::new(RecommendStrategy::Smart);

    let accounts = vec![
        create_test_account("account1", 95.0, StatusKind::Exhausted),
        create_test_account("account2", 90.0, StatusKind::Reauth),
        create_test_account("account3", 40.0, StatusKind::Live),
    ];

    let result = recommender.recommend(&accounts, 85.0);
    assert!(result.is_some());

    let recommended = accounts.iter().find(|a| Some(a.id) == result).unwrap();
    assert_eq!(recommended.label, "account3");
}

#[test]
fn test_recommender_no_available_accounts() {
    let recommender = Recommender::new(RecommendStrategy::Smart);

    let accounts = vec![
        create_test_account("account1", 95.0, StatusKind::Exhausted),
        create_test_account("account2", 90.0, StatusKind::Reauth),
    ];

    let result = recommender.recommend(&accounts, 85.0);
    assert!(result.is_none());
}

#[test]
fn test_config_default_values() {
    let config = ProxyConfig::default();

    assert_eq!(config.listen_addr, "127.0.0.1:8765");
    assert_eq!(config.threshold, 85.0);
    assert_eq!(config.cooldown_seconds, 5);
    assert_eq!(config.strategy, RecommendStrategy::Smart);
}

#[test]
fn test_config_serialization() {
    let config = ProxyConfig {
        enabled: true,
        listen_addr: "127.0.0.1:9999".to_string(),
        auto_switch: true,
        threshold: 90.0,
        cooldown_seconds: 10,
        strategy: RecommendStrategy::MaxRemaining,
        target_base: "https://example.com".to_string(),
    };

    let toml_str = toml::to_string(&config).unwrap();
    let deserialized: ProxyConfig = toml::from_str(&toml_str).unwrap();

    assert_eq!(deserialized.listen_addr, config.listen_addr);
    assert_eq!(deserialized.threshold, config.threshold);
    assert_eq!(deserialized.strategy, config.strategy);
}
