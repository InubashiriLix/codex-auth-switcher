use crate::i18n::{Language, translate};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

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
}
