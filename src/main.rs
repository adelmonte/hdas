use clap::{Parser, Subcommand};
use anyhow::Result;

mod config;
mod db;
mod monitor;
mod query;

#[derive(Parser)]
#[command(name = "hdas")]
#[command(about = "Home Directory Attribution System - track which packages create files in your home directory")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the monitor daemon (requires sudo)
    Monitor,
    /// List all cataloged files
    List,
    /// Show database and configuration statistics
    Stats,
    /// Query files by path pattern
    Query { pattern: String },
    /// Show files created by a package
    Package { name: String },
    /// Show files from uninstalled packages
    Orphans,
    /// Delete files created by a package
    Clean {
        package: String,
        #[arg(short, long)]
        force: bool,
    },
    /// Remove deleted files from the database
    Prune,
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show current configuration
    Show,
    /// Edit configuration in $EDITOR
    Edit,
    /// Create default config file
    Init,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Monitor => {
            if !nix::unistd::Uid::effective().is_root() {
                eprintln!("Monitor requires root privileges. Run with sudo.");
                std::process::exit(1);
            }
            monitor::run_monitor()?;
        }
        Commands::List => query::list_all()?,
        Commands::Stats => query::show_stats()?,
        Commands::Query { pattern } => query::query_file(&pattern)?,
        Commands::Package { name } => query::query_package(&name)?,
        Commands::Orphans => query::show_orphans()?,
        Commands::Clean { package, force } => query::clean_package(&package, force)?,
        Commands::Prune => query::prune()?,
        Commands::Config { action } => {
            match action {
                Some(ConfigAction::Show) | None => query::show_config()?,
                Some(ConfigAction::Edit) => query::edit_config()?,
                Some(ConfigAction::Init) => query::init_config()?,
            }
        }
    }

    Ok(())
}
