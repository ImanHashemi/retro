use anyhow::{Context, Result};
use colored::Colorize;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::git;

use super::git_root_or_cwd;

pub fn run(uninstall: bool, purge: bool, verbose: bool) -> Result<()> {
    if uninstall {
        return run_uninstall(purge);
    }

    let dir = retro_dir();

    // Create ~/.retro/ directory structure
    std::fs::create_dir_all(&dir).context("creating ~/.retro/")?;
    std::fs::create_dir_all(dir.join("backups")).context("creating ~/.retro/backups/")?;

    // Create config.toml if it doesn't exist
    let config_path = dir.join("config.toml");
    if !config_path.exists() {
        let config = Config::default();
        config.save(&config_path)?;
        println!("  {} {}", "Created".green(), config_path.display());
    } else {
        println!("  {} {}", "Exists".yellow(), config_path.display());
    }

    // Initialize database with WAL mode
    let db_path = dir.join("retro.db");
    let db_existed = db_path.exists();
    let conn = db::open_db(&db_path)?;

    if verbose {
        println!("[verbose] retro dir: {}", dir.display());
    }

    let is_wal = db::verify_wal_mode(&conn)?;
    let label = if db_existed { "Exists" } else { "Created" };
    let color_label = if db_existed {
        label.yellow()
    } else {
        label.green()
    };
    if is_wal {
        println!("  {} {} (WAL mode)", color_label, db_path.display());
    } else {
        println!(
            "  {} {} (warning: WAL mode not enabled)",
            label.yellow(),
            db_path.display()
        );
    }

    // Install git hooks if in a repo
    if git::is_in_git_repo() {
        let repo_root = git_root_or_cwd()?;
        match git::install_hooks(&repo_root) {
            Ok(installed) => {
                if installed.is_empty() {
                    println!("  {} git hooks (already installed)", "Exists".yellow());
                } else {
                    for hook in &installed {
                        println!("  {} git hook: {}", "Installed".green(), hook);
                    }
                }
            }
            Err(e) => {
                println!(
                    "  {} could not install git hooks: {e}",
                    "Warning".yellow()
                );
            }
        }
    } else {
        println!("  {} not in a git repository, skipping hooks", "Note".dimmed());
    }

    println!();
    println!("{}", "retro initialized successfully".green().bold());
    println!(
        "  Run {} to parse Claude Code sessions",
        "retro ingest".cyan()
    );

    Ok(())
}

fn run_uninstall(purge: bool) -> Result<()> {
    println!("{}", "Uninstalling retro...".cyan());

    // Remove git hooks from current repo
    if git::is_in_git_repo() {
        let repo_root = git_root_or_cwd()?;
        match git::remove_hooks(&repo_root) {
            Ok(modified) => {
                if modified.is_empty() {
                    println!("  {} no retro hooks found", "Note".dimmed());
                } else {
                    for hook in &modified {
                        println!("  {} hook: {}", "Removed".green(), hook);
                    }
                }
            }
            Err(e) => {
                println!("  {} removing hooks: {e}", "Warning".yellow());
            }
        }
    }

    if purge {
        let dir = retro_dir();
        if dir.exists() {
            std::fs::remove_dir_all(&dir).context("removing ~/.retro/")?;
            println!("  {} {}", "Deleted".green(), dir.display());
        }
        println!();
        println!(
            "{}",
            "retro fully uninstalled (hooks removed, data purged)."
                .green()
                .bold()
        );
    } else {
        println!();
        println!(
            "{}",
            "retro hooks removed. Data preserved in ~/.retro/."
                .green()
                .bold()
        );
        println!(
            "  Use {} to also delete all retro data.",
            "retro init --uninstall --purge".cyan()
        );
    }

    Ok(())
}
