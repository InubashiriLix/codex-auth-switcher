use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: Uuid,
    pub label: String,
    pub source: String,
    pub imported_at: DateTime<Utc>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub status: CheckStatus,
    /// Reserved for the remote/multi-tenant protocol. Version one only uses `local`.
    #[serde(default = "default_tenant_id")]
    pub tenant_id: String,
    /// Accounts are opt-in for proxy routing after upgrading an old index.
    #[serde(default)]
    pub proxy_enabled: bool,
}

fn default_tenant_id() -> String {
    "local".into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckStatus {
    pub kind: StatusKind,
    pub checked_at: Option<DateTime<Utc>>,
    pub detail: String,
    pub primary: Option<Quota>,
    pub secondary: Option<Quota>,
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
pub enum StatusKind {
    Live,
    Exhausted,
    Reauth,
    AccessDenied,
    Invalid,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Quota {
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct AccountIndex {
    #[serde(default)]
    pub accounts: Vec<Account>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Midnight,
    Nord,
    Gruvbox,
    Paper,
}

impl Theme {
    pub fn name(self) -> &'static str {
        match self {
            Self::Midnight => "Midnight",
            Self::Nord => "Nord",
            Self::Gruvbox => "Gruvbox",
            Self::Paper => "Paper",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Midnight => Self::Nord,
            Self::Nord => Self::Gruvbox,
            Self::Gruvbox => Self::Paper,
            Self::Paper => Self::Midnight,
        }
    }
}
