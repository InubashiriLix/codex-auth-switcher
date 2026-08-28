use crate::{error::*, paths::Paths};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    path::PathBuf,
    sync::mpsc::{channel, Receiver},
};

#[derive(Debug, Clone)]
pub enum ReloadEvent {
    ConfigChanged,
    AccountsChanged,
    SnapshotChanged,
}

#[derive(Clone)]
pub struct HotReloader {
    event_rx: Arc<Mutex<Receiver<notify::Result<Event>>>>,
    config_path: PathBuf,
    index_path: PathBuf,
}

use parking_lot::Mutex;
use std::sync::Arc;

impl HotReloader {
    pub fn new(paths: &Paths) -> Result<Self> {
        let (tx, rx) = channel();

        let mut watcher: RecommendedWatcher = notify::recommended_watcher(
            move |res: notify::Result<Event>| {
                let _ = tx.send(res);
            },
        )?;

        // 监控配置文件
        if paths.config_file.exists() {
            watcher.watch(&paths.config_file, RecursiveMode::NonRecursive)?;
        }

        // 监控账户索引
        if paths.index_file.exists() {
            watcher.watch(&paths.index_file, RecursiveMode::NonRecursive)?;
        }

        // 监控配置目录（递归）
        if paths.config_dir.exists() {
            watcher.watch(&paths.config_dir, RecursiveMode::Recursive)?;
        }

        // 保持watcher存活
        std::mem::forget(watcher);

        Ok(Self {
            event_rx: Arc::new(Mutex::new(rx)),
            config_path: paths.config_file.clone(),
            index_path: paths.index_file.clone(),
        })
    }

    pub fn poll(&self) -> Option<ReloadEvent> {
        let rx = self.event_rx.lock();
        match rx.try_recv() {
            Ok(Ok(event)) => {
                for path in &event.paths {
                    if path == &self.index_path {
                        return Some(ReloadEvent::AccountsChanged);
                    } else if path == &self.config_path {
                        return Some(ReloadEvent::ConfigChanged);
                    } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
                        return Some(ReloadEvent::SnapshotChanged);
                    }
                }
                None
            }
            _ => None,
        }
    }
}
