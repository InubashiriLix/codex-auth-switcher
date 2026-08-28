use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "codex-switcher")]
#[command(version, about = "Codex authentication manager with proxy support", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Enable proxy mode with TUI
    #[arg(long, conflicts_with = "daemon")]
    pub proxy: bool,

    /// Enable auto-switching in proxy mode
    #[arg(long, requires = "proxy")]
    pub auto_switch: bool,

    /// Run as background daemon (no TUI)
    #[arg(long, conflicts_with = "proxy")]
    pub daemon: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Check daemon status
    DaemonStatus,
    /// Stop daemon gracefully
    DaemonStop,
    /// Reload daemon configuration (hot reload)
    DaemonReload,
}
