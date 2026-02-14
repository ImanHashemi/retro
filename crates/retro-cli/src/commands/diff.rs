use anyhow::Result;
use colored::Colorize;
use retro_core::analysis::claude_cli::ClaudeCliBackend;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::lock::LockFile;
use retro_core::models::SuggestedTarget;
use retro_core::projection;
use retro_core::projection::claude_md;

use super::git_root_or_cwd;

pub fn run() -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
    let lock_path = dir.join("retro.lock");

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    let _lock = LockFile::acquire(&lock_path)
        .map_err(|e| anyhow::anyhow!("could not acquire lock: {e}"))?;

    let project = git_root_or_cwd()?;

    if !ClaudeCliBackend::is_available() {
        anyhow::bail!("claude CLI not found on PATH. Install Claude Code CLI to generate skills.");
    }

    let backend = ClaudeCliBackend::new(&config.ai);

    println!(
        "{}",
        "Building apply plan (this may call AI for skill generation)...".cyan()
    );

    let plan = projection::build_apply_plan(&conn, &config, &backend, Some(&project))?;

    if plan.is_empty() {
        println!(
            "{}",
            "No pending changes. Run `retro analyze` to discover patterns.".yellow()
        );
        return Ok(());
    }

    // Show diff-style output for each action
    println!();

    // CLAUDE.md diff
    let claude_md_actions: Vec<_> = plan
        .actions
        .iter()
        .filter(|a| a.target_type == SuggestedTarget::ClaudeMd)
        .collect();

    if !claude_md_actions.is_empty() {
        let target_path = &claude_md_actions[0].target_path;
        let rules: Vec<String> = claude_md_actions.iter().map(|a| a.content.clone()).collect();
        println!(
            "{} {}",
            "---".dimmed(),
            target_path.bold()
        );

        // Show existing managed section if any
        if let Ok(existing) = std::fs::read_to_string(target_path) {
            if let Some(old_rules) = claude_md::read_managed_section(&existing) {
                for rule in &old_rules {
                    println!("{} - {rule}", "-".red());
                }
            }
        }

        // Show new rules
        for rule in &rules {
            println!("{} - {rule}", "+".green());
        }
        println!();
    }

    // Skills diff
    for action in &plan.actions {
        if action.target_type == SuggestedTarget::Skill {
            println!(
                "{} {} {}",
                "---".dimmed(),
                action.target_path.bold(),
                "(new)".green()
            );
            for line in action.content.lines() {
                println!("{} {line}", "+".green());
            }
            println!();
        }
    }

    // Global agents diff
    for action in &plan.actions {
        if action.target_type == SuggestedTarget::GlobalAgent {
            println!(
                "{} {} {}",
                "---".dimmed(),
                action.target_path.bold(),
                "(new)".green()
            );
            for line in action.content.lines() {
                println!("{} {line}", "+".green());
            }
            println!();
        }
    }

    println!(
        "{}",
        "Run `retro apply` to apply these changes.".yellow().bold()
    );

    Ok(())
}
