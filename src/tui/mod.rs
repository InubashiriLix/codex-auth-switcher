mod ui;
mod theme;
mod draw;
mod events;
mod actions;

pub use ui::{Modal, Ui, Checking, ProbeEvent};
pub use theme::ThemeColors;
pub use draw::draw;
pub use events::{run_tui, handle_key};
pub use actions::*;

use crate::{
    config::Config,
    proxy::ProxyState,
    types::AccountIndex,
};

pub fn start_interactive(config: Config, index: AccountIndex, proxy_state: Option<ProxyState>) -> crate::Result<()> {
    ui::run_interactive_tui(config, index, proxy_state)
}
