mod commands;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "retro", about = "Active context curator for AI coding agents")]
struct Cli {
    /// Enable verbose debug output
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize retro: create ~/.retro/, config, database, and git hooks
    Init {
        /// Remove retro hooks from current repo (preserves ~/.retro/ data)
        #[arg(long)]
        uninstall: bool,
        /// When used with --uninstall, also delete ~/.retro/ entirely
        #[arg(long, requires = "uninstall")]
        purge: bool,
    },
    /// Ingest new sessions from Claude Code history (fast, no AI)
    Ingest {
        /// Ingest sessions for all projects, not just the current one
        #[arg(long)]
        global: bool,
        /// Silent mode for git hooks: skip if locked, check cooldown, suppress output
        #[arg(long)]
        auto: bool,
    },
    /// Analyze sessions to discover patterns (AI-powered)
    Analyze {
        /// Analyze sessions for all projects, not just the current one
        #[arg(long)]
        global: bool,
        /// Analysis window in days (default: from config, typically 14)
        #[arg(long)]
        since: Option<u32>,
        /// Silent mode for git hooks: skip if locked, check cooldown, suppress output
        #[arg(long)]
        auto: bool,
        /// Preview what would be analyzed without making AI calls
        #[arg(long)]
        dry_run: bool,
    },
    /// List discovered patterns
    Patterns {
        /// Filter by status: discovered, active, archived, dismissed
        #[arg(long)]
        status: Option<String>,
    },
    /// Generate content from patterns and queue for review (use `retro review` to approve)
    Apply {
        /// Preview what would be generated without making AI calls
        #[arg(long)]
        dry_run: bool,
        /// Apply patterns for all projects, not just the current one
        #[arg(long)]
        global: bool,
        /// Silent mode for git hooks: skip if locked, check cooldown, suppress output
        #[arg(long)]
        auto: bool,
    },
    /// Show pending changes in diff format (alias for apply --dry-run)
    Diff {
        /// Show changes for all projects, not just the current one
        #[arg(long)]
        global: bool,
    },
    /// Archive stale patterns and remove their projections (fast, no AI)
    Clean {
        /// Show what would be archived without making changes
        #[arg(long)]
        dry_run: bool,
    },
    /// AI-powered context review for redundancy and contradictions
    Audit {
        /// Show findings without making changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Show retro status: session counts, last analysis, patterns
    Status,
    /// Show audit log entries
    Log {
        /// Show entries from the last N days/hours (e.g., "7d", "24h")
        #[arg(long)]
        since: Option<String>,
    },
    /// Review pending suggestions: approve, skip, or dismiss generated items
    Review {
        /// Review items for all projects, not just the current one
        #[arg(long)]
        global: bool,
        /// Show pending items without prompting for action
        #[arg(long)]
        dry_run: bool,
    },
    /// Sync PR status: reset patterns from closed PRs back to discoverable
    Sync,
    /// Manage git hooks
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },
}

#[derive(Subcommand)]
enum HooksAction {
    /// Remove retro git hooks from the current repository
    Remove,
}

fn main() {
    let cli = Cli::parse();
    let verbose = cli.verbose;

    // Show nudge for interactive commands (not auto mode)
    let is_auto = matches!(
        &cli.command,
        Commands::Ingest { auto: true, .. }
            | Commands::Analyze { auto: true, .. }
            | Commands::Apply { auto: true, .. }
    );
    if !is_auto {
        commands::check_and_display_nudge();
    }

    let result = match cli.command {
        Commands::Init { uninstall, purge } => commands::init::run(uninstall, purge, verbose),
        Commands::Ingest { global, auto } => commands::ingest::run(global, auto, verbose),
        Commands::Analyze {
            global,
            since,
            auto,
            dry_run,
        } => commands::analyze::run(global, since, auto, dry_run, verbose),
        Commands::Patterns { status } => commands::patterns::run(status),
        Commands::Apply { global, dry_run, auto } => commands::apply::run(global, dry_run, auto, verbose),
        Commands::Diff { global } => commands::diff::run(global, verbose),
        Commands::Clean { dry_run } => commands::clean::run(dry_run, verbose),
        Commands::Audit { dry_run } => commands::audit::run(dry_run, verbose),
        Commands::Status => commands::status::run(),
        Commands::Log { since } => commands::log::run(since),
        Commands::Review { global, dry_run } => commands::review::run(global, dry_run, verbose),
        Commands::Sync => commands::sync::run(verbose),
        Commands::Hooks { action } => match action {
            HooksAction::Remove => commands::hooks::run_remove(),
        },
    };

    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
