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
use toml_edit::{DocumentMut, Item, Table, Value};
use uuid::Uuid;

pub const PROVIDER_ID: &str = "codex-switcher";
pub const DEFAULT_PROXY_BASE_URL: &str = "http://127.0.0.1:8765/backend-api/codex";

#[derive(Clone, Debug, PartialEq, Eq)]
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
        match managed_drift(&doc, &record.base_url) {
            Some(diff) => Ok(IntegrationStatus::Drifted(diff)),
            None => Ok(IntegrationStatus::Enabled),
        }
    }

    pub fn enable(&self) -> Result<()> {
        ensure_private_dir(&self.codex_home)?;
        let config_path = self.config_path();
        reject_symlink(&config_path)?;
        reject_symlink(&self.record_path())?;

        if self.record_path().exists() {
            return match self.status()? {
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
        if let Some(existing) = provider_item(&doc)
            && !provider_matches(existing, &self.base_url)
        {
            return Err(AppError::Message(
                "已存在名称为 codex-switcher 且内容不同的 model provider；请先处理冲突".into(),
            ));
        }

        let original_item = doc.get("model_provider");
        let original_present = original_item.is_some();
        let original_provider = original_item.and_then(Item::as_str).map(str::to_owned);
        if original_present && original_provider.is_none() {
            return Err(AppError::Message(
                "model_provider 不是字符串，拒绝修改".into(),
            ));
        }

        // A complete pre-change backup is created before the first managed write.
        if !self.backup_path().exists() {
            atomic_write(&self.backup_path(), &original)?;
        }

        doc["model_provider"] = toml_edit::value(PROVIDER_ID);
        let providers = doc
            .entry("model_providers")
            .or_insert(Item::Table(Table::new()));
        let providers = providers
            .as_table_mut()
            .ok_or_else(|| AppError::Message("model_providers 不是 TOML 表".into()))?;
        providers.insert(PROVIDER_ID, expected_provider(&self.base_url));

        let record = IntegrationRecord {
            version: 1,
            original_model_provider_present: original_present,
            original_model_provider: original_provider,
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
        if let Some(diff) = managed_drift(&doc, &record.base_url) {
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
        if let Some(providers) = doc.get_mut("model_providers").and_then(Item::as_table_mut) {
            providers.remove(PROVIDER_ID);
        }

        atomic_write(&self.config_path(), doc.to_string().as_bytes())?;
        fs::remove_file(self.record_path())?;
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

    fn temp_home() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("codex-switcher-integration-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn enable_and_disable_preserve_unknown_content() {
        let home = temp_home();
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
        assert!(enabled.contains("supports_websockets = false"));
        integration.disable().unwrap();
        let disabled = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(disabled.contains("model_provider = \"openai\""));
        assert!(disabled.contains("foo = 7"));
    }

    #[test]
    fn disable_detects_external_drift() {
        let home = temp_home();
        let integration = CodexIntegration::new(&home);
        integration.enable().unwrap();
        let path = home.join("config.toml");
        let raw = fs::read_to_string(&path)
            .unwrap()
            .replace("wire_api = \"responses\"", "wire_api = \"chat\"");
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
    fn conflicting_provider_is_not_overwritten() {
        let home = temp_home();
        fs::write(
            home.join("config.toml"),
            "[model_providers.codex-switcher]\nbase_url = \"http://elsewhere\"\n",
        )
        .unwrap();
        let error = CodexIntegration::new(&home)
            .enable()
            .unwrap_err()
            .to_string();
        assert!(error.contains("内容不同"));
    }
}
