// 共享类型和工具模块
pub mod account;
pub mod config;
pub mod diagnostics;
pub mod error;
pub(crate) mod filesystem;
pub mod i18n;
pub mod identity;
pub mod integration;
pub mod paths;
pub mod storage;
pub mod types;

// 核心功能模块
pub mod cli;
pub mod daemon;
pub mod proxy;
pub mod tui;

pub use error::{AppError, Result};
pub use paths::Paths;
