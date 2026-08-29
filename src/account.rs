use crate::{
    config::Config,
    error::*,
    paths::Paths,
    types::{Account, AccountIndex, CheckStatus, Quota, StatusKind},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use reqwest::{blocking::Client, header::*};
use serde_json::{Value, json};
use std::{env, fs, path::Path, time::Duration};
use uuid::Uuid;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/codex/usage";

pub fn load_index(p: &Paths) -> Result<AccountIndex> {
    if !p.index_file.exists() {
        return Ok(AccountIndex::default());
    }
    reject_symlink(&p.index_file)?;
    Ok(toml::from_str(&fs::read_to_string(&p.index_file)?)?)
}

pub fn save_index(p: &Paths, i: &AccountIndex) -> Result<()> {
    atomic_write(&p.index_file, toml::to_string_pretty(i)?.as_bytes())
}

pub fn snapshot_path(config: &Config, id: Uuid) -> std::path::PathBuf {
    config.accounts_dir.join(format!("{id}.auth.json"))
}

pub type AuthTokens = (
    Value,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

pub fn auth_tokens(v: &Value) -> Result<AuthTokens> {
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

pub fn import_value(
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
        tenant_id: "local".into(),
        proxy_enabled: false,
    });
    Ok(())
}

pub fn import_file(config: &Config, index: &mut AccountIndex, path: &Path) -> Result<()> {
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

pub fn import_current(config: &Config, index: &mut AccountIndex) -> Result<()> {
    import_file(config, index, &config.codex_home.join("auth.json"))
}

pub fn activate(config: &Config, account: &Account) -> Result<()> {
    // Validate before process inspection or any filesystem call. In
    // particular, never pass a corrupted overlong config value to mkdir.
    crate::config::validate_codex_home(&config.codex_home)?;
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

pub fn probe(config: &Config, account: &mut Account) {
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

fn current_codex_running() -> bool {
    let uid = libc_uid();
    std::process::Command::new("pgrep")
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

fn reject_symlink(path: &Path) -> Result<()> {
    if path.exists() && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(AppError::Message(format!(
            "出于安全原因，拒绝符号链接：{}",
            path.display()
        )));
    }
    Ok(())
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

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
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

    #[test]
    fn activation_rejects_invalid_home_before_filesystem_access() {
        let mut config = Config::defaults();
        config.codex_home = Path::new("not-an-absolute-codex-home").to_path_buf();
        let account = Account {
            id: Uuid::new_v4(),
            label: "test".into(),
            source: "test".into(),
            imported_at: Utc::now(),
            email: None,
            plan: None,
            account_id: None,
            status: CheckStatus::default(),
            tenant_id: "local".into(),
            proxy_enabled: false,
        };

        let error = activate(&config, &account).unwrap_err().to_string();
        assert!(error.contains("Codex 目录配置无效"));
        assert!(!error.contains("os error 36"));
    }
}
