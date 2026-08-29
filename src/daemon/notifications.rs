use crate::{
    i18n::{Language, LanguagePreference, translate, translate_with},
    types::Account,
};

pub struct NotificationManager {
    enabled: bool,
    language: Language,
}

impl NotificationManager {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            language: LanguagePreference::Auto.resolve(),
        }
    }

    pub fn with_language(enabled: bool, language: Language) -> Self {
        Self { enabled, language }
    }

    pub fn notify_switch_recommended(&self, account: &Account) {
        if !self.enabled {
            return;
        }

        let message = translate_with(
            self.language,
            "notification-recommended",
            [("account", account.label.as_str())],
        );
        self.send_notification("Codex Switcher", &message);
    }

    pub fn notify_switched(&self, from: &Account, to: &Account) {
        if !self.enabled {
            return;
        }

        let message = translate_with(
            self.language,
            "notification-switched",
            [("from", from.label.as_str()), ("to", to.label.as_str())],
        );
        self.send_notification("Codex Switcher", &message);
    }

    pub fn notify_switch_failed(&self, reason: &str) {
        if !self.enabled {
            return;
        }

        let message = translate_with(self.language, "notification-failed", [("reason", reason)]);
        self.send_notification(
            &format!(
                "Codex Switcher - {}",
                translate(self.language, "error", None)
            ),
            &message,
        );
    }

    pub fn notify_threshold_reached(&self, account: &Account, percent: f64) {
        if !self.enabled {
            return;
        }

        let percent_text = format!("{percent:.1}");
        let message = translate_with(
            self.language,
            "notification-threshold",
            [
                ("account", account.label.as_str()),
                ("percent", percent_text.as_str()),
            ],
        );
        self.send_notification(
            &format!(
                "Codex Switcher - {}",
                translate(self.language, "warning", None)
            ),
            &message,
        );
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
