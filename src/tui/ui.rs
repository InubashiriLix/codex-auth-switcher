use crate::{
    config::Config,
    daemon::ControlSnapshot,
    i18n::{Language, LanguagePreference, translate},
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
    LanguageSelector,
}

impl Modal {
    pub fn is_text_editor(self) -> bool {
        matches!(
            self,
            Self::Import | Self::Filter | Self::Rename | Self::Settings
        )
    }

    pub fn is_confirmation(self) -> bool {
        matches!(
            self,
            Self::ConfirmUseEmail
                | Self::ConfirmDelete
                | Self::ConfirmIntegrationEnable
                | Self::ConfirmIntegrationDisable
                | Self::ConfirmProxyStart
                | Self::ConfirmProxyStop
                | Self::ConfirmAutoSwitch
                | Self::ConfirmExit
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HelpPage {
    QuickStart,
    #[default]
    Account,
    Proxy,
    Safety,
}

impl HelpPage {
    pub const ALL: [Self; 4] = [Self::QuickStart, Self::Account, Self::Proxy, Self::Safety];

    pub fn title(self, language: Language) -> String {
        match self {
            Self::QuickStart => translate(language, "help-quick", None),
            Self::Account => translate(language, "help-account", None),
            Self::Proxy => translate(language, "help-proxy", None),
            Self::Safety => translate(language, "help-safety", None),
        }
    }

    pub fn next(self) -> Self {
        Self::ALL[(self as usize + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        Self::ALL[(self as usize + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    pub fn max_scroll(self) -> u16 {
        match self {
            Self::QuickStart => 30,
            Self::Account => 24,
            Self::Proxy => 34,
            Self::Safety => 30,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConfirmChoice {
    #[default]
    Cancel,
    Confirm,
}

impl ConfirmChoice {
    pub fn toggle(&mut self) {
        *self = match self {
            Self::Cancel => Self::Confirm,
            Self::Confirm => Self::Cancel,
        };
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputSuggestion {
    pub display: String,
    pub value: String,
    pub directory: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TextEditorState {
    pub value: String,
    /// UTF-8 byte offset, always kept on a character boundary.
    pub cursor: usize,
    pub suggestions: Vec<InputSuggestion>,
    pub suggestion_index: usize,
    pub error: Option<String>,
}

impl TextEditorState {
    pub fn reset(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.len();
        self.suggestions.clear();
        self.suggestion_index = 0;
        self.error = None;
    }

    pub fn clear(&mut self) {
        self.reset(String::new());
    }

    pub fn insert(&mut self, character: char) {
        self.value.insert(self.cursor, character);
        self.cursor += character.len_utf8();
        self.error = None;
    }

    pub fn move_left(&mut self) {
        self.cursor = previous_boundary(&self.value, self.cursor);
    }

    pub fn move_right(&mut self) {
        self.cursor = next_boundary(&self.value, self.cursor);
    }

    pub fn backspace(&mut self) {
        let previous = previous_boundary(&self.value, self.cursor);
        if previous != self.cursor {
            self.value.drain(previous..self.cursor);
            self.cursor = previous;
            self.error = None;
        }
    }

    pub fn delete(&mut self) {
        let next = next_boundary(&self.value, self.cursor);
        if next != self.cursor {
            self.value.drain(self.cursor..next);
            self.error = None;
        }
    }

    pub fn delete_previous_word(&mut self) {
        let mut start = self.cursor;
        while start > 0 {
            let previous = previous_boundary(&self.value, start);
            let character = self.value[previous..start].chars().next().unwrap_or(' ');
            if !character.is_whitespace() {
                break;
            }
            start = previous;
        }
        while start > 0 {
            let previous = previous_boundary(&self.value, start);
            let character = self.value[previous..start].chars().next().unwrap_or(' ');
            if character.is_whitespace() {
                break;
            }
            start = previous;
        }
        self.value.drain(start..self.cursor);
        self.cursor = start;
        self.error = None;
    }

    pub fn kill_before_cursor(&mut self) {
        self.value.drain(..self.cursor);
        self.cursor = 0;
        self.error = None;
    }

    pub fn kill_after_cursor(&mut self) {
        self.value.truncate(self.cursor);
        self.error = None;
    }

    pub fn next_suggestion(&mut self) {
        if !self.suggestions.is_empty() {
            self.suggestion_index = (self.suggestion_index + 1) % self.suggestions.len();
        }
    }

    pub fn previous_suggestion(&mut self) {
        if !self.suggestions.is_empty() {
            self.suggestion_index =
                (self.suggestion_index + self.suggestions.len() - 1) % self.suggestions.len();
        }
    }

    pub fn accept_suggestion(&mut self) {
        if let Some(suggestion) = self.suggestions.get(self.suggestion_index) {
            self.value.clone_from(&suggestion.value);
            self.cursor = self.value.len();
            self.error = None;
        }
    }
}

fn previous_boundary(value: &str, cursor: usize) -> usize {
    value[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_boundary(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .char_indices()
        .nth(1)
        .map_or(value.len(), |(index, _)| cursor + index)
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
    pub language_selected: usize,
    pub onboarding_checked: bool,
    pub filter: String,
    pub modal: Modal,
    pub detail: Option<DetailPage>,
    pub editor: TextEditorState,
    pub help_page: HelpPage,
    pub help_scroll: u16,
    pub confirm_choice: ConfirmChoice,
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
        mut config: Config,
        index: AccountIndex,
        proxy_state: Option<ProxyState>,
        workspace: Workspace,
    ) -> Self {
        let attached_daemon = crate::daemon::read_runtime(&crate::paths::paths()).is_ok();
        let (action_sender, action_updates) = std::sync::mpsc::channel();
        let language_selected = LanguagePreference::ALL
            .iter()
            .position(|language| *language == config.language)
            .unwrap_or(0);
        let initial_notice = config
            .startup_notice
            .take()
            .unwrap_or_else(|| translate(config.language.resolve(), "ready", None));
        let mut ui = Self {
            config,
            index,
            selected: 0,
            pool_selected: 0,
            instance_selected: 0,
            event_selected: 0,
            control_selected: 0,
            language_selected,
            onboarding_checked: false,
            filter: String::new(),
            modal: Modal::None,
            detail: None,
            editor: TextEditorState::default(),
            help_page: if workspace == Workspace::Proxy {
                HelpPage::Proxy
            } else {
                HelpPage::Account
            },
            help_scroll: 0,
            confirm_choice: ConfirmChoice::Cancel,
            notice: initial_notice,
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

    pub fn open_text_editor(&mut self, modal: Modal, value: impl Into<String>) {
        debug_assert!(modal.is_text_editor());
        self.modal = modal;
        self.editor.reset(value);
    }

    pub fn open_confirmation(&mut self, modal: Modal) {
        debug_assert!(modal.is_confirmation());
        self.modal = modal;
        self.confirm_choice = ConfirmChoice::Cancel;
    }

    pub fn language(&self) -> Language {
        self.config.language.resolve()
    }

    pub fn tr(&self, id: &str) -> String {
        translate(self.language(), id, None)
    }

    pub fn open_language_selector(&mut self) {
        self.language_selected = LanguagePreference::ALL
            .iter()
            .position(|language| *language == self.config.language)
            .unwrap_or(0);
        self.modal = Modal::LanguageSelector;
    }

    pub fn open_help(&mut self) {
        self.help_page = if self.workspace == Workspace::Proxy {
            HelpPage::Proxy
        } else {
            HelpPage::Account
        };
        self.help_scroll = 0;
        self.modal = Modal::Help;
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

    #[test]
    fn text_editor_keeps_a_utf8_safe_cursor() {
        let mut editor = TextEditorState::default();
        editor.reset("你a");
        editor.move_left();
        editor.insert('好');
        assert_eq!(editor.value, "你好a");
        assert_eq!(editor.cursor, "你好".len());

        editor.backspace();
        assert_eq!(editor.value, "你a");
        editor.delete_previous_word();
        assert_eq!(editor.value, "a");
        assert_eq!(editor.cursor, 0);
    }

    #[test]
    fn confirmation_always_opens_on_the_safe_choice() {
        let mut ui = Ui::new(
            Config::defaults(),
            AccountIndex::default(),
            None,
            Workspace::Accounts,
        );
        ui.confirm_choice = ConfirmChoice::Confirm;
        ui.open_confirmation(Modal::ConfirmDelete);
        assert_eq!(ui.confirm_choice, ConfirmChoice::Cancel);
    }
}
