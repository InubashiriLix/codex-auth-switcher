mod actions;
mod draw;
mod events;
mod theme;
mod ui;

pub use actions::*;
pub use draw::draw;
pub use events::{handle_key, run_tui};
pub use theme::ThemeColors;
pub use ui::{
    ActionUpdate, Checking, ControlUpdate, DetailPage, Modal, ProbeEvent, ProxyPanel, Ui, Workspace,
};

use crate::{config::Config, proxy::ProxyState, types::AccountIndex};

pub fn start_interactive(
    config: Config,
    index: AccountIndex,
    proxy_state: Option<ProxyState>,
) -> crate::Result<()> {
    ui::run_interactive_tui(config, index, proxy_state, Workspace::Accounts)
}

pub fn start_interactive_in(
    config: Config,
    index: AccountIndex,
    proxy_state: Option<ProxyState>,
    workspace: Workspace,
) -> crate::Result<()> {
    ui::run_interactive_tui(config, index, proxy_state, workspace)
}
