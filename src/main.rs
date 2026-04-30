use clap::{Parser, Subcommand, CommandFactory};
use anyhow::Result;

mod cleanup;
mod config;
mod db;
mod monitor;
mod pkgmgr;
mod query;

/// Home Directory Attribution System - track which packages create files in your home directory
#[derive(Parser)]
#[command(name = "hdas")]
#[command(version)]
#[command(help_template = "\
{about-with-newline}
{usage-heading} {usage}

Query:
  list           List all cataloged files and their package attributions
  package        Show all files created by a specific package
  dir            Show all tracked files under a directory
  query          Search files by path pattern
  orphans        Show files from packages that are no longer installed
  recheck        Re-check orphan files and fix misattributions

Cleanup:
  clean          Delete files created by a specific package
  clean-orphans  Delete all files from uninstalled packages
  prune          Remove stale records (deleted, excluded, ignored)
  forget         Drop database records for a package (no file deletion)

Info:
  status         Show monitor, database, and config at a glance
  explain        Show how a path would be tracked (depth truncation)

Admin:
  monitor        Start the eBPF monitor daemon (requires root)
  config         Manage configuration (show, edit, init, validate)
  ignore         Add a package to ignored_packages and prune its records
  exclude        Add a path to excluded_paths and prune its records

{options}
Use \"hdas help <command>\" for more information about a command.
")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output results as JSON
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Commands {
    // ── Querying ─────────────────────────────────────────────

    /// List all cataloged files and their package attributions
    List,
    /// Show all files created by a specific package
    Package {
        /// Package name to look up
        name: String,
    },
    /// Show all tracked files under a directory
    Dir {
        /// Directory path (absolute, relative to ~, or with ~/ prefix)
        path: String,
    },
    /// Query files by path pattern (supports SQL LIKE wildcards)
    Query {
        /// Path pattern to search for (e.g. "mozilla", "%.cache%")
        pattern: String,
    },
    /// Show files from packages that are no longer installed
    Orphans,
    /// Re-check orphan files against package manager and reassign ownership
    Recheck,

    // ── Cleanup ──────────────────────────────────────────────

    /// Delete files created by a specific package
    Clean {
        /// Package whose files should be deleted
        package: String,
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
        /// Show what would be deleted without actually deleting
        #[arg(short = 'n', long)]
        dry_run: bool,
    },
    /// Delete all files from uninstalled packages
    CleanOrphans {
        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
        /// Show what would be deleted without actually deleting
        #[arg(short = 'n', long)]
        dry_run: bool,
    },
    /// Remove stale records (deleted files, excluded paths, ignored packages)
    Prune,
    /// Drop database records for a package without deleting files or changing config
    Forget {
        /// Package name whose records should be removed
        package: String,
    },

    // ── Info ─────────────────────────────────────────────────

    /// Show monitor, database, and config status at a glance
    Status,
    /// Explain how a path would be tracked (show depth truncation)
    Explain {
        /// Full path to test (e.g. ~/.cache/mozilla/firefox/something)
        path: String,
    },

    // ── Administration ───────────────────────────────────────

    /// Start the eBPF monitor daemon (requires root)
    Monitor,
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Add a package to ignored_packages in config and prune its existing records
    Ignore {
        /// Package name to ignore
        package: String,
    },
    /// Add a path to excluded_paths in config and prune its existing records
    Exclude {
        /// Path to exclude (absolute, ~/relative, or relative to home)
        path: String,
    },

    // ── Hidden ───────────────────────────────────────────────

    /// Generate shell completion scripts
    #[command(hide = true)]
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Generate a man page
    #[command(hide = true)]
    ManPage,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration file contents
    Show,
    /// Open configuration in $EDITOR
    Edit,
    /// Create default config file if it doesn't exist
    Init,
    /// Validate configuration for errors and warnings
    Validate,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let json = cli.json;

    match cli.command {
        Commands::Monitor => {
            if !nix::unistd::Uid::effective().is_root() {
                eprintln!("Monitor requires root privileges. Run with sudo.");
                std::process::exit(1);
            }
            monitor::run_monitor()?;
        }
        Commands::List => query::list_all(json)?,
        Commands::Query { pattern } => query::query_file(&pattern, json)?,
        Commands::Package { name } => query::query_package(&name, json)?,
        Commands::Dir { path } => query::query_directory(&path, json)?,
        Commands::Orphans => query::show_orphans(json)?,
        Commands::Recheck => query::recheck(json)?,
        Commands::Clean { package, force, dry_run } => cleanup::clean_package(&package, force, dry_run, json)?,
        Commands::CleanOrphans { force, dry_run } => cleanup::clean_orphans(force, dry_run, json)?,
        Commands::Prune => cleanup::prune()?,
        Commands::Forget { package } => query::forget_package_cmd(&package)?,
        Commands::Config { action } => {
            match action {
                Some(ConfigAction::Show) | None => query::show_config()?,
                Some(ConfigAction::Edit) => query::edit_config()?,
                Some(ConfigAction::Init) => query::init_config()?,
                Some(ConfigAction::Validate) => query::validate_config(json)?,
            }
        }
        Commands::Status => query::show_status(json)?,
        Commands::Explain { path } => query::explain_path(&path, json)?,
        Commands::Ignore { package } => query::ignore_package_cmd(&package)?,
        Commands::Exclude { path } => query::exclude_path_cmd(&path)?,
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "hdas", &mut std::io::stdout());
        }
        Commands::ManPage => {
            clap_mangen::Man::new(Cli::command()).render(&mut std::io::stdout())?;
        }
    }

    Ok(())
}
