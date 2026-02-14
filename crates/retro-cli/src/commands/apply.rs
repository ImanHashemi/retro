use anyhow::Result;
use colored::Colorize;
use retro_core::analysis::claude_cli::ClaudeCliBackend;
use retro_core::audit_log;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::lock::LockFile;
use retro_core::models::SuggestedTarget;
use retro_core::projection;

use super::git_root_or_cwd;

pub fn run(dry_run: bool) -> Result<()> {
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

    let project = git_root_or_cwd()?;

    // Check claude CLI availability (needed for skill/agent generation)
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
            "No patterns qualify for projection. Run `retro analyze` first.".yellow()
        );
        return Ok(());
    }

    // Display the plan
    display_plan(&plan, dry_run);

    if dry_run {
        println!();
        println!(
            "{}",
            "Dry run — no files were modified. Run `retro apply` to apply changes."
                .yellow()
                .bold()
        );
        return Ok(());
    }

    // Execute the plan
    println!();
    println!("{}", "Applying changes...".cyan());

    let result = projection::execute_plan(&conn, &config, &plan, Some(&project))?;

    // Audit log
    let audit_details = serde_json::json!({
        "files_written": result.files_written,
        "patterns_activated": result.patterns_activated,
        "project": project,
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

fn display_plan(plan: &retro_core::models::ApplyPlan, dry_run: bool) {
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
