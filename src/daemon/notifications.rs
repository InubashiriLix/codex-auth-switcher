use crate::types::Account;

pub struct NotificationManager {
    enabled: bool,
}

impl NotificationManager {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn notify_switch_recommended(&self, account: &Account) {
        if !self.enabled {
            return;
        }

        let message = format!("推荐切换到账户: {}", account.label);
        self.send_notification("Codex Switcher", &message);
    }

    pub fn notify_switched(&self, from: &Account, to: &Account) {
        if !self.enabled {
            return;
        }

        let message = format!("已切换账户: {} → {}", from.label, to.label);
        self.send_notification("Codex Switcher", &message);
    }

    pub fn notify_switch_failed(&self, reason: &str) {
        if !self.enabled {
            return;
        }

        let message = format!("切换失败: {}", reason);
        self.send_notification("Codex Switcher - 错误", &message);
    }

    pub fn notify_threshold_reached(&self, account: &Account, percent: f64) {
        if !self.enabled {
            return;
        }

        let message = format!("账户 {} 使用率达到 {:.1}%", account.label, percent);
        self.send_notification("Codex Switcher - 警告", &message);
    }

    #[cfg(target_os = "linux")]
    fn send_notification(&self, title: &str, message: &str) {
        use notify_rust::Notification;
        let _ = Notification::new().summary(title).body(message).show();
    }

    #[cfg(target_os = "macos")]
    fn send_notification(&self, title: &str, message: &str) {
        use std::process::Command;
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            message, title
        );
        let _ = Command::new("osascript").args(["-e", &script]).spawn();
    }

    #[cfg(target_os = "windows")]
    fn send_notification(&self, title: &str, message: &str) {
        // Windows 通知实现
        eprintln!("[通知] {}: {}", title, message);
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    fn send_notification(&self, title: &str, message: &str) {
        eprintln!("[通知] {}: {}", title, message);
    }
}
