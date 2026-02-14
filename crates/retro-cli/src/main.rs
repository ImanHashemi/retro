mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "retro", about = "Active context curator for AI coding agents")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize retro: create ~/.retro/, config, and database
    Init,
    /// Ingest new sessions from Claude Code history (fast, no AI)
    Ingest {
        /// Ingest sessions for all projects, not just the current one
        #[arg(long)]
        global: bool,
    },
    /// Analyze sessions to discover patterns (AI-powered)
    Analyze {
        /// Analyze sessions for all projects, not just the current one
        #[arg(long)]
        global: bool,
        /// Analysis window in days (default: from config, typically 14)
        #[arg(long)]
        since: Option<u32>,
    },
    /// List discovered patterns
    Patterns {
        /// Filter by status: discovered, active, archived, dismissed
        #[arg(long)]
        status: Option<String>,
    },
    /// Project patterns into skills, CLAUDE.md rules, and global agents
    Apply {
        /// Show what would be changed without writing files
        #[arg(long)]
        dry_run: bool,
    },
    /// Show pending changes in diff format (alias for apply --dry-run)
    Diff,
    /// Show retro status: session counts, last analysis, patterns
    Status,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => commands::init::run(),
        Commands::Ingest { global } => commands::ingest::run(global),
        Commands::Analyze { global, since } => commands::analyze::run(global, since),
        Commands::Patterns { status } => commands::patterns::run(status),
        Commands::Apply { dry_run } => commands::apply::run(dry_run),
        Commands::Diff => commands::diff::run(),
        Commands::Status => commands::status::run(),
    };

    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
