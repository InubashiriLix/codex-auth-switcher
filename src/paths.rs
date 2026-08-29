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
    let home = user_home_dir();
    #[cfg(windows)]
    {
        let config_dir = env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join("CodexSwitcher");
        let data_dir = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Local"))
            .join("CodexSwitcher");
        return Paths {
            config_file: config_dir.join("config.toml"),
            index_file: data_dir.join("accounts.toml"),
            config_dir,
            pid_file: data_dir.join(format!("{APP}.pid")),
            runtime_file: data_dir.join(format!("{APP}.runtime.json")),
            database_file: data_dir.join("runtime.sqlite3"),
        };
    }
    #[cfg(not(windows))]
    {
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
}

/// Isolated storage used by the Windows LocalService daemon. The service root
/// is explicit so the service never silently falls back to a system profile.
pub fn service_paths(root: PathBuf) -> Paths {
    let config_dir = root.join("config");
    let data_dir = root.join("data");
    let runtime_dir = root.join("runtime");
    Paths {
        config_file: config_dir.join("config.toml"),
        index_file: data_dir.join("accounts.toml"),
        config_dir,
        pid_file: runtime_dir.join(format!("{APP}.pid")),
        runtime_file: runtime_dir.join(format!("{APP}.runtime.json")),
        database_file: data_dir.join("runtime.sqlite3"),
    }
}

pub fn user_home_dir() -> PathBuf {
    #[cfg(windows)]
    {
        env::var_os("USERPROFILE")
            .or_else(|| env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }
    #[cfg(not(windows))]
    {
        env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_paths_stay_inside_the_explicit_root() {
        let root = PathBuf::from("/var/lib/codex-switcher-test");
        let paths = service_paths(root.clone());
        for path in [
            &paths.config_file,
            &paths.index_file,
            &paths.pid_file,
            &paths.runtime_file,
            &paths.database_file,
        ] {
            assert!(path.starts_with(&root), "{}", path.display());
        }
    }
}
