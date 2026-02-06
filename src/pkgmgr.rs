use std::collections::HashSet;
use std::process::Command;

/// Detected system package manager
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkgMgr {
    Pacman,
    Dpkg,
    Rpm,
    Xbps,
    Apk,
}

impl PkgMgr {
    /// Auto-detect the system package manager by checking which binaries exist.
    pub fn detect() -> Option<Self> {
        // Order matters: check more specific ones first
        if which("pacman") { return Some(Self::Pacman); }
        if which("dpkg")   { return Some(Self::Dpkg); }
        if which("rpm")    { return Some(Self::Rpm); }
        if which("xbps-query") { return Some(Self::Xbps); }
        if which("apk")    { return Some(Self::Apk); }
        None
    }

    /// The name of this package manager's own binary (used for the
    /// "is the package manager itself doing the writing?" heuristic).
    pub fn manager_package_names(&self) -> &[&str] {
        match self {
            Self::Pacman => &["pacman"],
            Self::Dpkg   => &["dpkg", "apt"],
            Self::Rpm    => &["rpm", "dnf", "yum"],
            Self::Xbps   => &["xbps-install"],
            Self::Apk    => &["apk"],
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Pacman => "pacman",
            Self::Dpkg   => "dpkg",
            Self::Rpm    => "rpm",
            Self::Xbps   => "xbps",
            Self::Apk    => "apk",
        }
    }

    /// List every installed package name.
    pub fn list_installed(&self) -> Result<HashSet<String>, std::io::Error> {
        let output = match self {
            Self::Pacman => Command::new("pacman").args(["-Qq"]).output()?,
            Self::Dpkg   => Command::new("dpkg-query")
                .args(["-W", "-f", "${Package}\\n"])
                .output()?,
            Self::Rpm => Command::new("rpm")
                .args(["-qa", "--qf", "%{NAME}\\n"])
                .output()?,
            Self::Xbps => Command::new("xbps-query").arg("-l").output()?,
            Self::Apk => Command::new("apk")
                .args(["list", "--installed", "-q"])
                .output()?,
        };

        let text = String::from_utf8_lossy(&output.stdout);
        let set = match self {
            // xbps-query -l outputs "ii <pkg>-<ver>  <desc>" — we need column 2 minus the version
            Self::Xbps => text.lines().filter_map(|line| {
                let pkg_ver = line.split_whitespace().nth(1)?;
                // package name is everything before the last hyphen-version segment
                let last_dash = pkg_ver.rfind('-')?;
                Some(pkg_ver[..last_dash].to_string())
            }).collect(),
            // apk list -q outputs "pkg-ver" — strip trailing -ver
            Self::Apk => text.lines().filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() { return None; }
                // Alpine packages: name-version. Version starts after last hyphen
                // that is followed by a digit.
                let mut split_at = None;
                for (i, _) in trimmed.match_indices('-') {
                    if trimmed[i+1..].starts_with(|c: char| c.is_ascii_digit()) {
                        split_at = Some(i);
                        break;
                    }
                }
                match split_at {
                    Some(i) => Some(trimmed[..i].to_string()),
                    None => Some(trimmed.to_string()),
                }
            }).collect(),
            // pacman, dpkg, rpm give clean package-per-line
            _ => text.lines()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        };

        Ok(set)
    }

    /// Query which package owns a given filesystem path.
    pub fn query_owner(&self, path: &str) -> Option<String> {
        match self {
            Self::Pacman => {
                let output = Command::new("pacman")
                    .args(["-Qo", path])
                    .output().ok()?;
                if !output.status.success() { return None; }
                let text = String::from_utf8_lossy(&output.stdout);
                // "/<path> is owned by <package> <version>"
                text.split_whitespace().nth(4).map(|s| s.to_string())
            }
            Self::Dpkg => {
                let output = Command::new("dpkg")
                    .args(["-S", path])
                    .output().ok()?;
                if !output.status.success() { return None; }
                let text = String::from_utf8_lossy(&output.stdout);
                // "package: /path" or "package:arch: /path"
                let first_line = text.lines().next()?;
                let pkg = first_line.split(':').next()?.trim();
                if pkg.is_empty() { None } else { Some(pkg.to_string()) }
            }
            Self::Rpm => {
                let output = Command::new("rpm")
                    .args(["-qf", "--qf", "%{NAME}", path])
                    .output().ok()?;
                if !output.status.success() { return None; }
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if text.is_empty() || text.contains("not owned") { None } else { Some(text) }
            }
            Self::Xbps => {
                let output = Command::new("xbps-query")
                    .args(["-o", path])
                    .output().ok()?;
                if !output.status.success() { return None; }
                let text = String::from_utf8_lossy(&output.stdout);
                // "<pkg>-<ver>: /path"
                let first_line = text.lines().next()?;
                let pkg_ver = first_line.split(':').next()?.trim();
                let last_dash = pkg_ver.rfind('-')?;
                let name = &pkg_ver[..last_dash];
                if name.is_empty() { None } else { Some(name.to_string()) }
            }
            Self::Apk => {
                let output = Command::new("apk")
                    .args(["info", "--who-owns", path])
                    .output().ok()?;
                if !output.status.success() { return None; }
                let text = String::from_utf8_lossy(&output.stdout);
                // "<path> is owned by <package>-<version>"
                let owned_by = text.find("is owned by ")?;
                let after = &text[owned_by + 12..];
                let pkg_ver = after.trim();
                // Strip trailing -version (first hyphen followed by digit)
                let mut split_at = None;
                for (i, _) in pkg_ver.match_indices('-') {
                    if pkg_ver[i+1..].starts_with(|c: char| c.is_ascii_digit()) {
                        split_at = Some(i);
                        break;
                    }
                }
                match split_at {
                    Some(i) => Some(pkg_ver[..i].to_string()),
                    None => Some(pkg_ver.to_string()),
                }
            }
        }
    }

    /// Returns true if the given package name is the package manager itself.
    pub fn is_self_package(&self, pkg: &str) -> bool {
        self.manager_package_names().iter().any(|&n| n == pkg)
    }
}

fn which(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
