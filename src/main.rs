#[cfg(test)]
use clap::Parser;
use codex_switcher::{
    Paths,
    account::load_index,
    cli::{Cli, Commands, ServiceSubcommand},
    config::{Config, load_config},
    daemon::{
        check_daemon_status, control_request, run_daemon, send_reload_signal, send_stop_signal,
    },
    error::Result,
    i18n::{Language, LanguagePreference, translate},
    paths::paths,
    types::AccountIndex,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() -> Result<()> {
    let p = paths();
    let bootstrap_language = bootstrap_language(&p);
    let cli = Cli::parse_localized(bootstrap_language);
    init_tracing(terminal_logging_enabled(&cli));

    // 处理守护进程控制命令（同步）
    if let Some(command) = &cli.command {
        if let Commands::Service(service) = command {
            return handle_service_command(&service.command);
        }
        return handle_daemon_command_sync(command, &p, bootstrap_language);
    }

    let mut config = load_config(&p)?;
    // Healthy v1 integrations used a distinct provider id, which made Codex
    // hide older sessions. Upgrade it before the TUI or daemon starts.
    codex_switcher::integration::CodexIntegration::new(&config.codex_home).migrate_if_needed()?;
    if cli.auto_switch {
        config.proxy.auto_switch = true;
    }
    if cli.daemon {
        config.proxy.enabled = true;
    }
    let index = load_index(&p)?;

    if cli.daemon {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async_main(cli, config, index, p))
    } else if cli.proxy {
        run_proxy_tui(config, index, p)
    } else {
        codex_switcher::tui::start_interactive(config, index, None)
    }
}

fn terminal_logging_enabled(cli: &Cli) -> bool {
    cli.daemon
}

/// A ratatui application owns the terminal while it is running. Writing
/// tracing output to stdout/stderr at the same time corrupts the alternate
/// screen, so only the headless daemon gets a terminal log layer. Proxy work
/// performed by an embedded daemon inherits the silent TUI subscriber.
fn init_tracing(log_to_terminal: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "codex_switcher=info".into());
    if log_to_terminal {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::sink))
            .init();
    }
}

async fn async_main(cli: Cli, config: Config, index: AccountIndex, p: Paths) -> Result<()> {
    if cli.daemon {
        run_daemon(config, index, p).await
    } else {
        unreachable!()
    }
}

fn run_proxy_tui(mut config: Config, index: AccountIndex, paths: Paths) -> Result<()> {
    // If the port belongs to our daemon, attach and leave it running when the
    // TUI exits. Otherwise create an embedded daemon and own its lifetime.
    let daemon_available = tokio::runtime::Runtime::new()
        .ok()
        .and_then(|runtime| {
            runtime
                .block_on(control_request(&paths, hyper::Method::GET, "/v1/snapshot"))
                .ok()
        })
        .is_some();
    if daemon_available {
        let _ = tokio::runtime::Runtime::new()?.block_on(control_request(
            &paths,
            hyper::Method::POST,
            "/v1/proxy/start",
        ));
        return codex_switcher::tui::start_interactive_in(
            config,
            index,
            None,
            codex_switcher::tui::Workspace::Proxy,
        );
    }
    config.proxy.enabled = true;
    let daemon_config = config.clone();
    let daemon_index = index.clone();
    let daemon_paths = paths.clone();
    let handle = std::thread::spawn(move || {
        tokio::runtime::Runtime::new()?.block_on(run_daemon(
            daemon_config,
            daemon_index,
            daemon_paths,
        ))
    });
    for _ in 0..40 {
        if paths.runtime_file.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    if !paths.runtime_file.exists() {
        return Err(codex_switcher::AppError::Message(
            "内嵌代理未能启动控制面".into(),
        ));
    }
    let mut current_config = config;
    let mut current_index = index;
    loop {
        if let Err(error) = codex_switcher::tui::start_interactive_in(
            current_config,
            current_index,
            None,
            codex_switcher::tui::Workspace::Proxy,
        ) {
            let _ = tokio::runtime::Runtime::new()?.block_on(control_request(
                &paths,
                hyper::Method::POST,
                "/v1/daemon/stop",
            ));
            let _ = handle.join();
            return Err(error);
        }

        let active = tokio::runtime::Runtime::new()?
            .block_on(control_request(&paths, hyper::Method::GET, "/v1/snapshot"))
            .ok()
            .and_then(|snapshot| {
                snapshot
                    .get("active_requests")
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or(0);
        if active == 0 {
            break;
        }

        println!("仍有 {active} 个活动请求。停止内嵌代理并最多排空 30 秒？[y/N]");
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim().eq_ignore_ascii_case("y") {
            break;
        }

        println!("已取消退出并返回 TUI，内嵌代理继续运行。");
        current_config = load_config(&paths)?;
        current_index = load_index(&paths)?;
    }

    let _ = tokio::runtime::Runtime::new()?.block_on(control_request(
        &paths,
        hyper::Method::POST,
        "/v1/daemon/stop",
    ));
    handle
        .join()
        .map_err(|_| codex_switcher::AppError::Message("内嵌代理线程异常退出".into()))?
}

fn handle_daemon_command_sync(command: &Commands, paths: &Paths, language: Language) -> Result<()> {
    let call = |endpoint: &str| -> Result<serde_json::Value> {
        tokio::runtime::Runtime::new()?.block_on(control_request(
            paths,
            hyper::Method::POST,
            endpoint,
        ))
    };
    match command {
        Commands::DaemonStatus => {
            match tokio::runtime::Runtime::new()?.block_on(control_request(
                paths,
                hyper::Method::GET,
                "/v1/snapshot",
            )) {
                Ok(snapshot) => println!(
                    "{}\n{}",
                    translate(language, "cli-status-running", None),
                    serde_json::to_string_pretty(&snapshot)?
                ),
                Err(_) => println!(
                    "{}: {}",
                    translate(language, "cli-status", None),
                    check_daemon_status(paths)?
                ),
            }
            Ok(())
        }
        Commands::DaemonStop => {
            println!("{}", translate(language, "cli-stopping", None));
            if call("/v1/daemon/stop").is_err() {
                send_stop_signal(paths)?;
            }
            println!("{}", translate(language, "cli-stop-sent", None));
            Ok(())
        }
        Commands::DaemonReload => {
            println!("{}", translate(language, "cli-reloading", None));
            if call("/v1/daemon/reload").is_err() {
                send_reload_signal(paths)?;
            }
            println!("{}", translate(language, "cli-reload-sent", None));
            Ok(())
        }
        Commands::Service(_) => unreachable!("service commands are handled before daemon commands"),
    }
}

fn handle_service_command(command: &ServiceSubcommand) -> Result<()> {
    use codex_switcher::daemon::windows_service::{ServiceCommand, handle};
    let command = match command {
        ServiceSubcommand::Install {
            service_root,
            codex_home,
        } => ServiceCommand::Install {
            service_root: service_root.clone(),
            codex_home: codex_home.clone(),
        },
        ServiceSubcommand::Start => ServiceCommand::Start,
        ServiceSubcommand::Stop => ServiceCommand::Stop,
        ServiceSubcommand::Status => ServiceCommand::Status,
        ServiceSubcommand::Uninstall => ServiceCommand::Uninstall,
        ServiceSubcommand::Run { service_root } => ServiceCommand::Run {
            service_root: service_root.clone(),
        },
    };
    handle(command)
}

fn bootstrap_language(paths: &Paths) -> Language {
    let preference = std::fs::read_to_string(&paths.config_file)
        .ok()
        .and_then(|raw| raw.parse::<toml::Value>().ok())
        .and_then(|value| value.get("language").cloned())
        .and_then(|value| value.try_into::<LanguagePreference>().ok())
        .unwrap_or_default();
    preference.resolve()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_headless_daemon_writes_tracing_to_the_terminal() {
        assert!(!terminal_logging_enabled(&Cli::parse_from([
            "codex-switcher"
        ])));
        assert!(!terminal_logging_enabled(&Cli::parse_from([
            "codex-switcher",
            "--proxy",
        ])));
        assert!(terminal_logging_enabled(&Cli::parse_from([
            "codex-switcher",
            "--daemon",
        ])));
    }
}
