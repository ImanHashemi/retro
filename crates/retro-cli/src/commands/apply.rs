use anyhow::Result;
use colored::Colorize;
use retro_core::analysis::claude_cli::ClaudeCliBackend;
use retro_core::audit_log;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::lock::LockFile;
use retro_core::models::{ApplyPlan, SuggestedTarget};
use retro_core::projection;
use retro_core::projection::claude_md;

use super::git_root_or_cwd;

/// Output mode for displaying the plan.
pub enum DisplayMode {
    /// Show plan summary (used by `retro apply --dry-run`)
    Plan { dry_run: bool },
    /// Show diff-style output (used by `retro diff`)
    Diff,
}

/// Shared entry point: build the apply plan and either display or execute it.
pub fn run_apply(global: bool, dry_run: bool, display_mode: DisplayMode) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
    let audit_path = dir.join("audit.jsonl");
    let lock_path = dir.join("retro.lock");

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    // Acquire lockfile
    let _lock = LockFile::acquire(&lock_path)
        .map_err(|e| anyhow::anyhow!("could not acquire lock: {e}"))?;

    let project = if global {
        None
    } else {
        Some(git_root_or_cwd()?)
    };

    // Check claude CLI availability (needed for skill/agent generation)
    if !ClaudeCliBackend::is_available() {
        anyhow::bail!("claude CLI not found on PATH. Install Claude Code CLI to generate skills.");
    }

    let backend = ClaudeCliBackend::new(&config.ai);

    println!(
        "{}",
        "Building apply plan (this may call AI for skill generation)...".cyan()
    );

    let plan = projection::build_apply_plan(&conn, &config, &backend, project.as_deref())?;

    if plan.is_empty() {
        println!(
            "{}",
            "No patterns qualify for projection. Run `retro analyze` first.".yellow()
        );
        return Ok(());
    }

    // Display based on mode
    match &display_mode {
        DisplayMode::Plan { dry_run: dr } => display_plan(&plan, *dr),
        DisplayMode::Diff => display_diff(&plan),
    }

    if dry_run {
        println!();
        let hint = match display_mode {
            DisplayMode::Diff => "Dry run — no files were modified. Run `retro apply` to apply changes.",
            _ => "Dry run — no files were modified. Run `retro apply` to apply changes.",
        };
        println!("{}", hint.yellow().bold());
        return Ok(());
    }

    // Confirm before writing
    println!();
    print!(
        "{} ",
        format!(
            "Apply {} changes? [y/N]",
            plan.actions.len()
        )
        .yellow()
        .bold()
    );
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if !input.trim().eq_ignore_ascii_case("y") {
        println!("{}", "Aborted.".dimmed());
        return Ok(());
    }

    // Execute the plan
    println!("{}", "Applying changes...".cyan());

    let result = projection::execute_plan(&conn, &config, &plan, project.as_deref())?;

    // Audit log
    let audit_details = serde_json::json!({
        "files_written": result.files_written,
        "patterns_activated": result.patterns_activated,
        "project": project,
        "global": global,
    });
    audit_log::append(&audit_path, "apply", audit_details)?;

    // Summary
    println!();
    println!("{}", "Apply complete!".green().bold());
    println!(
        "  {} {}",
        "Files written:".white(),
        result.files_written.to_string().green()
    );
    println!(
        "  {} {}",
        "Patterns activated:".white(),
        result.patterns_activated.to_string().green()
    );

    let shared_count = plan.shared_actions().len();
    if shared_count > 0 {
        println!();
        println!(
            "  {} shared changes were written locally.",
            shared_count.to_string().yellow()
        );
        println!(
            "  {}",
            "PR creation will be available in a future version.".dimmed()
        );
    }

    Ok(())
}

/// CLI entry point for `retro apply`.
pub fn run(global: bool, dry_run: bool) -> Result<()> {
    run_apply(global, dry_run, DisplayMode::Plan { dry_run })
}

fn display_plan(plan: &ApplyPlan, dry_run: bool) {
    let personal = plan.personal_actions();
    let shared = plan.shared_actions();

    let label = if dry_run { "Proposed" } else { "Planned" };

    println!();
    println!(
        "{} {} changes:",
        label,
        plan.actions.len().to_string().cyan()
    );

    if !shared.is_empty() {
        println!();
        println!(
            "  {} ({} items):",
            "Shared (project)".yellow().bold(),
            shared.len()
        );
        for action in &shared {
            let icon = match action.target_type {
                SuggestedTarget::Skill => "skill",
                SuggestedTarget::ClaudeMd => "rule ",
                _ => "     ",
            };
            println!(
                "    {} [{}] {}",
                "+".green(),
                icon.dimmed(),
                action.pattern_description.white()
            );
            println!("           → {}", action.target_path.dimmed());
        }
    }

    if !personal.is_empty() {
        println!();
        println!(
            "  {} ({} items):",
            "Personal (auto-apply)".green().bold(),
            personal.len()
        );
        for action in &personal {
            println!(
                "    {} [agent] {}",
                "+".green(),
                action.pattern_description.white()
            );
            println!("           → {}", action.target_path.dimmed());
        }
    }
}

fn display_diff(plan: &ApplyPlan) {
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
        println!("{} {}", "---".dimmed(), target_path.bold());

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
}
