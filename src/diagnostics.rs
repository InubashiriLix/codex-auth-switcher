//! Local, credential-free reliability diagnostics and support bundles.

use crate::{
    account::{auth_tokens, snapshot_path},
    config::Config,
    error::{AppError, Result},
    integration::{CodexIntegration, IntegrationStatus},
    paths::Paths,
    storage::MetadataStore,
    types::AccountIndex,
};
use chrono::{DateTime, Utc};
use flate2::{Compression, write::GzEncoder};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs, net::TcpListener, path::Path};
use tar::Builder;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: DoctorStatus,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Pass,
    Warn,
    Fail,
    Skipped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoctorReport {
    pub generated_at: DateTime<Utc>,
    pub network_checked: bool,
    pub healthy: bool,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    fn push(
        &mut self,
        name: &str,
        status: DoctorStatus,
        detail: impl Into<String>,
        hint: Option<&str>,
    ) {
        self.checks.push(DoctorCheck {
            name: name.into(),
            status,
            detail: detail.into(),
            hint: hint.map(str::to_owned),
        });
    }
}

pub fn doctor(
    config: &Config,
    paths: &Paths,
    accounts: &AccountIndex,
    network: bool,
) -> DoctorReport {
    let mut report = DoctorReport {
        generated_at: Utc::now(),
        network_checked: network,
        healthy: true,
        checks: Vec::new(),
    };
    match crate::config::validate_codex_home(&config.codex_home) {
        Ok(()) if config.codex_home.is_dir() => {
            report.push("codex_home", DoctorStatus::Pass, "Codex 主目录可用", None)
        }
        Ok(()) => report.push(
            "codex_home",
            DoctorStatus::Warn,
            "Codex 主目录尚不存在",
            Some("先运行官方 Codex 完成登录"),
        ),
        Err(error) => report.push(
            "codex_home",
            DoctorStatus::Fail,
            error.to_string(),
            Some("修正 config.toml 中的 codex_home"),
        ),
    }
    let integration = CodexIntegration::new(&config.codex_home).status();
    match integration {
        Ok(IntegrationStatus::Enabled) => report.push(
            "integration",
            DoctorStatus::Pass,
            "Codex 本地接入已启用",
            None,
        ),
        Ok(IntegrationStatus::Disabled) => report.push(
            "integration",
            DoctorStatus::Warn,
            "Codex 本地接入未启用",
            Some("在代理工作区启用接入"),
        ),
        Ok(IntegrationStatus::Drifted(detail)) => report.push(
            "integration",
            DoctorStatus::Fail,
            format!("配置漂移：{detail}"),
            Some("检查或回滚 config.toml 的受管字段"),
        ),
        Err(error) => report.push(
            "integration",
            DoctorStatus::Fail,
            error.to_string(),
            Some("修复 Codex config.toml 后重试"),
        ),
    }
    let mut valid = 0usize;
    let mut missing = 0usize;
    for account in &accounts.accounts {
        let path = snapshot_path(config, account.id);
        match fs::read_to_string(&path)
            .map_err(AppError::from)
            .and_then(|raw| serde_json::from_str(&raw).map_err(AppError::from))
            .and_then(|value| auth_tokens(&value).map(|_| ()))
        {
            Ok(()) => valid += 1,
            Err(_) => missing += 1,
        }
    }
    let snapshot_status = if missing == 0 {
        DoctorStatus::Pass
    } else {
        DoctorStatus::Fail
    };
    report.push(
        "account_snapshots",
        snapshot_status,
        format!("{valid} 个可读取快照，{missing} 个无效或缺失"),
        (missing > 0).then_some("重新导入或用当前官方登录更新该账户"),
    );
    match fs::metadata(&paths.database_file) {
        Ok(metadata) if metadata.is_file() && metadata.len() > 0 => report.push(
            "metadata_database",
            DoctorStatus::Pass,
            "运行时数据库可读取",
            None,
        ),
        Ok(_) => report.push(
            "metadata_database",
            DoctorStatus::Warn,
            "运行时数据库为空",
            Some("启动守护进程后会自动初始化"),
        ),
        Err(_) => report.push(
            "metadata_database",
            DoctorStatus::Warn,
            "运行时数据库尚未创建",
            Some("启动守护进程后会自动初始化"),
        ),
    }
    match TcpListener::bind(&config.proxy.listen_addr) {
        Ok(listener) => {
            drop(listener);
            report.push(
                "proxy_port",
                DoctorStatus::Pass,
                format!("{} 可监听", config.proxy.listen_addr),
                None,
            );
        }
        Err(error) => report.push(
            "proxy_port",
            DoctorStatus::Fail,
            format!("{}：{error}", config.proxy.listen_addr),
            Some("停止占用该端口的进程或调整 listen_addr"),
        ),
    }
    if network {
        let checked = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .and_then(|client| client.head(&config.proxy.target_base).send());
        match checked {
            Ok(response) => report.push(
                "upstream_network",
                DoctorStatus::Pass,
                format!("上游可达（HTTP {}）", response.status()),
                None,
            ),
            Err(error) => report.push(
                "upstream_network",
                DoctorStatus::Fail,
                format!("上游预检失败：{error}"),
                Some("检查 DNS、TLS、代理和网络出口"),
            ),
        }
    } else {
        report.push(
            "upstream_network",
            DoctorStatus::Skipped,
            "未使用 --network，未发起上游请求",
            Some("需要检查网络时运行 doctor --network"),
        );
    }
    report.healthy = !report
        .checks
        .iter()
        .any(|check| check.status == DoctorStatus::Fail);
    report
}

pub fn write_support_bundle(
    output: &Path,
    config: &Config,
    paths: &Paths,
    accounts: &AccountIndex,
    store: Option<&MetadataStore>,
    network: bool,
) -> Result<DoctorReport> {
    let report = doctor(config, paths, accounts, network);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(output)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut archive = Builder::new(encoder);
    append_json(&mut archive, "doctor.json", &report)?;
    append_json(
        &mut archive,
        "accounts.json",
        &json!({"accounts": accounts.accounts}),
    )?;
    append_json(
        &mut archive,
        "environment.json",
        &json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "daemon_runtime_present": paths.runtime_file.exists(),
            "database_present": paths.database_file.exists(),
        }),
    )?;
    if paths.config_file.exists() {
        let raw = fs::read_to_string(&paths.config_file)?;
        append_text(&mut archive, "config.toml", &redact(&raw))?;
    }
    if let Some(store) = store {
        append_json(&mut archive, "events.json", &store.recent_events(200, 0)?)?;
        append_json(
            &mut archive,
            "requests.json",
            &store.recent_requests(200, 0, None, None, None)?,
        )?;
        append_json(
            &mut archive,
            "account-events.json",
            &store.recent_account_events(None, 200, 0)?,
        )?;
    }
    let encoder = archive
        .into_inner()
        .map_err(|error| AppError::Message(error.to_string()))?;
    encoder
        .finish()
        .map_err(|error| AppError::Message(error.to_string()))?;
    Ok(report)
}

fn append_json<T: Serialize>(
    archive: &mut Builder<GzEncoder<fs::File>>,
    name: &str,
    value: &T,
) -> Result<()> {
    append_text(archive, name, &serde_json::to_string_pretty(value)?)
}

fn append_text(archive: &mut Builder<GzEncoder<fs::File>>, name: &str, text: &str) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(text.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive
        .append_data(&mut header, name, text.as_bytes())
        .map_err(|error| AppError::Message(error.to_string()))
}

fn redact(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if ["token", "authorization", "api_key", "secret", "password"]
                .iter()
                .any(|marker| lower.contains(marker))
            {
                "[sensitive configuration redacted]".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use flate2::read::GzDecoder;
    use std::io::Read;

    #[test]
    fn offline_doctor_does_not_make_network_check() {
        let mut config = Config::defaults();
        config.codex_home = std::env::temp_dir().join("codex-switcher-doctor-missing");
        let paths = crate::paths::Paths {
            config_file: std::env::temp_dir().join("doctor.toml"),
            index_file: std::env::temp_dir().join("doctor-index.toml"),
            config_dir: std::env::temp_dir(),
            pid_file: std::env::temp_dir().join("doctor.pid"),
            runtime_file: std::env::temp_dir().join("doctor.runtime"),
            database_file: std::env::temp_dir().join("doctor.sqlite"),
        };
        let report = doctor(&config, &paths, &AccountIndex::default(), false);
        assert!(!report.network_checked);
        assert!(
            report
                .checks
                .iter()
                .any(|check| check.name == "upstream_network"
                    && check.status == DoctorStatus::Skipped)
        );
    }

    #[test]
    fn redaction_hides_secretish_configuration_lines() {
        assert!(redact("access_token = 'secret'").contains("redacted"));
        assert_eq!(redact("theme = 'nord'"), "theme = 'nord'");
    }

    #[test]
    fn support_bundle_redacts_configuration_secrets() {
        let root =
            std::env::temp_dir().join(format!("codex-switcher-bundle-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let paths = crate::paths::Paths {
            config_file: root.join("config.toml"),
            index_file: root.join("accounts.toml"),
            config_dir: root.clone(),
            pid_file: root.join("daemon.pid"),
            runtime_file: root.join("runtime.json"),
            database_file: root.join("runtime.sqlite3"),
        };
        fs::write(
            &paths.config_file,
            "theme = 'nord'\napi_token = 'secret-value'\n",
        )
        .unwrap();
        let mut config = Config::defaults();
        config.codex_home = root.join("codex");
        config.accounts_dir = root.join("accounts");
        let output = root.join("support.tar.gz");
        write_support_bundle(
            &output,
            &config,
            &paths,
            &AccountIndex::default(),
            None,
            false,
        )
        .unwrap();
        let mut archive = tar::Archive::new(GzDecoder::new(fs::File::open(&output).unwrap()));
        let mut contents = String::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            entry.read_to_string(&mut contents).unwrap();
        }
        assert!(!contents.contains("secret-value"));
        assert!(contents.contains("sensitive configuration redacted"));
        let _ = fs::remove_dir_all(root);
    }
}
