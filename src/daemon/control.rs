use crate::{error::*, paths::Paths};
use std::fs;

pub fn write_pid_file(paths: &Paths) -> Result<()> {
    let pid = std::process::id();
    fs::write(&paths.pid_file, pid.to_string())?;
    Ok(())
}

pub fn remove_pid_file(paths: &Paths) -> Result<()> {
    if paths.pid_file.exists() {
        fs::remove_file(&paths.pid_file)?;
    }
    Ok(())
}

pub fn read_pid_file(paths: &Paths) -> Result<Option<u32>> {
    if !paths.pid_file.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&paths.pid_file)?;
    let pid = content
        .trim()
        .parse::<u32>()
        .map_err(|_| AppError::Message("Invalid PID file".into()))?;

    Ok(Some(pid))
}

pub fn check_daemon_status(paths: &Paths) -> Result<DaemonStatus> {
    let pid = match read_pid_file(paths)? {
        Some(pid) => pid,
        None => return Ok(DaemonStatus::NotRunning),
    };

    // 检查进程是否存在
    #[cfg(unix)]
    {
        let exists = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if exists {
            Ok(DaemonStatus::Running(pid))
        } else {
            // PID文件存在但进程不存在，清理它
            let _ = remove_pid_file(paths);
            Ok(DaemonStatus::Stale)
        }
    }

    #[cfg(not(unix))]
    {
        Ok(DaemonStatus::Unknown(pid))
    }
}

pub fn send_reload_signal(paths: &Paths) -> Result<()> {
    let pid =
        read_pid_file(paths)?.ok_or_else(|| AppError::Message("Daemon is not running".into()))?;

    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-HUP", &pid.to_string()])
            .output()?;
    }

    #[cfg(not(unix))]
    {
        return Err(AppError::Message(
            "Signal sending not supported on this platform".into(),
        ));
    }

    Ok(())
}

pub fn send_stop_signal(paths: &Paths) -> Result<()> {
    let pid =
        read_pid_file(paths)?.ok_or_else(|| AppError::Message("Daemon is not running".into()))?;

    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()?;
    }

    #[cfg(not(unix))]
    {
        return Err(AppError::Message(
            "Signal sending not supported on this platform".into(),
        ));
    }

    Ok(())
}

#[derive(Debug)]
pub enum DaemonStatus {
    Running(u32),
    NotRunning,
    Stale,
    Unknown(u32),
}

impl std::fmt::Display for DaemonStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DaemonStatus::Running(pid) => write!(f, "运行中 (PID: {})", pid),
            DaemonStatus::NotRunning => write!(f, "未运行"),
            DaemonStatus::Stale => write!(f, "PID文件过期（进程已退出）"),
            DaemonStatus::Unknown(pid) => write!(f, "未知状态 (PID: {})", pid),
        }
    }
}
