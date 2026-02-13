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
    /// Show retro status: session counts, last analysis, patterns
    Status,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => commands::init::run(),
        Commands::Ingest { global } => commands::ingest::run(global),
        Commands::Status => commands::status::run(),
    };

    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
