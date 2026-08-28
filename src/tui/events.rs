use super::{DetailPage, Modal, ProxyPanel, Ui, Workspace, actions::*, draw};
use crate::{
    error::{AppError, Result},
    paths::Paths,
    proxy::RuntimeState,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use hyper::Method;
use ratatui::{Terminal, backend::CrosstermBackend};
use serde_json::json;
use std::{
    io,
    path::PathBuf,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const TICK_RATE: Duration = Duration::from_millis(100);

pub fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ui: &mut Ui,
    paths: &Paths,
) -> Result<()> {
    start_control_watcher(paths, ui);
    let mut last_tick = Instant::now();
    loop {
        poll_probe(paths, ui)?;
        poll_control_updates(ui);
        poll_action_updates(ui);
        terminal.draw(|frame| draw(frame, ui))?;
        let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && handle_key(key, paths, ui)?
        {
            break;
        }
        if last_tick.elapsed() >= TICK_RATE {
            ui.tick += 1;
            last_tick = Instant::now();
        }
    }

    ui.control_stop.store(true, Ordering::Relaxed);
    if ui.owned_daemon.is_some() {
        let _ = call_control(paths, Method::POST, "/v1/daemon/stop", None);
        if let Some(handle) = ui.owned_daemon.take() {
            let _ = handle.join();
        }
    }
    Ok(())
}

pub fn handle_key(key: KeyEvent, paths: &Paths, ui: &mut Ui) -> Result<bool> {
    if ui.modal != Modal::None {
        return handle_modal(key, paths, ui);
    }
    if ui.detail.is_some() {
        return handle_detail_key(key, paths, ui);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return request_exit(ui);
    }
    if key.code == KeyCode::Char('q') {
        return request_exit(ui);
    }
    if key.code == KeyCode::Char('m') {
        ui.modal = Modal::ModeSelector;
        return Ok(false);
    }
    if key.code == KeyCode::Char('?') {
        ui.modal = Modal::Help;
        return Ok(false);
    }
    if key.code == KeyCode::Char('t') {
        ui.config.theme = ui.config.theme.next();
        ui.notice =
            save_current_config(paths, &ui.config).unwrap_or_else(|error| error.to_string());
        return Ok(false);
    }

    if ui.checking.is_some()
        && matches!(
            key.code,
            KeyCode::Char('r' | 'R' | 'a' | 'i' | 'n' | 'd') | KeyCode::Enter
        )
    {
        ui.notice = "检测进行中；完成后再修改账户".into();
        return Ok(false);
    }

    match ui.workspace {
        Workspace::Accounts => handle_account_key(key, paths, ui),
        Workspace::Proxy => handle_proxy_key(key, paths, ui),
    }
}

fn handle_detail_key(key: KeyEvent, paths: &Paths, ui: &mut Ui) -> Result<bool> {
    if key.code == KeyCode::Esc {
        ui.detail = None;
        return Ok(false);
    }
    if key.code == KeyCode::Char('q') {
        return request_exit(ui);
    }
    if !matches!(ui.detail, Some(DetailPage::Control)) {
        return Ok(false);
    }
    match key.code {
        KeyCode::Char('j') => ui.control_selected = (ui.control_selected + 1).min(4),
        KeyCode::Char('k') => ui.control_selected = ui.control_selected.saturating_sub(1),
        KeyCode::Enter | KeyCode::Char(' ') => match ui.control_selected {
            0 => {
                ui.detail = None;
                ui.modal = if matches!(
                    ui.runtime_state(),
                    RuntimeState::Running | RuntimeState::Paused | RuntimeState::Starting
                ) {
                    Modal::ConfirmProxyStop
                } else {
                    Modal::ConfirmProxyStart
                };
            }
            1 => {
                ui.detail = None;
                ui.modal = if ui.integration_enabled() {
                    Modal::ConfirmIntegrationDisable
                } else {
                    Modal::ConfirmIntegrationEnable
                };
            }
            2 => {
                if ui.config.proxy.auto_switch {
                    set_auto_switch(paths, ui, false);
                } else {
                    ui.detail = None;
                    ui.modal = Modal::ConfirmAutoSwitch;
                }
            }
            3 => cycle_strategy(paths, ui),
            4 => cycle_threshold(paths, ui),
            _ => {}
        },
        _ => {}
    }
    Ok(false)
}

fn request_exit(ui: &mut Ui) -> Result<bool> {
    let active = ui
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.active_requests.len())
        .unwrap_or(0);
    if ui.owned_daemon.is_some() && active > 0 {
        ui.modal = Modal::ConfirmExit;
        ui.notice = format!("仍有 {active} 个活动请求");
        Ok(false)
    } else {
        Ok(true)
    }
}

fn handle_account_key(key: KeyEvent, paths: &Paths, ui: &mut Ui) -> Result<bool> {
    let visible = ui.visible();
    match key.code {
        KeyCode::Char('j') => {
            if !visible.is_empty() {
                ui.selected = (ui.selected + 1).min(visible.len() - 1);
            }
        }
        KeyCode::Char('k') => ui.selected = ui.selected.saturating_sub(1),
        KeyCode::Char('g') => ui.selected = 0,
        KeyCode::Char('G') => ui.selected = visible.len().saturating_sub(1),
        KeyCode::Esc => {
            ui.filter.clear();
            ui.notice = "已清除过滤".into();
        }
        KeyCode::Char('/') => {
            ui.input.clear();
            ui.modal = Modal::Filter;
        }
        KeyCode::Char('a') => {
            ui.notice = import_current_auth(&ui.config, &mut ui.index, paths)
                .unwrap_or_else(|error| error.to_string());
        }
        KeyCode::Char('i') => {
            ui.input.clear();
            ui.modal = Modal::Import;
        }
        KeyCode::Char('n') => {
            if let Some(index) = ui.selected_id() {
                if ui.index.accounts[index].email.is_some() {
                    ui.modal = Modal::ConfirmUseEmail;
                } else {
                    ui.input.clone_from(&ui.index.accounts[index].label);
                    ui.modal = Modal::Rename;
                }
            }
        }
        KeyCode::Char('d') => {
            if ui.selected_id().is_some() {
                ui.modal = Modal::ConfirmDelete;
            }
        }
        KeyCode::Char('s') => {
            ui.input = ui.config.codex_home.display().to_string();
            ui.modal = Modal::Settings;
        }
        KeyCode::Char('r') => {
            if let Some(index) = ui.selected_id() {
                let account = ui.index.accounts[index].clone();
                ui.notice = format!("正在检测 {}…", account.label);
                start_probe(ui, vec![account]);
            }
        }
        KeyCode::Char('R') => {
            ui.notice = "正在检测全部账户…".into();
            start_probe(ui, ui.index.accounts.clone());
        }
        KeyCode::Enter => {
            if let Some(index) = ui.selected_id() {
                let account = &ui.index.accounts[index];
                ui.notice =
                    activate_account(&ui.config, account).unwrap_or_else(|error| error.to_string());
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_proxy_key(key: KeyEvent, paths: &Paths, ui: &mut Ui) -> Result<bool> {
    match key.code {
        KeyCode::Tab => ui.proxy_panel = ui.proxy_panel.next(),
        KeyCode::BackTab => ui.proxy_panel = ui.proxy_panel.previous(),
        KeyCode::Char('1') => ui.proxy_panel = ProxyPanel::Control,
        KeyCode::Char('2') => ui.proxy_panel = ProxyPanel::Pool,
        KeyCode::Char('3') => ui.proxy_panel = ProxyPanel::Instances,
        KeyCode::Char('4') => ui.proxy_panel = ProxyPanel::Events,
        KeyCode::Char('j') => move_proxy_selection(ui, true),
        KeyCode::Char('k') => move_proxy_selection(ui, false),
        KeyCode::Enter => open_proxy_detail(ui),
        KeyCode::Char(' ') if ui.proxy_panel == ProxyPanel::Pool => toggle_pool(paths, ui),
        KeyCode::Char('r') if ui.proxy_panel == ProxyPanel::Pool => {
            if let Some(index) = ui.pool_selected_id() {
                start_probe(ui, vec![ui.index.accounts[index].clone()]);
                ui.notice = "正在检测所选账户…".into();
            }
        }
        KeyCode::Char('R') if ui.proxy_panel == ProxyPanel::Pool => {
            start_probe(ui, ui.index.accounts.clone());
            ui.notice = "正在检测全部账户…".into();
        }
        KeyCode::Char('x') if ui.proxy_panel == ProxyPanel::Pool => switch_route(paths, ui),
        KeyCode::Char('s') => {
            ui.modal = if matches!(
                ui.runtime_state(),
                RuntimeState::Running | RuntimeState::Paused | RuntimeState::Starting
            ) {
                Modal::ConfirmProxyStop
            } else {
                Modal::ConfirmProxyStart
            };
        }
        KeyCode::Char('p') => toggle_pause(paths, ui),
        KeyCode::Char('c') => {
            ui.modal = if ui.integration_enabled() {
                Modal::ConfirmIntegrationDisable
            } else {
                Modal::ConfirmIntegrationEnable
            };
        }
        KeyCode::Char('a') => {
            if !ui.config.proxy.auto_switch {
                ui.modal = Modal::ConfirmAutoSwitch;
            } else {
                set_auto_switch(paths, ui, false);
            }
        }
        KeyCode::Esc => ui.notice = "使用 m 切换工作区".into(),
        _ => {}
    }
    Ok(false)
}

fn move_proxy_selection(ui: &mut Ui, down: bool) {
    let (selected, length) = match ui.proxy_panel {
        ProxyPanel::Control => (&mut ui.control_selected, 7),
        ProxyPanel::Pool => (&mut ui.pool_selected, ui.index.accounts.len()),
        ProxyPanel::Instances => (
            &mut ui.instance_selected,
            ui.snapshot
                .as_ref()
                .map(|snapshot| snapshot.instances.len())
                .unwrap_or(0),
        ),
        ProxyPanel::Events => (
            &mut ui.event_selected,
            ui.recent_requests.len() + ui.recent_events.len(),
        ),
    };
    if down {
        *selected = (*selected + 1).min(length.saturating_sub(1));
    } else {
        *selected = selected.saturating_sub(1);
    }
}

fn open_proxy_detail(ui: &mut Ui) {
    ui.detail = match ui.proxy_panel {
        ProxyPanel::Control => Some(DetailPage::Control),
        ProxyPanel::Pool => ui
            .pool_selected_id()
            .map(|index| DetailPage::Account(ui.index.accounts[index].id)),
        ProxyPanel::Instances => Some(DetailPage::Instance(ui.instance_selected)),
        ProxyPanel::Events if ui.event_selected < ui.recent_requests.len() => {
            Some(DetailPage::Request(ui.event_selected))
        }
        ProxyPanel::Events => Some(DetailPage::Event(
            ui.event_selected.saturating_sub(ui.recent_requests.len()),
        )),
    };
}

fn toggle_pool(paths: &Paths, ui: &mut Ui) {
    let Some(index) = ui.pool_selected_id() else {
        return;
    };
    let enabled = !ui.index.accounts[index].proxy_enabled;
    let id = ui.index.accounts[index].id;
    if ui.attached_daemon {
        let endpoint = format!("/v1/accounts/{id}/pool");
        enqueue_control(
            paths,
            ui,
            Method::POST,
            &endpoint,
            Some(json!({"enabled":enabled})),
            if enabled {
                "账户已加入代理池"
            } else {
                "账户已移出代理池"
            },
        );
        ui.index.accounts[index].proxy_enabled = enabled;
    } else {
        ui.index.accounts[index].proxy_enabled = enabled;
        ui.notice = match crate::account::save_index(paths, &ui.index) {
            Ok(()) if enabled => "账户已加入代理池".into(),
            Ok(()) => "账户已移出代理池".into(),
            Err(error) => error.to_string(),
        };
    }
}

fn switch_route(paths: &Paths, ui: &mut Ui) {
    let Some(index) = ui.pool_selected_id() else {
        return;
    };
    let account = &ui.index.accounts[index];
    if !account.proxy_enabled {
        ui.notice = "请先按 Space 将账户加入代理池".into();
        return;
    }
    let endpoint = format!("/v1/accounts/{}/switch", account.id);
    let message = format!("将在下一安全请求边界切换到 {}", account.label);
    enqueue_control(paths, ui, Method::POST, &endpoint, None, &message);
}

fn toggle_pause(paths: &Paths, ui: &mut Ui) {
    let endpoint = match ui.runtime_state() {
        RuntimeState::Running => "/v1/proxy/pause",
        RuntimeState::Paused => "/v1/proxy/resume",
        _ => {
            ui.notice = "代理未运行，无法暂停或恢复".into();
            return;
        }
    };
    let message = if endpoint.ends_with("pause") {
        "路由已暂停"
    } else {
        "路由已恢复"
    };
    enqueue_control(paths, ui, Method::POST, endpoint, None, message);
}

fn start_proxy(paths: &Paths, ui: &mut Ui) {
    if !ui.attached_daemon
        && let Some(handle) = ui.owned_daemon.as_ref()
    {
        if handle.is_finished() {
            if let Some(handle) = ui.owned_daemon.take() {
                let _ = handle.join();
            }
        } else {
            ui.notice = "内嵌代理正在启动，请稍候".into();
            return;
        }
    }
    if ui.eligible_accounts() == 0 {
        ui.notice = "无法启动：请先检测账户并按 Space 加入代理池".into();
        return;
    }
    if ui.attached_daemon {
        enqueue_control(
            paths,
            ui,
            Method::POST,
            "/v1/proxy/start",
            None,
            "数据代理正在启动…",
        );
        return;
    }
    let mut config = ui.config.clone();
    config.proxy.enabled = true;
    if let Err(error) = crate::config::save_config(paths, &config) {
        ui.notice = error.to_string();
        return;
    }
    ui.config = config.clone();
    let index = ui.index.clone();
    let daemon_paths = paths.clone();
    ui.owned_daemon = Some(std::thread::spawn(move || {
        tokio::runtime::Runtime::new()?.block_on(crate::daemon::run_daemon(
            config,
            index,
            daemon_paths,
        ))
    }));
    ui.notice = "正在启动内嵌代理…".into();
}

fn stop_proxy(paths: &Paths, ui: &mut Ui) {
    enqueue_control(
        paths,
        ui,
        Method::POST,
        "/v1/proxy/stop",
        None,
        "数据代理已停止，控制面仍可用",
    );
}

fn set_auto_switch(paths: &Paths, ui: &mut Ui, enabled: bool) {
    ui.config.proxy.auto_switch = enabled;
    let local = crate::config::save_config(paths, &ui.config);
    if ui.attached_daemon {
        enqueue_control(
            paths,
            ui,
            Method::PATCH,
            "/v1/config",
            Some(json!({"auto_switch":enabled})),
            if enabled {
                "已明确启用自动切换"
            } else {
                "已关闭自动切换"
            },
        );
    }
    ui.notice = match local {
        Ok(()) if enabled => "已明确启用自动切换".into(),
        Ok(()) => "已关闭自动切换".into(),
        Err(error) => error.to_string(),
    };
}

fn cycle_strategy(paths: &Paths, ui: &mut Ui) {
    ui.config.proxy.strategy = match ui.config.proxy.strategy.clone() {
        crate::config::RecommendStrategy::Smart => crate::config::RecommendStrategy::MaxRemaining,
        crate::config::RecommendStrategy::MaxRemaining => {
            crate::config::RecommendStrategy::RoundRobin
        }
        crate::config::RecommendStrategy::RoundRobin => crate::config::RecommendStrategy::Smart,
    };
    update_proxy_config(
        paths,
        ui,
        json!({"strategy":ui.config.proxy.strategy.clone()}),
    );
}

fn cycle_threshold(paths: &Paths, ui: &mut Ui) {
    ui.config.proxy.threshold = match ui.config.proxy.threshold.round() as u64 {
        0..=70 => 85.0,
        71..=85 => 90.0,
        86..=90 => 95.0,
        _ => 70.0,
    };
    update_proxy_config(paths, ui, json!({"threshold":ui.config.proxy.threshold}));
}

fn update_proxy_config(paths: &Paths, ui: &mut Ui, patch: serde_json::Value) {
    let local = crate::config::save_config(paths, &ui.config);
    if ui.attached_daemon {
        enqueue_control(
            paths,
            ui,
            Method::PATCH,
            "/v1/config",
            Some(patch),
            "代理设置已更新",
        );
    }
    ui.notice = local
        .map(|_| "代理设置已更新".into())
        .unwrap_or_else(|error| error.to_string());
}

fn set_integration(paths: &Paths, ui: &mut Ui, enabled: bool) {
    if ui.attached_daemon {
        enqueue_control(
            paths,
            ui,
            Method::POST,
            if enabled {
                "/v1/integration/enable"
            } else {
                "/v1/integration/disable"
            },
            None,
            if enabled {
                "已启用 Codex 接入；请重启 Codex"
            } else {
                "已停用 Codex 接入；请重启 Codex"
            },
        );
        return;
    }
    let result = {
        let integration = crate::integration::CodexIntegration::new(&ui.config.codex_home);
        if enabled {
            integration.enable()
        } else {
            integration.disable()
        }
    };
    ui.notice = result
        .map(|_| {
            if enabled {
                "已启用 Codex 接入；请重启 Codex"
            } else {
                "已停用 Codex 接入；请重启 Codex"
            }
            .into()
        })
        .unwrap_or_else(|error| error.to_string());
}

fn handle_modal(key: KeyEvent, paths: &Paths, ui: &mut Ui) -> Result<bool> {
    if key.code == KeyCode::Esc {
        ui.modal = Modal::None;
        ui.input.clear();
        return Ok(false);
    }
    match ui.modal {
        Modal::ModeSelector => match key.code {
            KeyCode::Char('1') => ui.switch_workspace(Workspace::Accounts),
            KeyCode::Char('2') => ui.switch_workspace(Workspace::Proxy),
            _ => {}
        },
        Modal::Onboarding => match key.code {
            KeyCode::Char('1') => {
                if ui.index.accounts.is_empty() {
                    ui.switch_workspace(Workspace::Accounts);
                    ui.notice = "按 a 导入当前 Codex 登录，或按 i 导入 JSON/路径".into();
                } else {
                    ui.modal = Modal::None;
                    ui.proxy_panel = ProxyPanel::Pool;
                    ui.notice = "j/k 选择账户，r 检测，Space 加入代理池".into();
                }
            }
            KeyCode::Char('2') => {
                ui.modal = Modal::None;
                start_proxy(paths, ui);
            }
            KeyCode::Char('3') => {
                ui.modal = if ui.integration_enabled() {
                    Modal::ConfirmIntegrationDisable
                } else {
                    Modal::ConfirmIntegrationEnable
                };
            }
            _ => {}
        },
        Modal::Help if key.code == KeyCode::Char('q') => ui.modal = Modal::None,
        Modal::ConfirmExit if key.code == KeyCode::Char('y') => return Ok(true),
        Modal::ConfirmDelete if key.code == KeyCode::Char('y') => {
            if let Some(index) = ui.selected_id() {
                ui.notice = delete_account(&ui.config, &mut ui.index, paths, index)
                    .unwrap_or_else(|error| error.to_string());
                ui.selected = ui.selected.saturating_sub(1);
            }
            ui.modal = Modal::None;
        }
        Modal::ConfirmUseEmail if key.code == KeyCode::Char('y') => {
            if let Some(index) = ui.selected_id()
                && let Some(email) = ui.index.accounts[index].email.clone()
            {
                ui.notice = rename_account(&mut ui.index, paths, index, email)
                    .unwrap_or_else(|error| error.to_string());
            }
            ui.modal = Modal::None;
        }
        Modal::ConfirmUseEmail if key.code == KeyCode::Char('n') => {
            if let Some(index) = ui.selected_id() {
                ui.input.clone_from(&ui.index.accounts[index].label);
                ui.modal = Modal::Rename;
            }
        }
        Modal::ConfirmProxyStart if key.code == KeyCode::Char('y') => {
            ui.modal = Modal::None;
            start_proxy(paths, ui);
        }
        Modal::ConfirmProxyStop if key.code == KeyCode::Char('y') => {
            ui.modal = Modal::None;
            stop_proxy(paths, ui);
        }
        Modal::ConfirmAutoSwitch if key.code == KeyCode::Char('y') => {
            ui.modal = Modal::None;
            set_auto_switch(paths, ui, true);
        }
        Modal::ConfirmIntegrationEnable if key.code == KeyCode::Char('y') => {
            ui.modal = Modal::None;
            set_integration(paths, ui, true);
        }
        Modal::ConfirmIntegrationDisable if key.code == KeyCode::Char('y') => {
            ui.modal = Modal::None;
            set_integration(paths, ui, false);
        }
        Modal::Import | Modal::Filter | Modal::Rename | Modal::Settings => match key.code {
            KeyCode::Enter => {
                finish_text_modal(paths, ui);
                ui.input.clear();
            }
            KeyCode::Backspace => {
                ui.input.pop();
            }
            KeyCode::Char(character) => ui.input.push(character),
            _ => {}
        },
        _ => {}
    }
    Ok(false)
}

fn finish_text_modal(paths: &Paths, ui: &mut Ui) {
    ui.notice = match ui.modal {
        Modal::Import => {
            if ui.input.trim_start().starts_with('{') {
                import_from_json(&ui.config, &mut ui.index, paths, &ui.input, None)
            } else {
                import_from_path(&ui.config, &mut ui.index, paths, ui.input.trim())
            }
        }
        Modal::Filter => {
            ui.filter.clone_from(&ui.input);
            ui.selected = 0;
            Ok(if ui.filter.is_empty() {
                "已清除过滤".into()
            } else {
                format!("正在过滤：{}", ui.filter)
            })
        }
        Modal::Rename => {
            let index = ui
                .selected_id()
                .ok_or_else(|| AppError::Message("账户不存在".into()));
            index.and_then(|index| {
                rename_account(&mut ui.index, paths, index, ui.input.trim().to_string())
            })
        }
        Modal::Settings => {
            let path = PathBuf::from(ui.input.trim());
            if path.as_os_str().is_empty() {
                Err(AppError::Message("路径不能为空".into()))
            } else {
                ui.config.codex_home = path;
                save_current_config(paths, &ui.config)
            }
        }
        _ => Ok(String::new()),
    }
    .unwrap_or_else(|error| error.to_string());
    ui.modal = Modal::None;
}

fn call_control(
    paths: &Paths,
    method: Method,
    endpoint: &str,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    tokio::runtime::Runtime::new()?.block_on(crate::daemon::control_request_json(
        paths,
        method,
        endpoint,
        body.as_ref(),
    ))
}

fn enqueue_control(
    paths: &Paths,
    ui: &mut Ui,
    method: Method,
    endpoint: &str,
    body: Option<serde_json::Value>,
    success: &str,
) {
    let paths = paths.clone();
    let endpoint = endpoint.to_string();
    let success = success.to_string();
    let sender = ui.action_sender.clone();
    std::thread::spawn(move || {
        let result = tokio::runtime::Runtime::new()
            .map_err(AppError::from)
            .and_then(|runtime| {
                runtime.block_on(crate::daemon::control_request_json(
                    &paths,
                    method,
                    &endpoint,
                    body.as_ref(),
                ))
            });
        let update = match result {
            Ok(_) => super::ActionUpdate::Success(success),
            Err(error) => super::ActionUpdate::Error(error.to_string()),
        };
        let _ = sender.send(update);
    });
    ui.notice = "正在执行…".into();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, types::AccountIndex};

    fn ui() -> Ui {
        Ui::new(
            Config::defaults(),
            AccountIndex::default(),
            None,
            Workspace::Accounts,
        )
    }

    fn key(character: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)
    }

    #[test]
    fn mode_menu_switches_workspaces_with_numbers() {
        let root = crate::paths::paths();
        let mut ui = ui();
        handle_key(key('m'), &root, &mut ui).unwrap();
        assert_eq!(ui.modal, Modal::ModeSelector);
        handle_key(key('2'), &root, &mut ui).unwrap();
        assert_eq!(ui.workspace, Workspace::Proxy);
    }

    #[test]
    fn proxy_panel_navigation_uses_tab_numbers_and_vim_keys() {
        let root = crate::paths::paths();
        let mut ui = ui();
        ui.workspace = Workspace::Proxy;
        handle_key(key('2'), &root, &mut ui).unwrap();
        assert_eq!(ui.proxy_panel, ProxyPanel::Pool);
        handle_key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &root,
            &mut ui,
        )
        .unwrap();
        assert_eq!(ui.proxy_panel, ProxyPanel::Instances);
    }

    #[test]
    fn escape_only_closes_detail_in_proxy_workspace() {
        let root = crate::paths::paths();
        let mut ui = ui();
        ui.workspace = Workspace::Proxy;
        ui.detail = Some(DetailPage::Control);
        handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &root,
            &mut ui,
        )
        .unwrap();
        assert_eq!(ui.workspace, Workspace::Proxy);
        assert!(ui.detail.is_none());
    }

    #[test]
    fn arrow_keys_do_not_replace_vim_navigation() {
        let root = crate::paths::paths();
        let mut ui = ui();
        ui.workspace = Workspace::Proxy;
        ui.proxy_panel = ProxyPanel::Pool;
        let before = ui.pool_selected;
        handle_key(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &root,
            &mut ui,
        )
        .unwrap();
        assert_eq!(ui.pool_selected, before);
    }

    #[test]
    fn onboarding_routes_empty_users_to_account_import() {
        let root = crate::paths::paths();
        let mut ui = ui();
        ui.workspace = Workspace::Proxy;
        ui.modal = Modal::Onboarding;
        handle_key(key('1'), &root, &mut ui).unwrap();
        assert_eq!(ui.workspace, Workspace::Accounts);
        assert!(ui.notice.contains("导入"));
    }

    #[test]
    fn proxy_root_escape_does_not_change_workspace() {
        let root = crate::paths::paths();
        let mut ui = ui();
        ui.workspace = Workspace::Proxy;
        handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            &root,
            &mut ui,
        )
        .unwrap();
        assert_eq!(ui.workspace, Workspace::Proxy);
    }
}
