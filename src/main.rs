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

Cleanup:
  clean          Delete files created by a specific package
  clean-orphans  Delete all files from uninstalled packages
  prune          Remove stale database records

Info:
  status         Show service, database, and config at a glance
  stats          Show detailed database and configuration statistics
  explain        Show how a path would be tracked (depth truncation)

Admin:
  monitor        Start the eBPF monitor daemon (requires root)
  config         Manage configuration (show, edit, init, validate)

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
    /// Remove database records for files that no longer exist on disk
    Prune,

    // ── Info ─────────────────────────────────────────────────

    /// Show service, database, and config status at a glance
    Status,
    /// Show database and configuration statistics
    Stats,
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
        Commands::Stats => query::show_stats(json)?,
        Commands::Query { pattern } => query::query_file(&pattern, json)?,
        Commands::Package { name } => query::query_package(&name, json)?,
        Commands::Dir { path } => query::query_directory(&path, json)?,
        Commands::Orphans => query::show_orphans(json)?,
        Commands::Clean { package, force, dry_run } => cleanup::clean_package(&package, force, dry_run, json)?,
        Commands::CleanOrphans { force, dry_run } => cleanup::clean_orphans(force, dry_run, json)?,
        Commands::Prune => cleanup::prune()?,
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
        Commands::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "hdas", &mut std::io::stdout());
        }
        Commands::ManPage => {
            clap_mangen::Man::new(Cli::command()).render(&mut std::io::stdout())?;
        }
    }

    Ok(())
}
