use anyhow::Result;
use std::path::Path;

use crate::db::{Database, FileRecord};

fn format_size(bytes: u64) -> String {
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

fn get_path_size(path: &Path) -> u64 {
    if path.is_file() {
        path.metadata().map(|m| m.len()).unwrap_or(0)
    } else if path.is_dir() {
        walkdir::WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum()
    } else {
        0
    }
}

struct CleanTarget {
    record: FileRecord,
    size: u64,
    is_dir: bool,
}

impl CleanTarget {
    fn from_record(record: FileRecord) -> Option<Self> {
        let path = Path::new(&record.path);
        if !path.exists() {
            return None;
        }
        let is_dir = path.is_dir();
        let size = get_path_size(path);
        Some(CleanTarget { record, size, is_dir })
    }
}

fn display_target(target: &CleanTarget) {
    let type_indicator = if target.is_dir { "dir " } else { "file" };
    let size = format_size(target.size);
    println!("  [{:>6}] [{}] {}", size, type_indicator, target.record.path);
}

pub fn clean_package(package: &str, force: bool, dry_run: bool) -> Result<()> {
    let db = Database::new()?;
    let records = db.query_package(package)?;

    let targets: Vec<_> = records
        .into_iter()
        .filter_map(CleanTarget::from_record)
        .collect();

    if targets.is_empty() {
        println!("No existing files found for package: {}", package);
        return Ok(());
    }

    let total_size: u64 = targets.iter().map(|t| t.size).sum();
    let dir_count = targets.iter().filter(|t| t.is_dir).count();
    let file_count = targets.len() - dir_count;

    if dry_run {
        println!("Would delete {} file(s), {} director(ies) [{}]:",
            file_count, dir_count, format_size(total_size));
    } else {
        println!("Will delete {} file(s), {} director(ies) [{}]:",
            file_count, dir_count, format_size(total_size));
    }

    for target in &targets {
        display_target(target);
    }

    if dry_run {
        println!("\n(dry run - no files were deleted)");
        return Ok(());
    }

    if !force {
        println!();
        print!("Proceed? [y/N]: ");
        use std::io::{self, BufRead, Write};
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;

        let response = line.trim().to_lowercase();
        if response != "y" && response != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut deleted_paths: Vec<String> = Vec::new();
    let mut error_count = 0;

    for target in &targets {
        let path = Path::new(&target.record.path);

        let result = if target.is_dir {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };

        match result {
            Ok(_) => {
                println!("Deleted: {}", target.record.path);
                deleted_paths.push(target.record.path.clone());
            }
            Err(e) => {
                eprintln!("Error deleting {}: {}", target.record.path, e);
                error_count += 1;
            }
        }
    }

    println!();
    println!(
        "Summary: {} deleted, {} errors",
        deleted_paths.len(),
        error_count
    );

    if !deleted_paths.is_empty() {
        let removed = db.delete_file_records(&deleted_paths)?;
        println!("Removed {} database record(s)", removed);
    }

    Ok(())
}

pub fn clean_orphans(force: bool, dry_run: bool) -> Result<()> {
    let db = Database::new()?;
    let orphan_packages = db.get_orphans()?;

    if orphan_packages.is_empty() {
        println!("No orphaned packages found!");
        return Ok(());
    }

    let mut all_targets: Vec<(String, CleanTarget)> = Vec::new();

    for pkg in &orphan_packages {
        let records = db.query_package(pkg)?;
        for record in records {
            if let Some(target) = CleanTarget::from_record(record) {
                all_targets.push((pkg.clone(), target));
            }
        }
    }

    if all_targets.is_empty() {
        println!("No existing files from orphaned packages.");
        if !dry_run {
            let pruned = db.prune_deleted()?;
            if pruned > 0 {
                println!("Pruned {} stale database record(s)", pruned);
            }
        }
        return Ok(());
    }

    let total_size: u64 = all_targets.iter().map(|(_, t)| t.size).sum();
    let dir_count = all_targets.iter().filter(|(_, t)| t.is_dir).count();
    let file_count = all_targets.len() - dir_count;

    if dry_run {
        println!("Would delete {} file(s), {} director(ies) from {} orphaned package(s) [{}]:\n",
            file_count, dir_count, orphan_packages.len(), format_size(total_size));
    } else {
        println!("Will delete {} file(s), {} director(ies) from {} orphaned package(s) [{}]:\n",
            file_count, dir_count, orphan_packages.len(), format_size(total_size));
    }

    let mut current_pkg = String::new();
    for (pkg, target) in &all_targets {
        if pkg != &current_pkg {
            if !current_pkg.is_empty() {
                println!();
            }
            println!("{}:", pkg);
            current_pkg = pkg.clone();
        }
        display_target(target);
    }

    if dry_run {
        println!("\n(dry run - no files were deleted)");
        return Ok(());
    }

    if !force {
        println!();
        print!("Proceed? [y/N]: ");
        use std::io::{self, BufRead, Write};
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;

        let response = line.trim().to_lowercase();
        if response != "y" && response != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut deleted_paths: Vec<String> = Vec::new();
    let mut error_count = 0;

    for (_, target) in &all_targets {
        let path = Path::new(&target.record.path);

        let result = if target.is_dir {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };

        match result {
            Ok(_) => {
                println!("Deleted: {}", target.record.path);
                deleted_paths.push(target.record.path.clone());
            }
            Err(e) => {
                eprintln!("Error deleting {}: {}", target.record.path, e);
                error_count += 1;
            }
        }
    }

    println!();
    println!(
        "Summary: {} deleted, {} errors",
        deleted_paths.len(),
        error_count
    );

    if !deleted_paths.is_empty() {
        let removed = db.delete_file_records(&deleted_paths)?;
        println!("Removed {} database record(s)", removed);
    }

    Ok(())
}

pub fn prune() -> Result<()> {
    let db = Database::new()?;
    let pruned = db.prune_deleted()?;

    if pruned > 0 {
        println!("Pruned {} deleted file(s) from database", pruned);
    } else {
        println!("No deleted files to prune");
    }

    Ok(())
}
