use super::{draw, Modal, Ui, actions::*};
use crate::{
    error::Result,
    paths::Paths,
};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

const TICK_RATE: Duration = Duration::from_millis(100);

pub fn run_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, ui: &mut Ui, paths: &Paths) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        // 轮询后台任务
        poll_probe(paths, ui)?;

        terminal.draw(|f| draw(f, ui))?;

        let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if handle_key(key, paths, ui)? {
                    return Ok(());
                }
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            ui.tick += 1;
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
        (_, KeyCode::Enter) => {
            if let Some(i) = ui.selected_id() {
                let account = &ui.index.accounts[i];
                ui.notice = match activate_account(&ui.config, account) {
                    Ok(msg) => msg,
                    Err(e) => e.to_string(),
                };
            }
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
        KeyCode::Char('y') if ui.modal == Modal::ConfirmUseEmail => {
            if let Some(i) = ui.selected_id() {
                if let Some(email) = ui.index.accounts[i].email.clone() {
                    ui.notice = match rename_account(&mut ui.index, p, i, email) {
                        Ok(msg) => msg,
                        Err(e) => e.to_string(),
                    };
                }
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
                    if let Some(i) = ui.selected_id() {
                        if !ui.input.trim().is_empty() {
                            ui.notice = match rename_account(&mut ui.index, p, i, ui.input.trim().to_string()) {
                                Ok(msg) => msg,
                                Err(e) => e.to_string(),
                            };
                        }
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
                Modal::Help | Modal::ConfirmUseEmail | Modal::ConfirmDelete | Modal::ProxySettings => {}
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
                Modal::Import | Modal::Filter | Modal::Rename | Modal::Settings | Modal::ProxySettings
            ) =>
        {
            ui.input.push(c);
        }
        _ => {}
    }

    Ok(false)
}
