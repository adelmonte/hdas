use anyhow::Result;
use serde::{Deserialize, Deserializer};
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

impl<'de> Deserialize<'de> for MonitoredDir {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct FullDir {
            path: String,
            depth: Option<u32>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum MonitoredDirHelper {
            Simple(String),
            Full(FullDir),
        }

        match MonitoredDirHelper::deserialize(deserializer)? {
            MonitoredDirHelper::Simple(path) => Ok(MonitoredDir { path, depth: None }),
            MonitoredDirHelper::Full(full) => Ok(MonitoredDir { path: full.path, depth: full.depth }),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_monitored_dirs")]
    pub monitored_dirs: Vec<MonitoredDir>,

    #[serde(default = "default_ignored_processes")]
    pub ignored_processes: Vec<String>,

    #[serde(default = "default_ignored_packages")]
    pub ignored_packages: Vec<String>,

    #[serde(default = "default_excluded_paths")]
    pub excluded_paths: Vec<String>,

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

/// Shared infrastructure dirs written by many unrelated programs, so no single
/// package legitimately "owns" them. Excluded by default so they are never
/// attributed (or offered for deletion). Built from the real home at runtime
/// because excluded_paths are matched as absolute prefixes.
fn default_excluded_paths() -> Vec<String> {
    let home = crate::db::get_user_home();
    [
        ".config/kdedefaults",
        ".config/dconf",
        ".config/pulse",
        ".cache/fontconfig",
        ".cache/mesa_shader_cache",
        ".cache/mesa_shader_cache_db",
        ".cache/thumbnails",
    ]
    .iter()
    .map(|p| home.join(p).to_string_lossy().into_owned())
    .collect()
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
            excluded_paths: default_excluded_paths(),
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

    pub fn ensure_exists() -> Result<()> {
        let path = Self::path();
        if path.exists() {
            return Ok(());
        }

        let (_, uid, gid) = crate::db::get_user_info();
        if let Some(parent) = path.parent() {
            crate::db::create_dir_all_with_owner(parent, uid, gid)?;
        }

        std::fs::write(&path, default_config_content())?;
        chown_to_user(&path, uid, gid);
        Ok(())
    }

    /// Append a string value to a top-level array key, editing the config
    /// file in place so comments and formatting are preserved.
    /// Returns false if the value was already present.
    pub fn add_to_array(key: &str, value: &str) -> Result<bool> {
        Self::ensure_exists()?;
        let path = Self::path();
        let mut content = std::fs::read_to_string(&path)?;

        let mut doc: toml_edit::DocumentMut = content
            .parse()
            .map_err(|e| anyhow::anyhow!("cannot edit {}: {}", path.display(), e))?;

        if doc.get(key).is_none() {
            // Top-level keys must precede [[monitored_dirs]] sections,
            // so insert the new key at the very top of the file
            content = format!("{} = []\n{}", key, content);
            doc = content
                .parse()
                .map_err(|e| anyhow::anyhow!("cannot edit {}: {}", path.display(), e))?;
        }

        let arr = doc[key]
            .as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("config key '{}' is not an array", key))?;

        if arr.iter().any(|v| v.as_str() == Some(value)) {
            return Ok(false);
        }
        arr.push(value);

        std::fs::write(&path, doc.to_string())?;
        let (_, uid, gid) = crate::db::get_user_info();
        chown_to_user(&path, uid, gid);
        Ok(true)
    }
}

fn chown_to_user(path: &std::path::Path, uid: Option<u32>, gid: Option<u32>) {
    if let (Some(u), Some(g)) = (uid, gid) {
        if let Err(e) = std::os::unix::fs::chown(path, Some(u), Some(g)) {
            eprintln!("Warning: failed to chown {}: {}", path.display(), e);
        }
    }
}

pub fn default_config_content() -> String {
    let mut excluded_block = String::from("excluded_paths = [\n");
    for p in default_excluded_paths() {
        excluded_block.push_str(&format!("    \"{}\",\n", p));
    }
    excluded_block.push(']');

    let template = r#"# HDAS Configuration File

# NOTE: In TOML, top-level keys must appear BEFORE any [[array]] sections.
# Place all settings above the [[monitored_dirs]] entries.

ignored_processes = [
    "nvim", "vim", "vi", "nano", "emacs", "code", "subl", "hx", "kate", "gedit",
    "cat", "bat", "less", "more", "head", "tail",
    "ls", "find", "fd", "rg", "grep", "ag", "file", "stat", "wc", "du", "tree",
    "bash", "zsh", "fish",
]

# Packages to skip entirely (noisy apps like browsers)
ignored_packages = []

# Paths to exclude from monitoring even if under a monitored_dir.
# Defaults below are shared infrastructure dirs (no single owner) - edit freely.
# Other examples:
#     "/etc/ssl/",
#     "/etc/pacman.d/gnupg/",
__EXCLUDED_PATHS__

# Default depth for monitored dirs without explicit depth (1 = app dir like ~/.cache/mozilla)
# Note: ~/.local/share, ~/.local/state, and ~/.local/lib automatically add +1 depth
tracking_depth = 1

auto_prune = true

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
"#;
    template.replace("__EXCLUDED_PATHS__", &excluded_block)
}
