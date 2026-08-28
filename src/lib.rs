// 共享类型和工具模块
pub mod types;
pub mod config;
pub mod account;
pub mod error;
pub mod paths;

// 核心功能模块
pub mod proxy;
pub mod daemon;
pub mod tui;
pub mod cli;

pub use error::{AppError, Result};
pub use paths::Paths;
