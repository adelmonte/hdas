use anyhow::Result;
use chrono::{DateTime, Datelike, Local, Utc};
use owo_colors::OwoColorize;
use serde::Serialize;
use std::io::IsTerminal;
use std::path::Path;

use crate::config::Config;
use crate::db::FileRecord;

fn use_color() -> bool {
    std::io::stdout().is_terminal()
}

fn format_time(timestamp: i64) -> String {
    let Some(dt) = DateTime::<Utc>::from_timestamp(timestamp, 0) else {
        return "            ".to_string();
    };

    let local: DateTime<Local> = dt.into();
    let now = Local::now();

    if local.year() == now.year() {
        local.format("%b %d %H:%M").to_string()
    } else {
        local.format("%b %d  %Y").to_string()
    }
}

pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

fn display_record(record: &FileRecord, show_accessor: bool) {
    let color = use_color();
    let exists_char = if Path::new(&record.path).exists() { "✓" } else { "✗" };
    let time = format_time(record.created_at);

    if color {
        let exists_colored = if Path::new(&record.path).exists() {
            format!("{}", exists_char.green())
        } else {
            format!("{}", exists_char.red())
        };
        println!(
            "{} [{}] {} ({})",
            time.dimmed(),
            exists_colored,
            record.path,
            record.created_by_package.cyan()
        );
    } else {
        println!("{} [{}] {} ({})", time, exists_char, record.path, record.created_by_package);
    }

    if show_accessor
        && (record.last_accessed_by_package != record.created_by_package
            || record.last_accessed_by_process != record.created_by_process)
    {
        let access_time = format_time(record.last_accessed_at);
        if color {
            println!(
                "{}",
                format!(
                    "{}      └─ last accessed by {} ({})",
                    access_time,
                    record.last_accessed_by_package,
                    record.last_accessed_by_process
                )
                .dimmed()
            );
        } else {
            println!(
                "{}      └─ last accessed by {} ({})",
                access_time,
                record.last_accessed_by_package,
                record.last_accessed_by_process
            );
        }
    }
}

fn maybe_prune(db: &crate::db::Database, json: bool) -> Result<usize> {
    let config = Config::load()?;
    if config.auto_prune {
        let pruned = db.prune_deleted()?;
        if pruned > 0 && !json {
            eprintln!("(auto-pruned {} deleted file(s))", pruned);
        }
        Ok(pruned)
    } else {
        Ok(0)
    }
}

pub fn query_file(pattern: &str, json: bool) -> Result<()> {
    let db = crate::db::Database::new()?;
    maybe_prune(&db, json)?;
    let records = db.query_file(pattern)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }

    if records.is_empty() {
        println!("No records found for: {}", pattern);
        return Ok(());
    }

    println!("Found {} file(s) matching '{}':\n", records.len(), pattern);
    for record in records {
        display_record(&record, true);
    }

    Ok(())
}

pub fn query_package(package: &str, json: bool) -> Result<()> {
    let db = crate::db::Database::new()?;
    maybe_prune(&db, json)?;
    let records = db.query_package(package)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }

    if records.is_empty() {
        println!("No files found for package: {}", package);
        return Ok(());
    }

    println!("Files created by {} ({} total):\n", package, records.len());
    for record in records {
        let exists = if Path::new(&record.path).exists() { "✓" } else { "✗" };
        let time = format_time(record.created_at);

        if use_color() {
            let exists_colored = if Path::new(&record.path).exists() {
                format!("{}", exists.green())
            } else {
                format!("{}", exists.red())
            };
            println!("{} [{}] {}", time.dimmed(), exists_colored, record.path);
        } else {
            println!("{} [{}] {}", time, exists, record.path);
        }
    }

    Ok(())
}

pub fn query_directory(dir: &str, json: bool) -> Result<()> {
    let db = crate::db::Database::new()?;
    maybe_prune(&db, json)?;

    let home = crate::db::get_user_home();
    let expanded = if dir.starts_with('/') {
        dir.to_string()
    } else if let Some(rest) = dir.strip_prefix("~/") {
        home.join(rest).to_string_lossy().into_owned()
    } else if dir == "~" {
        home.to_string_lossy().into_owned()
    } else {
        home.join(dir).to_string_lossy().into_owned()
    };

    let records = db.query_directory(&expanded)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }

    if records.is_empty() {
        println!("No files found under: {}", dir);
        return Ok(());
    }

    println!("Files under {} ({} total):\n", dir, records.len());
    for record in records {
        display_record(&record, true);
    }

    Ok(())
}

#[derive(Serialize)]
struct OrphanPackage {
    package: String,
    files: Vec<OrphanFile>,
    total: usize,
    existing: usize,
    deleted: usize,
}

#[derive(Serialize)]
struct OrphanFile {
    path: String,
    exists: bool,
}

pub fn show_orphans(json: bool) -> Result<()> {
    let db = crate::db::Database::new()?;
    let orphans = db.get_orphans()?;

    if orphans.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("No orphaned files found!");
        }
        return Ok(());
    }

    if json {
        let mut output = Vec::new();
        for pkg in orphans {
            let records = db.query_package(&pkg)?;
            if !records.is_empty() {
                let files: Vec<OrphanFile> = records.iter().map(|r| {
                    let exists = Path::new(&r.path).exists();
                    OrphanFile { path: r.path.clone(), exists }
                }).collect();
                let existing = files.iter().filter(|f| f.exists).count();
                let deleted = files.len() - existing;
                output.push(OrphanPackage {
                    total: files.len(),
                    existing,
                    deleted,
                    package: pkg,
                    files,
                });
            }
        }
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let color = use_color();
    println!("Files from uninstalled packages:\n");
    for pkg in orphans {
        let records = db.query_package(&pkg)?;
        if !records.is_empty() {
            let existing_count = records.iter().filter(|r| Path::new(&r.path).exists()).count();
            let deleted_count = records.len() - existing_count;

            if color {
                print!("{} ({} file(s)", pkg.yellow(), records.len());
            } else {
                print!("{} ({} file(s)", pkg, records.len());
            }
            if deleted_count > 0 {
                print!(", {} already deleted", deleted_count);
            }
            println!("):");

            for record in records {
                let exists = Path::new(&record.path).exists();
                if exists {
                    println!("  {}", record.path);
                } else {
                    println!("  {} (deleted)", record.path);
                }
            }
            println!();
        }
    }

    Ok(())
}

pub fn list_all(json: bool) -> Result<()> {
    let db = crate::db::Database::new()?;
    maybe_prune(&db, json)?;
    let records = db.list_all()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }

    if records.is_empty() {
        println!("No files cataloged yet. Run 'sudo hdas monitor' to start tracking.");
        return Ok(());
    }

    println!("Cataloged files ({} total):\n", records.len());
    for record in records {
        display_record(&record, true);
    }

    Ok(())
}

#[derive(Serialize)]
struct StatsOutput {
    database_path: String,
    files_tracked: usize,
    packages_seen: usize,
    config_path: String,
    monitored_dirs: Vec<String>,
    ignored_processes_count: usize,
    auto_prune: bool,
}

pub fn show_stats(json: bool) -> Result<()> {
    let db = crate::db::Database::new()?;
    let (files, packages, db_path) = db.get_stats()?;
    let config = Config::load()?;

    if json {
        let dirs: Vec<String> = config.monitored_dirs.iter().map(|d| {
            match d.depth {
                Some(depth) => format!("{}(depth={})", d.path, depth),
                None => d.path.clone(),
            }
        }).collect();
        let output = StatsOutput {
            database_path: db_path,
            files_tracked: files,
            packages_seen: packages,
            config_path: Config::path().to_string_lossy().into_owned(),
            monitored_dirs: dirs,
            ignored_processes_count: config.ignored_processes.len(),
            auto_prune: config.auto_prune,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let color = use_color();

    if color {
        println!("{}", "HDAS Database Statistics".bold());
        println!("{}", "========================".bold());
    } else {
        println!("HDAS Database Statistics");
        println!("========================");
    }
    println!("Database: {}", db_path);
    println!("Files tracked: {}", files);
    println!("Packages seen: {}", packages);
    println!();
    if color {
        println!("{}", "Configuration".bold());
        println!("{}", "-------------".bold());
    } else {
        println!("Configuration");
        println!("-------------");
    }
    println!("Config file: {}", Config::path().display());
    print!("Monitored dirs: ");
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
    println!("Ignored processes: {}", config.ignored_processes.len());
    println!("Auto-prune: {}", config.auto_prune);

    Ok(())
}

pub fn show_config() -> Result<()> {
    let path = Config::path();

    println!("Configuration file: {}", path.display());
    println!();

    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        println!("{}", content);
    } else {
        println!("(using defaults - no config file exists)");
        println!();
        println!("{}", crate::config::default_config_content());
    }

    Ok(())
}

pub fn edit_config() -> Result<()> {
    Config::ensure_exists()?;

    let path = Config::path();
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    println!("Opening {} with {}...", path.display(), editor);

    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()?;

    if status.success() {
        match Config::load() {
            Ok(_) => println!("Configuration updated successfully."),
            Err(e) => eprintln!("Warning: Config file has errors: {}", e),
        }
    }

    Ok(())
}

pub fn init_config() -> Result<()> {
    let path = Config::path();

    if path.exists() {
        println!("Config file already exists: {}", path.display());
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(&path, crate::config::default_config_content())?;
    println!("Created default config at: {}", path.display());

    Ok(())
}

#[derive(Serialize)]
struct ValidationOutput {
    valid: bool,
    errors: Vec<String>,
    warnings: Vec<String>,
}

pub fn validate_config(json: bool) -> Result<()> {
    let config = Config::load()?;
    let home = crate::db::get_user_home();

    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Check monitored dirs exist
    for dir in &config.monitored_dirs {
        let full_path = if dir.path.starts_with('/') {
            std::path::PathBuf::from(&dir.path)
        } else {
            home.join(&dir.path)
        };
        if !full_path.exists() {
            warnings.push(format!("Monitored directory does not exist: {}", full_path.display()));
        }
    }

    // Check duplicate monitored dirs
    let mut seen_dirs = std::collections::HashSet::new();
    for dir in &config.monitored_dirs {
        if !seen_dirs.insert(&dir.path) {
            errors.push(format!("Duplicate monitored directory: {}", dir.path));
        }
    }

    // Check duplicate ignored processes
    let mut seen_procs = std::collections::HashSet::new();
    for proc in &config.ignored_processes {
        if !seen_procs.insert(proc) {
            warnings.push(format!("Duplicate ignored process: {}", proc));
        }
    }

    // Check ignored process names look like binary names
    for proc in &config.ignored_processes {
        if proc.contains('/') || proc.contains(' ') {
            warnings.push(format!(
                "Ignored process '{}' looks like a path or contains spaces — should be a binary name",
                proc
            ));
        }
    }

    // Check tracking depth
    if config.tracking_depth > 5 {
        warnings.push(format!(
            "Global tracking_depth={} is unusually high (most users want 1-3)",
            config.tracking_depth
        ));
    }

    // Check per-dir depths
    for dir in &config.monitored_dirs {
        if let Some(depth) = dir.depth {
            if depth > 5 {
                warnings.push(format!(
                    "Per-directory depth={} for '{}' is unusually high",
                    depth, dir.path
                ));
            }
        }
    }

    let valid = errors.is_empty();

    if json {
        let output = ValidationOutput { valid, errors, warnings };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let color = use_color();

    if errors.is_empty() && warnings.is_empty() {
        if color {
            println!("{}", "Configuration is valid.".green());
        } else {
            println!("Configuration is valid.");
        }
        return Ok(());
    }

    for err in &errors {
        if color {
            println!("{} {}", "error:".red().bold(), err);
        } else {
            println!("error: {}", err);
        }
    }

    for warn in &warnings {
        if color {
            println!("{} {}", "warning:".yellow().bold(), warn);
        } else {
            println!("warning: {}", warn);
        }
    }

    if valid {
        println!("\nConfiguration is valid (with warnings).");
    } else {
        println!("\nConfiguration has errors.");
    }

    Ok(())
}

#[derive(Serialize)]
struct StatusOutput {
    service_active: Option<bool>,
    service_status: String,
    database_path: String,
    database_size: String,
    database_size_bytes: u64,
    files_tracked: usize,
    packages_seen: usize,
    last_event: Option<String>,
    last_event_timestamp: Option<i64>,
    config_path: String,
    config_exists: bool,
}

pub fn show_status(json: bool) -> Result<()> {
    let db = crate::db::Database::new()?;
    let (files, packages, db_path_str) = db.get_stats()?;
    let last_event = db.get_last_event_time()?;
    let config_path = Config::path();
    let config_exists = config_path.exists();

    // Get DB file size
    let db_path = std::path::Path::new(&db_path_str);
    let db_size_bytes = db_path.metadata().map(|m| m.len()).unwrap_or(0);
    let db_size = format_size(db_size_bytes);

    // Check service status
    let username = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
    let service_name = format!("hdas@{}.service", username);
    let service_output = std::process::Command::new("systemctl")
        .args(["is-active", &service_name])
        .output();

    let (service_active, service_status) = match service_output {
        Ok(output) => {
            let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let active = status == "active";
            (Some(active), status)
        }
        Err(_) => (None, "unknown".to_string()),
    };

    let last_event_str = last_event.map(|ts| format_time(ts));

    if json {
        let output = StatusOutput {
            service_active,
            service_status,
            database_path: db_path_str,
            database_size: db_size,
            database_size_bytes: db_size_bytes,
            files_tracked: files,
            packages_seen: packages,
            last_event: last_event_str,
            last_event_timestamp: last_event,
            config_path: config_path.to_string_lossy().into_owned(),
            config_exists,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let color = use_color();

    if color {
        println!("{}", "HDAS Status".bold());
        println!("{}", "===========".bold());
    } else {
        println!("HDAS Status");
        println!("===========");
    }

    // Service
    print!("Service: {} ", service_name);
    if color {
        match service_active {
            Some(true) => println!("({})", "active".green()),
            Some(false) => println!("({})", service_status.red()),
            None => println!("({})", "unknown".dimmed()),
        }
    } else {
        println!("({})", service_status);
    }

    // Database
    println!("Database: {} ({})", db_path_str, db_size);
    println!("Files tracked: {}", files);
    println!("Packages seen: {}", packages);
    match &last_event_str {
        Some(t) => println!("Last event: {}", t),
        None => println!("Last event: (none)"),
    }

    // Config
    println!();
    print!("Config: {} ", config_path.display());
    if config_exists {
        if color {
            println!("({})", "exists".green());
        } else {
            println!("(exists)");
        }
    } else {
        if color {
            println!("({})", "using defaults".yellow());
        } else {
            println!("(using defaults)");
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct ExplainOutput {
    input_path: String,
    expanded_path: String,
    tracked_path: Option<String>,
    monitored: bool,
    matched_dir: Option<String>,
    depth_used: Option<u32>,
}

pub fn explain_path(path: &str, json: bool) -> Result<()> {
    let config = Config::load()?;
    let home = crate::db::get_user_home();

    // Expand the path
    let expanded = if path.starts_with('/') {
        path.to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest).to_string_lossy().into_owned()
    } else if path == "~" {
        home.to_string_lossy().into_owned()
    } else {
        home.join(path).to_string_lossy().into_owned()
    };

    let tracked = crate::monitor::get_tracked_path(
        &expanded,
        &home,
        &config.monitored_dirs,
        config.tracking_depth,
    );

    // Figure out which dir matched and what depth was used
    let (matched_dir, depth_used) = find_matching_dir(&expanded, &home, &config);

    if json {
        let output = ExplainOutput {
            input_path: path.to_string(),
            expanded_path: expanded.clone(),
            tracked_path: tracked.clone(),
            monitored: tracked.is_some(),
            matched_dir,
            depth_used,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let color = use_color();

    println!("Input:    {}", path);
    println!("Expanded: {}", expanded);

    match &tracked {
        Some(tp) => {
            if color {
                println!("Tracked:  {}", tp.green());
            } else {
                println!("Tracked:  {}", tp);
            }
            if let Some(ref dir) = find_matching_dir(&expanded, &home, &config).0 {
                println!("Matched:  monitored dir '{}'", dir);
            }
            if let Some(d) = find_matching_dir(&expanded, &home, &config).1 {
                println!("Depth:    {}", d);
            }
        }
        None => {
            if color {
                println!("Tracked:  {} (not under any monitored directory)", "no".red());
            } else {
                println!("Tracked:  no (not under any monitored directory)");
            }
        }
    }

    Ok(())
}

fn find_matching_dir(
    expanded: &str,
    home: &std::path::Path,
    config: &Config,
) -> (Option<String>, Option<u32>) {
    let home_str = home.to_string_lossy();

    // Check absolute dirs first
    for dir in &config.monitored_dirs {
        if dir.path.starts_with('/') {
            let base = dir.path.trim_end_matches('/');
            if expanded.starts_with(base) && (expanded.len() == base.len() || expanded[base.len()..].starts_with('/')) {
                let depth = dir.depth.unwrap_or(config.tracking_depth);
                return (Some(dir.path.clone()), Some(depth));
            }
        }
    }

    // Check relative dirs
    if let Some(relative) = expanded.strip_prefix(home_str.as_ref()) {
        let relative = relative.trim_start_matches('/');
        for dir in &config.monitored_dirs {
            if dir.path.starts_with('/') {
                continue;
            }
            let dir_name = dir.path.trim_start_matches('.');
            let prefix = format!(".{}/", dir_name);
            if relative.starts_with(&prefix) {
                let depth = dir.depth.unwrap_or(config.tracking_depth);
                return (Some(dir.path.clone()), Some(depth));
            }
        }
    }

    (None, None)
}
