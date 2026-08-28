use clap::Parser;
use codex_switcher::{
    account::load_index,
    cli::{Cli, Commands},
    config::{load_config, Config},
    daemon::{check_daemon_status, run_daemon, send_reload_signal, send_stop_signal},
    error::Result,
    paths::paths,
    types::AccountIndex,
    Paths,
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

    let config = load_config(&p)?;
    let index = load_index(&p)?;

    // 根据参数决定运行模式
    if cli.daemon || cli.proxy {
        // 异步模式（守护进程/代理）
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async_main(cli, config, index, p))
    } else {
        // 同步TUI模式
        codex_switcher::tui::start_interactive(config, index, None)
    }
}

async fn async_main(cli: Cli, config: Config, index: AccountIndex, p: Paths) -> Result<()> {
    if cli.daemon {
        run_daemon(config, index, p).await
    } else if cli.proxy {
        println!("代理模式启动中，监听地址: {}", config.proxy.listen_addr);
        println!("请配置 Codex CLI: export HTTPS_PROXY=http://{}", config.proxy.listen_addr);
        println!("按 Ctrl+C 退出");
        run_daemon(config, index, p).await
    } else {
        unreachable!()
    }
}

fn handle_daemon_command_sync(command: &Commands, paths: &Paths) -> Result<()> {
    match command {
        Commands::DaemonStatus => {
            let status = check_daemon_status(paths)?;
            println!("守护进程状态: {}", status);
            Ok(())
        }
        Commands::DaemonStop => {
            println!("正在停止守护进程...");
            send_stop_signal(paths)?;
            println!("停止信号已发送");
            Ok(())
        }
        Commands::DaemonReload => {
            println!("正在热重载配置...");
            send_reload_signal(paths)?;
            println!("重载信号已发送");
            Ok(())
        }
    }
}
