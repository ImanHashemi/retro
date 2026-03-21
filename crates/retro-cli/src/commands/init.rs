use anyhow::{Context, Result};
use colored::Colorize;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::git;
use retro_core::git::HookInstallResult;
use retro_core::util::shorten_path_buf;

use super::git_root_or_cwd;

pub fn run(uninstall: bool, purge: bool, verbose: bool) -> Result<()> {
    if uninstall {
        return run_uninstall(purge, verbose);
    }

    let dir = retro_dir();

    // Create ~/.retro/ directory structure
    std::fs::create_dir_all(&dir).context("creating ~/.retro/")?;
    std::fs::create_dir_all(dir.join("backups")).context("creating ~/.retro/backups/")?;

    // Truncate hook stderr log on fresh init
    let hook_stderr_path = dir.join("hook-stderr.log");
    if hook_stderr_path.exists() {
        let _ = std::fs::write(&hook_stderr_path, "");
    }

    // Create config.toml if it doesn't exist
    let config_path = dir.join("config.toml");
    if !config_path.exists() {
        let config = Config::default();
        config.save(&config_path)?;
        println!("  {} {}", "Created".green(), shorten_path_buf(&config_path));
    } else {
        println!("  {} {}", "Exists".yellow(), shorten_path_buf(&config_path));
    }

    // Initialize database with WAL mode
    let db_path = dir.join("retro.db");
    let db_existed = db_path.exists();
    let conn = db::open_db(&db_path)?;

    if verbose {
        eprintln!("[verbose] retro dir: {}", dir.display());
    }

    let is_wal = db::verify_wal_mode(&conn)?;
    let label = if db_existed { "Exists" } else { "Created" };
    let color_label = if db_existed {
        label.yellow()
    } else {
        label.green()
    };
    if is_wal {
        println!("  {} {} (WAL mode)", color_label, shorten_path_buf(&db_path));
    } else {
        println!(
            "  {} {} (warning: WAL mode not enabled)",
            label.yellow(),
            shorten_path_buf(&db_path)
        );
    }

    // Install git hooks if in a repo
    if git::is_in_git_repo() {
        let repo_root = git_root_or_cwd()?;
        match git::install_hooks(&repo_root) {
            Ok(results) => {
                for (hook, result) in &results {
                    match result {
                        HookInstallResult::Installed => {
                            println!("  {} git hook: {} (ingest → analyze → apply)", "Installed".green(), hook);
                        }
                        HookInstallResult::Updated => {
                            println!("  {} git hook: {} (ingest → analyze → apply)", "Updated".green(), hook);
                        }
                        HookInstallResult::UpToDate => {
                            println!("  {} git hook: {} (already up to date)", "Exists".yellow(), hook);
                        }
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

    // Create briefings directory
    let briefings_dir = dir.join("briefings");
    if !briefings_dir.exists() {
        std::fs::create_dir_all(&briefings_dir).context("creating briefings dir")?;
        println!("  {} {}", "Created".green(), shorten_path_buf(&briefings_dir));
    }

    // Install launchd scheduled runner (macOS only)
    if cfg!(target_os = "macos") {
        let config = Config::load(&config_path)?;
        match crate::launchd::install_and_load(&config) {
            Ok(()) => {
                println!("  {} scheduled runner (every {}s)", "Started".green(), config.runner.interval_seconds);
            }
            Err(e) => {
                println!("  {} could not start scheduled runner: {e}", "Warning".yellow());
            }
        }
    }

    // Create briefing skill if in a repo
    if git::is_in_git_repo() {
        let repo_root = git_root_or_cwd()?;
        let project_id = db::generate_project_slug(&repo_root);
        create_briefing_skill(&repo_root, &project_id)?;
    }

    println!();
    println!("{}", "retro initialized successfully".green().bold());
    println!("  Run {} to open the dashboard", "retro dash".cyan());

    Ok(())
}

fn create_briefing_skill(project_root: &str, project_id: &str) -> Result<()> {
    let skills_dir = std::path::Path::new(project_root).join(".claude").join("skills");
    let skill_path = skills_dir.join("retro-briefing.md");
    if skill_path.exists() {
        println!("  {} briefing skill: {}", "Exists".yellow(), skill_path.display());
        return Ok(());
    }
    std::fs::create_dir_all(&skills_dir).context("creating .claude/skills/")?;
    let content = format!(
        "---\nname: retro-briefing\ndescription: Read Retro's session briefing at the start of each conversation\n---\nAt the start of each conversation, read ~/.retro/briefings/{project_id}.md\nif it exists and briefly acknowledge any new learnings or pending suggestions.\n"
    );
    std::fs::write(&skill_path, content).context("writing briefing skill")?;
    println!("  {} briefing skill: {}", "Created".green(), skill_path.display());
    Ok(())
}

fn run_uninstall(purge: bool, _verbose: bool) -> Result<()> {
    println!("{}", "Uninstalling retro...".cyan());

    // Unload and remove launchd plist (macOS only)
    if cfg!(target_os = "macos") {
        let _ = crate::launchd::unload();
        let plist = crate::launchd::plist_path();
        if plist.exists() {
            let _ = std::fs::remove_file(&plist);
            println!("  {} launchd plist", "Removed".green());
        }
    }

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
            println!("  {} {}", "Deleted".green(), shorten_path_buf(&dir));
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
