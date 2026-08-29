//! Native Windows Service integration for the headless daemon.
//!
//! The service deliberately runs as LocalService and receives an explicit
//! service root. It never falls back to a personal user profile.

use crate::error::{AppError, Result};
use std::path::PathBuf;

pub const SERVICE_NAME: &str = "CodexSwitcher";
pub const SERVICE_DISPLAY_NAME: &str = "Codex Switcher";

#[derive(Clone, Debug)]
pub enum ServiceCommand {
    Install {
        service_root: PathBuf,
        codex_home: PathBuf,
    },
    Start,
    Stop,
    Status,
    Uninstall,
    Run {
        service_root: PathBuf,
    },
}

#[cfg(not(windows))]
pub fn handle(_command: ServiceCommand) -> Result<()> {
    Err(AppError::Message(
        "Windows Service 管理仅支持 Windows 平台".into(),
    ))
}

#[cfg(windows)]
mod native {
    use super::*;
    use crate::{
        account::load_index,
        config::{Config, load_config, save_config},
        daemon::{control_request, run_daemon},
        paths::service_paths,
    };
    use std::{
        ffi::{OsStr, OsString},
        process::Command,
        sync::mpsc,
        thread,
        time::Duration,
    };
    use windows_service::{
        define_windows_service,
        service::{
            ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState,
            ServiceStatus, ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    define_windows_service!(ffi_service_main, service_main);

    fn service_error(error: impl std::fmt::Display) -> AppError {
        AppError::Message(format!("Windows Service 操作失败：{error}"))
    }

    pub fn handle(command: ServiceCommand) -> Result<()> {
        match command {
            ServiceCommand::Install {
                service_root,
                codex_home,
            } => install(service_root, codex_home),
            ServiceCommand::Start => with_service(ServiceAccess::START, |service| {
                service.start::<&OsStr>(&[]).map_err(service_error)
            }),
            ServiceCommand::Stop => with_service(ServiceAccess::STOP, |service| {
                service.stop().map_err(service_error)
            }),
            ServiceCommand::Status => with_service(ServiceAccess::QUERY_STATUS, |service| {
                let status = service.query_status().map_err(service_error)?;
                println!("{:?}", status.current_state);
                Ok(())
            }),
            ServiceCommand::Uninstall => with_service(ServiceAccess::DELETE, |service| {
                service.delete().map_err(service_error)
            }),
            ServiceCommand::Run { service_root } => run_dispatcher(service_root),
        }
    }

    fn manager(access: ServiceManagerAccess) -> Result<ServiceManager> {
        ServiceManager::local_computer(None::<&str>, access).map_err(service_error)
    }

    fn with_service<T>(
        access: ServiceAccess,
        operation: impl FnOnce(&windows_service::service::Service) -> Result<T>,
    ) -> Result<T> {
        let manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = manager
            .open_service(SERVICE_NAME, access)
            .map_err(service_error)?;
        operation(&service)
    }

    fn install(service_root: PathBuf, codex_home: PathBuf) -> Result<()> {
        if !service_root.is_absolute() || !codex_home.is_absolute() {
            return Err(AppError::Message(
                "service-root 和 codex-home 必须是绝对路径".into(),
            ));
        }
        let paths = service_paths(service_root.clone());
        let mut config = if paths.config_file.exists() {
            load_config(&paths)?
        } else {
            Config::defaults()
        };
        config.codex_home = codex_home;
        config.accounts_dir = service_root.join("data").join("accounts");
        config.proxy.enabled = true;
        save_config(&paths, &config)?;
        grant_service_access(&service_root)?;
        grant_service_access(&codex_home)?;

        let executable_path = std::env::current_exe()?;
        let manager = manager(ServiceManagerAccess::CREATE_SERVICE)?;
        let service_info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::OnDemand,
            error_control: ServiceErrorControl::Normal,
            executable_path,
            launch_arguments: vec![
                OsString::from("service"),
                OsString::from("run"),
                OsString::from("--service-root"),
                service_root.into_os_string(),
            ],
            dependencies: vec![],
            account_name: Some(OsString::from("NT AUTHORITY\\LocalService")),
            account_password: None,
        };
        let service_access = ServiceAccess::CHANGE_CONFIG
            | ServiceAccess::START
            | ServiceAccess::STOP
            | ServiceAccess::QUERY_STATUS;
        let service = match manager.create_service(&service_info, service_access) {
            Ok(service) => service,
            Err(_) => {
                let service = manager
                    .open_service(SERVICE_NAME, service_access)
                    .map_err(service_error)?;
                // MSI installation creates the demand-start service before a
                // user supplies their roots. Keep a subsequent `service
                // install` authoritative so an explicit root always becomes
                // the service's next launch configuration.
                service
                    .change_config(&service_info)
                    .map_err(service_error)?;
                service
            }
        };
        let status = service.query_status().map_err(service_error)?;
        if status.current_state != ServiceState::Running {
            service.start::<&OsStr>(&[]).map_err(service_error)?;
        }
        Ok(())
    }

    /// Give LocalService and the installing identity access to the explicitly
    /// selected roots. We only add grants; uninstall intentionally preserves
    /// user data and never attempts to guess which ACLs it may safely remove.
    fn grant_service_access(path: &std::path::Path) -> Result<()> {
        std::fs::create_dir_all(path)?;
        let local_service = "*S-1-5-19:(OI)(CI)M";
        run_icacls(path, local_service)?;
        if let Ok(output) = Command::new("whoami").output()
            && output.status.success()
        {
            let identity = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !identity.is_empty() {
                run_icacls(path, &format!("{identity}:(OI)(CI)M"))?;
            }
        }
        Ok(())
    }

    fn run_icacls(path: &std::path::Path, grant: &str) -> Result<()> {
        let output = Command::new("icacls")
            .arg(path)
            .args(["/grant", grant, "/t", "/c"])
            .output()
            .map_err(service_error)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(AppError::Message(format!(
                "无法为 Windows Service 配置目录权限：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    fn run_dispatcher(service_root: PathBuf) -> Result<()> {
        // SCM supplies the command line to `service_main`; this environment
        // value is only a fallback for local diagnostics.
        unsafe { std::env::set_var("CODEX_SWITCHER_SERVICE_ROOT", service_root) };
        service_dispatcher::start(SERVICE_NAME, ffi_service_main).map_err(service_error)
    }

    fn service_main(arguments: Vec<OsString>) {
        let root = arguments
            .windows(2)
            .find(|arguments| arguments[0] == OsStr::new("--service-root"))
            .map(|arguments| PathBuf::from(&arguments[1]))
            .or_else(|| std::env::var_os("CODEX_SWITCHER_SERVICE_ROOT").map(PathBuf::from));
        if let Some(root) = root {
            let _ = run_service(root);
        }
    }

    fn run_service(service_root: PathBuf) -> Result<()> {
        let (stop_tx, stop_rx) = mpsc::channel();
        let status_handle =
            service_control_handler::register(SERVICE_NAME, move |event| match event {
                windows_service::service::ServiceControl::Stop => {
                    let _ = stop_tx.send(());
                    ServiceControlHandlerResult::NoError
                }
                windows_service::service::ServiceControl::Interrogate => {
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            })
            .map_err(service_error)?;
        status_handle
            .set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: ServiceState::Running,
                controls_accepted: windows_service::service::ServiceControlAccept::STOP,
                exit_code: windows_service::service::ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            })
            .map_err(service_error)?;

        let paths = service_paths(service_root);
        let stop_paths = paths.clone();
        thread::spawn(move || {
            if stop_rx.recv().is_ok() {
                for _ in 0..120 {
                    if stop_paths.runtime_file.exists() {
                        if let Ok(runtime) = tokio::runtime::Runtime::new() {
                            let _ = runtime.block_on(control_request(
                                &stop_paths,
                                hyper::Method::POST,
                                "/v1/daemon/stop",
                            ));
                        }
                        break;
                    }
                    thread::sleep(Duration::from_millis(250));
                }
            }
        });

        let config = load_config(&paths)?;
        let index = load_index(&paths)?;
        let result = tokio::runtime::Runtime::new()?.block_on(run_daemon(config, index, paths));
        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: windows_service::service::ServiceControlAccept::empty(),
            exit_code: windows_service::service::ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });
        result
    }
}

#[cfg(windows)]
pub use native::handle;
