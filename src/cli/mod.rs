use crate::i18n::{Language, translate};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use std::path::PathBuf;

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

    /// Force the interactive TUI into its full Unicode/color presentation
    #[arg(long, visible_alias = "force-ascii", conflicts_with = "daemon")]
    pub force_tty_mode: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Check daemon status
    DaemonStatus,
    /// Stop daemon gracefully
    DaemonStop,
    /// Reload daemon configuration (hot reload)
    DaemonReload,
    /// Manage the native Windows Service
    Service(ServiceArgs),
}

#[derive(Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceSubcommand,
}

#[derive(Subcommand)]
pub enum ServiceSubcommand {
    /// Install and start the LocalService daemon (Windows only)
    Install {
        #[arg(long)]
        service_root: PathBuf,
        #[arg(long)]
        codex_home: PathBuf,
    },
    /// Start the installed Windows Service
    Start,
    /// Stop the installed Windows Service
    Stop,
    /// Show Windows Service status
    Status,
    /// Remove the Windows Service registration
    Uninstall,
    /// Internal SCM entrypoint
    #[command(hide = true)]
    Run {
        #[arg(long)]
        service_root: PathBuf,
    },
}

impl Cli {
    fn localized_command(language: Language) -> clap::Command {
        let template = format!(
            "{{about-with-newline}}\n{}: {{usage}}\n\n{}:\n{{subcommands}}\n\n{}:\n{{options}}",
            translate(language, "cli-usage", None),
            translate(language, "cli-commands", None),
            translate(language, "cli-options", None),
        );
        let mut command = Self::command()
            .about(translate(language, "cli-about", None))
            .help_template(template)
            .disable_help_flag(true)
            .disable_version_flag(true)
            .disable_help_subcommand(true)
            .arg(
                clap::Arg::new("localized-help")
                    .short('h')
                    .long("help")
                    .help(translate(language, "cli-print-help", None))
                    .action(clap::ArgAction::Help),
            )
            .arg(
                clap::Arg::new("localized-version")
                    .short('V')
                    .long("version")
                    .help(translate(language, "cli-print-version", None))
                    .action(clap::ArgAction::Version),
            )
            .mut_arg("proxy", |arg| {
                arg.help(translate(language, "cli-proxy", None))
            })
            .mut_arg("auto_switch", |arg| {
                arg.help(translate(language, "cli-auto-switch", None))
            })
            .mut_arg("daemon", |arg| {
                arg.help(translate(language, "cli-daemon", None))
            })
            .mut_arg("force_tty_mode", |arg| {
                arg.help(translate(language, "cli-force-ascii", None))
            });
        for (name, key) in [
            ("daemon-status", "cli-daemon-status"),
            ("daemon-stop", "cli-daemon-stop"),
            ("daemon-reload", "cli-daemon-reload"),
        ] {
            if let Some(subcommand) = command.find_subcommand_mut(name) {
                *subcommand = subcommand.clone().about(translate(language, key, None));
            }
        }
        if let Some(service) = command.find_subcommand_mut("service") {
            let help = || {
                clap::Arg::new("service-help")
                    .short('h')
                    .long("help")
                    .help(translate(language, "cli-print-help", None))
                    .action(clap::ArgAction::Help)
            };
            *service = service
                .clone()
                .about(translate(language, "cli-service", None))
                .arg(help());
            for (name, key) in [
                ("install", "cli-service-install"),
                ("start", "cli-service-start"),
                ("stop", "cli-service-stop"),
                ("status", "cli-service-status"),
                ("uninstall", "cli-service-uninstall"),
            ] {
                if let Some(action) = service.find_subcommand_mut(name) {
                    let mut localized = action
                        .clone()
                        .about(translate(language, key, None))
                        .arg(help());
                    if name == "install" {
                        localized = localized
                            .mut_arg("service_root", |arg| {
                                arg.help(translate(language, "cli-service-root", None))
                            })
                            .mut_arg("codex_home", |arg| {
                                arg.help(translate(language, "cli-codex-home", None))
                            });
                    }
                    *action = localized;
                }
            }
        }
        command
    }

    pub fn parse_localized(language: Language) -> Self {
        let matches = Self::localized_command(language).get_matches();
        Self::from_arg_matches(&matches).unwrap_or_else(|error| error.exit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_text_is_localized_before_argument_parsing() {
        let help = Cli::localized_command(Language::Es)
            .render_long_help()
            .to_string();
        assert!(help.contains("Gestor de autenticación"), "{help}");
        assert!(help.contains("Activar el modo proxy"), "{help}");
    }

    #[test]
    fn service_install_requires_explicit_roots() {
        assert!(Cli::try_parse_from(["codex-switcher", "service", "install"]).is_err());
        assert!(
            Cli::try_parse_from([
                "codex-switcher",
                "service",
                "install",
                "--service-root",
                "/service-root",
                "--codex-home",
                "/codex-home",
            ])
            .is_ok()
        );
    }

    #[test]
    fn force_ascii_cannot_be_combined_with_the_headless_daemon() {
        assert!(Cli::try_parse_from(["codex-switcher", "--force-ascii", "--daemon"]).is_err());
    }
}
