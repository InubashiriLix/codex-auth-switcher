use crate::{error::*, i18n::LanguagePreference, paths::Paths, types::Theme};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};
use toml_edit::DocumentMut;
use uuid::Uuid;

const MAX_PATH_BYTES: usize = 4095;
const MAX_COMPONENT_BYTES: usize = 255;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub codex_home: PathBuf,
    pub accounts_dir: PathBuf,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub language: LanguagePreference,
    #[serde(default)]
    pub mode: OperationMode,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub onboarding_acknowledged: bool,
    /// One-shot diagnostic produced while loading and repairing a damaged
    /// configuration. It is shown by the TUI/logs and never persisted.
    #[serde(skip)]
    pub startup_notice: Option<String>,
}

impl Config {
    pub fn defaults() -> Self {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let data = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Self {
            codex_home: env::var_os("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex")),
            accounts_dir: data.join("codex-switcher").join("accounts"),
            theme: Theme::default(),
            language: LanguagePreference::default(),
            mode: OperationMode::Interactive,
            proxy: ProxyConfig::default(),
            retention: RetentionConfig::default(),
            onboarding_acknowledged: false,
            startup_notice: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RetentionConfig {
    #[serde(default = "default_retention_days")]
    pub days: i64,
    #[serde(default = "default_max_requests")]
    pub max_requests: usize,
    #[serde(default = "default_max_events")]
    pub max_events: usize,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            days: default_retention_days(),
            max_requests: default_max_requests(),
            max_events: default_max_events(),
        }
    }
}

fn default_retention_days() -> i64 {
    7
}
fn default_max_requests() -> usize {
    50_000
}
fn default_max_events() -> usize {
    10_000
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OperationMode {
    #[default]
    Interactive,
    ProxyDaemon {
        #[serde(default)]
        auto_switch: bool,
        #[serde(default = "default_true")]
        notify_user: bool,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    #[serde(default)]
    pub auto_switch: bool,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    #[serde(default = "default_cooldown")]
    pub cooldown_seconds: u64,
    #[serde(default)]
    pub strategy: RecommendStrategy,
    #[serde(default = "default_target_base")]
    pub target_base: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen_addr: default_listen_addr(),
            auto_switch: false,
            threshold: default_threshold(),
            cooldown_seconds: default_cooldown(),
            strategy: RecommendStrategy::default(),
            target_base: default_target_base(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendStrategy {
    #[default]
    Smart,
    MaxRemaining,
    RoundRobin,
}

fn default_listen_addr() -> String {
    "127.0.0.1:8765".into()
}

fn default_threshold() -> f64 {
    85.0
}

fn default_cooldown() -> u64 {
    5
}

fn default_target_base() -> String {
    "https://chatgpt.com".into()
}

pub fn load_config(p: &Paths) -> Result<Config> {
    if !p.config_file.exists() {
        return Ok(Config::defaults());
    }
    reject_symlink(&p.config_file)?;
    let raw = fs::read_to_string(&p.config_file)?;
    let mut config: Config = toml::from_str(&raw)?;
    if let Some(reason) = codex_home_repair_reason(&config.codex_home)? {
        config = repair_invalid_codex_home(p, &raw, config, &reason)?;
    }
    Ok(config)
}

pub fn save_config(p: &Paths, c: &Config) -> Result<()> {
    validate_codex_home(&c.codex_home)?;
    ensure_private_dir(&p.config_dir)?;
    atomic_write(&p.config_file, toml::to_string_pretty(c)?.as_bytes())
}

/// Reject impossible or ambiguous Codex homes before any filesystem write.
pub fn validate_codex_home(path: &Path) -> Result<()> {
    match codex_home_repair_reason(path)? {
        Some(reason) => Err(AppError::Message(reason)),
        None => Ok(()),
    }
}

/// Returns a deterministic validation failure that is safe to auto-repair.
/// Transient inspection failures must not replace the configured directory.
fn codex_home_repair_reason(path: &Path) -> Result<Option<String>> {
    if !path.is_absolute() {
        return Ok(Some("Codex 目录配置无效：必须使用绝对路径".into()));
    }
    if path.as_os_str().as_encoded_bytes().len() > MAX_PATH_BYTES {
        return Ok(Some("Codex 目录配置无效：路径总长度超过 4095 字节".into()));
    }
    if path
        .components()
        .any(|component| component.as_os_str().as_encoded_bytes().len() > MAX_COMPONENT_BYTES)
    {
        return Ok(Some("Codex 目录配置无效：单个路径组件超过 255 字节".into()));
    }
    match fs::metadata(path) {
        Ok(metadata) if !metadata.is_dir() => {
            Ok(Some("Codex 目录配置无效：目标已存在但不是目录".into()))
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::Message(format!("无法检查 Codex 目录：{error}"))),
    }
}

fn repair_invalid_codex_home(
    paths: &Paths,
    raw: &str,
    mut config: Config,
    reason: &str,
) -> Result<Config> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::Message(format!("{reason}；HOME 未设置，无法自动恢复")))?;
    let fallback = home.join(".codex");
    validate_codex_home(&fallback).map_err(|error| {
        AppError::Message(format!("{reason}；默认 Codex 目录也不可用：{error}"))
    })?;

    ensure_private_dir(&paths.config_dir)?;
    let backup_name = format!(
        "config.toml.invalid-{}-{}.bak",
        Utc::now().format("%Y%m%d-%H%M%S"),
        Uuid::new_v4()
    );
    let backup = paths.config_file.with_file_name(&backup_name);
    atomic_write(&backup, raw.as_bytes())?;

    // Edit the original TOML document rather than serializing Config, so
    // comments and fields unknown to this version survive the recovery.
    let mut document = raw
        .parse::<DocumentMut>()
        .map_err(|error| AppError::Message(format!("配置无法修复：{error}")))?;
    let fallback_text = fallback
        .to_str()
        .ok_or_else(|| AppError::Message("默认 Codex 目录不是有效 UTF-8".into()))?;
    document["codex_home"] = toml_edit::value(fallback_text);
    atomic_write(&paths.config_file, document.to_string().as_bytes())?;

    let notice = format!(
        "检测到损坏的 Codex 目录配置，已备份为 {backup_name} 并恢复到 {}",
        fallback.display()
    );
    tracing::warn!(reason, backup = %backup.display(), "{notice}");
    config.codex_home = fallback;
    config.startup_notice = Some(notice);
    Ok(config)
}

fn reject_symlink(path: &std::path::Path) -> Result<()> {
    if path.exists() && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(AppError::Message(format!(
            "出于安全原因，拒绝符号链接：{}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_private_dir(path: &std::path::Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use uuid::Uuid;
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    reject_symlink(path)?;
    let temp = path.with_file_name(format!(".tmp-{}", Uuid::new_v4()));
    fs::write(&temp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    }
    crate::filesystem::atomic_replace(&temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths() -> Paths {
        let root =
            std::env::temp_dir().join(format!("codex-switcher-config-recovery-{}", Uuid::new_v4()));
        Paths {
            config_file: root.join("config.toml"),
            index_file: root.join("accounts.toml"),
            config_dir: root.clone(),
            pid_file: root.join("daemon.pid"),
            runtime_file: root.join("runtime.json"),
            database_file: root.join("runtime.sqlite3"),
        }
    }

    #[test]
    fn invalid_overlong_home_is_backed_up_and_repaired() {
        let paths = test_paths();
        fs::create_dir_all(&paths.config_dir).unwrap();
        let invalid = format!("'{}{}'", "/tmp/", "x".repeat(1_987));
        let raw = format!(
            "# retained comment\ncodex_home = {invalid:?}\naccounts_dir = \"/tmp/accounts\"\nunknown_key = \"keep-me\"\n"
        );
        fs::write(&paths.config_file, &raw).unwrap();

        let loaded = load_config(&paths).unwrap();
        let expected = PathBuf::from(std::env::var_os("HOME").unwrap()).join(".codex");
        assert_eq!(loaded.codex_home, expected);
        assert!(
            loaded.startup_notice.as_deref().is_some_and(|notice| {
                notice.contains("损坏") && notice.contains("已备份")
            })
        );

        let repaired = fs::read_to_string(&paths.config_file).unwrap();
        assert!(repaired.contains("# retained comment"));
        assert!(repaired.contains("unknown_key = \"keep-me\""));
        assert!(!repaired.contains(&invalid));

        let backups = fs::read_dir(&paths.config_dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.invalid-")
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(backups[0].path()).unwrap(), raw);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                backups[0].metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn codex_home_validation_rejects_ambiguous_and_impossible_paths() {
        assert!(validate_codex_home(Path::new("relative/.codex")).is_err());
        assert!(validate_codex_home(&PathBuf::from("/").join("x".repeat(256))).is_err());
        assert!(validate_codex_home(&PathBuf::from("/").join("x".repeat(4096))).is_err());

        let root = std::env::temp_dir().join(format!("codex-switcher-home-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        assert!(validate_codex_home(&root).is_ok());
        assert!(validate_codex_home(&root.join("not-created-yet")).is_ok());
        let file = root.join("regular-file");
        fs::write(&file, b"not a directory").unwrap();
        assert!(validate_codex_home(&file).is_err());
    }

    #[test]
    fn save_config_rejects_invalid_home_without_touching_disk() {
        let paths = test_paths();
        let mut config = Config::defaults();
        config.codex_home = PathBuf::from("relative/.codex");
        let error = save_config(&paths, &config).unwrap_err().to_string();
        assert!(error.contains("绝对路径"));
        assert!(!paths.config_file.exists());
    }

    #[test]
    fn language_preference_is_backward_compatible_and_persistent() {
        let mut old: Config =
            toml::from_str("codex_home = '/tmp/codex'\naccounts_dir = '/tmp/accounts'\n").unwrap();
        assert_eq!(old.language, LanguagePreference::Auto);

        let paths = test_paths();
        old.language = LanguagePreference::Fr;
        save_config(&paths, &old).unwrap();
        assert_eq!(
            load_config(&paths).unwrap().language,
            LanguagePreference::Fr
        );
    }
}
