use crate::{
    config::Config,
    daemon::ControlSnapshot,
    proxy::{ProxyState, RuntimeState},
    storage::{MetricsWindow, RequestSummary, RuntimeEvent},
    types::{Account, AccountIndex, StatusKind},
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::AtomicBool,
        mpsc::{Receiver, Sender},
    },
    thread::JoinHandle,
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Workspace {
    #[default]
    Accounts,
    Proxy,
}

impl Workspace {
    pub fn badge(self) -> &'static str {
        match self {
            Self::Accounts => "ACCOUNT",
            Self::Proxy => "PROXY",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProxyPanel {
    #[default]
    Control,
    Pool,
    Instances,
    Events,
}

impl ProxyPanel {
    pub const ALL: [Self; 4] = [Self::Control, Self::Pool, Self::Instances, Self::Events];

    pub fn next(self) -> Self {
        Self::ALL[(self as usize + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        Self::ALL[(self as usize + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    pub fn number(self) -> usize {
        self as usize + 1
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetailPage {
    Account(Uuid),
    Control,
    Instance(usize),
    Request(usize),
    Event(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    ConfirmIntegrationEnable,
    ConfirmIntegrationDisable,
    ConfirmProxyStart,
    ConfirmProxyStop,
    ConfirmAutoSwitch,
    ConfirmExit,
    Onboarding,
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

pub enum ControlUpdate {
    Connected {
        snapshot: Box<ControlSnapshot>,
        events: Vec<RuntimeEvent>,
        requests: Vec<RequestSummary>,
        metrics: Box<MetricsWindow>,
        metrics_1m: Box<MetricsWindow>,
    },
    Disconnected,
}

pub enum ActionUpdate {
    Success(String),
    Error(String),
}

pub struct Ui {
    pub config: Config,
    pub index: AccountIndex,
    pub selected: usize,
    pub pool_selected: usize,
    pub instance_selected: usize,
    pub event_selected: usize,
    pub control_selected: usize,
    pub onboarding_checked: bool,
    pub filter: String,
    pub modal: Modal,
    pub detail: Option<DetailPage>,
    pub input: String,
    pub notice: String,
    pub tick: u64,
    pub proxy_state: Option<ProxyState>,
    pub checking: Option<Checking>,
    pub workspace: Workspace,
    pub proxy_panel: ProxyPanel,
    pub attached_daemon: bool,
    pub owned_daemon: Option<JoinHandle<crate::Result<()>>>,
    pub control_updates: Option<Receiver<ControlUpdate>>,
    pub control_stop: Arc<AtomicBool>,
    pub action_sender: Sender<ActionUpdate>,
    pub action_updates: Receiver<ActionUpdate>,
    pub snapshot: Option<ControlSnapshot>,
    pub recent_events: Vec<RuntimeEvent>,
    pub recent_requests: Vec<RequestSummary>,
    pub metrics: Option<MetricsWindow>,
    pub metrics_1m: Option<MetricsWindow>,
    pub request_history: VecDeque<u64>,
    pub failure_history: VecDeque<u64>,
    pub ttfb_history: VecDeque<u64>,
}

impl Ui {
    pub fn new(
        config: Config,
        index: AccountIndex,
        proxy_state: Option<ProxyState>,
        workspace: Workspace,
    ) -> Self {
        let attached_daemon = crate::daemon::read_runtime(&crate::paths::paths()).is_ok();
        let (action_sender, action_updates) = std::sync::mpsc::channel();
        let mut ui = Self {
            config,
            index,
            selected: 0,
            pool_selected: 0,
            instance_selected: 0,
            event_selected: 0,
            control_selected: 0,
            onboarding_checked: false,
            filter: String::new(),
            modal: Modal::None,
            detail: None,
            input: String::new(),
            notice: "就绪".into(),
            tick: 0,
            proxy_state,
            checking: None,
            workspace,
            proxy_panel: ProxyPanel::Control,
            attached_daemon,
            owned_daemon: None,
            control_updates: None,
            control_stop: Arc::new(AtomicBool::new(false)),
            action_sender,
            action_updates,
            snapshot: None,
            recent_events: Vec::new(),
            recent_requests: Vec::new(),
            metrics: None,
            metrics_1m: None,
            request_history: VecDeque::with_capacity(30),
            failure_history: VecDeque::with_capacity(30),
            ttfb_history: VecDeque::with_capacity(30),
        };
        if workspace == Workspace::Proxy && !attached_daemon && ui.needs_onboarding() {
            ui.modal = Modal::Onboarding;
            ui.onboarding_checked = true;
        }
        ui
    }

    pub fn visible(&self) -> Vec<usize> {
        self.index
            .accounts
            .iter()
            .enumerate()
            .filter(|(_, account)| {
                self.filter.is_empty()
                    || account
                        .label
                        .to_lowercase()
                        .contains(&self.filter.to_lowercase())
                    || account.email.as_ref().is_some_and(|email| {
                        email.to_lowercase().contains(&self.filter.to_lowercase())
                    })
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub fn selected_id(&self) -> Option<usize> {
        self.visible().get(self.selected).copied()
    }

    pub fn pool_selected_id(&self) -> Option<usize> {
        (!self.index.accounts.is_empty())
            .then(|| self.pool_selected.min(self.index.accounts.len() - 1))
    }

    pub fn runtime_state(&self) -> RuntimeState {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.proxy.runtime_state.clone())
            .unwrap_or_default()
    }

    pub fn eligible_accounts(&self) -> usize {
        self.snapshot.as_ref().map_or_else(
            || {
                self.index
                    .accounts
                    .iter()
                    .filter(|account| {
                        account.proxy_enabled
                            && account.status.kind == StatusKind::Live
                            && account.status.checked_at.is_some_and(|checked| {
                                chrono::Utc::now()
                                    .signed_duration_since(checked)
                                    .num_seconds()
                                    <= 90
                            })
                            && account.status.primary.as_ref().is_some_and(|quota| {
                                quota.used_percent < self.config.proxy.threshold
                            })
                    })
                    .count()
            },
            |snapshot| snapshot.eligible_accounts,
        )
    }

    pub fn integration_enabled(&self) -> bool {
        self.snapshot.as_ref().map_or_else(
            || {
                matches!(
                    crate::integration::CodexIntegration::new(&self.config.codex_home).status(),
                    Ok(crate::integration::IntegrationStatus::Enabled)
                )
            },
            |snapshot| {
                matches!(
                    &snapshot.integration_state,
                    crate::integration::IntegrationStatus::Enabled
                )
            },
        )
    }

    pub fn needs_onboarding(&self) -> bool {
        let pool_empty = !self
            .index
            .accounts
            .iter()
            .any(|account| account.proxy_enabled);
        let integration_drifted = self.snapshot.as_ref().map_or_else(
            || {
                matches!(
                    crate::integration::CodexIntegration::new(&self.config.codex_home).status(),
                    Ok(crate::integration::IntegrationStatus::Drifted(_))
                )
            },
            |snapshot| {
                matches!(
                    &snapshot.integration_state,
                    crate::integration::IntegrationStatus::Drifted(_)
                )
            },
        );
        !self.config.onboarding_acknowledged || pool_empty || integration_drifted
    }

    pub fn switch_workspace(&mut self, workspace: Workspace) {
        self.workspace = workspace;
        self.detail = None;
        self.modal = Modal::None;
        if workspace == Workspace::Proxy && !self.onboarding_checked && self.needs_onboarding() {
            self.modal = Modal::Onboarding;
            self.onboarding_checked = true;
        }
    }

    pub fn push_metric_sample(&mut self, requests: u64, failures: u64, ttfb: u64) {
        for (history, value) in [
            (&mut self.request_history, requests),
            (&mut self.failure_history, failures),
            (&mut self.ttfb_history, ttfb),
        ] {
            if history.len() == 30 {
                history.pop_front();
            }
            history.push_back(value);
        }
    }

    pub fn clamp_event_selection(&mut self) {
        let length = self.recent_requests.len() + self.recent_events.len();
        self.event_selected = self.event_selected.min(length.saturating_sub(1));
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
    workspace: Workspace,
) -> Result<()> {
    let paths = crate::paths::paths();
    let mut ui = Ui::new(config, index, proxy_state, workspace);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CheckStatus;

    #[test]
    fn acknowledged_onboarding_does_not_return_just_because_proxy_is_stopped() {
        let mut config = Config::defaults();
        config.onboarding_acknowledged = true;
        config.codex_home = std::env::temp_dir().join(format!(
            "codex-switcher-onboarding-{}",
            uuid::Uuid::new_v4()
        ));
        let index = AccountIndex {
            accounts: vec![Account {
                id: uuid::Uuid::new_v4(),
                label: "test".into(),
                source: "test".into(),
                imported_at: chrono::Utc::now(),
                email: None,
                plan: None,
                account_id: None,
                status: CheckStatus::default(),
                tenant_id: "local".into(),
                proxy_enabled: true,
            }],
        };
        let mut ui = Ui::new(config, index, None, Workspace::Accounts);
        assert_eq!(ui.runtime_state(), RuntimeState::Stopped);
        assert!(!ui.needs_onboarding());

        ui.config.onboarding_acknowledged = false;
        assert!(ui.needs_onboarding());
    }
}
