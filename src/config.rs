use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_monitored_dirs")]
    pub monitored_dirs: Vec<String>,

    #[serde(default = "default_ignored_processes")]
    pub ignored_processes: Vec<String>,

    #[serde(default = "default_ignored_packages")]
    pub ignored_packages: Vec<String>,

    #[serde(default = "default_tracking_depth")]
    pub tracking_depth: u32,

    #[serde(default = "default_auto_prune")]
    pub auto_prune: bool,
}

fn default_monitored_dirs() -> Vec<String> {
    vec![
        ".cache".to_string(),
        ".local".to_string(),
        ".config".to_string(),
    ]
}

fn default_ignored_processes() -> Vec<String> {
    vec![
        "nvim".to_string(),
        "vim".to_string(),
        "vi".to_string(),
        "nano".to_string(),
        "emacs".to_string(),
        "code".to_string(),
        "subl".to_string(),
        "hx".to_string(),
        "kate".to_string(),
        "gedit".to_string(),
        "cat".to_string(),
        "bat".to_string(),
        "less".to_string(),
        "more".to_string(),
        "head".to_string(),
        "tail".to_string(),
        "ls".to_string(),
        "find".to_string(),
        "fd".to_string(),
        "rg".to_string(),
        "grep".to_string(),
        "ag".to_string(),
        "file".to_string(),
        "stat".to_string(),
        "wc".to_string(),
        "du".to_string(),
        "tree".to_string(),
        "bash".to_string(),
        "zsh".to_string(),
        "fish".to_string(),
    ]
}

fn default_ignored_packages() -> Vec<String> {
    vec![]
}

fn default_tracking_depth() -> u32 {
    1
}

fn default_auto_prune() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            monitored_dirs: default_monitored_dirs(),
            ignored_processes: default_ignored_processes(),
            ignored_packages: default_ignored_packages(),
            tracking_depth: default_tracking_depth(),
            auto_prune: default_auto_prune(),
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        crate::db::get_user_home()
            .join(".config")
            .join("hdas")
            .join("config.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::path();

        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();

        if let Some(parent) = path.parent() {
            let (_, uid, gid) = crate::db::get_user_info();
            crate::db::create_dir_all_with_owner(parent, uid, gid)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;

        let (_, uid, gid) = crate::db::get_user_info();
        if let (Some(u), Some(g)) = (uid, gid) {
            let _ = std::os::unix::fs::chown(&path, Some(u), Some(g));
        }

        Ok(())
    }

    pub fn ensure_exists() -> Result<()> {
        let path = Self::path();
        if !path.exists() {
            Config::default().save()?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn is_ignored_process(&self, process: &str) -> bool {
        self.ignored_processes.iter().any(|p| p == process)
    }

    #[allow(dead_code)]
    pub fn is_monitored_path(&self, path: &str) -> bool {
        for dir in &self.monitored_dirs {
            let pattern1 = format!("/.{}/", dir.trim_start_matches('.'));
            let pattern2 = format!(".{}/", dir.trim_start_matches('.'));

            if path.contains(&pattern1) || path.starts_with(&pattern2) {
                return true;
            }

            if path.ends_with(&format!("/.{}", dir.trim_start_matches('.'))) {
                return true;
            }
        }
        false
    }

    #[allow(dead_code)]
    pub fn ignored_set(&self) -> HashSet<&str> {
        self.ignored_processes.iter().map(|s| s.as_str()).collect()
    }
}

pub fn default_config_content() -> String {
    r#"# HDAS Configuration File

monitored_dirs = [
    ".cache",
    ".local",
    ".config",
]

ignored_processes = [
    "nvim", "vim", "vi", "nano", "emacs", "code", "subl", "hx", "kate", "gedit",
    "cat", "bat", "less", "more", "head", "tail",
    "ls", "find", "fd", "rg", "grep", "ag", "file", "stat", "wc", "du", "tree",
    "bash", "zsh", "fish",
]

# Packages to skip entirely (noisy apps like browsers)
ignored_packages = []

# How deep to track under monitored dirs (1 = app dir like ~/.cache/mozilla)
# Note: ~/.local/share, ~/.local/state, and ~/.local/lib automatically add +1 depth
tracking_depth = 1

auto_prune = true
"#
    .to_string()
}
