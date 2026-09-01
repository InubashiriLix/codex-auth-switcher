mod control;
mod control_api;
mod hot_reload;
mod notifications;
pub mod windows_service;

pub use control::{
    DaemonStatus, check_daemon_status, remove_pid_file, send_reload_signal, send_stop_signal,
    write_pid_file,
};
pub use control_api::{
    AccountRuntimeSnapshot, Alert, ControlContext, ControlServer, ControlSnapshot, HealthSnapshot,
    InstanceSummary, ProxySnapshot, RuntimeDescriptor, control_request, control_request_json,
    control_stream, read_runtime,
};
pub use hot_reload::{HotReloader, ReloadEvent};
pub use notifications::NotificationManager;

use crate::{
    account::load_index,
    config::{Config, load_config},
    error::*,
    paths::Paths,
    proxy::{ProxyRuntime, ProxyServer, ProxyState, Recommender, TokenMonitor},
    storage::{MetadataStore, RetentionPolicy, RuntimeEvent},
    types::{AccountIndex, StatusKind},
};
use chrono::Utc;
use parking_lot::RwLock;
use std::{fs, future::Future, pin::Pin, sync::Arc, time::Duration};
#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal};
use tracing::{error, info, warn};
use uuid::Uuid;

struct PidFileCleanup(std::path::PathBuf);

impl Drop for PidFileCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub struct DaemonState {
    pub config: Config,
    pub accounts: Arc<RwLock<AccountIndex>>,
    pub current_account: Arc<RwLock<Option<Uuid>>>,
    pub proxy_server: Arc<ProxyServer>,
    pub proxy_state: ProxyState,
    pub proxy_runtime: ProxyRuntime,
    pub hot_reloader: HotReloader,
    pub recommender: Recommender,
    pub paths: Paths,
    pub metadata_store: Arc<MetadataStore>,
    pub accounts_revision: Arc<RwLock<u64>>,
}

impl DaemonState {
    pub fn new(config: Config, index: AccountIndex, paths: Paths) -> Result<Self> {
        let metadata_store = Arc::new(MetadataStore::open(
            &paths.database_file,
            RetentionPolicy {
                days: config.retention.days,
                max_requests: config.retention.max_requests,
                max_events: config.retention.max_events,
            },
        )?);
        if metadata_store.accounts_revision()? == 0 && paths.index_file.exists() {
            let backup = paths.index_file.with_extension("toml.pre-sqlite.bak");
            if !backup.exists() {
                fs::copy(&paths.index_file, backup)?;
            }
        }
        let (index, revision) = metadata_store.reconcile_accounts(&index)?;
        let accounts_revision = Arc::new(RwLock::new(revision));
        let accounts = Arc::new(RwLock::new(index));
        let current_account = Arc::new(RwLock::new(Self::find_active_account(&config, &accounts)));
        let proxy_state = ProxyState::new();

        // 设置初始统计
        if let Some(id) = *current_account.read() {
            proxy_state.stats.write().current_account = Some(id);
        }

        let proxy_server = Arc::new(ProxyServer::new(
            config.clone(),
            config.proxy.clone(),
            accounts.clone(),
            current_account.clone(),
            proxy_state.stats.clone(),
        ));
        let proxy_runtime = ProxyRuntime::new(proxy_server.clone(), proxy_state.clone());

        let recommender = Recommender::new(config.proxy.strategy.clone());
        let hot_reloader = HotReloader::new(&paths)?;
        proxy_server.attach_metadata_store(metadata_store.clone());

        Ok(Self {
            config,
            accounts,
            current_account,
            proxy_server,
            proxy_state,
            proxy_runtime,
            hot_reloader,
            recommender,
            paths,
            metadata_store,
            accounts_revision,
        })
    }

    fn find_active_account(config: &Config, accounts: &Arc<RwLock<AccountIndex>>) -> Option<Uuid> {
        let active_path = config.codex_home.join("auth.json");
        if !active_path.exists() {
            return None;
        }

        let active_content = fs::read(&active_path).ok()?;
        let accounts = accounts.read();

        accounts
            .accounts
            .iter()
            .find(|account| {
                let snapshot_path = crate::account::snapshot_path(config, account.id);
                fs::read(&snapshot_path)
                    .ok()
                    .map(|content| content == active_content)
                    .unwrap_or(false)
            })
            .map(|account| account.id)
    }

    pub async fn handle_reload(&mut self, event: ReloadEvent) -> Result<()> {
        match event {
            ReloadEvent::AccountsChanged => {
                info!("Accounts changed, reloading...");
                let new_index = load_index(&self.paths)?;
                let revision = self.metadata_store.replace_accounts(&new_index)?;
                let new_index = self.metadata_store.load_accounts()?;

                // 检查当前活跃账户是否还存在
                let current_id = *self.current_account.read();
                if let Some(id) = current_id {
                    let still_valid = new_index.accounts.iter().any(|a| a.id == id);

                    if !still_valid {
                        warn!("Current account removed, switching to fallback");
                        self.switch_to_fallback(&new_index).await?;
                    }
                }

                // 更新账户列表
                *self.accounts.write() = new_index;
                *self.accounts_revision.write() = revision;
                info!("Account index reloaded");
            }

            ReloadEvent::ConfigChanged => {
                info!("Config changed, reloading...");
                let new_config = load_config(&self.paths)?;

                // 只更新可以安全热更新的部分
                self.config.proxy = new_config.proxy.clone();
                self.config.theme = new_config.theme;
                self.config.language = new_config.language;

                info!("Configuration reloaded");
            }

            ReloadEvent::SnapshotChanged => {
                info!("Account snapshot changed, will re-probe on next check");
            }
        }
        Ok(())
    }

    async fn switch_to_fallback(&mut self, index: &AccountIndex) -> Result<()> {
        // 等待所有活跃请求完成
        let timeout = Duration::from_secs(30);
        let drained = self
            .proxy_server
            .connection_tracker()
            .wait_for_drain(timeout);

        if !drained {
            warn!(
                "Timeout waiting for {} connections, forcing switch",
                self.proxy_server.connection_tracker().active_count()
            );
        }

        // 找到第一个可用账户
        if let Some(account) = index
            .accounts
            .iter()
            .find(|a| a.enabled && a.proxy_enabled && a.status.kind == StatusKind::Live)
        {
            *self.current_account.write() = Some(account.id);
            self.proxy_state.stats.write().current_account = Some(account.id);
            info!("Switched to fallback account: {}", account.label);
        } else {
            warn!("No live accounts available for fallback");
        }

        Ok(())
    }

    pub async fn switch_account(&mut self, target_id: Uuid) -> Result<()> {
        let accounts = self.accounts.read();
        let target = accounts
            .accounts
            .iter()
            .find(|a| a.id == target_id)
            .ok_or_else(|| AppError::Message("Target account not found".into()))?;

        // 等待活跃请求完成
        let timeout = Duration::from_secs(self.config.proxy.cooldown_seconds);
        let _ = self
            .proxy_server
            .connection_tracker()
            .wait_for_drain(timeout);

        // 执行切换
        *self.current_account.write() = Some(target_id);

        let mut stats = self.proxy_state.stats.write();
        stats.current_account = Some(target_id);
        stats.auto_switches += 1;
        stats.last_switch = Some(Utc::now());

        info!("Switched to account: {}", target.label);

        // 发送桌面通知
        let message = crate::i18n::translate_with(
            self.config.language.resolve(),
            "notification-switched-to",
            [("account", target.label.as_str())],
        );
        if let Err(e) = self.send_notification(&message) {
            warn!("Failed to send notification: {}", e);
        }

        Ok(())
    }

    fn send_notification(&self, message: &str) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            use notify_rust::Notification;
            Notification::new()
                .summary("Codex Switcher")
                .body(message)
                .timeout(5000)
                .show()
                .map_err(|e| AppError::Message(format!("Notification error: {}", e)))?;
        }
        #[cfg(not(target_os = "linux"))]
        let _ = message;
        Ok(())
    }
}

pub fn run_daemon(
    config: Config,
    index: AccountIndex,
    paths: Paths,
) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
    Box::pin(run_daemon_impl(config, index, paths))
}

async fn run_daemon_impl(config: Config, index: AccountIndex, paths: Paths) -> Result<()> {
    let recovery_notice = config.startup_notice.clone();
    // 初始化守护进程状态
    let mut state = DaemonState::new(config.clone(), index, paths.clone())?;

    let _ = state.metadata_store.record_event(&RuntimeEvent {
        id: Uuid::new_v4().to_string(),
        occurred_at: Utc::now(),
        tenant_id: "local".into(),
        device_id: std::env::var("HOSTNAME").unwrap_or_else(|_| "local-device".into()),
        client_instance_id: None,
        kind: "daemon_started".into(),
        account_id: None,
        detail: "代理守护进程已启动".into(),
        message: Some(crate::i18n::LocalizedMessage::new("event-daemon-started")),
    });
    if let Some(notice) = recovery_notice {
        let _ = state.metadata_store.record_event(&RuntimeEvent {
            id: Uuid::new_v4().to_string(),
            occurred_at: Utc::now(),
            tenant_id: "local".into(),
            device_id: std::env::var("HOSTNAME").unwrap_or_else(|_| "local-device".into()),
            client_instance_id: None,
            kind: "config_recovered".into(),
            account_id: None,
            detail: crate::storage::sanitize(&notice),
            message: Some(crate::i18n::LocalizedMessage::new("event-config-recovered")),
        });
    }

    // 写入PID文件
    write_pid_file(&paths)?;
    let _pid_file_cleanup = PidFileCleanup(paths.pid_file.clone());

    // Cross-platform, authenticated control plane. The descriptor is private
    // and contains a random 256-bit bearer token.
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    let control = ControlServer::bind(ControlContext {
        config: Arc::new(RwLock::new(state.config.clone())),
        accounts: state.accounts.clone(),
        current_account: state.current_account.clone(),
        proxy_server: state.proxy_server.clone(),
        proxy_state: state.proxy_state.clone(),
        proxy_runtime: state.proxy_runtime.clone(),
        paths: paths.clone(),
        shutdown: shutdown_tx.clone(),
        metadata_store: state.metadata_store.clone(),
        accounts_revision: state.accounts_revision.clone(),
    })
    .await?;
    let control_handle = tokio::spawn(async move {
        if let Err(error) = control.serve().await {
            error!(%error, "control server exited");
        }
    });
    let cleanup_store = state.metadata_store.clone();
    let _cleanup_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let _ = cleanup_store.cleanup();
        }
    });

    if state.config.proxy.enabled {
        if has_eligible_proxy_account(&state.accounts.read(), state.config.proxy.threshold) {
            state.proxy_runtime.start();
        } else {
            state
                .proxy_runtime
                .mark_blocked("没有已入池且健康的账户，数据代理未启动");
        }
    }

    // 启动热重载监控
    let _reload_handle = {
        let mut state_clone = state.clone();
        tokio::spawn(async move {
            loop {
                if let Some(event) = state_clone.hot_reloader.poll()
                    && let Err(e) = state_clone.handle_reload(event).await
                {
                    error!("Hot reload failed: {}", e);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
    };

    // 启动Token监控
    let _monitor_handle = {
        let monitor = Arc::new(
            TokenMonitor::new(
                state.config.clone(),
                state.accounts.clone(),
                state.current_account.clone(),
                &state.config.proxy,
            )
            .with_router(state.proxy_server.router().clone())
            .with_store(
                state.metadata_store.clone(),
                state.accounts_revision.clone(),
                state.paths.clone(),
            ),
        );
        tokio::spawn(async move {
            monitor.start_monitoring().await;
        })
    };

    // 通知systemd服务已就绪
    #[cfg(unix)]
    {
        let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);
    }
    info!("Daemon started and ready");

    // 处理信号。控制 API 是跨平台主入口，Unix 信号仅保留兼容。
    #[cfg(unix)]
    {
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sighup = signal(SignalKind::hangup())?;
        let mut sigint = signal(SignalKind::interrupt())?;

        tokio::select! {
            _ = shutdown_rx.changed() => {
                info!("Received authenticated control shutdown");
            }
            _ = sigterm.recv() => {
                info!("Received SIGTERM, shutting down gracefully");
            }
            _ = sigint.recv() => {
                info!("Received SIGINT, shutting down gracefully");
            }
            _ = sighup.recv() => {
                info!("Received SIGHUP, reloading configuration");
                #[cfg(unix)]
                {
                    let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Reloading]);
                }
                if let Err(e) = force_reload_all(&mut state).await {
                    error!("Force reload failed: {}", e);
                }
                #[cfg(unix)]
                {
                    let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);
                }
                // 克隆数据，释放锁
                let accounts_clone = state.accounts.read().clone();
                // 继续运行，重新调用
                return Box::pin(run_daemon_impl(state.config, accounts_clone, state.paths)).await;
            }
        }
    }

    #[cfg(not(unix))]
    tokio::select! {
        _ = shutdown_rx.changed() => info!("Received authenticated control shutdown"),
        _ = tokio::signal::ctrl_c() => info!("Received Ctrl+C, shutting down gracefully"),
    }

    // 优雅关闭
    shutdown_gracefully(&state).await?;
    let _ = shutdown_tx.send(true);
    let _ = control_handle.await;

    // 清理PID文件
    remove_pid_file(&paths)?;

    Ok(())
}

async fn shutdown_gracefully(state: &DaemonState) -> Result<()> {
    info!("Draining active connections...");
    #[cfg(unix)]
    {
        let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Stopping]);
    }

    let drained = state.proxy_runtime.stop(Duration::from_secs(30)).await;

    if !drained {
        warn!(
            "Force closing {} connections",
            state.proxy_server.connection_tracker().active_count()
        );
    }

    info!("Shutdown complete");
    Ok(())
}

async fn force_reload_all(state: &mut DaemonState) -> Result<()> {
    let new_config = load_config(&state.paths)?;
    let new_index = load_index(&state.paths)?;
    let revision = state.metadata_store.replace_accounts(&new_index)?;
    let new_index = state.metadata_store.load_accounts()?;

    state.config = new_config;
    *state.accounts.write() = new_index;
    *state.accounts_revision.write() = revision;

    info!("Full reload completed");
    Ok(())
}

// 让DaemonState可以克隆（用于在不同任务间共享）
impl Clone for DaemonState {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            accounts: self.accounts.clone(),
            current_account: self.current_account.clone(),
            proxy_server: self.proxy_server.clone(),
            proxy_state: self.proxy_state.clone(),
            proxy_runtime: self.proxy_runtime.clone(),
            hot_reloader: self.hot_reloader.clone(),
            recommender: Recommender::new(self.config.proxy.strategy.clone()),
            paths: self.paths.clone(),
            metadata_store: self.metadata_store.clone(),
            accounts_revision: self.accounts_revision.clone(),
        }
    }
}

fn has_eligible_proxy_account(index: &AccountIndex, threshold: f64) -> bool {
    index.accounts.iter().any(|account| {
        account.enabled
            && account.proxy_enabled
            && account.status.kind == StatusKind::Live
            && account.status.checked_at.is_some_and(|checked| {
                Utc::now().signed_duration_since(checked).num_seconds() <= 90
            })
            && account
                .status
                .primary
                .as_ref()
                .is_some_and(|quota| quota.used_percent < threshold)
    })
}
