mod commands;
mod launchd;
mod tui;
mod ui;

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
        /// Initialize the v3 personal store (git-backed ~/.retro, global hooks)
        #[arg(long, conflicts_with = "uninstall")]
        v3: bool,
        /// Clone an existing v3 knowledge repo instead of starting fresh (implies --v3)
        #[arg(long, value_name = "REMOTE", conflicts_with = "uninstall")]
        from: Option<String>,
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
        /// Show patterns for all projects, not just the current one
        #[arg(long)]
        global: bool,
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
    /// Rebuild the v3 store index from knowledge files (safe anytime)
    Reindex,
    /// (v3 hook entry) Enqueue a finished session for analysis — called by the SessionEnd hook
    Observe,
    /// (v3 hook entry) Catch-up scan + session briefing — called by the SessionStart hook
    Brief,
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
    /// Agentic CLAUDE.md rewrite: AI explores codebase and proposes a complete rewrite via PR
    Curate {
        /// Preview context summary without making AI calls
        #[arg(long)]
        dry_run: bool,
    },
    /// Run the full v2 pipeline (observe -> ingest -> analyze -> project -> apply)
    Run {
        /// Show detailed output
        #[arg(long)]
        verbose: bool,
        /// Preview only, don't make changes
        #[arg(long)]
        dry_run: bool,
        /// (v3) Quiet background mode: exit silently if another run holds the lock
        #[arg(long)]
        background: bool,
    },
    /// Start the scheduled runner (launchd on macOS)
    Start,
    /// Stop the scheduled runner
    Stop,
    /// Sync PR status: reset patterns from closed PRs back to discoverable
    Sync,
    /// Open the TUI dashboard
    Dash,
    /// (v3) Open the dashboard (local web UI)
    Ui {
        /// Don't auto-open the browser
        #[arg(long)]
        no_open: bool,
    },
    /// (v3) End-to-end health verification (read-only)
    Doctor,
    /// (v3) Store-wide lint: near-duplicates and stale candidates (no AI calls)
    Lint {
        /// Report only; don't queue findings as briefing notifications
        #[arg(long)]
        dry_run: bool,
    },
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

    // Show nudge for interactive commands (not auto mode, not hook entries)
    let is_auto = matches!(
        &cli.command,
        Commands::Ingest { auto: true, .. }
            | Commands::Analyze { auto: true, .. }
            | Commands::Apply { auto: true, .. }
            | Commands::Observe
            | Commands::Brief
            | Commands::Run {
                background: true,
                ..
            }
    );
    if !is_auto {
        commands::check_and_display_nudge();
    }

    let result = match cli.command {
        Commands::Init {
            uninstall,
            purge,
            v3,
            from,
        } => commands::init::run(uninstall, purge, verbose, v3, from),
        Commands::Ingest { global, auto } => commands::ingest::run(global, auto, verbose),
        Commands::Analyze {
            global,
            since,
            auto,
            dry_run,
        } => commands::analyze::run(global, since, auto, dry_run, verbose),
        Commands::Patterns { status, global } => commands::patterns::run(status, global),
        Commands::Apply { global, dry_run, auto } => commands::apply::run(global, dry_run, auto, verbose),
        Commands::Diff { global } => commands::diff::run(global, verbose),
        Commands::Clean { dry_run } => commands::clean::run(dry_run, verbose),
        Commands::Audit { dry_run } => commands::audit::run(dry_run, verbose),
        Commands::Status => commands::status::run(),
        Commands::Reindex => commands::reindex::run(),
        Commands::Observe => commands::observe::run(),
        Commands::Brief => commands::brief::run(),
        Commands::Log { since } => commands::log::run(since),
        Commands::Review { global, dry_run } => commands::review::run(global, dry_run, verbose),
        Commands::Curate { dry_run } => commands::curate::run(dry_run, verbose),
        Commands::Run { verbose: run_verbose, dry_run, background } => commands::run::run(verbose || run_verbose, dry_run, background),
        Commands::Start => commands::start::run(verbose),
        Commands::Stop => commands::stop::run(verbose),
        Commands::Sync => commands::sync::run(verbose),
        Commands::Dash => commands::dash::run(verbose),
        Commands::Ui { no_open } => commands::ui::run(no_open),
        Commands::Doctor => commands::doctor::run(),
        Commands::Lint { dry_run } => commands::lint::run(dry_run),
        Commands::Hooks { action } => match action {
            HooksAction::Remove => commands::hooks::run_remove(),
        },
    };

    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
