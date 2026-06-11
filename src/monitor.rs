use anyhow::Result;
use std::fs;
use std::mem::MaybeUninit;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use libbpf_rs::skel::{SkelBuilder, OpenSkel};
use libbpf_rs::{MapCore, MapFlags, OpenObject};

mod monitor_skel {
    include!(concat!(env!("OUT_DIR"), "/monitor.skel.rs"));
}

use monitor_skel::*;

const MAX_PATTERNS: usize = 16;
const PATTERN_LEN: usize = 64;
const AT_FDCWD: i32 = -100;
// Negative lookups are retried after this long, so a package installed while
// the monitor is running stops resolving as "unknown" without a restart.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(60);

type PackageCache = RefCell<HashMap<String, (Option<String>, Instant)>>;

fn get_ppid(pid: u32) -> Option<u32> {
    let stat_path = format!("/proc/{}/stat", pid);
    let content = fs::read_to_string(&stat_path).ok()?;
    let last_paren = content.rfind(')')?;
    let after_comm = &content[last_paren + 2..];
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    fields.get(1)?.parse().ok()
}

fn get_exe_path(pid: u32) -> Option<String> {
    let exe_path = format!("/proc/{}/exe", pid);
    fs::read_link(&exe_path).ok().map(|p| {
        let path_str = p.to_string_lossy();
        path_str.trim_end_matches(" (deleted)").to_string()
    })
}

fn query_owner_cached(path: &str, pm: &crate::pkgmgr::PkgMgr, cache: &PackageCache) -> Option<String> {
    if let Some((result, at)) = cache.borrow().get(path) {
        if result.is_some() || at.elapsed() < NEGATIVE_CACHE_TTL {
            return result.clone();
        }
    }

    let result = pm.query_owner(path);

    cache.borrow_mut().insert(path.to_string(), (result.clone(), Instant::now()));
    result
}


#[derive(Clone)]
pub struct PackageInfo {
    pub package: String,
    pub process: String,
    pub via_parent: bool,
}

fn get_package_for_pid_tree(pid: u32, comm: &str, pm: &crate::pkgmgr::PkgMgr, cache: &PackageCache) -> PackageInfo {
    let mut current_pid = pid;
    let mut depth = 0;
    const MAX_DEPTH: u32 = 10;

    if let Some(exe) = get_exe_path(pid) {
        if let Some(pkg) = query_owner_cached(&exe, pm, cache) {
            return PackageInfo {
                package: pkg,
                process: comm.to_string(),
                via_parent: false,
            };
        }
    }

    while depth < MAX_DEPTH {
        let ppid = match get_ppid(current_pid) {
            Some(p) if p > 1 => p,
            _ => break,
        };

        if let Some(exe) = get_exe_path(ppid) {
            if let Some(pkg) = query_owner_cached(&exe, pm, cache) {
                let parent_comm = get_comm(ppid).unwrap_or_else(|| "unknown".to_string());
                return PackageInfo {
                    package: pkg,
                    process: parent_comm,
                    via_parent: true,
                };
            }
        }

        current_pid = ppid;
        depth += 1;
    }

    PackageInfo {
        package: "unknown".to_string(),
        process: comm.to_string(),
        via_parent: false,
    }
}

fn get_comm(pid: u32) -> Option<String> {
    let comm_path = format!("/proc/{}/comm", pid);
    fs::read_to_string(&comm_path)
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn get_tracked_path(
    full_path: &str,
    home: &std::path::Path,
    monitored_dirs: &[crate::config::MonitoredDir],
    default_depth: u32,
) -> Option<String> {
    let full_path = full_path.trim_end_matches('/');

    for dir in monitored_dirs {
        if !dir.path.starts_with('/') {
            continue;
        }
        let base = dir.path.trim_end_matches('/');
        let is_under = full_path.len() > base.len()
            && full_path.starts_with(base)
            && full_path[base.len()..].starts_with('/');
        if !is_under {
            continue;
        }
        let depth = dir.depth.unwrap_or(default_depth);
        if depth == 0 {
            return Some(full_path.to_string());
        }
        let after_base = &full_path[base.len()..];
        let tracked_parts: Vec<&str> = after_base
            .split('/')
            .filter(|s| !s.is_empty())
            .take(depth as usize)
            .collect();
        if tracked_parts.is_empty() {
            // The monitored root itself — never attribute (or delete) it
            return None;
        }
        return Some(format!("{}/{}", base, tracked_parts.join("/")));
    }

    let home_str = home.to_string_lossy();
    let relative = std::path::Path::new(full_path)
        .strip_prefix(home)
        .ok()?
        .to_string_lossy()
        .into_owned();

    for dir in monitored_dirs {
        if dir.path.starts_with('/') {
            continue;
        }
        let base = dir.path.trim_matches('/');
        let prefix = format!("{}/", base);
        if !relative.starts_with(&prefix) {
            continue;
        }
        let depth = dir.depth.unwrap_or(default_depth);
        if depth == 0 {
            return Some(full_path.to_string());
        }
        let after_base = &relative[prefix.len()..];
        let parts: Vec<&str> = after_base.split('/').filter(|s| !s.is_empty()).collect();

        let effective_depth = if base == ".local"
            && parts.first().is_some_and(|s| matches!(*s, "share" | "state" | "lib"))
        {
            depth + 1
        } else {
            depth
        };

        let tracked_parts: Vec<&str> = parts.into_iter().take(effective_depth as usize).collect();
        if tracked_parts.is_empty() {
            // The monitored root itself — never attribute (or delete) it
            return None;
        }
        return Some(format!("{}/{}/{}", home_str, base, tracked_parts.join("/")));
    }
    None
}

/// Build the kernel-side prefix filters from the configured monitored dirs.
/// Home-relative dirs get two patterns: the absolute form (for absolute
/// opens) and the bare relative form (for cwd-relative opens). The match is
/// only a coarse prefix filter — precise filtering happens in userspace.
fn build_kernel_patterns(home: &std::path::Path, monitored_dirs: &[crate::config::MonitoredDir]) -> Vec<String> {
    let home_str = home.to_string_lossy();
    let home_str = home_str.trim_end_matches('/');
    let mut patterns = Vec::new();

    for dir in monitored_dirs {
        if dir.path.starts_with('/') {
            patterns.push(dir.path.trim_end_matches('/').to_string());
        } else {
            let base = dir.path.trim_matches('/');
            patterns.push(format!("{}/{}", home_str, base));
            patterns.push(base.to_string());
        }
    }

    patterns.sort();
    patterns.dedup();
    patterns
}

pub fn run_monitor() -> Result<()> {
    let config = crate::config::Config::load()?;

    let pm = crate::pkgmgr::PkgMgr::detect()
        .ok_or_else(|| anyhow::anyhow!("No supported package manager found (need pacman, dpkg, rpm, xbps, or apk)"))?;

    println!("HDAS Monitor starting...");
    println!("Package manager: {}", pm.name());
    print!("Monitored directories: ");
    for (i, dir) in config.monitored_dirs.iter().enumerate() {
        if i > 0 {
            print!(", ");
        }
        match dir.depth {
            Some(d) => print!("{}(depth={})", dir.path, d),
            None => print!("{}", dir.path),
        }
    }
    println!();
    println!("Ignored processes: {} configured", config.ignored_processes.len());
    println!("Ignored packages: {} configured", config.ignored_packages.len());
    println!("Default tracking depth: {}", config.tracking_depth);
    println!("Process tree walking: enabled");
    println!();

    let skel_builder = MonitorSkelBuilder::default();
    let mut open_object = MaybeUninit::<OpenObject>::uninit();
    let open_skel = skel_builder.open(&mut open_object)?;
    let skel = open_skel.load()?;

    let db = crate::db::Database::new()?;
    let home = crate::db::get_user_home();

    // Populate the kernel prefix filter before attaching, so no events
    // arrive while the map is empty.
    let patterns = build_kernel_patterns(&home, &config.monitored_dirs);
    if patterns.len() > MAX_PATTERNS {
        eprintln!(
            "Warning: more than {} kernel filter patterns; these monitored dirs will not be captured: {:?}",
            MAX_PATTERNS,
            &patterns[MAX_PATTERNS..]
        );
    }
    for (i, pattern) in patterns.iter().take(MAX_PATTERNS).enumerate() {
        let mut value = [0u8; PATTERN_LEN];
        let bytes = pattern.as_bytes();
        // Truncation keeps a valid (just less selective) prefix filter
        let n = bytes.len().min(PATTERN_LEN - 1);
        value[..n].copy_from_slice(&bytes[..n]);
        skel.maps.patterns.update(&(i as u32).to_ne_bytes(), &value, MapFlags::ANY)?;
    }

    let _link_openat = skel
        .progs
        .trace_openat
        .attach_tracepoint("syscalls", "sys_enter_openat")?;
    let _link_openat2 = skel
        .progs
        .trace_openat2
        .attach_tracepoint("syscalls", "sys_enter_openat2")?;

    println!("Monitor running. Press Ctrl+C to stop.");
    println!();

    let monitored_dirs = config.monitored_dirs.clone();
    let tracking_depth = config.tracking_depth;
    let excluded_paths = config.excluded_paths.clone();
    let ignored_processes: std::collections::HashSet<String> = config
        .ignored_processes
        .iter()
        .cloned()
        .collect();
    let ignored_packages: std::collections::HashSet<String> = config
        .ignored_packages
        .iter()
        .cloned()
        .collect();

    let package_cache: PackageCache = RefCell::new(HashMap::new());
    let monitor_pid = std::process::id();

    let perf = libbpf_rs::PerfBufferBuilder::new(&skel.maps.events)
        .sample_cb(move |_cpu, data: &[u8]| {
            if data.len() < std::mem::size_of::<Event>() || data.as_ptr().align_offset(std::mem::align_of::<Event>()) != 0 {
                return;
            }

            let event = unsafe { &*(data.as_ptr() as *const Event) };

            if event.pid == monitor_pid {
                return;
            }

            let comm_len = event.comm.iter().position(|&b| b == 0).unwrap_or(event.comm.len());
            let comm = std::str::from_utf8(&event.comm[..comm_len]).unwrap_or("unknown");

            let name_len = event.filename.iter().position(|&b| b == 0).unwrap_or(event.filename.len());
            let filename = match std::str::from_utf8(&event.filename[..name_len]) {
                Ok(f) if !f.is_empty() => f,
                _ => return,
            };

            let full_path = if filename.starts_with('/') {
                std::path::PathBuf::from(filename)
            } else {
                // Resolve relative opens against the process's actual cwd;
                // dirfd-relative opens can't be resolved, so drop them.
                if event.dfd != AT_FDCWD {
                    return;
                }
                match fs::read_link(format!("/proc/{}/cwd", event.pid)) {
                    Ok(cwd) => cwd.join(filename),
                    Err(_) => return,
                }
            };

            let full_path_str = full_path.to_string_lossy();

            if excluded_paths.iter().any(|ex| {
                let base = ex.trim_end_matches('/');
                full_path_str.starts_with(base)
                    && (full_path_str.len() == base.len() || full_path_str[base.len()..].starts_with('/'))
            }) {
                return;
            }

            let tracked_path = match get_tracked_path(&full_path_str, &home, &monitored_dirs, tracking_depth) {
                Some(p) => p,
                None => return,
            };

            // Check DB early — if we already have a known creator, skip entirely.
            // This avoids expensive package manager queries for files we've already seen.
            let path_exists = db.path_exists(&tracked_path);
            if path_exists && db.path_has_known_creator(&tracked_path) {
                return;
            }

            // Also skip early if this is an ignored process and the path is already tracked
            // (even with unknown creator — ignored procs only update last_accessed)
            if path_exists && ignored_processes.contains(comm) {
                return;
            }

            // Skip events from our own descendants (package manager queries we
            // spawn open files under /etc). Checked only after path filtering
            // since it costs up to 5 /proc reads.
            let mut ancestor = event.pid;
            for _ in 0..5 {
                match get_ppid(ancestor) {
                    Some(p) if p > 1 => {
                        if p == monitor_pid {
                            return;
                        }
                        ancestor = p;
                    }
                    _ => break,
                }
            }

            // Only now do the expensive package resolution
            let mut pkg_info = get_package_for_pid_tree(event.pid, comm, &pm, &package_cache);

            if pm.is_self_package(&pkg_info.package) || pkg_info.package == "unknown" {
                if let Some(owner) = query_owner_cached(&full_path_str, &pm, &package_cache) {
                    pkg_info.package = owner;
                }
            }

            if ignored_packages.contains(&pkg_info.package) {
                return;
            }

            let is_ignored_proc = ignored_processes.contains(&pkg_info.process);

            // For parent-resolved ignored processes on existing paths, skip
            if path_exists && is_ignored_proc {
                return;
            }

            if let Err(e) = db.record_access(
                &tracked_path,
                &pkg_info.package,
                &pkg_info.process,
                is_ignored_proc
            ) {
                eprintln!("DB error: {}", e);
            }

            let indicator = if is_ignored_proc {
                "~"
            } else if pkg_info.via_parent {
                "^"
            } else {
                "+"
            };

            let via = if pkg_info.via_parent {
                format!(" via {}", pkg_info.process)
            } else {
                String::new()
            };

            println!("[{}] {} ({}){} -> {}",
                indicator,
                pkg_info.package,
                comm,
                via,
                tracked_path
            );
        })
        .lost_cb(|cpu, count| {
            eprintln!("Warning: lost {} event(s) on CPU {}", count, cpu);
        })
        .build()?;

    loop {
        match perf.poll(std::time::Duration::from_millis(100)) {
            Ok(()) => {}
            Err(e) if e.kind() == libbpf_rs::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
}

#[repr(C)]
struct Event {
    pid: u32,
    dfd: i32,
    comm: [u8; 16],
    filename: [u8; 256],
}

#[cfg(test)]
mod tests {
    use super::get_tracked_path;
    use crate::config::MonitoredDir;
    use std::path::Path;

    fn dirs(spec: &[(&str, Option<u32>)]) -> Vec<MonitoredDir> {
        spec.iter()
            .map(|(p, d)| MonitoredDir { path: p.to_string(), depth: *d })
            .collect()
    }

    #[test]
    fn depth_truncates_to_app_dir() {
        let home = Path::new("/home/u");
        let d = dirs(&[(".cache", None)]);
        assert_eq!(
            get_tracked_path("/home/u/.cache/mozilla/firefox/x", home, &d, 1),
            Some("/home/u/.cache/mozilla".to_string())
        );
    }

    #[test]
    fn depth_zero_keeps_leading_dot() {
        let home = Path::new("/home/u");
        let d = dirs(&[(".cache", Some(0))]);
        assert_eq!(
            get_tracked_path("/home/u/.cache/mozilla/firefox/x", home, &d, 1),
            Some("/home/u/.cache/mozilla/firefox/x".to_string())
        );
    }

    #[test]
    fn monitored_root_itself_is_not_tracked() {
        let home = Path::new("/home/u");
        let d = dirs(&[(".cache", None)]);
        assert_eq!(get_tracked_path("/home/u/.cache/", home, &d, 1), None);
        assert_eq!(get_tracked_path("/home/u/.cache", home, &d, 1), None);
    }

    #[test]
    fn local_share_gets_extra_depth() {
        let home = Path::new("/home/u");
        let d = dirs(&[(".local", None)]);
        assert_eq!(
            get_tracked_path("/home/u/.local/share/app/data/f", home, &d, 1),
            Some("/home/u/.local/share/app".to_string())
        );
        assert_eq!(
            get_tracked_path("/home/u/.local/bin/tool", home, &d, 1),
            Some("/home/u/.local/bin".to_string())
        );
    }

    #[test]
    fn absolute_dir_tracking() {
        let home = Path::new("/home/u");
        let d = dirs(&[("/etc/", Some(0))]);
        assert_eq!(
            get_tracked_path("/etc/pacman.conf", home, &d, 1),
            Some("/etc/pacman.conf".to_string())
        );
        // The root itself and lookalike prefixes don't match
        assert_eq!(get_tracked_path("/etc/", home, &d, 1), None);
        assert_eq!(get_tracked_path("/etcetera/x", home, &d, 1), None);
    }

    #[test]
    fn absolute_dir_depth_truncation() {
        let home = Path::new("/home/u");
        let d = dirs(&[("/etc/", Some(1))]);
        assert_eq!(
            get_tracked_path("/etc/ssl/certs/ca.pem", home, &d, 1),
            Some("/etc/ssl".to_string())
        );
    }

    #[test]
    fn non_dot_home_dir() {
        let home = Path::new("/home/u");
        let d = dirs(&[("Downloads", None)]);
        assert_eq!(
            get_tracked_path("/home/u/Downloads/app/file", home, &d, 1),
            Some("/home/u/Downloads/app".to_string())
        );
    }

    #[test]
    fn other_users_home_does_not_match() {
        let home = Path::new("/home/u");
        let d = dirs(&[(".cache", None)]);
        assert_eq!(get_tracked_path("/home/u2/.cache/foo/x", home, &d, 1), None);
    }
}
