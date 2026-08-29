//! Safe, reversible integration with Codex's user-level `config.toml`.
//!
//! Only the `model_provider` key and the `codex-switcher` provider table are
//! managed.  Unknown keys, formatting and comments are retained by
//! `toml_edit`; a disable refuses to overwrite externally changed managed
//! values.

use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item};
#[cfg(test)]
use toml_edit::{Table, Value};
use uuid::Uuid;

pub const PROVIDER_ID: &str = "codex-switcher";
pub const DEFAULT_PROXY_BASE_URL: &str = "http://127.0.0.1:8765/backend-api/codex";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum IntegrationStatus {
    Disabled,
    Enabled,
    Drifted(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IntegrationRecord {
    version: u32,
    original_model_provider_present: bool,
    original_model_provider: Option<String>,
    #[serde(default)]
    original_openai_base_url_present: bool,
    #[serde(default)]
    original_openai_base_url: Option<String>,
    #[serde(default)]
    remove_legacy_provider_on_disable: bool,
    base_url: String,
}

#[derive(Clone, Debug)]
pub struct CodexIntegration {
    codex_home: PathBuf,
    base_url: String,
}

impl CodexIntegration {
    pub fn new(codex_home: impl Into<PathBuf>) -> Self {
        Self::with_base_url(codex_home, DEFAULT_PROXY_BASE_URL)
    }

    pub fn with_base_url(codex_home: impl Into<PathBuf>, base_url: impl Into<String>) -> Self {
        Self {
            codex_home: codex_home.into(),
            base_url: base_url.into(),
        }
    }

    pub fn config_path(&self) -> PathBuf {
        self.codex_home.join("config.toml")
    }
    pub fn backup_path(&self) -> PathBuf {
        self.codex_home.join("config.toml.codex-switcher.bak")
    }
    pub fn record_path(&self) -> PathBuf {
        self.codex_home.join(".codex-switcher-integration.json")
    }

    pub fn status(&self) -> Result<IntegrationStatus> {
        if !self.record_path().exists() {
            return Ok(IntegrationStatus::Disabled);
        }
        let record = self.read_record()?;
        let doc = self.read_document()?;
        let drift = if record.version == 1 {
            legacy_managed_drift(&doc, &record.base_url)
        } else {
            managed_drift(&doc, &record.base_url)
        };
        match drift {
            Some(diff) => Ok(IntegrationStatus::Drifted(diff)),
            None => Ok(IntegrationStatus::Enabled),
        }
    }

    /// Upgrade a healthy v1 managed integration to the session-preserving v2
    /// layout. Drifted or already-current configurations are left untouched.
    pub fn migrate_if_needed(&self) -> Result<bool> {
        if !self.record_path().exists() {
            return Ok(false);
        }
        let record = self.read_record()?;
        if record.version != 1 || self.status()? != IntegrationStatus::Enabled {
            return Ok(false);
        }
        self.migrate_legacy_record(record)?;
        Ok(true)
    }

    pub fn enable(&self) -> Result<()> {
        ensure_private_dir(&self.codex_home)?;
        let config_path = self.config_path();
        reject_symlink(&config_path)?;
        reject_symlink(&self.record_path())?;

        if self.record_path().exists() {
            let record = self.read_record()?;
            return match self.status()? {
                IntegrationStatus::Enabled if record.version == 1 => {
                    self.migrate_legacy_record(record)
                }
                IntegrationStatus::Enabled => Ok(()),
                IntegrationStatus::Drifted(diff) => Err(AppError::Message(format!(
                    "Codex 配置已被外部修改，拒绝覆盖：{diff}"
                ))),
                IntegrationStatus::Disabled => unreachable!(),
            };
        }

        let original = if config_path.exists() {
            fs::read(&config_path)?
        } else {
            Vec::new()
        };
        let mut doc = parse_document(&original)?;
        let original_item = doc.get("model_provider");
        let original_present = original_item.is_some();
        let original_provider = original_item.and_then(Item::as_str).map(str::to_owned);
        if original_present && original_provider.is_none() {
            return Err(AppError::Message(
                "model_provider 不是字符串，拒绝修改".into(),
            ));
        }
        let original_base_url_item = doc.get("openai_base_url");
        let original_base_url_present = original_base_url_item.is_some();
        let original_base_url = original_base_url_item
            .and_then(Item::as_str)
            .map(str::to_owned);
        if original_base_url_present && original_base_url.is_none() {
            return Err(AppError::Message(
                "openai_base_url 不是字符串，拒绝修改".into(),
            ));
        }

        // A complete pre-change backup is created before the first managed write.
        if !self.backup_path().exists() {
            atomic_write(&self.backup_path(), &original)?;
        }

        // Keep Codex's built-in provider identity so sessions created before,
        // during and after proxy use stay in the same resume namespace.
        doc["model_provider"] = toml_edit::value("openai");
        doc["openai_base_url"] = toml_edit::value(self.base_url.clone());

        let record = IntegrationRecord {
            version: 2,
            original_model_provider_present: original_present,
            original_model_provider: original_provider,
            original_openai_base_url_present: original_base_url_present,
            original_openai_base_url: original_base_url,
            remove_legacy_provider_on_disable: false,
            base_url: self.base_url.clone(),
        };
        atomic_write(&config_path, doc.to_string().as_bytes())?;
        atomic_write(&self.record_path(), &serde_json::to_vec_pretty(&record)?)?;
        Ok(())
    }

    pub fn disable(&self) -> Result<()> {
        if !self.record_path().exists() {
            return Ok(());
        }
        let record = self.read_record()?;
        let mut doc = self.read_document()?;
        let drift = if record.version == 1 {
            legacy_managed_drift(&doc, &record.base_url)
        } else {
            managed_drift(&doc, &record.base_url)
        };
        if let Some(diff) = drift {
            return Err(AppError::Message(format!(
                "Codex 配置已被外部修改，拒绝覆盖：{diff}"
            )));
        }

        if record.original_model_provider_present {
            let value = record
                .original_model_provider
                .ok_or_else(|| AppError::Message("接入记录中的原 model_provider 无效".into()))?;
            doc["model_provider"] = toml_edit::value(value);
        } else {
            doc.remove("model_provider");
        }
        if (record.version == 1 || record.remove_legacy_provider_on_disable)
            && let Some(providers) = doc.get_mut("model_providers").and_then(Item::as_table_mut)
        {
            providers.remove(PROVIDER_ID);
        }
        if record.version >= 2 {
            if record.original_openai_base_url_present {
                let value = record.original_openai_base_url.ok_or_else(|| {
                    AppError::Message("接入记录中的原 openai_base_url 无效".into())
                })?;
                doc["openai_base_url"] = toml_edit::value(value);
            } else {
                doc.remove("openai_base_url");
            }
        }

        atomic_write(&self.config_path(), doc.to_string().as_bytes())?;
        fs::remove_file(self.record_path())?;
        Ok(())
    }

    fn migrate_legacy_record(&self, legacy: IntegrationRecord) -> Result<()> {
        let mut doc = self.read_document()?;
        if let Some(diff) = legacy_managed_drift(&doc, &legacy.base_url) {
            return Err(AppError::Message(format!(
                "Codex 配置已被外部修改，拒绝迁移：{diff}"
            )));
        }
        let backup = if self.backup_path().exists() {
            parse_document(&fs::read(self.backup_path())?)?
        } else {
            DocumentMut::new()
        };
        let original_base_url_item = backup.get("openai_base_url");
        let original_base_url_present = original_base_url_item.is_some();
        let original_base_url = original_base_url_item
            .and_then(Item::as_str)
            .map(str::to_owned);
        if original_base_url_present && original_base_url.is_none() {
            return Err(AppError::Message(
                "备份中的 openai_base_url 不是字符串，拒绝迁移".into(),
            ));
        }

        doc["model_provider"] = toml_edit::value("openai");
        doc["openai_base_url"] = toml_edit::value(self.base_url.clone());
        // Keep the old provider as an inactive alias while proxy mode is on.
        // This lets sessions created by the buggy v1 integration still be
        // resumed, while all new sessions use the built-in `openai` identity.
        let record = IntegrationRecord {
            version: 2,
            original_model_provider_present: legacy.original_model_provider_present,
            original_model_provider: legacy.original_model_provider,
            original_openai_base_url_present: original_base_url_present,
            original_openai_base_url: original_base_url,
            remove_legacy_provider_on_disable: true,
            base_url: self.base_url.clone(),
        };
        atomic_write(&self.config_path(), doc.to_string().as_bytes())?;
        atomic_write(&self.record_path(), &serde_json::to_vec_pretty(&record)?)?;
        Ok(())
    }

    fn read_record(&self) -> Result<IntegrationRecord> {
        reject_symlink(&self.record_path())?;
        Ok(serde_json::from_slice(&fs::read(self.record_path())?)?)
    }

    fn read_document(&self) -> Result<DocumentMut> {
        let path = self.config_path();
        reject_symlink(&path)?;
        let raw = if path.exists() {
            fs::read(path)?
        } else {
            Vec::new()
        };
        parse_document(&raw)
    }
}

fn parse_document(raw: &[u8]) -> Result<DocumentMut> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| AppError::Message("Codex config.toml 不是有效 UTF-8".into()))?;
    text.parse::<DocumentMut>()
        .map_err(|e| AppError::Message(format!("Codex config.toml 无法解析：{e}")))
}

fn provider_item(doc: &DocumentMut) -> Option<&Item> {
    doc.get("model_providers")
        .and_then(Item::as_table)
        .and_then(|table| table.get(PROVIDER_ID))
}

#[cfg(test)]
fn expected_provider(base_url: &str) -> Item {
    let mut table = Table::new();
    table.insert("name", Item::Value(Value::from("Codex Switcher")));
    table.insert("base_url", Item::Value(Value::from(base_url)));
    table.insert("wire_api", Item::Value(Value::from("responses")));
    table.insert("requires_openai_auth", Item::Value(Value::from(true)));
    table.insert("supports_websockets", Item::Value(Value::from(false)));
    Item::Table(table)
}

fn provider_matches(item: &Item, base_url: &str) -> bool {
    let Some(table) = item.as_table() else {
        return false;
    };
    table.get("name").and_then(Item::as_str) == Some("Codex Switcher")
        && table.get("base_url").and_then(Item::as_str) == Some(base_url)
        && table.get("wire_api").and_then(Item::as_str) == Some("responses")
        && table.get("requires_openai_auth").and_then(Item::as_bool) == Some(true)
        && table.get("supports_websockets").and_then(Item::as_bool) == Some(false)
        && table.len() == 5
}

fn managed_drift(doc: &DocumentMut, base_url: &str) -> Option<String> {
    if doc.get("model_provider").and_then(Item::as_str) != Some("openai") {
        return Some("model_provider 不再是 openai".into());
    }
    if doc.get("openai_base_url").and_then(Item::as_str) != Some(base_url) {
        return Some("openai_base_url 已变化".into());
    }
    None
}

fn legacy_managed_drift(doc: &DocumentMut, base_url: &str) -> Option<String> {
    if doc.get("model_provider").and_then(Item::as_str) != Some(PROVIDER_ID) {
        return Some("model_provider 不再是 codex-switcher".into());
    }
    match provider_item(doc) {
        Some(item) if provider_matches(item, base_url) => None,
        Some(_) => Some("model_providers.codex-switcher 内容已变化".into()),
        None => Some("model_providers.codex-switcher 已被删除".into()),
    }
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

    fn temp_home() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("codex-switcher-integration-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn enable_and_disable_preserve_unknown_content() {
        let home = temp_home();
        fs::create_dir_all(home.join("sessions/2026/08/28")).unwrap();
        let session_path = home.join("sessions/2026/08/28/rollout.jsonl");
        fs::write(&session_path, b"session sentinel\n").unwrap();
        let history_path = home.join("history.jsonl");
        fs::write(&history_path, b"history sentinel\n").unwrap();
        let state_path = home.join("state_5.sqlite");
        fs::write(&state_path, b"sqlite sentinel").unwrap();
        fs::write(
            home.join("config.toml"),
            "# keep me\nmodel_provider = \"openai\"\nfoo = 7\n",
        )
        .unwrap();
        let integration = CodexIntegration::new(&home);
        integration.enable().unwrap();
        let enabled = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(enabled.contains("# keep me"));
        assert!(enabled.contains("foo = 7"));
        assert!(enabled.contains("model_provider = \"openai\""));
        assert!(enabled.contains("openai_base_url = \"http://127.0.0.1:8765/backend-api/codex\""));
        integration.disable().unwrap();
        let disabled = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(disabled.contains("model_provider = \"openai\""));
        assert!(!disabled.contains("openai_base_url"));
        assert!(disabled.contains("foo = 7"));
        assert_eq!(fs::read(session_path).unwrap(), b"session sentinel\n");
        assert_eq!(fs::read(history_path).unwrap(), b"history sentinel\n");
        assert_eq!(fs::read(state_path).unwrap(), b"sqlite sentinel");
    }

    #[test]
    fn disable_detects_external_drift() {
        let home = temp_home();
        let integration = CodexIntegration::new(&home);
        integration.enable().unwrap();
        let path = home.join("config.toml");
        let raw = fs::read_to_string(&path)
            .unwrap()
            .replace(DEFAULT_PROXY_BASE_URL, "http://elsewhere");
        fs::write(path, raw).unwrap();
        assert!(
            integration
                .disable()
                .unwrap_err()
                .to_string()
                .contains("外部修改")
        );
    }

    #[test]
    fn unrelated_custom_provider_is_preserved() {
        let home = temp_home();
        fs::write(
            home.join("config.toml"),
            "[model_providers.codex-switcher]\nbase_url = \"http://elsewhere\"\n",
        )
        .unwrap();
        let integration = CodexIntegration::new(&home);
        integration.enable().unwrap();
        let enabled = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(enabled.contains("base_url = \"http://elsewhere\""));
        integration.disable().unwrap();
        let disabled = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(disabled.contains("base_url = \"http://elsewhere\""));
    }

    #[test]
    fn legacy_record_migrates_without_touching_unknown_config() {
        let home = temp_home();
        fs::write(
            home.join("config.toml.codex-switcher.bak"),
            "# original\nopenai_base_url = \"https://original.example\"\nfoo = 9\n",
        )
        .unwrap();
        let mut legacy_doc = DocumentMut::new();
        legacy_doc["model_provider"] = toml_edit::value(PROVIDER_ID);
        legacy_doc["foo"] = toml_edit::value(9);
        let providers = legacy_doc
            .entry("model_providers")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
            .unwrap();
        providers.insert(PROVIDER_ID, expected_provider(DEFAULT_PROXY_BASE_URL));
        fs::write(home.join("config.toml"), legacy_doc.to_string()).unwrap();
        fs::write(
            home.join(".codex-switcher-integration.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "original_model_provider_present": false,
                "original_model_provider": null,
                "base_url": DEFAULT_PROXY_BASE_URL
            }))
            .unwrap(),
        )
        .unwrap();

        let integration = CodexIntegration::new(&home);
        integration.enable().unwrap();
        let migrated = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(migrated.contains("model_provider = \"openai\""));
        assert!(migrated.contains("openai_base_url = \"http://127.0.0.1:8765/backend-api/codex\""));
        assert!(migrated.contains("model_providers.codex-switcher"));
        assert!(migrated.contains("foo = 9"));

        integration.disable().unwrap();
        let restored = fs::read_to_string(home.join("config.toml")).unwrap();
        let restored_doc = restored.parse::<DocumentMut>().unwrap();
        assert!(restored_doc.get("model_provider").is_none());
        assert!(!restored.contains("model_providers.codex-switcher"));
        assert!(restored.contains("openai_base_url = \"https://original.example\""));
    }
}
