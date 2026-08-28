use crate::{
    config::Config,
    proxy::ProxyState,
    types::{Account, AccountIndex},
};
use std::sync::mpsc::Receiver;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiTab {
    Overview,
    Instances,
    Accounts,
    Events,
    Settings,
}

impl UiTab {
    pub const ALL: [Self; 5] = [
        Self::Overview,
        Self::Instances,
        Self::Accounts,
        Self::Events,
        Self::Settings,
    ];
    pub fn next(self) -> Self {
        Self::ALL[(self as usize + 1) % Self::ALL.len()]
    }
    pub fn previous(self) -> Self {
        Self::ALL[(self as usize + Self::ALL.len() - 1) % Self::ALL.len()]
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "总览",
            Self::Instances => "实例",
            Self::Accounts => "账户池",
            Self::Events => "事件",
            Self::Settings => "设置",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Modal {
    None,
    Import,
    Filter,
    Rename,
    ConfirmUseEmail,
    Settings,
    ConfirmDelete,
    Help,
    ModeSelector,
    ProxySettings,
    ConfirmIntegrationEnable,
    ConfirmIntegrationDisable,
}

pub struct Checking {
    pub receiver: Receiver<ProbeEvent>,
    pub total: usize,
    pub completed: usize,
    pub current: String,
}

pub enum ProbeEvent {
    Started { label: String },
    Completed(Box<Account>),
    Finished,
}

pub struct Ui {
    pub config: Config,
    pub index: AccountIndex,
    pub selected: usize,
    pub filter: String,
    pub modal: Modal,
    pub input: String,
    pub notice: String,
    pub tick: u64,
    pub proxy_state: Option<ProxyState>,
    pub checking: Option<Checking>,
    pub tab: UiTab,
    pub attached_daemon: bool,
    pub routing_paused: bool,
    pub active_requests: Vec<serde_json::Value>,
}

impl Ui {
    pub fn new(config: Config, index: AccountIndex, proxy_state: Option<ProxyState>) -> Self {
        Self {
            config,
            index,
            selected: 0,
            filter: String::new(),
            modal: Modal::None,
            input: String::new(),
            notice: "就绪".into(),
            tick: 0,
            proxy_state,
            checking: None,
            tab: UiTab::Overview,
            attached_daemon: crate::daemon::read_runtime(&crate::paths::paths()).is_ok(),
            routing_paused: false,
            active_requests: Vec::new(),
        }
    }

    pub fn visible(&self) -> Vec<usize> {
        self.index
            .accounts
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                self.filter.is_empty()
                    || a.label.to_lowercase().contains(&self.filter.to_lowercase())
                    || a.email
                        .as_ref()
                        .is_some_and(|x| x.to_lowercase().contains(&self.filter.to_lowercase()))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected_id(&self) -> Option<usize> {
        self.visible().get(self.selected).copied()
    }
}

use crate::error::Result;
use crossterm::{
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;

pub fn run_interactive_tui(
    config: Config,
    index: AccountIndex,
    proxy_state: Option<ProxyState>,
) -> Result<()> {
    let paths = crate::paths::paths();
    let mut ui = Ui::new(config, index, proxy_state);

    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let result = super::events::run_tui(&mut terminal, &mut ui, &paths);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
