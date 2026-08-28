use super::{Modal, Ui, UiTab, actions::*, draw};
use crate::{error::Result, paths::Paths};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

const TICK_RATE: Duration = Duration::from_millis(100);

pub fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ui: &mut Ui,
    paths: &Paths,
) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        // 轮询后台任务
        poll_probe(paths, ui)?;

        terminal.draw(|f| draw(f, ui))?;

        let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());

        if event::poll(timeout)?
            && let Event::Key(key) = event::read()?
            && handle_key(key, paths, ui)?
        {
            return Ok(());
        }

        if last_tick.elapsed() >= TICK_RATE {
            ui.tick += 1;
            if ui.tick.is_multiple_of(10) {
                refresh_control_snapshot(paths, ui);
            }
            last_tick = Instant::now();
        }
    }
}

pub fn handle_key(key: KeyEvent, p: &Paths, ui: &mut Ui) -> Result<bool> {
    // Ctrl+C 退出
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Ok(true);
    }

    // 如果有modal，处理modal逻辑
    if ui.modal != Modal::None {
        return handle_modal(key, p, ui);
    }

    // 如果正在检测，阻止某些操作
    if ui.checking.is_some()
        && matches!(
            key.code,
            KeyCode::Char('r' | 'R' | 'a' | 'i' | 'n' | 'd') | KeyCode::Enter
        )
    {
        ui.notice = "检测进行中；完成后再修改账户。".into();
        return Ok(false);
    }

    // 主界面按键处理
    let visible = ui.visible();
    match (key.modifiers, key.code) {
        (KeyModifiers::SHIFT, KeyCode::BackTab) | (_, KeyCode::BackTab) => {
            ui.tab = ui.tab.previous()
        }
        (_, KeyCode::Tab) => ui.tab = ui.tab.next(),
        (_, KeyCode::Char('q')) => return Ok(true),
        (_, KeyCode::Esc) => {
            ui.filter.clear();
            ui.notice = "已清除过滤".into();
        }
        (_, KeyCode::Char('j')) | (_, KeyCode::Down) => {
            if !visible.is_empty() {
                ui.selected = (ui.selected + 1).min(visible.len() - 1);
            }
        }
        (_, KeyCode::Char('k')) | (_, KeyCode::Up) => {
            ui.selected = ui.selected.saturating_sub(1);
        }
        (KeyModifiers::CONTROL, KeyCode::Char('n')) => {
            ui.selected = (ui.selected + 10).min(visible.len().saturating_sub(1));
        }
        (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
            ui.selected = ui.selected.saturating_sub(10);
        }
        (_, KeyCode::Char('g')) => ui.selected = 0,
        (_, KeyCode::Char('G')) => ui.selected = visible.len().saturating_sub(1),
        (_, KeyCode::Char('?')) => {
            ui.modal = Modal::Help;
        }
        (_, KeyCode::Char('m')) => {
            ui.modal = Modal::ModeSelector;
        }
        (_, KeyCode::Char('/')) => {
            ui.modal = Modal::Filter;
            ui.input.clear();
        }
        (_, KeyCode::Char('a')) => {
            ui.notice = match import_current_auth(&ui.config, &mut ui.index, p) {
                Ok(msg) => msg,
                Err(e) => e.to_string(),
            };
        }
        (_, KeyCode::Char('i')) => {
            ui.modal = Modal::Import;
            ui.input.clear();
        }
        (_, KeyCode::Char('n')) => {
            if let Some(i) = ui.selected_id() {
                if ui.index.accounts[i].email.is_some() {
                    ui.modal = Modal::ConfirmUseEmail;
                } else {
                    ui.input = ui.index.accounts[i].label.clone();
                    ui.modal = Modal::Rename;
                }
            }
        }
        (_, KeyCode::Char('d')) => {
            if ui.selected_id().is_some() {
                ui.modal = Modal::ConfirmDelete;
            }
        }
        (_, KeyCode::Char('s')) => {
            ui.input = ui.config.codex_home.display().to_string();
            ui.modal = Modal::Settings;
        }
        (_, KeyCode::Char('t')) => {
            ui.config.theme = ui.config.theme.next();
            ui.notice = match save_current_config(p, &ui.config) {
                Ok(_) => format!("已切换主题：{}", ui.config.theme.name()),
                Err(e) => e.to_string(),
            };
        }
        (_, KeyCode::Enter) if ui.tab == UiTab::Settings => {
            let integration = crate::integration::CodexIntegration::new(&ui.config.codex_home);
            ui.notice = match integration.status() {
                Ok(crate::integration::IntegrationStatus::Enabled) => {
                    ui.modal = Modal::ConfirmIntegrationDisable;
                    "请确认停用 Codex 代理接入".into()
                }
                Ok(crate::integration::IntegrationStatus::Disabled) => {
                    ui.modal = Modal::ConfirmIntegrationEnable;
                    "请确认启用 Codex 代理接入".into()
                }
                Ok(crate::integration::IntegrationStatus::Drifted(diff)) => {
                    format!("配置漂移，拒绝覆盖：{diff}")
                }
                Err(e) => e.to_string(),
            };
        }
        (_, KeyCode::Enter) if ui.tab == UiTab::Accounts => {
            if let Some(i) = ui.selected_id() {
                let account = &ui.index.accounts[i];
                ui.notice = match activate_account(&ui.config, account) {
                    Ok(msg) => msg,
                    Err(e) => e.to_string(),
                };
            }
        }
        (_, KeyCode::Enter) => {
            ui.notice = "当前页没有可展开的条目".into();
        }
        (_, KeyCode::Char('r')) => {
            if let Some(i) = ui.selected_id() {
                let account = ui.index.accounts[i].clone();
                ui.notice = format!("正在检测 {}…", account.label);
                start_probe(ui, vec![account]);
            }
        }
        (_, KeyCode::Char('R')) => {
            ui.notice = "正在检测全部账户…".into();
            start_probe(ui, ui.index.accounts.clone());
        }
        (_, KeyCode::Char(' ')) if ui.tab == UiTab::Accounts => {
            if let Some(i) = ui.selected_id() {
                ui.index.accounts[i].proxy_enabled = !ui.index.accounts[i].proxy_enabled;
                ui.notice = match crate::account::save_index(p, &ui.index) {
                    Ok(()) => {
                        if ui.index.accounts[i].proxy_enabled {
                            "账户已加入代理池".into()
                        } else {
                            "账户已移出代理池".into()
                        }
                    }
                    Err(e) => e.to_string(),
                };
            }
        }
        (_, KeyCode::Char(' ')) if ui.tab == UiTab::Settings => {
            ui.config.proxy.auto_switch = !ui.config.proxy.auto_switch;
            ui.notice = match save_current_config(p, &ui.config) {
                Ok(_) => {
                    if ui.attached_daemon {
                        let _ = tokio::runtime::Runtime::new().and_then(|runtime| {
                            runtime
                                .block_on(crate::daemon::control_request(
                                    p,
                                    hyper::Method::POST,
                                    "/v1/daemon/reload",
                                ))
                                .map(|_| ())
                                .map_err(|error| std::io::Error::other(error.to_string()))
                        });
                    }
                    if ui.config.proxy.auto_switch {
                        "已明确启用自动切换".into()
                    } else {
                        "已关闭自动切换".into()
                    }
                }
                Err(error) => error.to_string(),
            };
        }
        (_, KeyCode::Char('p')) => {
            ui.notice = if ui.attached_daemon {
                let endpoint = if ui.routing_paused {
                    "/v1/proxy/resume"
                } else {
                    "/v1/proxy/pause"
                };
                match tokio::runtime::Runtime::new().and_then(|runtime| {
                    runtime
                        .block_on(crate::daemon::control_request(
                            p,
                            hyper::Method::POST,
                            endpoint,
                        ))
                        .map_err(|error| std::io::Error::other(error.to_string()))
                }) {
                    Ok(_) => {
                        ui.routing_paused = !ui.routing_paused;
                        if ui.routing_paused {
                            "路由已暂停".into()
                        } else {
                            "路由已恢复".into()
                        }
                    }
                    Err(error) => format!("控制面请求失败：{error}"),
                }
            } else {
                "当前没有已附着代理".into()
            };
        }
        (_, KeyCode::Char('x')) if ui.tab == UiTab::Accounts => {
            if let Some(i) = ui.selected_id() {
                let account = &ui.index.accounts[i];
                if ui.attached_daemon {
                    let endpoint = format!("/v1/accounts/{}/switch", account.id);
                    ui.notice = match tokio::runtime::Runtime::new().and_then(|runtime| {
                        runtime
                            .block_on(crate::daemon::control_request(
                                p,
                                hyper::Method::POST,
                                &endpoint,
                            ))
                            .map_err(|error| std::io::Error::other(error.to_string()))
                    }) {
                        Ok(_) => format!("已在下一安全请求边界切换到 {}", account.label),
                        Err(error) => format!("切换失败：{error}"),
                    };
                } else if let Some(proxy) = ui.proxy_state.as_ref() {
                    proxy.stats.write().current_account = Some(account.id);
                    ui.notice = format!("已在安全请求边界切换到 {}", account.label);
                }
            }
        }
        _ => {}
    }

    Ok(false)
}

fn handle_modal(key: KeyEvent, p: &Paths, ui: &mut Ui) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') if ui.modal == Modal::Help => {
            ui.modal = Modal::None;
        }
        KeyCode::Esc => {
            ui.modal = Modal::None;
            ui.input.clear();
        }
        KeyCode::Char('y') if ui.modal == Modal::ConfirmDelete => {
            if let Some(i) = ui.selected_id() {
                ui.notice = match delete_account(&ui.config, &mut ui.index, p, i) {
                    Ok(msg) => {
                        ui.selected = ui.selected.saturating_sub(1);
                        msg
                    }
                    Err(e) => e.to_string(),
                };
            }
            ui.modal = Modal::None;
        }
        KeyCode::Char('y')
            if matches!(
                ui.modal,
                Modal::ConfirmIntegrationEnable | Modal::ConfirmIntegrationDisable
            ) =>
        {
            let integration = crate::integration::CodexIntegration::new(&ui.config.codex_home);
            let result = if ui.modal == Modal::ConfirmIntegrationEnable {
                integration
                    .enable()
                    .map(|_| "已启用 Codex 代理接入；请重启 Codex".into())
            } else {
                integration
                    .disable()
                    .map(|_| "已停用 Codex 代理接入；请重启 Codex".into())
            };
            ui.notice = result.unwrap_or_else(|error| error.to_string());
            ui.modal = Modal::None;
        }
        KeyCode::Char('y') if ui.modal == Modal::ConfirmUseEmail => {
            if let Some(i) = ui.selected_id()
                && let Some(email) = ui.index.accounts[i].email.clone()
            {
                ui.notice = match rename_account(&mut ui.index, p, i, email) {
                    Ok(msg) => msg,
                    Err(e) => e.to_string(),
                };
            }
            ui.modal = Modal::None;
        }
        KeyCode::Char('n') if ui.modal == Modal::ConfirmUseEmail => {
            if let Some(i) = ui.selected_id() {
                ui.input = ui.index.accounts[i].label.clone();
                ui.modal = Modal::Rename;
            }
        }
        KeyCode::Enter => {
            match ui.modal {
                Modal::Import => {
                    let result = if ui.input.trim_start().starts_with('{') {
                        import_from_json(&ui.config, &mut ui.index, p, &ui.input, None)
                    } else {
                        import_from_path(&ui.config, &mut ui.index, p, ui.input.trim())
                    };
                    ui.notice = match result {
                        Ok(msg) => msg,
                        Err(e) => e.to_string(),
                    };
                }
                Modal::Filter => {
                    ui.filter = ui.input.clone();
                    ui.selected = 0;
                    ui.notice = if ui.filter.is_empty() {
                        "已清除过滤".into()
                    } else {
                        format!("按名称或邮箱过滤：{}", ui.filter)
                    };
                }
                Modal::Rename => {
                    if let Some(i) = ui.selected_id()
                        && !ui.input.trim().is_empty()
                    {
                        ui.notice = match rename_account(
                            &mut ui.index,
                            p,
                            i,
                            ui.input.trim().to_string(),
                        ) {
                            Ok(msg) => msg,
                            Err(e) => e.to_string(),
                        };
                    }
                }
                Modal::Settings => {
                    let path = PathBuf::from(ui.input.trim());
                    if path.as_os_str().is_empty() {
                        ui.notice = "路径不能为空".into();
                    } else {
                        ui.config.codex_home = path;
                        ui.notice = match save_current_config(p, &ui.config) {
                            Ok(msg) => msg,
                            Err(e) => e.to_string(),
                        };
                    }
                }
                Modal::ModeSelector => {
                    // TODO: 实现模式切换逻辑
                    ui.notice = "模式切换功能正在开发中".into();
                }
                Modal::Help
                | Modal::ConfirmUseEmail
                | Modal::ConfirmDelete
                | Modal::ConfirmIntegrationEnable
                | Modal::ConfirmIntegrationDisable
                | Modal::ProxySettings => {}
                Modal::None => unreachable!(),
            }
            ui.modal = Modal::None;
            ui.input.clear();
        }
        KeyCode::Backspace => {
            ui.input.pop();
        }
        KeyCode::Char(c)
            if matches!(
                ui.modal,
                Modal::Import
                    | Modal::Filter
                    | Modal::Rename
                    | Modal::Settings
                    | Modal::ProxySettings
            ) =>
        {
            ui.input.push(c);
        }
        _ => {}
    }

    Ok(false)
}
