mod commands;
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
    /// Initialize the v3 personal store (git-backed ~/.retro, global hooks)
    Init {
        /// Clone an existing v3 knowledge repo instead of starting fresh
        #[arg(long, value_name = "REMOTE")]
        from: Option<String>,
    },
    /// Migrate v2 knowledge and environment to v3 (idempotent, v2 db untouched)
    Migrate {
        /// Preview without writing
        #[arg(long)]
        dry_run: bool,
    },
    /// Run the v3 pipeline: drain queue, analyze, project, commit, push
    Run {
        /// Show detailed output
        #[arg(long)]
        verbose: bool,
        /// Preview only, don't make changes
        #[arg(long)]
        dry_run: bool,
        /// Quiet background mode: exit silently if another run holds the lock
        #[arg(long)]
        background: bool,
    },
    /// (v3 hook entry) Enqueue a finished session for analysis — called by the SessionEnd hook
    Observe,
    /// (v3 hook entry) Catch-up scan + session briefing — called by the SessionStart hook
    Brief,
    /// Rebuild the v3 store index from knowledge files (safe anytime)
    Reindex,
    /// Show retro status: store stats, queue, budget, health
    Status,
    /// End-to-end health verification (read-only)
    Doctor,
    /// Store-wide lint: near-duplicates and stale candidates (no AI calls)
    Lint {
        /// Report only; don't queue findings as briefing notifications
        #[arg(long)]
        dry_run: bool,
    },
    /// Open the dashboard (local web UI)
    Ui {
        /// Don't auto-open the browser
        #[arg(long)]
        no_open: bool,
    },
    /// Remove retro (hooks, projections, launchd remnants). Store kept unless --purge
    Uninstall {
        /// Also delete ~/.retro (asks for confirmation)
        #[arg(long)]
        purge: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let verbose = cli.verbose;

    // Show nudge for interactive commands (not hook entries or background runs)
    let is_auto = matches!(
        &cli.command,
        Commands::Observe
            | Commands::Brief
            // suppress the "run `retro run`" nudge on the way out the door
            | Commands::Uninstall { .. }
            | Commands::Run {
                background: true,
                ..
            }
    );
    if !is_auto {
        commands::check_and_display_nudge();
    }

    let result = match cli.command {
        Commands::Init { from } => commands::init::run(from),
        Commands::Migrate { dry_run } => commands::migrate::run(dry_run),
        Commands::Run { verbose: run_verbose, dry_run, background } => {
            commands::run::run(verbose || run_verbose, dry_run, background)
        }
        Commands::Observe => commands::observe::run(),
        Commands::Brief => commands::brief::run(),
        Commands::Reindex => commands::reindex::run(),
        Commands::Status => commands::status::run(),
        Commands::Doctor => commands::doctor::run(),
        Commands::Lint { dry_run } => commands::lint::run(dry_run),
        Commands::Ui { no_open } => commands::ui::run(no_open),
        Commands::Uninstall { purge } => commands::uninstall::run(purge),
    };

    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
