use crate::{error::*, paths::Paths, types::Theme};
use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub codex_home: PathBuf,
    pub accounts_dir: PathBuf,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub mode: OperationMode,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub onboarding_acknowledged: bool,
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
            mode: OperationMode::Interactive,
            proxy: ProxyConfig::default(),
            retention: RetentionConfig::default(),
            onboarding_acknowledged: false,
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
    Ok(toml::from_str(&fs::read_to_string(&p.config_file)?)?)
}

pub fn save_config(p: &Paths, c: &Config) -> Result<()> {
    ensure_private_dir(&p.config_dir)?;
    atomic_write(&p.config_file, toml::to_string_pretty(c)?.as_bytes())
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
    let temp = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    fs::write(&temp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    }
    crate::filesystem::atomic_replace(&temp, path)?;
    Ok(())
}
