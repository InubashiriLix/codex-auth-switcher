use crate::{
    account::{activate, import_current, import_file, import_value, probe, save_index},
    config::{Config, save_config},
    error::*,
    i18n::{translate, translate_with},
    paths::Paths,
    types::{Account, AccountIndex},
};
use futures_util::StreamExt;
use std::{
    path::Path,
    sync::{Arc, atomic::Ordering, mpsc},
    thread,
    time::Duration,
};

use super::{ActionUpdate, Checking, ControlUpdate, ProbeEvent, Ui};

/// 导入当前Codex认证
pub fn import_current_auth(
    config: &Config,
    index: &mut AccountIndex,
    paths: &Paths,
) -> Result<String> {
    import_current(config, index)?;
    save_index(paths, index)?;
    Ok(translate(
        config.language.resolve(),
        "notice-imported-current",
        None,
    ))
}

/// 从文件路径导入
pub fn import_from_path(
    config: &Config,
    index: &mut AccountIndex,
    paths: &Paths,
    path: &str,
) -> Result<String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(AppError::Message(translate_with(
            config.language.resolve(),
            "error-file-missing",
            [("path", path)],
        )));
    }

    import_file(config, index, p)?;
    save_index(paths, index)?;
    Ok(translate_with(
        config.language.resolve(),
        "notice-imported-path",
        [("path", path)],
    ))
}

/// 从JSON字符串导入
pub fn import_from_json(
    config: &Config,
    index: &mut AccountIndex,
    paths: &Paths,
    json_str: &str,
    name: Option<String>,
) -> Result<String> {
    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        AppError::Message(translate_with(
            config.language.resolve(),
            "error-json",
            [("error", e.to_string())],
        ))
    })?;

    import_value(config, index, value, "手动输入".into(), name)?;
    save_index(paths, index)?;
    Ok(translate(
        config.language.resolve(),
        "notice-imported-json",
        None,
    ))
}

/// 激活账户
pub fn activate_account(config: &Config, account: &Account) -> Result<String> {
    activate(config, account)?;
    Ok(translate_with(
        config.language.resolve(),
        "notice-activated",
        [("account", account.label.as_str())],
    ))
}

/// 重命名账户
pub fn rename_account(
    config: &Config,
    index: &mut AccountIndex,
    paths: &Paths,
    account_idx: usize,
    new_name: String,
) -> Result<String> {
    if let Some(account) = index.accounts.get_mut(account_idx) {
        let old_name = account.label.clone();
        account.label = new_name.clone();
        save_index(paths, index)?;
        Ok(translate_with(
            config.language.resolve(),
            "notice-renamed",
            [("from", old_name), ("to", new_name)],
        ))
    } else {
        Err(AppError::Message(translate(
            config.language.resolve(),
            "error-account-missing",
            None,
        )))
    }
}

/// 删除账户
pub fn delete_account(
    config: &Config,
    index: &mut AccountIndex,
    paths: &Paths,
    account_idx: usize,
) -> Result<String> {
    if account_idx < index.accounts.len() {
        let account = index.accounts.remove(account_idx);

        // 删除快照文件
        let snapshot_path = crate::account::snapshot_path(config, account.id);
        if snapshot_path.exists() {
            std::fs::remove_file(snapshot_path)?;
        }

        save_index(paths, index)?;
        Ok(translate_with(
            config.language.resolve(),
            "notice-deleted",
            [("account", account.label)],
        ))
    } else {
        Err(AppError::Message(translate(
            config.language.resolve(),
            "error-account-missing",
            None,
        )))
    }
}

/// 检测单个账户
pub fn probe_account(
    config: &Config,
    index: &mut AccountIndex,
    paths: &Paths,
    account_idx: usize,
) -> Result<String> {
    if let Some(account) = index.accounts.get_mut(account_idx) {
        let label = account.label.clone();
        probe(config, account);
        save_index(paths, index)?;
        Ok(translate_with(
            config.language.resolve(),
            "notice-checked",
            [("account", label)],
        ))
    } else {
        Err(AppError::Message(translate(
            config.language.resolve(),
            "error-account-missing",
            None,
        )))
    }
}

/// 检测所有账户
pub fn probe_all_accounts(
    config: &Config,
    index: &mut AccountIndex,
    paths: &Paths,
) -> Result<String> {
    let count = index.accounts.len();
    for account in &mut index.accounts {
        probe(config, account);
    }
    save_index(paths, index)?;
    Ok(translate_with(
        config.language.resolve(),
        "notice-checked-all",
        [("count", count)],
    ))
}

/// 保存配置
pub fn save_current_config(paths: &Paths, config: &Config) -> Result<String> {
    save_config(paths, config)?;
    Ok(translate(config.language.resolve(), "config-saved", None))
}

/// 启动后台检测（非阻塞）
pub fn start_probe(ui: &mut Ui, accounts: Vec<Account>) {
    let total = accounts.len();
    let (sender, receiver) = mpsc::channel();
    let config = ui.config.clone();

    thread::spawn(move || {
        for mut account in accounts {
            if sender
                .send(ProbeEvent::Started {
                    label: account.label.clone(),
                })
                .is_err()
            {
                break;
            }
            probe(&config, &mut account);
            if sender
                .send(ProbeEvent::Completed(Box::new(account)))
                .is_err()
            {
                break;
            }
        }
        let _ = sender.send(ProbeEvent::Finished);
    });

    ui.checking = Some(Checking {
        receiver,
        total,
        completed: 0,
        current: String::new(),
    });
}

/// 轮询后台检测结果
pub fn poll_probe(paths: &Paths, ui: &mut Ui) -> Result<()> {
    let mut finished = false;
    let mut total_count = 0;

    if let Some(checking) = &mut ui.checking {
        total_count = checking.total;
        while let Ok(event) = checking.receiver.try_recv() {
            match event {
                ProbeEvent::Started { label } => {
                    checking.current = label;
                }
                ProbeEvent::Completed(account) => {
                    let account = *account;
                    if let Some(pos) = ui.index.accounts.iter().position(|a| a.id == account.id) {
                        ui.index.accounts[pos] = account;
                    }
                    checking.completed += 1;
                }
                ProbeEvent::Finished => {
                    finished = true;
                    break;
                }
            }
        }
    }

    if finished {
        save_index(paths, &ui.index)?;
        ui.notice = crate::i18n::translate_with(
            ui.language(),
            "notice-checked-all",
            [("count", total_count.to_string())],
        );
        ui.checking = None;
    }

    Ok(())
}

pub fn start_control_watcher(paths: &Paths, ui: &mut Ui) {
    if ui.control_updates.is_some() {
        return;
    }
    let (sender, receiver) = mpsc::channel();
    let paths = paths.clone();
    let stop = Arc::clone(&ui.control_stop);
    thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Runtime::new() else {
            return;
        };
        runtime.block_on(async move {
            while !stop.load(Ordering::Relaxed) {
                match crate::daemon::control_stream(&paths).await {
                    Ok(response) => {
                        let mut stream = response.bytes_stream();
                        while !stop.load(Ordering::Relaxed) {
                            match tokio::time::timeout(Duration::from_secs(3), stream.next()).await
                            {
                                Ok(Some(Ok(_))) => {
                                    if sender.send(fetch_control_update(&paths).await).is_err() {
                                        return;
                                    }
                                }
                                _ => break,
                            }
                        }
                    }
                    Err(_) => {
                        if sender.send(fetch_control_update(&paths).await).is_err() {
                            return;
                        }
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        });
    });
    ui.control_updates = Some(receiver);
}

async fn fetch_control_update(paths: &Paths) -> ControlUpdate {
    let snapshot = crate::daemon::control_request(paths, hyper::Method::GET, "/v1/snapshot")
        .await
        .ok()
        .and_then(|value| serde_json::from_value(value).ok());
    let events = crate::daemon::control_request(paths, hyper::Method::GET, "/v1/events")
        .await
        .ok()
        .and_then(|value| value.get("items").cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let requests = crate::daemon::control_request(paths, hyper::Method::GET, "/v1/requests")
        .await
        .ok()
        .and_then(|value| value.get("items").cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    let metrics = crate::daemon::control_request(
        paths,
        hyper::Method::GET,
        "/v1/metrics?window=300&bucket=10",
    )
    .await
    .ok()
    .and_then(|value| serde_json::from_value(value).ok())
    .unwrap_or_default();
    let metrics_1m =
        crate::daemon::control_request(paths, hyper::Method::GET, "/v1/metrics?window=60&bucket=5")
            .await
            .ok()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
    snapshot.map_or(ControlUpdate::Disconnected, |snapshot| {
        ControlUpdate::Connected {
            snapshot: Box::new(snapshot),
            events,
            requests,
            metrics: Box::new(metrics),
            metrics_1m: Box::new(metrics_1m),
        }
    })
}

pub fn poll_control_updates(ui: &mut Ui) {
    let mut updates = Vec::new();
    if let Some(receiver) = &ui.control_updates {
        while let Ok(update) = receiver.try_recv() {
            updates.push(update);
        }
    }
    for update in updates {
        match update {
            ControlUpdate::Connected {
                snapshot,
                events,
                requests,
                metrics,
                metrics_1m,
            } => {
                let snapshot = *snapshot;
                let metrics = *metrics;
                let metrics_1m = *metrics_1m;
                ui.attached_daemon = true;
                if ui.checking.is_none() {
                    ui.index.accounts.clone_from(&snapshot.accounts);
                }
                let failures = snapshot.stats.failed_requests;
                let ttfb = snapshot.stats.last_ttfb_ms.unwrap_or(0);
                let proxy = ui
                    .proxy_state
                    .get_or_insert_with(crate::proxy::ProxyState::new);
                proxy
                    .running
                    .store(snapshot.proxy.running, Ordering::Relaxed);
                *proxy.stats.write() = snapshot.stats.clone();
                ui.snapshot = Some(snapshot);
                ui.recent_events = events;
                ui.recent_requests = requests;
                ui.clamp_event_selection();
                ui.request_history = metrics_1m
                    .buckets
                    .iter()
                    .map(|bucket| {
                        bucket.requests.saturating_mul(1000) / metrics_1m.bucket_seconds.max(1)
                    })
                    .collect();
                ui.failure_history = metrics
                    .buckets
                    .iter()
                    .map(|bucket| bucket.failures)
                    .collect();
                ui.ttfb_history = metrics
                    .buckets
                    .iter()
                    .map(|bucket| bucket.ttfb_p95_ms.unwrap_or(0))
                    .collect();
                ui.metrics = Some(metrics);
                ui.metrics_1m = Some(metrics_1m);
                if !ui.onboarding_checked {
                    ui.onboarding_checked = true;
                    if ui.workspace == crate::tui::Workspace::Proxy && ui.needs_onboarding() {
                        ui.modal = crate::tui::Modal::Onboarding;
                    }
                }
                if ui.request_history.is_empty() {
                    let rps_milli = ui
                        .metrics_1m
                        .as_ref()
                        .map_or(0, |window| (window.rps * 1000.0).round() as u64);
                    let ttfb_p95 = ui
                        .metrics
                        .as_ref()
                        .and_then(|window| window.ttfb_p95_ms)
                        .unwrap_or(ttfb);
                    ui.push_metric_sample(rps_milli, failures, ttfb_p95);
                }
            }
            ControlUpdate::Disconnected => {
                ui.attached_daemon = false;
                ui.snapshot = None;
            }
        }
    }
}

pub fn poll_action_updates(ui: &mut Ui) {
    while let Ok(update) = ui.action_updates.try_recv() {
        ui.notice = match update {
            ActionUpdate::Success(message) | ActionUpdate::Error(message) => message,
        };
    }
}
