//! Compatibility layer for the current Codex OAuth refresh flow.

use crate::{
    account::{auth_tokens, snapshot_path},
    config::Config,
    error::{AppError, Result},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use parking_lot::Mutex as SyncMutex;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashMap, fs, path::Path, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

const TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const CODEX_PUBLIC_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefreshOutcome {
    Refreshed,
    StillValid,
    ReauthRequired,
}

#[derive(Clone)]
pub struct TokenRefresher {
    config: Config,
    client: reqwest::Client,
    locks: Arc<SyncMutex<HashMap<Uuid, Arc<Mutex<()>>>>>,
    endpoint: &'static str,
}

#[derive(Deserialize)]
struct RefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

impl TokenRefresher {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            locks: Arc::new(SyncMutex::new(HashMap::new())),
            endpoint: TOKEN_ENDPOINT,
        }
    }

    #[cfg(test)]
    pub fn with_endpoint_for_tests(config: Config, endpoint: &'static str) -> Self {
        let mut this = Self::new(config);
        this.endpoint = endpoint;
        this
    }

    pub async fn refresh(&self, account_id: Uuid, force: bool) -> Result<RefreshOutcome> {
        self.refresh_inner(account_id, force, None).await
    }

    pub async fn refresh_rejected(
        &self,
        account_id: Uuid,
        rejected_access: &str,
    ) -> Result<RefreshOutcome> {
        self.refresh_inner(account_id, true, Some(rejected_access))
            .await
    }

    async fn refresh_inner(
        &self,
        account_id: Uuid,
        force: bool,
        rejected_access: Option<&str>,
    ) -> Result<RefreshOutcome> {
        let lock = self
            .locks
            .lock()
            .entry(account_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-read after acquiring the per-account single-flight lock: another
        // request may already have refreshed and rotated the token.
        let path = snapshot_path(&self.config, account_id);
        let raw = fs::read_to_string(&path)?;
        let value: Value = serde_json::from_str(&raw)?;
        let (_, access, _, _, _) = auth_tokens(&value)?;
        if rejected_access.is_some_and(|rejected| rejected != access) {
            return Ok(RefreshOutcome::StillValid);
        }
        if !force && !expires_within(&access, 300) {
            return Ok(RefreshOutcome::StillValid);
        }
        let refresh_token = value
            .pointer("/tokens/refresh_token")
            .or_else(|| value.get("refresh_token"))
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| AppError::Message("账户没有 refresh token，需要重新登录".into()))?;

        let response = self
            .client
            .post(self.endpoint)
            .form(&[
                ("client_id", CODEX_PUBLIC_CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            // Never include the provider response body: it can echo sensitive data.
            return if status.as_u16() == 400 || status.as_u16() == 401 {
                Ok(RefreshOutcome::ReauthRequired)
            } else {
                Err(AppError::Message(format!(
                    "OAuth 刷新返回 HTTP {}",
                    status.as_u16()
                )))
            };
        }

        let refreshed: RefreshResponse = response
            .json()
            .await
            .map_err(|_| AppError::Message("OAuth 刷新响应格式无效".into()))?;
        if refreshed.access_token.is_empty() {
            return Err(AppError::Message("OAuth 刷新响应没有 access token".into()));
        }
        let old_refresh = value
            .pointer("/tokens/refresh_token")
            .and_then(Value::as_str)
            .unwrap_or("");
        let old_id = value
            .pointer("/tokens/id_token")
            .and_then(Value::as_str)
            .unwrap_or("");
        let account = value
            .pointer("/tokens/account_id")
            .cloned()
            .unwrap_or(Value::Null);
        let canonical = json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": null,
            "tokens": {
                "access_token": refreshed.access_token,
                "refresh_token": refreshed.refresh_token.as_deref().unwrap_or(old_refresh),
                "id_token": refreshed.id_token.as_deref().unwrap_or(old_id),
                "account_id": account,
            },
            "last_refresh": Utc::now().to_rfc3339(),
        });
        atomic_write(&path, &serde_json::to_vec_pretty(&canonical)?)?;
        Ok(RefreshOutcome::Refreshed)
    }
}

pub fn expires_within(jwt: &str, seconds: i64) -> bool {
    let expiration = jwt
        .split('.')
        .nth(1)
        .and_then(|payload| URL_SAFE_NO_PAD.decode(payload).ok())
        .and_then(|payload| serde_json::from_slice::<Value>(&payload).ok())
        .and_then(|claims| claims.get("exp").and_then(Value::as_i64));
    expiration.is_none_or(|exp| exp <= Utc::now().timestamp() + seconds)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(AppError::Message("拒绝写入符号链接认证快照".into()));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_token_is_treated_as_expiring() {
        assert!(expires_within("not-a-jwt", 300));
    }
}
