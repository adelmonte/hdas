use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct MonitoredDir {
    pub path: String,
    pub depth: Option<u32>,
}

impl MonitoredDir {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            depth: None,
        }
    }
}

impl Serialize for MonitoredDir {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeMap;
        if self.depth.is_some() {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("path", &self.path)?;
            map.serialize_entry("depth", &self.depth)?;
            map.end()
        } else {
            serializer.serialize_str(&self.path)
        }
    }
}

impl<'de> Deserialize<'de> for MonitoredDir {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum MonitoredDirHelper {
            Simple(String),
            Full { path: String, depth: Option<u32> },
        }

        match MonitoredDirHelper::deserialize(deserializer)? {
            MonitoredDirHelper::Simple(path) => Ok(MonitoredDir { path, depth: None }),
            MonitoredDirHelper::Full { path, depth } => Ok(MonitoredDir { path, depth }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_monitored_dirs")]
    pub monitored_dirs: Vec<MonitoredDir>,

    #[serde(default = "default_ignored_processes")]
    pub ignored_processes: Vec<String>,

    #[serde(default = "default_ignored_packages")]
    pub ignored_packages: Vec<String>,

    #[serde(default = "default_tracking_depth")]
    pub tracking_depth: u32,

    #[serde(default = "default_auto_prune")]
    pub auto_prune: bool,
}

fn default_monitored_dirs() -> Vec<MonitoredDir> {
    vec![
        MonitoredDir::new(".cache"),
        MonitoredDir::new(".local"),
        MonitoredDir::new(".config"),
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
            if let Err(e) = std::os::unix::fs::chown(&path, Some(u), Some(g)) {
                eprintln!("Warning: failed to chown {}: {}", path.display(), e);
            }
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
}

pub fn default_config_content() -> String {
    r#"# HDAS Configuration File

# Directories to monitor
# Use [[monitored_dirs]] for per-directory depth, or simple strings for global depth
#
# Depth controls how much of the path is kept after the monitored dir:
#   depth=1: ~/.cache/mozilla/firefox/... -> ~/.cache/mozilla
#   depth=2: ~/.cache/mozilla/firefox/... -> ~/.cache/mozilla/firefox
#   depth=0: full path, no truncation
#
# Use `hdas explain <path>` to see how a path would be tracked.

[[monitored_dirs]]
path = ".cache"

[[monitored_dirs]]
path = ".local"

[[monitored_dirs]]
path = ".config"

# [[monitored_dirs]]
# path = "/etc/"
# depth = 0

ignored_processes = [
    "nvim", "vim", "vi", "nano", "emacs", "code", "subl", "hx", "kate", "gedit",
    "cat", "bat", "less", "more", "head", "tail",
    "ls", "find", "fd", "rg", "grep", "ag", "file", "stat", "wc", "du", "tree",
    "bash", "zsh", "fish",
]

# Packages to skip entirely (noisy apps like browsers)
ignored_packages = []

# Default depth for monitored dirs without explicit depth (1 = app dir like ~/.cache/mozilla)
# Note: ~/.local/share, ~/.local/state, and ~/.local/lib automatically add +1 depth
tracking_depth = 1

auto_prune = true
"#
    .to_string()
}
