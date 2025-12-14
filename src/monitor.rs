use anyhow::Result;
use std::fs;
use std::mem::MaybeUninit;
use std::collections::HashMap;
use libbpf_rs::skel::{SkelBuilder, OpenSkel};
use libbpf_rs::OpenObject;

mod monitor_skel {
    include!(concat!(env!("OUT_DIR"), "/monitor.skel.rs"));
}

use monitor_skel::*;

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

fn query_pacman(binary_path: &str) -> Option<String> {
    let output = std::process::Command::new("pacman")
        .arg("-Qo")
        .arg(binary_path)
        .output()
        .ok()?;

    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        text.split_whitespace().nth(4).map(|s| s.to_string())
    } else {
        None
    }
}

#[derive(Clone)]
pub struct PackageInfo {
    pub package: String,
    pub process: String,
    pub via_parent: bool,
}

pub fn get_package_for_pid_tree(pid: u32, comm: &str) -> PackageInfo {
    let mut visited: HashMap<u32, Option<String>> = HashMap::new();
    let mut current_pid = pid;
    let mut depth = 0;
    const MAX_DEPTH: u32 = 10;

    if let Some(exe) = get_exe_path(pid) {
        if let Some(pkg) = query_pacman(&exe) {
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

        if let Some(cached) = visited.get(&ppid) {
            if let Some(pkg) = cached {
                let parent_comm = get_comm(ppid).unwrap_or_else(|| "unknown".to_string());
                return PackageInfo {
                    package: pkg.clone(),
                    process: parent_comm,
                    via_parent: true,
                };
            }
            current_pid = ppid;
            depth += 1;
            continue;
        }

        if let Some(exe) = get_exe_path(ppid) {
            if let Some(pkg) = query_pacman(&exe) {
                visited.insert(ppid, Some(pkg.clone()));
                let parent_comm = get_comm(ppid).unwrap_or_else(|| "unknown".to_string());
                return PackageInfo {
                    package: pkg,
                    process: parent_comm,
                    via_parent: true,
                };
            }
        }

        visited.insert(ppid, None);
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

fn get_tracked_path(full_path: &str, home: &std::path::Path, monitored_dirs: &[String], depth: u32) -> Option<String> {
    let home_str = home.to_string_lossy();
    let relative = full_path.strip_prefix(home_str.as_ref())?.trim_start_matches('/');

    for dir in monitored_dirs {
        let dir_name = dir.trim_start_matches('.');
        let prefix = format!(".{}/", dir_name);

        if relative.starts_with(&prefix) {
            let after_base = relative.strip_prefix(&prefix)?;
            let parts: Vec<&str> = after_base.split('/').collect();

            if depth == 0 {
                return Some(format!("{}/{}", home_str, relative.trim_start_matches('.')));
            }

            let tracked_parts: Vec<&str> = parts.iter().take(depth as usize).cloned().collect();
            if tracked_parts.is_empty() {
                return Some(format!("{}/.{}", home_str, dir_name));
            }
            return Some(format!("{}/.{}/{}", home_str, dir_name, tracked_parts.join("/")));
        }
    }
    None
}

pub fn run_monitor() -> Result<()> {
    let config = crate::config::Config::load()?;

    println!("HDAS Monitor starting...");
    println!("Monitored directories: {:?}", config.monitored_dirs);
    println!("Ignored processes: {} configured", config.ignored_processes.len());
    println!("Ignored packages: {} configured", config.ignored_packages.len());
    println!("Tracking depth: {}", config.tracking_depth);
    println!("Process tree walking: enabled");
    println!();

    let skel_builder = MonitorSkelBuilder::default();
    let mut open_object = MaybeUninit::<OpenObject>::uninit();
    let open_skel = skel_builder.open(&mut open_object)?;
    let skel = open_skel.load()?;

    let _link = skel
        .progs
        .trace_openat
        .attach_tracepoint("syscalls", "sys_enter_openat")?;

    let db = crate::db::Database::new()?;
    let home = crate::db::get_user_home();

    println!("Monitor running. Press Ctrl+C to stop.");
    println!();

    let monitored_dirs = config.monitored_dirs.clone();
    let tracking_depth = config.tracking_depth;
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

    let perf = libbpf_rs::PerfBufferBuilder::new(&skel.maps.events)
        .sample_cb(move |_cpu, data: &[u8]| {
            if data.len() < std::mem::size_of::<Event>() {
                return;
            }

            let event = unsafe { &*(data.as_ptr() as *const Event) };

            let comm = std::str::from_utf8(&event.comm)
                .unwrap_or("unknown")
                .trim_end_matches('\0');

            let filename = std::str::from_utf8(&event.filename)
                .unwrap_or("unknown")
                .trim_end_matches('\0');

            let is_monitored = monitored_dirs.iter().any(|dir| {
                let dir_name = dir.trim_start_matches('.');
                let pattern1 = format!("/.{}/", dir_name);
                let pattern2 = format!(".{}/", dir_name);
                filename.contains(&pattern1) || filename.starts_with(&pattern2)
                    || filename.ends_with(&format!("/.{}", dir_name))
            });

            if !is_monitored {
                return;
            }

            let full_path = if filename.starts_with('/') {
                std::path::PathBuf::from(filename)
            } else {
                let mut p = home.clone();
                p.push(filename);
                p
            };

            let full_path_str = full_path.to_string_lossy();
            let tracked_path = match get_tracked_path(&full_path_str, &home, &monitored_dirs, tracking_depth) {
                Some(p) => p,
                None => return,
            };

            let pkg_info = get_package_for_pid_tree(event.pid, comm);

            if ignored_packages.contains(&pkg_info.package) {
                return;
            }

            let is_ignored_proc = ignored_processes.contains(&pkg_info.process);

            if db.path_exists(&tracked_path) {
                return;
            }

            let indicator = if is_ignored_proc {
                "~"
            } else if pkg_info.via_parent {
                "^"
            } else {
                "+"
            };

            if let Err(e) = db.record_access(
                &tracked_path,
                &pkg_info.package,
                &pkg_info.process,
                is_ignored_proc
            ) {
                eprintln!("DB error: {}", e);
            }

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
        .build()?;

    loop {
        perf.poll(std::time::Duration::from_millis(100))?;
    }
}

#[repr(C)]
struct Event {
    pid: u32,
    comm: [u8; 16],
    filename: [u8; 256],
}
