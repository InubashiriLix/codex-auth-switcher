use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Local, Utc};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap},
};
use reqwest::{
    blocking::Client,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;
use uuid::Uuid;

const APP: &str = "codex-switcher";
const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/codex/usage";
const HELP_TEXT: &str = "j/k 或 ↑/↓ 移动 · Ctrl-n/Ctrl-p 翻页 · Enter 启用\na 当前认证 · i 路径/JSON · n 重命名 · d 删除\nr 单个检测 · R 全部检测 · / 过滤 · s 设置 · t 切换主题\nCtrl-C 退出程序\nUse q or Esc to exit this helper window";

#[derive(Debug, Error)]
enum AppError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    TomlDe(#[from] toml::de::Error),
    #[error(transparent)]
    TomlSer(#[from] toml::ser::Error),
}
type Result<T> = std::result::Result<T, AppError>;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Config {
    codex_home: PathBuf,
    accounts_dir: PathBuf,
    #[serde(default)]
    theme: Theme,
}
impl Config {
    fn defaults() -> Self {
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
            accounts_dir: data.join(APP).join("accounts"),
            theme: Theme::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Theme {
    #[default]
    Midnight,
    Nord,
    Gruvbox,
    Paper,
}

#[derive(Clone, Copy, Debug)]
struct ThemeColors {
    background: Color,
    surface: Color,
    text: Color,
    muted: Color,
    border: Color,
    focus: Color,
    selected_bg: Color,
    selected_text: Color,
    success: Color,
    warning: Color,
    error: Color,
    unknown: Color,
    progress_track: Color,
    progress_fill: Color,
    progress_text: Color,
}

impl Theme {
    fn name(self) -> &'static str {
        match self {
            Self::Midnight => "Midnight（深色高对比）",
            Self::Nord => "Nord（冷色低饱和）",
            Self::Gruvbox => "Gruvbox（暖色深色）",
            Self::Paper => "Paper（浅色高对比）",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Midnight => Self::Nord,
            Self::Nord => Self::Gruvbox,
            Self::Gruvbox => Self::Paper,
            Self::Paper => Self::Midnight,
        }
    }

    fn colors(self) -> ThemeColors {
        let rgb = Color::Rgb;
        match self {
            Self::Midnight => ThemeColors {
                background: rgb(10, 15, 26),
                surface: rgb(20, 29, 45),
                text: rgb(242, 247, 255),
                muted: rgb(170, 187, 209),
                border: rgb(91, 118, 154),
                focus: rgb(98, 210, 255),
                selected_bg: rgb(32, 79, 116),
                selected_text: rgb(255, 255, 255),
                success: rgb(87, 230, 150),
                warning: rgb(255, 202, 87),
                error: rgb(255, 111, 116),
                unknown: rgb(165, 180, 201),
                progress_track: rgb(50, 65, 85),
                progress_fill: rgb(42, 176, 123),
                progress_text: rgb(255, 255, 255),
            },
            Self::Nord => ThemeColors {
                background: rgb(46, 52, 64),
                surface: rgb(59, 66, 82),
                text: rgb(236, 239, 244),
                muted: rgb(216, 222, 233),
                border: rgb(136, 192, 208),
                focus: rgb(136, 192, 208),
                selected_bg: rgb(67, 94, 115),
                selected_text: rgb(255, 255, 255),
                success: rgb(163, 190, 140),
                warning: rgb(235, 203, 139),
                error: rgb(191, 97, 106),
                unknown: rgb(180, 188, 204),
                progress_track: rgb(76, 86, 106),
                progress_fill: rgb(94, 129, 172),
                progress_text: rgb(255, 255, 255),
            },
            Self::Gruvbox => ThemeColors {
                background: rgb(40, 40, 40),
                surface: rgb(60, 56, 54),
                text: rgb(251, 241, 199),
                muted: rgb(213, 196, 161),
                border: rgb(168, 153, 132),
                focus: rgb(131, 165, 152),
                selected_bg: rgb(104, 92, 70),
                selected_text: rgb(255, 251, 235),
                success: rgb(184, 187, 38),
                warning: rgb(250, 189, 47),
                error: rgb(251, 73, 52),
                unknown: rgb(189, 174, 147),
                progress_track: rgb(80, 73, 69),
                progress_fill: rgb(152, 151, 26),
                progress_text: rgb(255, 255, 255),
            },
            Self::Paper => ThemeColors {
                background: rgb(250, 250, 247),
                surface: rgb(255, 255, 255),
                text: rgb(20, 28, 38),
                muted: rgb(74, 88, 105),
                border: rgb(68, 93, 118),
                focus: rgb(0, 93, 160),
                selected_bg: rgb(0, 93, 160),
                selected_text: rgb(255, 255, 255),
                success: rgb(0, 112, 62),
                warning: rgb(153, 92, 0),
                error: rgb(181, 35, 24),
                unknown: rgb(85, 99, 115),
                progress_track: rgb(202, 214, 224),
                progress_fill: rgb(0, 102, 178),
                progress_text: rgb(255, 255, 255),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct AccountIndex {
    #[serde(default)]
    accounts: Vec<Account>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Account {
    id: Uuid,
    label: String,
    source: String,
    imported_at: DateTime<Utc>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    status: CheckStatus,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CheckStatus {
    kind: StatusKind,
    checked_at: Option<DateTime<Utc>>,
    detail: String,
    primary: Option<Quota>,
    secondary: Option<Quota>,
}
impl Default for CheckStatus {
    fn default() -> Self {
        Self {
            kind: StatusKind::Unknown,
            checked_at: None,
            detail: "尚未检测".into(),
            primary: None,
            secondary: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum StatusKind {
    Live,
    Exhausted,
    Reauth,
    AccessDenied,
    Invalid,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Quota {
    used_percent: f64,
    window_minutes: Option<u64>,
    resets_at: Option<i64>,
}

#[derive(Clone, Debug)]
struct Paths {
    config_file: PathBuf,
    index_file: PathBuf,
    config_dir: PathBuf,
}
fn paths() -> Paths {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let config_dir = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join(APP);
    let data = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
        .join(APP);
    Paths {
        config_file: config_dir.join("config.toml"),
        index_file: data.join("accounts.toml"),
        config_dir,
    }
}
fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn reject_symlink(path: &Path) -> Result<()> {
    if path.exists() && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(AppError::Message(format!(
            "出于安全原因，拒绝符号链接：{}",
            path.display()
        )));
    }
    Ok(())
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
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
    fs::rename(temp, path)?;
    Ok(())
}
fn load_config(p: &Paths) -> Result<Config> {
    if !p.config_file.exists() {
        return Ok(Config::defaults());
    }
    reject_symlink(&p.config_file)?;
    Ok(toml::from_str(&fs::read_to_string(&p.config_file)?)?)
}
fn save_config(p: &Paths, c: &Config) -> Result<()> {
    ensure_private_dir(&p.config_dir)?;
    atomic_write(&p.config_file, toml::to_string_pretty(c)?.as_bytes())
}
fn load_index(p: &Paths) -> Result<AccountIndex> {
    if !p.index_file.exists() {
        return Ok(AccountIndex::default());
    }
    reject_symlink(&p.index_file)?;
    Ok(toml::from_str(&fs::read_to_string(&p.index_file)?)?)
}
fn save_index(p: &Paths, i: &AccountIndex) -> Result<()> {
    atomic_write(&p.index_file, toml::to_string_pretty(i)?.as_bytes())
}
fn snapshot_path(config: &Config, id: Uuid) -> PathBuf {
    config.accounts_dir.join(format!("{id}.auth.json"))
}

fn current_codex_running() -> bool {
    let uid = libc_uid();
    Command::new("pgrep")
        .args(["-u", uid.as_str(), "-x", "codex"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
#[cfg(unix)]
fn libc_uid() -> String {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid().to_string() }
}
#[cfg(not(unix))]
fn libc_uid() -> String {
    String::new()
}

fn auth_tokens(
    v: &Value,
) -> Result<(
    Value,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
)> {
    let tokens = v.get("tokens").cloned();
    let access = tokens
        .as_ref()
        .and_then(|x| x.get("access_token"))
        .or_else(|| v.get("accessToken"))
        .or_else(|| v.get("access_token"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Message("未找到 access token".into()))?
        .to_string();
    let refresh = tokens
        .as_ref()
        .and_then(|x| x.get("refresh_token"))
        .or_else(|| v.get("refreshToken"))
        .or_else(|| v.get("refresh_token"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let id_token = tokens
        .as_ref()
        .and_then(|x| x.get("id_token"))
        .or_else(|| v.get("idToken"))
        .or_else(|| v.get("id_token"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let account_id = tokens
        .as_ref()
        .and_then(|x| x.get("account_id"))
        .or_else(|| v.get("account_id"))
        .or_else(|| v.get("accountId"))
        .or_else(|| v.pointer("/providerSpecificData/chatgptAccountId"))
        .or_else(|| v.pointer("/account/id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let claims = jwt_claims(&access);
    let id_claims = jwt_claims(id_token);
    let auth_claims = claims
        .get("https://api.openai.com/auth")
        .unwrap_or(&Value::Null);
    let id_auth_claims = id_claims
        .get("https://api.openai.com/auth")
        .unwrap_or(&Value::Null);
    let profile = claims
        .get("https://api.openai.com/profile")
        .unwrap_or(&Value::Null);
    let email = first_string(&[
        v.pointer("/user/email"),
        v.get("email"),
        v.pointer("/meta/label"),
        v.get("label"),
        v.pointer("/credentials/email"),
        v.pointer("/providerSpecificData/email"),
        profile.get("email"),
        id_claims.get("email"),
        claims.get("email"),
    ]);
    let account_id = account_id.or_else(|| {
        first_string(&[
            v.pointer("/tokens/accountId"),
            v.pointer("/tokens/chatgptAccountId"),
            v.pointer("/chatgpt_account_id"),
            v.pointer("/meta/chatgpt_account_id"),
            v.pointer("/credentials/chatgpt_account_id"),
            auth_claims.get("chatgpt_account_id"),
            id_auth_claims.get("chatgpt_account_id"),
        ])
    });
    let plan = first_string(&[
        v.pointer("/account/planType"),
        v.pointer("/account/plan_type"),
        v.get("planType"),
        v.get("plan_type"),
        v.pointer("/providerSpecificData/chatgptPlanType"),
        v.pointer("/providerSpecificData/chatgpt_plan_type"),
        v.pointer("/credentials/plan_type"),
        profile.get("plan_type"),
        auth_claims.get("chatgpt_plan_type"),
        id_auth_claims.get("chatgpt_plan_type"),
    ]);
    Ok((
        json!({"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{"access_token":access,"refresh_token":refresh,"id_token":id_token,"account_id":account_id},"last_refresh":Utc::now().to_rfc3339()}),
        access,
        account_id,
        email,
        plan,
    ))
}
fn first_string(values: &[Option<&Value>]) -> Option<String> {
    values.iter().flatten().find_map(|value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
}
fn jwt_claims(token: &str) -> Value {
    token
        .split('.')
        .nth(1)
        .and_then(|p| URL_SAFE_NO_PAD.decode(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null)
}
fn import_value(
    config: &Config,
    index: &mut AccountIndex,
    value: Value,
    source: String,
    name: Option<String>,
) -> Result<()> {
    let (canonical, _access, account_id, email, plan) = auth_tokens(&value)?;
    ensure_private_dir(&config.accounts_dir)?;
    let id = Uuid::new_v4();
    atomic_write(
        &snapshot_path(config, id),
        serde_json::to_vec_pretty(&canonical)?.as_slice(),
    )?;
    let label = name.unwrap_or_else(|| {
        email.clone().unwrap_or_else(|| {
            Path::new(&source)
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or("未命名账户")
                .to_string()
        })
    });
    index.accounts.push(Account {
        id,
        label,
        source,
        imported_at: Utc::now(),
        email,
        plan,
        account_id,
        status: CheckStatus::default(),
    });
    Ok(())
}
fn import_file(config: &Config, index: &mut AccountIndex, path: &Path) -> Result<()> {
    reject_symlink(path)?;
    let raw = fs::read_to_string(path)?;
    import_value(
        config,
        index,
        serde_json::from_str(&raw)?,
        path.display().to_string(),
        None,
    )
}
fn import_current(config: &Config, index: &mut AccountIndex) -> Result<()> {
    import_file(config, index, &config.codex_home.join("auth.json"))
}
fn migrate_legacy(config: &Config, index: &mut AccountIndex) -> Result<usize> {
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let old = home.join(".codex-accounts");
    let mut count = 0;
    for slot in ["a", "b"] {
        let f = old.join(format!("account-{slot}.auth.json"));
        if f.is_file() {
            import_file(config, index, &f)?;
            count += 1;
        }
    }
    Ok(count)
}
fn activate(config: &Config, account: &Account) -> Result<()> {
    if current_codex_running() {
        return Err(AppError::Message(
            "仍有 Codex 在运行；请退出后再切换认证。".into(),
        ));
    }
    let source = snapshot_path(config, account.id);
    reject_symlink(&source)?;
    if !source.is_file() {
        return Err(AppError::Message("找不到账户快照".into()));
    }
    ensure_private_dir(&config.codex_home)?;
    atomic_write(&config.codex_home.join("auth.json"), &fs::read(source)?)
}

fn quota(value: &Value, key: &str) -> Option<Quota> {
    let x = value
        .pointer(&format!("/rate_limits/{key}"))
        .or_else(|| value.pointer(&format!("/rateLimits/{key}")))
        .or_else(|| value.pointer(&format!("/rate_limit/{key}_window")))
        .or_else(|| value.pointer(&format!("/rateLimit/{key}Window")))?;
    let seconds = x.get("limit_window_seconds").and_then(Value::as_u64);
    Some(Quota {
        used_percent: x
            .get("used_percent")
            .or_else(|| x.get("usedPercent"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        window_minutes: x
            .get("window_minutes")
            .or_else(|| x.get("windowDurationMins"))
            .and_then(Value::as_u64)
            .or_else(|| seconds.map(|seconds| seconds / 60)),
        resets_at: x
            .get("resets_at")
            .or_else(|| x.get("resetsAt"))
            .or_else(|| x.get("reset_at"))
            .and_then(Value::as_i64),
    })
}
fn probe(config: &Config, account: &mut Account) {
    let result = (|| -> Result<CheckStatus> {
        let raw = fs::read_to_string(snapshot_path(config, account.id))?;
        let (_, access, account_id, discovered_email, discovered_plan) =
            auth_tokens(&serde_json::from_str::<Value>(&raw)?)?;
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {access}"))
                .map_err(|_| AppError::Message("认证头无效".into()))?,
        );
        if let Some(id) = account_id.or_else(|| account.account_id.clone()) {
            headers.insert(
                "ChatGPT-Account-Id",
                HeaderValue::from_str(&id).map_err(|_| AppError::Message("账号 ID 无效".into()))?,
            );
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .default_headers(headers)
            .build()
            .map_err(|e| AppError::Message(e.to_string()))?;
        let override_url = env::var("CODEX_SWITCHER_USAGE_URL").ok();
        let url = override_url.clone().unwrap_or_else(|| USAGE_URL.into());
        let mut res = client
            .get(url)
            .send()
            .map_err(|e| AppError::Message(format!("网络错误：{e}")))?;
        if override_url.is_none() && res.status().as_u16() == 404 {
            res = client
                .get(CODEX_USAGE_URL)
                .send()
                .map_err(|e| AppError::Message(format!("网络错误：{e}")))?;
        }
        let code = res.status().as_u16();
        if code == 401 {
            return Ok(CheckStatus {
                kind: StatusKind::Reauth,
                checked_at: Some(Utc::now()),
                detail: "认证失效，请重新登录".into(),
                primary: None,
                secondary: None,
            });
        }
        if code == 403 {
            return Ok(CheckStatus {
                kind: StatusKind::AccessDenied,
                checked_at: Some(Utc::now()),
                detail: "服务拒绝访问（可能为账户或工作区限制）".into(),
                primary: None,
                secondary: None,
            });
        }
        if !res.status().is_success() {
            return Ok(CheckStatus {
                kind: StatusKind::Unknown,
                checked_at: Some(Utc::now()),
                detail: format!("额度服务返回 HTTP {code}"),
                primary: None,
                secondary: None,
            });
        }
        let v: Value = res
            .json()
            .map_err(|e| AppError::Message(format!("额度响应无法解析：{e}")))?;
        account.email = first_string(&[v.get("email"), v.pointer("/profile/email")])
            .or(discovered_email)
            .or_else(|| account.email.clone());
        account.plan = first_string(&[
            v.get("plan_type"),
            v.get("planType"),
            v.pointer("/account/plan_type"),
        ])
        .or(discovered_plan)
        .or_else(|| account.plan.clone());
        let p = quota(&v, "primary");
        let s = quota(&v, "secondary");
        let classified_reached = v
            .pointer("/rate_limits/rate_limit_reached_type")
            .or_else(|| v.pointer("/rateLimits/rateLimitReachedType"))
            .or_else(|| v.pointer("/rate_limit_reached_type"))
            .and_then(Value::as_str)
            .is_some_and(|x| !x.is_empty());
        let reached = classified_reached
            || v.pointer("/rate_limit/limit_reached")
                .and_then(Value::as_bool)
                == Some(true)
            || {
                let windows: Vec<_> = [p.as_ref(), s.as_ref()].into_iter().flatten().collect();
                !windows.is_empty() && windows.iter().all(|q| q.used_percent >= 100.0)
            };
        Ok(CheckStatus {
            kind: if reached {
                StatusKind::Exhausted
            } else {
                StatusKind::Live
            },
            checked_at: Some(Utc::now()),
            detail: if reached {
                "额度窗口已耗尽".into()
            } else {
                "额度可用".into()
            },
            primary: p,
            secondary: s,
        })
    })();
    account.status = result.unwrap_or_else(|e| CheckStatus {
        kind: StatusKind::Unknown,
        checked_at: Some(Utc::now()),
        detail: e.to_string(),
        primary: None,
        secondary: None,
    });
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Modal {
    None,
    Import,
    Filter,
    Rename,
    ConfirmUseEmail,
    Settings,
    ConfirmDelete,
    Help,
    ConfirmMigration,
}
struct Ui {
    config: Config,
    index: AccountIndex,
    selected: usize,
    filter: String,
    modal: Modal,
    input: String,
    notice: String,
    tick: u64,
    checking: Option<Checking>,
}
struct Checking {
    receiver: Receiver<ProbeEvent>,
    total: usize,
    completed: usize,
    current: String,
}
enum ProbeEvent {
    Started { label: String },
    Completed(Account),
    Finished,
}
impl Ui {
    fn visible(&self) -> Vec<usize> {
        self.index
            .accounts
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                self.filter.is_empty()
                    || a.label.to_lowercase().contains(&self.filter.to_lowercase())
                    || a.email
                        .as_ref()
                        .is_some_and(|x| x.to_lowercase().contains(&self.filter.to_lowercase()))
            })
            .map(|(i, _)| i)
            .collect()
    }
    fn selected_id(&self) -> Option<usize> {
        self.visible().get(self.selected).copied()
    }
}
fn start_probe(ui: &mut Ui, accounts: Vec<Account>) {
    let total = accounts.len();
    let (sender, receiver) = mpsc::channel();
    let config = ui.config.clone();
    thread::spawn(move || {
        for mut account in accounts {
            if sender
                .send(ProbeEvent::Started {
                    label: account.label.clone(),
                })
                .is_err()
            {
                return;
            }
            probe(&config, &mut account);
            if sender.send(ProbeEvent::Completed(account)).is_err() {
                return;
            }
        }
        let _ = sender.send(ProbeEvent::Finished);
    });
    ui.checking = Some(Checking {
        receiver,
        total,
        completed: 0,
        current: String::new(),
    });
}
fn poll_probe(p: &Paths, ui: &mut Ui) -> Result<()> {
    let mut finished = false;
    if let Some(checking) = &mut ui.checking {
        while let Ok(event) = checking.receiver.try_recv() {
            match event {
                ProbeEvent::Started { label } => checking.current = label,
                ProbeEvent::Completed(account) => {
                    if let Some(old) = ui
                        .index
                        .accounts
                        .iter_mut()
                        .find(|old| old.id == account.id)
                    {
                        *old = account;
                        checking.completed += 1;
                        save_index(p, &ui.index)?;
                    }
                }
                ProbeEvent::Finished => finished = true,
            }
        }
    }
    if finished {
        ui.checking = None;
        ui.notice = "全部账户检测完成".into();
    }
    Ok(())
}
fn status_style(theme: ThemeColors, s: &StatusKind) -> (Color, &'static str) {
    match s {
        StatusKind::Live => (theme.success, "可用"),
        StatusKind::Exhausted => (theme.warning, "额度耗尽"),
        StatusKind::Reauth => (theme.error, "需登录"),
        StatusKind::AccessDenied => (theme.error, "访问拒绝"),
        StatusKind::Invalid => (theme.error, "无效"),
        StatusKind::Unknown => (theme.unknown, "未知"),
    }
}
fn active_account_id(ui: &Ui) -> Option<Uuid> {
    let active = fs::read(ui.config.codex_home.join("auth.json")).ok()?;
    ui.index
        .accounts
        .iter()
        .find(|account| {
            fs::read(snapshot_path(&ui.config, account.id))
                .ok()
                .is_some_and(|snapshot| snapshot == active)
        })
        .map(|account| account.id)
}
fn block<'a>(theme: ThemeColors, title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().fg(theme.border).bg(theme.surface))
}
fn draw_quota(f: &mut Frame, area: Rect, title: &str, q: Option<&Quota>, theme: ThemeColors) {
    let Some(q) = q else {
        f.render_widget(
            Paragraph::new("尚无额度数据\n按 r 检测此账户 · R 检测全部")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.warning).bg(theme.surface))
                .block(block(theme, title)),
            area,
        );
        return;
    };
    let reset = q
        .resets_at
        .and_then(|t| DateTime::from_timestamp(t, 0))
        .map(|x| x.with_timezone(&Local).format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "--".into());
    let window = q
        .window_minutes
        .map(|minutes| format!("{} 分钟", minutes))
        .unwrap_or_else(|| "未知窗口".into());
    let ratio = ((100. - q.used_percent).clamp(0., 100.) / 100.) as f64;
    let label = format!(
        "{:.0}% 剩余 · {window} · 重置 {reset}",
        (100. - q.used_percent).max(0.)
    );
    // Keep the label on the surface rather than overlaying it on the fill: a single
    // foreground cannot remain legible over both the filled and unfilled portions.
    let outer = block(theme, title);
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    let quota_lines = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);
    f.render_widget(
        Paragraph::new(label).style(Style::default().fg(theme.progress_text).bg(theme.surface)),
        quota_lines[0],
    );
    f.render_widget(
        Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(theme.progress_fill)
                    .bg(theme.progress_track),
            )
            .ratio(ratio),
        quota_lines[1],
    );
}
fn draw_overview(f: &mut Frame, area: Rect, ui: &Ui, theme: ThemeColors) {
    let live = ui
        .index
        .accounts
        .iter()
        .filter(|account| account.status.kind == StatusKind::Live)
        .count();
    let needs_attention = ui.index.accounts.len().saturating_sub(live);
    let activity = if let Some(checking) = &ui.checking {
        let spinner = ["|", "/", "-", "\\"][(ui.tick as usize) % 4];
        Line::from(vec![
            Span::styled(format!(" {spinner} "), Style::default().fg(theme.focus)),
            Span::styled(
                format!(
                    "正在检测 {}（{}/{}）",
                    checking.current, checking.completed, checking.total
                ),
                Style::default().fg(theme.warning),
            ),
        ])
    } else {
        Line::from(Span::styled(
            " 就绪 · r 检测所选账户 · R 检测全部",
            Style::default().fg(theme.muted),
        ))
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!(" {} 可用", live),
                Style::default().fg(theme.success),
            ),
            Span::styled(
                format!("  ·  {} 待关注", needs_attention),
                Style::default().fg(if needs_attention == 0 {
                    theme.muted
                } else {
                    theme.warning
                }),
            ),
            Span::styled("  ·  n 重命名", Style::default().fg(theme.muted)),
        ]),
        activity,
    ];
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(block(theme, "账户概览")),
        area,
    );
}
fn draw(f: &mut Frame, ui: &Ui) {
    let theme = ui.config.theme.colors();
    f.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        f.area(),
    );
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(f.area());
    let active_id = active_account_id(ui);
    let active_label = active_id
        .and_then(|id| ui.index.accounts.iter().find(|account| account.id == id))
        .map(|account| account.label.as_str())
        .unwrap_or("没有受管理的活动账户");
    let title = format!(
        " Codex Switcher  ·  当前生效: {active_label}  ·  {} 个账户  ·  {} ",
        ui.index.accounts.len(),
        ui.config.codex_home.display()
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                title,
                Style::default()
                    .fg(theme.focus)
                    .bg(theme.surface)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  [a]导入当前 [i]导入 [r/R]检测 [t]主题 [Enter]启用 [?]帮助",
                Style::default().fg(theme.text).bg(theme.surface),
            ),
        ]))
        .style(Style::default().fg(theme.text).bg(theme.surface))
        .block(block(theme, "")),
        areas[0],
    );
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(areas[1]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(5)])
        .split(chunks[0]);
    let visible = ui.visible();
    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .map(|(pos, i)| {
            let a = &ui.index.accounts[*i];
            let (c, st) = status_style(theme, &a.status.kind);
            let marker = if pos == ui.selected { "›" } else { " " };
            let active = if active_id == Some(a.id) {
                " ● 当前"
            } else {
                ""
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} "), Style::default().fg(theme.focus)),
                Span::styled(
                    format!("{:<24}", a.label.chars().take(24).collect::<String>()),
                    Style::default()
                        .fg(if pos == ui.selected {
                            theme.selected_text
                        } else {
                            theme.text
                        })
                        .add_modifier(if pos == ui.selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(format!(" {st}{active}"), Style::default().fg(c)),
            ]))
            .style(Style::default().bg(if pos == ui.selected {
                theme.selected_bg
            } else {
                theme.surface
            }))
        })
        .collect();
    f.render_widget(
        List::new(items)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(block(
                theme,
                if ui.filter.is_empty() {
                    "账户"
                } else {
                    "过滤结果"
                },
            )),
        left[0],
    );
    draw_overview(f, left[1], ui, theme);
    let right = chunks[1];
    if let Some(i) = ui.selected_id() {
        let a = &ui.index.accounts[i];
        let h = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),
                Constraint::Length(4),
                Constraint::Length(4),
                Constraint::Min(5),
            ])
            .split(right);
        let (c, st) = status_style(theme, &a.status.kind);
        let meta = vec![
            Line::from(vec![
                Span::styled(
                    &a.label,
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "  {st}{}",
                        if active_id == Some(a.id) {
                            "  ● 当前生效"
                        } else {
                            ""
                        }
                    ),
                    Style::default().fg(c),
                ),
            ]),
            Line::from(format!(
                "邮箱：{}    套餐：{}",
                a.email.as_deref().unwrap_or("未知"),
                a.plan.as_deref().unwrap_or("未知")
            )),
            Line::from(format!(
                "{} · {}",
                a.status.detail,
                a.status
                    .checked_at
                    .map(|x| x.with_timezone(&Local).format("%H:%M:%S").to_string())
                    .unwrap_or_else(|| "未检查".into())
            )),
        ];
        f.render_widget(
            Paragraph::new(meta)
                .style(Style::default().fg(theme.text).bg(theme.surface))
                .block(block(theme, "详情")),
            h[0],
        );
        draw_quota(f, h[1], "主要额度窗口", a.status.primary.as_ref(), theme);
        draw_quota(f, h[2], "次要额度窗口", a.status.secondary.as_ref(), theme);
        draw_overview(f, h[3], ui, theme);
    } else {
        let h = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(5)])
            .split(right);
        f.render_widget(
            Paragraph::new("没有账户。按 a 导入当前 Codex 登录，或按 i 导入 JSON/路径。")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.text).bg(theme.surface))
                .block(block(theme, "详情")),
            h[0],
        );
        draw_overview(f, h[1], ui, theme);
    }
    f.render_widget(
        Paragraph::new(format!(" {}", ui.notice))
            .style(Style::default().fg(theme.muted).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .style(Style::default().fg(theme.border).bg(theme.background)),
            ),
        areas[2],
    );
    if ui.modal != Modal::None {
        let popup = centered(70, 30, f.area());
        f.render_widget(Clear, popup);
        let (title, text) = match ui.modal {
            Modal::Import => ("导入：输入 JSON 或本地文件路径", ui.input.as_str()),
            Modal::Filter => ("过滤账户", ui.input.as_str()),
            Modal::Rename => ("重命名账户", ui.input.as_str()),
            Modal::ConfirmUseEmail => (
                "使用检测到的邮箱？",
                "按 y 使用当前邮箱作为名称；按 n 输入自定义名称；Esc 取消",
            ),
            Modal::Settings => ("Codex 目录（保存后生效）", ui.input.as_str()),
            Modal::ConfirmDelete => ("确认删除", "按 y 永久删除快照；Esc 取消"),
            Modal::ConfirmMigration => (
                "发现旧版账户",
                "按 y 导入 ~/.codex-accounts 的 A/B 快照（不会删除原文件）；Esc 跳过",
            ),
            Modal::Help => ("键位", HELP_TEXT),
            Modal::None => unreachable!(),
        };
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text).bg(theme.surface))
                .block(block(theme, title)),
            popup,
        );
    }
}
fn centered(x: u16, y: u16, area: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - y) / 2),
            Constraint::Percentage(y),
            Constraint::Percentage((100 - y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - x) / 2),
            Constraint::Percentage(x),
            Constraint::Percentage((100 - x) / 2),
        ])
        .split(v[1])[1]
}
fn main() -> Result<()> {
    let p = paths();
    let config = load_config(&p)?;
    let index = load_index(&p)?;
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let show_legacy = index.accounts.is_empty()
        && (home.join(".codex-accounts/account-a.auth.json").is_file()
            || home.join(".codex-accounts/account-b.auth.json").is_file());
    let mut ui = Ui {
        config,
        index,
        selected: 0,
        filter: String::new(),
        modal: if show_legacy {
            Modal::ConfirmMigration
        } else {
            Modal::None
        },
        input: String::new(),
        notice: "就绪".into(),
        tick: 0,
        checking: None,
    };
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;
    let result = run(&mut terminal, &p, &mut ui);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}
fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    p: &Paths,
    ui: &mut Ui,
) -> Result<()> {
    let mut last = Instant::now();
    loop {
        poll_probe(p, ui)?;
        terminal.draw(|f| draw(f, ui))?;
        let timeout = Duration::from_millis(100).saturating_sub(last.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if handle_key(key, p, ui)? {
                    return Ok(());
                }
            }
        }
        if last.elapsed() >= Duration::from_millis(100) {
            ui.tick += 1;
            last = Instant::now();
        }
    }
}
fn handle_key(key: KeyEvent, p: &Paths, ui: &mut Ui) -> Result<bool> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Ok(true);
    }
    if ui.modal != Modal::None {
        return handle_modal(key, p, ui);
    }
    if ui.checking.is_some()
        && matches!(
            key.code,
            KeyCode::Char('r' | 'R' | 'a' | 'i' | 'n' | 'd') | KeyCode::Enter
        )
    {
        ui.notice = "检测进行中；完成后再修改账户。".into();
        return Ok(false);
    }
    let visible = ui.visible();
    match (key.modifiers, key.code) {
        (_, KeyCode::Char('q')) => return Ok(true),
        (_, KeyCode::Esc) => {
            ui.filter.clear();
            ui.notice = "已清除过滤".into();
        }
        (_, KeyCode::Char('j')) | (_, KeyCode::Down) => {
            if !visible.is_empty() {
                ui.selected = (ui.selected + 1).min(visible.len() - 1);
            }
        }
        (_, KeyCode::Char('k')) | (_, KeyCode::Up) => {
            ui.selected = ui.selected.saturating_sub(1);
        }
        (KeyModifiers::CONTROL, KeyCode::Char('n')) => {
            ui.selected = (ui.selected + 10).min(visible.len().saturating_sub(1));
        }
        (KeyModifiers::CONTROL, KeyCode::Char('p')) => {
            ui.selected = ui.selected.saturating_sub(10);
        }
        (_, KeyCode::Char('g')) => ui.selected = 0,
        (_, KeyCode::Char('G')) => ui.selected = visible.len().saturating_sub(1),
        (_, KeyCode::Char('?')) => ui.modal = Modal::Help,
        (_, KeyCode::Char('/')) => {
            ui.modal = Modal::Filter;
            ui.input.clear();
        }
        (_, KeyCode::Char('a')) => match import_current(&ui.config, &mut ui.index) {
            Ok(()) => {
                save_index(p, &ui.index)?;
                ui.notice = "已导入当前 Codex 认证".into();
            }
            Err(e) => ui.notice = e.to_string(),
        },
        (_, KeyCode::Char('i')) => {
            ui.modal = Modal::Import;
            ui.input.clear();
        }
        (_, KeyCode::Char('n')) => {
            if let Some(i) = ui.selected_id() {
                if ui.index.accounts[i].email.is_some() {
                    ui.modal = Modal::ConfirmUseEmail;
                } else {
                    ui.input = ui.index.accounts[i].label.clone();
                    ui.modal = Modal::Rename;
                }
            }
        }
        (_, KeyCode::Char('d')) => {
            if ui.selected_id().is_some() {
                ui.modal = Modal::ConfirmDelete;
            }
        }
        (_, KeyCode::Char('s')) => {
            ui.input = ui.config.codex_home.display().to_string();
            ui.modal = Modal::Settings;
        }
        (_, KeyCode::Char('t')) => {
            ui.config.theme = ui.config.theme.next();
            save_config(p, &ui.config)?;
            ui.notice = format!("已切换主题：{}", ui.config.theme.name());
        }
        (_, KeyCode::Enter) => {
            if let Some(i) = ui.selected_id() {
                ui.notice = match activate(&ui.config, &ui.index.accounts[i]) {
                    Ok(()) => format!("已启用 {}", ui.index.accounts[i].label),
                    Err(e) => e.to_string(),
                };
            }
        }
        (_, KeyCode::Char('r')) => {
            if let Some(i) = ui.selected_id() {
                let account = ui.index.accounts[i].clone();
                ui.notice = format!("正在检测 {}…", account.label);
                start_probe(ui, vec![account]);
            }
        }
        (_, KeyCode::Char('R')) => {
            ui.notice = "正在检测全部账户…".into();
            start_probe(ui, ui.index.accounts.clone());
        }
        _ => {}
    }
    Ok(false)
}
fn handle_modal(key: KeyEvent, p: &Paths, ui: &mut Ui) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') if ui.modal == Modal::Help => {
            ui.modal = Modal::None;
        }
        KeyCode::Esc => {
            ui.modal = Modal::None;
            ui.input.clear();
        }
        KeyCode::Char('y') if ui.modal == Modal::ConfirmDelete => {
            if let Some(i) = ui.selected_id() {
                let a = ui.index.accounts.remove(i);
                let path = snapshot_path(&ui.config, a.id);
                reject_symlink(&path)?;
                if path.exists() {
                    fs::remove_file(path)?;
                }
                save_index(p, &ui.index)?;
                ui.selected = ui.selected.saturating_sub(1);
                ui.notice = "已永久删除账户快照".into();
            }
            ui.modal = Modal::None;
        }
        KeyCode::Char('y') if ui.modal == Modal::ConfirmUseEmail => {
            if let Some(i) = ui.selected_id() {
                if let Some(email) = ui.index.accounts[i].email.clone() {
                    ui.index.accounts[i].label = email;
                    save_index(p, &ui.index)?;
                    ui.notice = "已使用检测到的邮箱重命名".into();
                }
            }
            ui.modal = Modal::None;
        }
        KeyCode::Char('n') if ui.modal == Modal::ConfirmUseEmail => {
            if let Some(i) = ui.selected_id() {
                ui.input = ui.index.accounts[i].label.clone();
                ui.modal = Modal::Rename;
            }
        }
        KeyCode::Char('y') if ui.modal == Modal::ConfirmMigration => {
            let n = migrate_legacy(&ui.config, &mut ui.index)?;
            save_index(p, &ui.index)?;
            ui.notice = format!("已迁移 {n} 个旧账户");
            ui.modal = Modal::None;
        }
        KeyCode::Enter => {
            match ui.modal {
                Modal::Import => {
                    let result = if ui.input.trim_start().starts_with('{') {
                        serde_json::from_str(&ui.input)
                            .map_err(AppError::from)
                            .and_then(|v| {
                                import_value(&ui.config, &mut ui.index, v, "粘贴 JSON".into(), None)
                            })
                    } else {
                        import_file(&ui.config, &mut ui.index, Path::new(ui.input.trim()))
                    };
                    ui.notice = match result {
                        Ok(()) => {
                            save_index(p, &ui.index)?;
                            "导入成功".into()
                        }
                        Err(e) => e.to_string(),
                    };
                }
                Modal::Filter => {
                    ui.filter = ui.input.clone();
                    ui.selected = 0;
                    ui.notice = if ui.filter.is_empty() {
                        "已清除过滤".into()
                    } else {
                        format!("按名称或邮箱过滤：{}", ui.filter)
                    };
                }
                Modal::Rename => {
                    if let Some(i) = ui.selected_id() {
                        if !ui.input.trim().is_empty() {
                            ui.index.accounts[i].label = ui.input.trim().into();
                            save_index(p, &ui.index)?;
                            ui.notice = "已重命名".into();
                        }
                    }
                }
                Modal::Settings => {
                    let path = PathBuf::from(ui.input.trim());
                    if path.as_os_str().is_empty() {
                        ui.notice = "路径不能为空".into();
                    } else {
                        ui.config.codex_home = path;
                        save_config(p, &ui.config)?;
                        ui.notice = "设置已保存".into();
                    }
                }
                Modal::Help => {}
                Modal::ConfirmUseEmail => {}
                _ => {}
            }
            ui.modal = Modal::None;
            ui.input.clear();
        }
        KeyCode::Backspace => {
            ui.input.pop();
        }
        KeyCode::Char(c)
            if matches!(
                ui.modal,
                Modal::Import | Modal::Filter | Modal::Rename | Modal::Settings
            ) =>
        {
            ui.input.push(c)
        }
        _ => {}
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_native_auth() {
        let v = json!({"tokens":{"access_token":"x.y.z","refresh_token":"r","id_token":"i","account_id":"a"}});
        let (out, _, id, _, _) = auth_tokens(&v).unwrap();
        assert_eq!(id.as_deref(), Some("a"));
        assert_eq!(out["auth_mode"], "chatgpt");
    }
    #[test]
    fn parses_web_session() {
        let v = json!({"accessToken":"a.b.c","account":{"id":"acct","planType":"plus"},"user":{"email":"a@example.com"}});
        let (_, _, id, email, plan) = auth_tokens(&v).unwrap();
        assert_eq!(id.as_deref(), Some("acct"));
        assert_eq!(email.as_deref(), Some("a@example.com"));
        assert_eq!(plan.as_deref(), Some("plus"));
    }
    #[test]
    fn reads_identity_from_native_id_token_claims() {
        let id_token = format!("x.{}.z", URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"email":"native@example.com","https://api.openai.com/auth":{"chatgpt_account_id":"acct-from-id","chatgpt_plan_type":"pro"}})).unwrap()));
        let v = json!({"tokens":{"access_token":"x.y.z","id_token":id_token}});
        let (_, _, id, email, plan) = auth_tokens(&v).unwrap();
        assert_eq!(id.as_deref(), Some("acct-from-id"));
        assert_eq!(email.as_deref(), Some("native@example.com"));
        assert_eq!(plan.as_deref(), Some("pro"));
    }
    #[test]
    fn quota_parses_legacy_and_wham_shapes() {
        let v = json!({"rate_limits":{"primary":{"used_percent":42.0,"window_minutes":300,"resets_at":1}}});
        assert_eq!(quota(&v, "primary").unwrap().used_percent, 42.0);
        let wham = json!({"rate_limit":{"primary_window":{"used_percent":25.0,"limit_window_seconds":18000,"reset_at":2}}});
        let parsed = quota(&wham, "primary").unwrap();
        assert_eq!(parsed.used_percent, 25.0);
        assert_eq!(parsed.window_minutes, Some(300));
        assert_eq!(parsed.resets_at, Some(2));
    }

    #[test]
    fn old_config_defaults_to_midnight_and_theme_persists() {
        let config: Config =
            toml::from_str("codex_home = '/tmp/codex'\naccounts_dir = '/tmp/accounts'\n").unwrap();
        assert_eq!(config.theme, Theme::Midnight);
        let encoded = toml::to_string(&Config {
            theme: Theme::Paper,
            ..config
        })
        .unwrap();
        assert!(encoded.contains("theme = \"paper\""));
    }

    #[test]
    fn themes_define_distinct_progress_layers() {
        for theme in [Theme::Midnight, Theme::Nord, Theme::Gruvbox, Theme::Paper] {
            let colors = theme.colors();
            assert_ne!(colors.progress_fill, colors.progress_track);
            assert_ne!(colors.progress_text, colors.progress_fill);
            assert_ne!(colors.progress_text, colors.progress_track);
            assert_ne!(colors.selected_bg, colors.selected_text);
        }
    }

    #[test]
    fn themes_cycle_in_documented_order() {
        assert_eq!(Theme::Midnight.next(), Theme::Nord);
        assert_eq!(Theme::Nord.next(), Theme::Gruvbox);
        assert_eq!(Theme::Gruvbox.next(), Theme::Paper);
        assert_eq!(Theme::Paper.next(), Theme::Midnight);
        assert!(HELP_TEXT.contains("t 切换主题"));
    }

    #[test]
    fn theme_key_saves_selection() {
        let root = env::temp_dir().join(format!("codex-switcher-test-{}", Uuid::new_v4()));
        let p = Paths {
            config_file: root.join("config.toml"),
            index_file: root.join("accounts.toml"),
            config_dir: root.clone(),
        };
        let mut ui = Ui {
            config: Config {
                codex_home: root.join("codex"),
                accounts_dir: root.join("accounts"),
                theme: Theme::Midnight,
            },
            index: AccountIndex::default(),
            selected: 0,
            filter: String::new(),
            modal: Modal::None,
            input: String::new(),
            notice: String::new(),
            tick: 0,
            checking: None,
        };
        handle_key(
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
            &p,
            &mut ui,
        )
        .unwrap();
        assert_eq!(ui.config.theme, Theme::Nord);
        assert_eq!(load_config(&p).unwrap().theme, Theme::Nord);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rename_confirms_detected_email_before_custom_input() {
        let root = env::temp_dir().join(format!("codex-switcher-rename-{}", Uuid::new_v4()));
        let p = Paths {
            config_file: root.join("config.toml"),
            index_file: root.join("accounts.toml"),
            config_dir: root.clone(),
        };
        let account = Account {
            id: Uuid::new_v4(),
            label: "旧名称".into(),
            source: "test".into(),
            imported_at: Utc::now(),
            email: Some("detected@example.com".into()),
            plan: None,
            account_id: None,
            status: CheckStatus::default(),
        };
        let mut ui = Ui {
            config: Config {
                codex_home: root.join("codex"),
                accounts_dir: root.join("accounts"),
                theme: Theme::Midnight,
            },
            index: AccountIndex {
                accounts: vec![account],
            },
            selected: 0,
            filter: String::new(),
            modal: Modal::None,
            input: String::new(),
            notice: String::new(),
            tick: 0,
            checking: None,
        };
        handle_key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &p,
            &mut ui,
        )
        .unwrap();
        assert_eq!(ui.modal, Modal::ConfirmUseEmail);
        handle_key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &p,
            &mut ui,
        )
        .unwrap();
        assert_eq!(ui.modal, Modal::Rename);
        assert_eq!(ui.input, "旧名称");
        handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &p, &mut ui).unwrap();
        handle_key(
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
            &p,
            &mut ui,
        )
        .unwrap();
        handle_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
            &p,
            &mut ui,
        )
        .unwrap();
        assert_eq!(ui.index.accounts[0].label, "detected@example.com");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn default_theme_renders_selection_active_badge_and_quota_label() {
        use ratatui::backend::TestBackend;
        let root = env::temp_dir().join(format!("codex-switcher-draw-{}", Uuid::new_v4()));
        let codex_home = root.join("codex");
        let accounts_dir = root.join("accounts");
        fs::create_dir_all(&codex_home).unwrap();
        fs::create_dir_all(&accounts_dir).unwrap();
        let id = Uuid::new_v4();
        let auth = b"active-auth";
        fs::write(codex_home.join("auth.json"), auth).unwrap();
        fs::write(accounts_dir.join(format!("{id}.auth.json")), auth).unwrap();
        let account = Account {
            id,
            label: "演示账户".into(),
            source: "test".into(),
            imported_at: Utc::now(),
            email: None,
            plan: None,
            account_id: None,
            status: CheckStatus {
                kind: StatusKind::Live,
                checked_at: None,
                detail: "正常".into(),
                primary: Some(Quota {
                    used_percent: 20.0,
                    window_minutes: Some(60),
                    resets_at: None,
                }),
                secondary: None,
            },
        };
        let ui = Ui {
            config: Config {
                codex_home,
                accounts_dir,
                theme: Theme::Midnight,
            },
            index: AccountIndex {
                accounts: vec![account],
            },
            selected: 0,
            filter: String::new(),
            modal: Modal::None,
            input: String::new(),
            notice: String::new(),
            tick: 0,
            checking: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &ui)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_buffer_contains(buffer, "当前");
        assert_buffer_contains(buffer, "80% 剩余");
        let colors = Theme::Midnight.colors();
        assert!(
            buffer
                .content()
                .iter()
                .any(|cell| cell.bg == colors.selected_bg)
        );
        assert!(
            buffer
                .content()
                .iter()
                .any(|cell| cell.fg == colors.progress_text)
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn assert_buffer_contains(buffer: &ratatui::buffer::Buffer, text: &str) {
        let rendered: String = buffer
            .content()
            .iter()
            .flat_map(|cell| cell.symbol().chars())
            .filter(|ch| !ch.is_whitespace())
            .collect();
        let expected: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert!(
            rendered.contains(&expected),
            "missing {text:?} in rendered UI"
        );
    }
}
