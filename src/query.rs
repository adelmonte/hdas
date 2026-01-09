use anyhow::Result;
use chrono::{DateTime, Datelike, Local, Utc};
use std::path::Path;

use crate::config::Config;
use crate::db::FileRecord;

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

fn display_record(record: &FileRecord, show_accessor: bool) {
    let exists = if Path::new(&record.path).exists() { "✓" } else { "✗" };
    let time = format_time(record.created_at);

    println!("{} [{}] {} ({})", time, exists, record.path, record.created_by_package);

    if show_accessor
        && (record.last_accessed_by_package != record.created_by_package
            || record.last_accessed_by_process != record.created_by_process)
    {
        let access_time = format_time(record.last_accessed_at);
        println!(
            "{}      └─ last accessed by {} ({})",
            access_time,
            record.last_accessed_by_package,
            record.last_accessed_by_process
        );
    }
}

fn maybe_prune(db: &crate::db::Database) -> Result<usize> {
    let config = Config::load()?;
    if config.auto_prune {
        db.prune_deleted()
    } else {
        Ok(0)
    }
}

pub fn query_file(pattern: &str) -> Result<()> {
    let db = crate::db::Database::new()?;

    let pruned = maybe_prune(&db)?;
    if pruned > 0 {
        eprintln!("(auto-pruned {} deleted file(s))", pruned);
    }

    let records = db.query_file(pattern)?;

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

pub fn query_package(package: &str) -> Result<()> {
    let db = crate::db::Database::new()?;

    let pruned = maybe_prune(&db)?;
    if pruned > 0 {
        eprintln!("(auto-pruned {} deleted file(s))", pruned);
    }

    let records = db.query_package(package)?;

    if records.is_empty() {
        println!("No files found for package: {}", package);
        return Ok(());
    }

    println!("Files created by {} ({} total):\n", package, records.len());
    for record in records {
        let exists = if Path::new(&record.path).exists() { "✓" } else { "✗" };
        let time = format_time(record.created_at);
        println!("{} [{}] {}", time, exists, record.path);
    }

    Ok(())
}

pub fn query_directory(dir: &str) -> Result<()> {
    let db = crate::db::Database::new()?;

    let pruned = maybe_prune(&db)?;
    if pruned > 0 {
        eprintln!("(auto-pruned {} deleted file(s))", pruned);
    }

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

pub fn show_orphans() -> Result<()> {
    let db = crate::db::Database::new()?;
    let orphans = db.get_orphans()?;

    if orphans.is_empty() {
        println!("No orphaned files found!");
        return Ok(());
    }

    println!("Files from uninstalled packages:\n");
    for pkg in orphans {
        let records = db.query_package(&pkg)?;
        if !records.is_empty() {
            let existing_count = records.iter().filter(|r| Path::new(&r.path).exists()).count();
            let deleted_count = records.len() - existing_count;

            print!("{} ({} file(s)", pkg, records.len());
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

pub fn list_all() -> Result<()> {
    let db = crate::db::Database::new()?;

    let pruned = maybe_prune(&db)?;
    if pruned > 0 {
        eprintln!("(auto-pruned {} deleted file(s))", pruned);
    }

    let records = db.list_all()?;

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

pub fn show_stats() -> Result<()> {
    let db = crate::db::Database::new()?;
    let (files, packages, db_path) = db.get_stats()?;
    let config = Config::load()?;

    println!("HDAS Database Statistics");
    println!("========================");
    println!("Database: {}", db_path);
    println!("Files tracked: {}", files);
    println!("Packages seen: {}", packages);
    println!();
    println!("Configuration");
    println!("-------------");
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

pub fn clean_package(package: &str, force: bool) -> Result<()> {
    let db = crate::db::Database::new()?;
    let records = db.query_package(package)?;

    let files: Vec<_> = records
        .into_iter()
        .filter(|r| Path::new(&r.path).exists())
        .collect();

    if files.is_empty() {
        println!("No existing files found for package: {}", package);
        return Ok(());
    }

    println!("Will delete {} file(s):", files.len());
    for record in &files {
        println!("  {}", record.path);
    }

    if !force {
        println!();
        print!("Proceed? [y/N]: ");
        use std::io::{self, BufRead, Write};
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;

        if line.trim().to_lowercase() != "y" {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut deleted_count = 0;
    let mut error_count = 0;

    for record in &files {
        let path = Path::new(&record.path);

        let result = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };

        match result {
            Ok(_) => {
                println!("Deleted: {}", record.path);
                deleted_count += 1;
            }
            Err(e) => {
                eprintln!("Error deleting {}: {}", record.path, e);
                error_count += 1;
            }
        }
    }

    println!();
    println!(
        "Summary: {} deleted, {} errors",
        deleted_count, error_count
    );

    if deleted_count > 0 {
        db.delete_package_records(package)?;
        println!("Removed {} database record(s) for {}", deleted_count, package);
    }

    Ok(())
}

pub fn prune() -> Result<()> {
    let db = crate::db::Database::new()?;
    let pruned = db.prune_deleted()?;

    if pruned > 0 {
        println!("Pruned {} deleted file(s) from database", pruned);
    } else {
        println!("No deleted files to prune");
    }

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
