use super::{
    DetailPage, HelpPage, InputSuggestion, Modal, ProxyPanel, Ui, Workspace, actions::*, draw,
};
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
    collections::HashSet,
    fs, io,
    path::{MAIN_SEPARATOR, Path, PathBuf},
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
        ui.open_help();
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
                let modal = if matches!(
                    ui.runtime_state(),
                    RuntimeState::Running | RuntimeState::Paused | RuntimeState::Starting
                ) {
                    Modal::ConfirmProxyStop
                } else {
                    Modal::ConfirmProxyStart
                };
                ui.open_confirmation(modal);
            }
            1 => {
                ui.detail = None;
                let modal = if ui.integration_enabled() {
                    Modal::ConfirmIntegrationDisable
                } else {
                    Modal::ConfirmIntegrationEnable
                };
                ui.open_confirmation(modal);
            }
            2 => {
                if ui.config.proxy.auto_switch {
                    set_auto_switch(paths, ui, false);
                } else {
                    ui.detail = None;
                    ui.open_confirmation(Modal::ConfirmAutoSwitch);
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
        ui.open_confirmation(Modal::ConfirmExit);
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
            ui.open_text_editor(Modal::Filter, ui.filter.clone());
            refresh_completions(ui);
        }
        KeyCode::Char('a') => {
            ui.notice = import_current_auth(&ui.config, &mut ui.index, paths)
                .unwrap_or_else(|error| error.to_string());
        }
        KeyCode::Char('i') => {
            ui.open_text_editor(Modal::Import, String::new());
            refresh_completions(ui);
        }
        KeyCode::Char('n') => {
            if let Some(index) = ui.selected_id() {
                if ui.index.accounts[index].email.is_some() {
                    ui.open_confirmation(Modal::ConfirmUseEmail);
                } else {
                    let value = ui.index.accounts[index].label.clone();
                    ui.open_text_editor(Modal::Rename, value);
                    refresh_completions(ui);
                }
            }
        }
        KeyCode::Char('d') => {
            if ui.selected_id().is_some() {
                ui.open_confirmation(Modal::ConfirmDelete);
            }
        }
        KeyCode::Char('s') => {
            ui.open_text_editor(Modal::Settings, ui.config.codex_home.display().to_string());
            refresh_completions(ui);
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
            let modal = if matches!(
                ui.runtime_state(),
                RuntimeState::Running | RuntimeState::Paused | RuntimeState::Starting
            ) {
                Modal::ConfirmProxyStop
            } else {
                Modal::ConfirmProxyStart
            };
            ui.open_confirmation(modal);
        }
        KeyCode::Char('p') => toggle_pause(paths, ui),
        KeyCode::Char('c') => {
            let modal = if ui.integration_enabled() {
                Modal::ConfirmIntegrationDisable
            } else {
                Modal::ConfirmIntegrationEnable
            };
            ui.open_confirmation(modal);
        }
        KeyCode::Char('a') => {
            if !ui.config.proxy.auto_switch {
                ui.open_confirmation(Modal::ConfirmAutoSwitch);
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
    if ui.proxy_panel == ProxyPanel::Events
        && ui.recent_requests.is_empty()
        && ui.recent_events.is_empty()
    {
        ui.notice = "暂无可查看的近期事件".into();
        return;
    }
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
        "代理与 Codex 接入均已关闭；请重启 Codex",
    );
}

fn acknowledge_onboarding(paths: &Paths, ui: &mut Ui) {
    if ui.config.onboarding_acknowledged {
        return;
    }
    ui.config.onboarding_acknowledged = true;
    if let Err(error) = crate::config::save_config(paths, &ui.config) {
        ui.notice = format!("首次设置状态保存失败：{error}");
    }
    if ui.attached_daemon {
        enqueue_control(
            paths,
            ui,
            Method::PATCH,
            "/v1/config",
            Some(json!({"onboarding_acknowledged":true})),
            "首次设置提示已记住",
        );
    }
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
    if key.code == KeyCode::Esc
        || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
    {
        if ui.modal == Modal::Onboarding {
            acknowledge_onboarding(paths, ui);
        }
        ui.modal = Modal::None;
        ui.editor.clear();
        return Ok(false);
    }
    if ui.modal == Modal::Help {
        handle_help_key(key, ui);
        return Ok(false);
    }
    if ui.modal.is_confirmation() {
        return handle_confirmation_key(key, paths, ui);
    }
    if ui.modal.is_text_editor() {
        handle_editor_key(key, paths, ui);
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
                acknowledge_onboarding(paths, ui);
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
                acknowledge_onboarding(paths, ui);
                ui.modal = Modal::None;
                start_proxy(paths, ui);
            }
            KeyCode::Char('3') => {
                acknowledge_onboarding(paths, ui);
                let modal = if ui.integration_enabled() {
                    Modal::ConfirmIntegrationDisable
                } else {
                    Modal::ConfirmIntegrationEnable
                };
                ui.open_confirmation(modal);
            }
            _ => {}
        },
        _ => {}
    }
    Ok(false)
}

fn handle_help_key(key: KeyEvent, ui: &mut Ui) {
    let mut page = None;
    match key.code {
        KeyCode::Char('q') => ui.modal = Modal::None,
        KeyCode::Char('1') => page = Some(HelpPage::QuickStart),
        KeyCode::Char('2') => page = Some(HelpPage::Account),
        KeyCode::Char('3') => page = Some(HelpPage::Proxy),
        KeyCode::Char('4') => page = Some(HelpPage::Safety),
        KeyCode::Tab => page = Some(ui.help_page.next()),
        KeyCode::BackTab => page = Some(ui.help_page.previous()),
        KeyCode::Char('j') | KeyCode::Down => {
            ui.help_scroll = (ui.help_scroll + 1).min(ui.help_page.max_scroll())
        }
        KeyCode::Char('k') | KeyCode::Up => ui.help_scroll = ui.help_scroll.saturating_sub(1),
        KeyCode::PageDown => ui.help_scroll = (ui.help_scroll + 6).min(ui.help_page.max_scroll()),
        KeyCode::PageUp => ui.help_scroll = ui.help_scroll.saturating_sub(6),
        KeyCode::Home => ui.help_scroll = 0,
        KeyCode::End => ui.help_scroll = ui.help_page.max_scroll(),
        _ => {}
    }
    if let Some(page) = page {
        ui.help_page = page;
        ui.help_scroll = 0;
    }
}

fn handle_confirmation_key(key: KeyEvent, paths: &Paths, ui: &mut Ui) -> Result<bool> {
    match key.code {
        KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
            ui.confirm_choice.toggle();
            Ok(false)
        }
        KeyCode::Char('y') => execute_confirmation(paths, ui, true),
        KeyCode::Char('n') => execute_confirmation(paths, ui, false),
        KeyCode::Enter => execute_confirmation(
            paths,
            ui,
            ui.confirm_choice == super::ConfirmChoice::Confirm,
        ),
        _ => Ok(false),
    }
}

fn execute_confirmation(paths: &Paths, ui: &mut Ui, confirmed: bool) -> Result<bool> {
    let modal = ui.modal;
    ui.modal = Modal::None;
    if !confirmed {
        if modal == Modal::ConfirmUseEmail
            && let Some(index) = ui.selected_id()
        {
            let value = ui.index.accounts[index].label.clone();
            ui.open_text_editor(Modal::Rename, value);
            refresh_completions(ui);
        }
        return Ok(false);
    }
    match modal {
        Modal::ConfirmExit => return Ok(true),
        Modal::ConfirmDelete => {
            if let Some(index) = ui.selected_id() {
                ui.notice = delete_account(&ui.config, &mut ui.index, paths, index)
                    .unwrap_or_else(|error| error.to_string());
                ui.selected = ui.selected.saturating_sub(1);
            }
        }
        Modal::ConfirmUseEmail => {
            if let Some(index) = ui.selected_id()
                && let Some(email) = ui.index.accounts[index].email.clone()
            {
                ui.notice = rename_account(&mut ui.index, paths, index, email)
                    .unwrap_or_else(|error| error.to_string());
            }
        }
        Modal::ConfirmProxyStart => start_proxy(paths, ui),
        Modal::ConfirmProxyStop => stop_proxy(paths, ui),
        Modal::ConfirmAutoSwitch => set_auto_switch(paths, ui, true),
        Modal::ConfirmIntegrationEnable => set_integration(paths, ui, true),
        Modal::ConfirmIntegrationDisable => set_integration(paths, ui, false),
        _ => {}
    }
    Ok(false)
}

fn handle_editor_key(key: KeyEvent, paths: &Paths, ui: &mut Ui) {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let text_changed = if control {
        match key.code {
            KeyCode::Char('a') => ui.editor.cursor = 0,
            KeyCode::Char('e') => ui.editor.cursor = ui.editor.value.len(),
            KeyCode::Char('n') => ui.editor.next_suggestion(),
            KeyCode::Char('p') => ui.editor.previous_suggestion(),
            KeyCode::Char('w') => ui.editor.delete_previous_word(),
            KeyCode::Char('u') => ui.editor.kill_before_cursor(),
            KeyCode::Char('k') => ui.editor.kill_after_cursor(),
            _ => {}
        }
        matches!(key.code, KeyCode::Char('w' | 'u' | 'k'))
    } else {
        match key.code {
            KeyCode::Enter => {
                submit_text_editor(paths, ui);
                return;
            }
            KeyCode::Left => ui.editor.move_left(),
            KeyCode::Right => ui.editor.move_right(),
            KeyCode::Home => ui.editor.cursor = 0,
            KeyCode::End => ui.editor.cursor = ui.editor.value.len(),
            KeyCode::Backspace => ui.editor.backspace(),
            KeyCode::Delete => ui.editor.delete(),
            KeyCode::Up => ui.editor.previous_suggestion(),
            KeyCode::Down => ui.editor.next_suggestion(),
            KeyCode::Tab => {
                ui.editor.accept_suggestion();
                refresh_completions(ui);
                return;
            }
            KeyCode::Char(character) => ui.editor.insert(character),
            _ => {}
        }
        matches!(
            key.code,
            KeyCode::Backspace | KeyCode::Delete | KeyCode::Char(_)
        )
    };
    if text_changed {
        refresh_completions(ui);
    }
}

fn submit_text_editor(paths: &Paths, ui: &mut Ui) {
    let value = ui.editor.value.clone();
    let result = match ui.modal {
        Modal::Import => {
            if value.trim_start().starts_with('{') {
                import_from_json(&ui.config, &mut ui.index, paths, &value, None)
            } else {
                import_from_path(&ui.config, &mut ui.index, paths, value.trim())
            }
        }
        Modal::Filter => {
            ui.filter.clone_from(&value);
            ui.selected = 0;
            Ok(if ui.filter.is_empty() {
                "已清除过滤".into()
            } else {
                format!("正在过滤：{}", ui.filter)
            })
        }
        Modal::Rename if value.trim().is_empty() => {
            Err(AppError::Message("账户名称不能为空".into()))
        }
        Modal::Rename => ui
            .selected_id()
            .ok_or_else(|| AppError::Message("账户不存在".into()))
            .and_then(|index| {
                rename_account(&mut ui.index, paths, index, value.trim().to_string())
            }),
        Modal::Settings => {
            let path = PathBuf::from(value.trim());
            if path.as_os_str().is_empty() {
                Err(AppError::Message("路径不能为空".into()))
            } else {
                let mut candidate = ui.config.clone();
                candidate.codex_home = path;
                match save_current_config(paths, &candidate) {
                    Ok(notice) => {
                        ui.config = candidate;
                        Ok(notice)
                    }
                    Err(error) => Err(error),
                }
            }
        }
        _ => Ok(String::new()),
    };
    match result {
        Ok(notice) => {
            ui.notice = notice;
            ui.modal = Modal::None;
            ui.editor.clear();
        }
        Err(error) => ui.editor.error = Some(error.to_string()),
    }
}

fn refresh_completions(ui: &mut Ui) {
    let suggestions = match ui.modal {
        Modal::Import => path_suggestions(&ui.editor.value, false),
        Modal::Settings => path_suggestions(&ui.editor.value, true),
        Modal::Filter => account_suggestions(ui, true),
        Modal::Rename => account_suggestions(ui, false),
        _ => Vec::new(),
    };
    ui.editor.suggestions = suggestions;
    ui.editor.suggestion_index = 0;
}

fn path_suggestions(input: &str, directories_only: bool) -> Vec<InputSuggestion> {
    if input.trim_start().starts_with('{') {
        return Vec::new();
    }
    let input_path = Path::new(input);
    let ends_with_separator = input.ends_with(MAIN_SEPARATOR);
    let (parent, prefix) = if ends_with_separator {
        (input_path, "")
    } else {
        (
            input_path.parent().unwrap_or_else(|| Path::new(".")),
            input_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(""),
        )
    };
    let show_hidden = prefix.starts_with('.');
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut suggestions = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(prefix) || (!show_hidden && name.starts_with('.')) {
                return None;
            }
            let directory = entry.file_type().ok()?.is_dir();
            if directories_only && !directory {
                return None;
            }
            let mut value = entry.path().to_string_lossy().into_owned();
            if directory && !value.ends_with(MAIN_SEPARATOR) {
                value.push(MAIN_SEPARATOR);
            }
            Some(InputSuggestion {
                display: format!("{}{}", name, if directory { "/" } else { "" }),
                value,
                directory,
            })
        })
        .collect::<Vec<_>>();
    suggestions.sort_by(|left, right| {
        right.directory.cmp(&left.directory).then_with(|| {
            left.display
                .to_lowercase()
                .cmp(&right.display.to_lowercase())
        })
    });
    suggestions.truncate(50);
    suggestions
}

fn account_suggestions(ui: &Ui, filtering: bool) -> Vec<InputSuggestion> {
    let query = ui.editor.value.to_lowercase();
    let mut seen = HashSet::new();
    let mut suggestions = Vec::new();
    if filtering {
        for account in &ui.index.accounts {
            for value in [Some(account.label.as_str()), account.email.as_deref()]
                .into_iter()
                .flatten()
            {
                if (query.is_empty() || value.to_lowercase().contains(&query))
                    && seen.insert(value.to_owned())
                {
                    suggestions.push(InputSuggestion {
                        display: value.to_owned(),
                        value: value.to_owned(),
                        directory: false,
                    });
                }
            }
        }
    } else if let Some(index) = ui.selected_id() {
        let account = &ui.index.accounts[index];
        for value in [Some(account.label.as_str()), account.email.as_deref()]
            .into_iter()
            .flatten()
        {
            if seen.insert(value.to_owned()) {
                suggestions.push(InputSuggestion {
                    display: value.to_owned(),
                    value: value.to_owned(),
                    directory: false,
                });
            }
        }
    }
    suggestions
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

    fn test_paths() -> Paths {
        let root = std::env::temp_dir().join(format!(
            "codex-switcher-settings-test-{}",
            uuid::Uuid::new_v4()
        ));
        Paths {
            config_file: root.join("config.toml"),
            index_file: root.join("accounts.toml"),
            config_dir: root.clone(),
            pid_file: root.join("daemon.pid"),
            runtime_file: root.join("runtime.json"),
            database_file: root.join("runtime.sqlite3"),
        }
    }

    #[test]
    fn invalid_settings_path_does_not_poison_live_or_saved_config() {
        let paths = test_paths();
        let mut ui = ui();
        let original = std::env::temp_dir().join(format!(
            "codex-switcher-valid-home-{}",
            uuid::Uuid::new_v4()
        ));
        ui.config.codex_home.clone_from(&original);
        ui.open_text_editor(Modal::Settings, "'relative-and-shell-quoted/.codex'");

        submit_text_editor(&paths, &mut ui);

        assert_eq!(ui.config.codex_home, original);
        assert!(
            ui.editor
                .error
                .as_deref()
                .is_some_and(|error| error.contains("绝对路径"))
        );
        assert_eq!(ui.modal, Modal::Settings);
        assert!(!paths.config_file.exists());
    }

    #[test]
    fn enter_on_confirmation_uses_safe_default() {
        let paths = test_paths();
        let mut ui = ui();
        ui.open_confirmation(Modal::ConfirmDelete);

        handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &paths,
            &mut ui,
        )
        .unwrap();

        assert_eq!(ui.modal, Modal::None);
        assert!(ui.index.accounts.is_empty());
    }

    #[test]
    fn help_center_supports_pages_and_keyboard_scrolling() {
        let paths = test_paths();
        let mut ui = ui();
        ui.open_help();
        handle_key(key('4'), &paths, &mut ui).unwrap();
        assert_eq!(ui.help_page, HelpPage::Safety);
        handle_key(key('j'), &paths, &mut ui).unwrap();
        assert_eq!(ui.help_scroll, 1);
        handle_key(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            &paths,
            &mut ui,
        )
        .unwrap();
        assert_eq!(ui.help_page, HelpPage::Proxy);
        assert_eq!(ui.help_scroll, 0);
    }

    #[test]
    fn path_completion_prioritizes_directories_and_hides_dotfiles() {
        let root = std::env::temp_dir().join(format!(
            "codex-switcher-completion-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("folder")).unwrap();
        std::fs::write(root.join("auth.json"), "{}").unwrap();
        std::fs::write(root.join(".secret"), "hidden").unwrap();

        let all = path_suggestions(&format!("{}/", root.display()), false);
        assert_eq!(
            all.first().map(|item| item.display.as_str()),
            Some("folder/")
        );
        assert!(all.iter().any(|item| item.display == "auth.json"));
        assert!(!all.iter().any(|item| item.display == ".secret"));

        let directories = path_suggestions(&format!("{}/", root.display()), true);
        assert_eq!(directories.len(), 1);
        assert!(directories[0].directory);
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

    #[test]
    fn event_navigation_clamps_at_both_ends() {
        let mut ui = ui();
        ui.workspace = Workspace::Proxy;
        ui.proxy_panel = ProxyPanel::Events;
        ui.recent_events = (0..3)
            .map(|index| crate::storage::RuntimeEvent {
                id: index.to_string(),
                occurred_at: chrono::Utc::now(),
                tenant_id: "local".into(),
                device_id: "test".into(),
                client_instance_id: None,
                kind: format!("event-{index}"),
                account_id: None,
                detail: "safe".into(),
            })
            .collect();
        let root = crate::paths::paths();
        for _ in 0..5 {
            handle_key(key('j'), &root, &mut ui).unwrap();
        }
        assert_eq!(ui.event_selected, 2);
        for _ in 0..5 {
            handle_key(key('k'), &root, &mut ui).unwrap();
        }
        assert_eq!(ui.event_selected, 0);

        ui.event_selected = 2;
        ui.recent_events.truncate(1);
        ui.clamp_event_selection();
        assert_eq!(ui.event_selected, 0);
    }
}
