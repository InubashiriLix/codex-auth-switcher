use clap::Parser;
use codex_switcher::{
    Paths,
    account::load_index,
    cli::{Cli, Commands},
    config::{Config, load_config},
    daemon::{
        check_daemon_status, control_request, run_daemon, send_reload_signal, send_stop_signal,
    },
    error::Result,
    paths::paths,
    types::AccountIndex,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "codex_switcher=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();
    let p = paths();

    // 处理守护进程控制命令（同步）
    if let Some(command) = &cli.command {
        return handle_daemon_command_sync(command, &p);
    }

    let mut config = load_config(&p)?;
    if cli.auto_switch {
        config.proxy.auto_switch = true;
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

async fn async_main(cli: Cli, config: Config, index: AccountIndex, p: Paths) -> Result<()> {
    if cli.daemon {
        run_daemon(config, index, p).await
    } else {
        unreachable!()
    }
}

fn run_proxy_tui(config: Config, index: AccountIndex, paths: Paths) -> Result<()> {
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
        return codex_switcher::tui::start_interactive(config, index, None);
    }
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
        if let Err(error) =
            codex_switcher::tui::start_interactive(current_config, current_index, None)
        {
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

fn handle_daemon_command_sync(command: &Commands, paths: &Paths) -> Result<()> {
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
                    "守护进程状态: 运行中\n{}",
                    serde_json::to_string_pretty(&snapshot)?
                ),
                Err(_) => println!("守护进程状态: {}", check_daemon_status(paths)?),
            }
            Ok(())
        }
        Commands::DaemonStop => {
            println!("正在停止守护进程...");
            if call("/v1/daemon/stop").is_err() {
                send_stop_signal(paths)?;
            }
            println!("停止信号已发送");
            Ok(())
        }
        Commands::DaemonReload => {
            println!("正在热重载配置...");
            if call("/v1/daemon/reload").is_err() {
                send_reload_signal(paths)?;
            }
            println!("重载信号已发送");
            Ok(())
        }
    }
}
