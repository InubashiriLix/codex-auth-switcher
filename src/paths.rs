use std::{env, path::PathBuf};

const APP: &str = "codex-switcher";

#[derive(Clone, Debug)]
pub struct Paths {
    pub config_file: PathBuf,
    pub index_file: PathBuf,
    pub config_dir: PathBuf,
    pub pid_file: PathBuf,
    pub runtime_file: PathBuf,
    pub database_file: PathBuf,
}

pub fn paths() -> Paths {
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

    let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    Paths {
        config_file: config_dir.join("config.toml"),
        index_file: data.join("accounts.toml"),
        config_dir,
        pid_file: runtime_dir.join(format!("{APP}.pid")),
        runtime_file: runtime_dir.join(format!("{APP}.runtime.json")),
        database_file: data.join("runtime.sqlite3"),
    }
}
