use anyhow::Result;
use owo_colors::OwoColorize;
use serde::Serialize;
use std::io::IsTerminal;
use std::path::Path;

use crate::db::{Database, FileRecord};
use crate::query::format_size;

fn use_color() -> bool {
    std::io::stdout().is_terminal()
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
    is_symlink: bool,
}

impl CleanTarget {
    fn from_record(record: FileRecord) -> Option<Self> {
        let path = Path::new(&record.path);
        // Use symlink_metadata to detect symlinks (including broken ones)
        let meta = match path.symlink_metadata() {
            Ok(m) => m,
            Err(_) => return None,
        };
        let is_symlink = meta.file_type().is_symlink();
        let is_dir = !is_symlink && path.is_dir();
        let size = if is_symlink { 0 } else { get_path_size(path) };
        Some(CleanTarget { record, size, is_dir, is_symlink })
    }
}

fn display_target(target: &CleanTarget) {
    let color = use_color();
    let type_indicator = if target.is_symlink {
        "link"
    } else if target.is_dir {
        "dir "
    } else {
        "file"
    };
    let size = format_size(target.size);

    if color {
        let type_colored = if target.is_symlink {
            format!("{}", type_indicator.bold())
        } else if target.is_dir {
            format!("{}", type_indicator.yellow())
        } else {
            type_indicator.to_string()
        };
        println!("  [{:>6}] [{}] {}", size.dimmed(), type_colored, target.record.path);
    } else {
        println!("  [{:>6}] [{}] {}", size, type_indicator, target.record.path);
    }
}

#[derive(Serialize)]
struct CleanPreview {
    package: Option<String>,
    targets: Vec<CleanTargetInfo>,
    total_size: u64,
    file_count: usize,
    dir_count: usize,
    symlink_count: usize,
}

#[derive(Serialize)]
struct CleanTargetInfo {
    path: String,
    size: u64,
    is_dir: bool,
    is_symlink: bool,
}

#[derive(Serialize)]
struct CleanResult {
    deleted: Vec<String>,
    errors: Vec<CleanError>,
    records_removed: usize,
}

#[derive(Serialize)]
struct CleanError {
    path: String,
    error: String,
}

pub fn clean_package(package: &str, force: bool, dry_run: bool, json: bool) -> Result<()> {
    let db = Database::new()?;
    let records = db.query_package(package)?;

    let targets: Vec<_> = records
        .into_iter()
        .filter_map(CleanTarget::from_record)
        .collect();

    if targets.is_empty() {
        if json {
            let result = CleanPreview {
                package: Some(package.to_string()),
                targets: vec![],
                total_size: 0,
                file_count: 0,
                dir_count: 0,
                symlink_count: 0,
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        } else {
            println!("No existing files found for package: {}", package);
        }
        return Ok(());
    }

    let total_size: u64 = targets.iter().map(|t| t.size).sum();
    let dir_count = targets.iter().filter(|t| t.is_dir).count();
    let symlink_count = targets.iter().filter(|t| t.is_symlink).count();
    let file_count = targets.len() - dir_count - symlink_count;

    if json && dry_run {
        let preview = CleanPreview {
            package: Some(package.to_string()),
            targets: targets.iter().map(|t| CleanTargetInfo {
                path: t.record.path.clone(),
                size: t.size,
                is_dir: t.is_dir,
                is_symlink: t.is_symlink,
            }).collect(),
            total_size,
            file_count,
            dir_count,
            symlink_count,
        };
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }

    if !json {
        let color = use_color();
        if dry_run {
            println!("Would delete {} file(s), {} director(ies), {} symlink(s) [{}]:",
                file_count, dir_count, symlink_count, format_size(total_size));
        } else {
            println!("Will delete {} file(s), {} director(ies), {} symlink(s) [{}]:",
                file_count, dir_count, symlink_count, format_size(total_size));
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
            if color {
                print!("{}", "Proceed? [y/N]: ".bold());
            } else {
                print!("Proceed? [y/N]: ");
            }
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
    }

    let mut deleted_paths: Vec<String> = Vec::new();
    let mut errors: Vec<CleanError> = Vec::new();

    for target in &targets {
        let path = Path::new(&target.record.path);

        let result = if target.is_symlink {
            std::fs::remove_file(path)
        } else if target.is_dir {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };

        match result {
            Ok(_) => {
                if !json {
                    if target.is_symlink {
                        println!("Deleted symlink: {}", target.record.path);
                    } else {
                        println!("Deleted: {}", target.record.path);
                    }
                }
                deleted_paths.push(target.record.path.clone());
            }
            Err(e) => {
                if !json {
                    eprintln!("Error deleting {}: {}", target.record.path, e);
                }
                errors.push(CleanError {
                    path: target.record.path.clone(),
                    error: e.to_string(),
                });
            }
        }
    }

    let records_removed = if !deleted_paths.is_empty() {
        db.delete_file_records(&deleted_paths)?
    } else {
        0
    };

    if json {
        let result = CleanResult {
            deleted: deleted_paths,
            errors,
            records_removed,
        };
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        let color = use_color();
        println!();
        if color {
            println!(
                "Summary: {} deleted, {} errors",
                deleted_paths.len().to_string().green(),
                errors.len().to_string().red()
            );
        } else {
            println!(
                "Summary: {} deleted, {} errors",
                deleted_paths.len(),
                errors.len()
            );
        }
        if records_removed > 0 {
            println!("Removed {} database record(s)", records_removed);
        }
    }

    Ok(())
}

pub fn clean_orphans(force: bool, dry_run: bool, json: bool) -> Result<()> {
    let db = Database::new()?;
    let orphan_packages = db.get_orphans()?;

    if orphan_packages.is_empty() {
        if json {
            println!("{}", serde_json::to_string_pretty(&CleanResult {
                deleted: vec![],
                errors: vec![],
                records_removed: 0,
            })?);
        } else {
            println!("No orphaned packages found!");
        }
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
        if json {
            let mut records_removed = 0;
            if !dry_run {
                records_removed = db.prune_deleted()?;
            }
            println!("{}", serde_json::to_string_pretty(&CleanResult {
                deleted: vec![],
                errors: vec![],
                records_removed,
            })?);
        } else {
            println!("No existing files from orphaned packages.");
            if !dry_run {
                let pruned = db.prune_deleted()?;
                if pruned > 0 {
                    println!("Pruned {} stale database record(s)", pruned);
                }
            }
        }
        return Ok(());
    }

    let total_size: u64 = all_targets.iter().map(|(_, t)| t.size).sum();
    let dir_count = all_targets.iter().filter(|(_, t)| t.is_dir).count();
    let symlink_count = all_targets.iter().filter(|(_, t)| t.is_symlink).count();
    let file_count = all_targets.len() - dir_count - symlink_count;

    if json && dry_run {
        let preview = CleanPreview {
            package: None,
            targets: all_targets.iter().map(|(_, t)| CleanTargetInfo {
                path: t.record.path.clone(),
                size: t.size,
                is_dir: t.is_dir,
                is_symlink: t.is_symlink,
            }).collect(),
            total_size,
            file_count,
            dir_count,
            symlink_count,
        };
        println!("{}", serde_json::to_string_pretty(&preview)?);
        return Ok(());
    }

    if !json {
        let color = use_color();
        if dry_run {
            println!("Would delete {} file(s), {} director(ies), {} symlink(s) from {} orphaned package(s) [{}]:\n",
                file_count, dir_count, symlink_count, orphan_packages.len(), format_size(total_size));
        } else {
            println!("Will delete {} file(s), {} director(ies), {} symlink(s) from {} orphaned package(s) [{}]:\n",
                file_count, dir_count, symlink_count, orphan_packages.len(), format_size(total_size));
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
            if color {
                print!("{}", "Proceed? [y/N]: ".bold());
            } else {
                print!("Proceed? [y/N]: ");
            }
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
    }

    let mut deleted_paths: Vec<String> = Vec::new();
    let mut errors: Vec<CleanError> = Vec::new();

    for (_, target) in &all_targets {
        let path = Path::new(&target.record.path);

        let result = if target.is_symlink {
            std::fs::remove_file(path)
        } else if target.is_dir {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };

        match result {
            Ok(_) => {
                if !json {
                    if target.is_symlink {
                        println!("Deleted symlink: {}", target.record.path);
                    } else {
                        println!("Deleted: {}", target.record.path);
                    }
                }
                deleted_paths.push(target.record.path.clone());
            }
            Err(e) => {
                if !json {
                    eprintln!("Error deleting {}: {}", target.record.path, e);
                }
                errors.push(CleanError {
                    path: target.record.path.clone(),
                    error: e.to_string(),
                });
            }
        }
    }

    let records_removed = if !deleted_paths.is_empty() {
        db.delete_file_records(&deleted_paths)?
    } else {
        0
    };

    if json {
        let result = CleanResult {
            deleted: deleted_paths,
            errors,
            records_removed,
        };
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        let color = use_color();
        println!();
        if color {
            println!(
                "Summary: {} deleted, {} errors",
                deleted_paths.len().to_string().green(),
                errors.len().to_string().red()
            );
        } else {
            println!(
                "Summary: {} deleted, {} errors",
                deleted_paths.len(),
                errors.len()
            );
        }
        if records_removed > 0 {
            println!("Removed {} database record(s)", records_removed);
        }
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
