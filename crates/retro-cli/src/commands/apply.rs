use anyhow::Result;
use colored::Colorize;
use retro_core::analysis::claude_cli::ClaudeCliBackend;
use retro_core::audit_log;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::git;
use retro_core::lock::LockFile;
use retro_core::models::{ApplyPlan, ApplyTrack, SuggestedTarget};
use retro_core::projection;
use retro_core::projection::claude_md;

use retro_core::util::shorten_path;

use super::{git_root_or_cwd, within_cooldown};

/// Output mode for displaying the plan.
pub enum DisplayMode {
    /// Show plan summary (used by `retro apply --dry-run`)
    Plan { dry_run: bool },
    /// Show diff-style output (used by `retro diff`)
    Diff,
}

/// Shared entry point: build the apply plan and either display or execute it.
pub fn run_apply(global: bool, dry_run: bool, auto: bool, display_mode: DisplayMode, verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
    let audit_path = dir.join("audit.jsonl");
    let lock_path = dir.join("retro.lock");

    if !db_path.exists() {
        if auto {
            return Ok(());
        }
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    // Auto mode: acquire lockfile silently, check cooldown, run without prompts
    if auto {
        let _lock = match LockFile::try_acquire(&lock_path) {
            Some(lock) => lock,
            None => {
                if verbose {
                    eprintln!("[verbose] skipping apply: another process holds the lock");
                }
                return Ok(());
            }
        };

        // Cooldown check
        if let Ok(Some(ref last)) = db::last_applied_at(&conn)
            && within_cooldown(last, config.hooks.apply_cooldown_minutes)
        {
            if verbose {
                eprintln!(
                    "[verbose] skipping apply: within cooldown ({}m)",
                    config.hooks.apply_cooldown_minutes
                );
            }
            return Ok(());
        }

        // Data gate: any un-projected patterns?
        if !db::has_unprojected_patterns(&conn, config.analysis.confidence_threshold)? {
            if verbose {
                eprintln!("[verbose] skipping apply: no un-projected patterns");
            }
            return Ok(());
        }

        let project = if global {
            None
        } else {
            Some(git_root_or_cwd()?)
        };

        // Check claude CLI availability
        if !ClaudeCliBackend::is_available() {
            if verbose {
                eprintln!("[verbose] skipping apply: claude CLI not available");
            }
            return Ok(());
        }

        let backend = ClaudeCliBackend::new(&config.ai);

        // Build and execute plan silently
        match projection::build_apply_plan(&conn, &config, &backend, project.as_deref()) {
            Ok(plan) => {
                if plan.is_empty() {
                    if verbose {
                        eprintln!("[verbose] apply: no actions in plan");
                    }
                    return Ok(());
                }

                let mut pr_url: Option<String> = None;

                // Phase 1: Personal actions on current branch
                if let Err(e) = projection::execute_plan(
                    &conn,
                    &config,
                    &plan,
                    project.as_deref(),
                    Some(&ApplyTrack::Personal),
                ) {
                    if verbose {
                        eprintln!("[verbose] apply personal error: {e}");
                    }
                    let _ = audit_log::append(
                        &audit_path,
                        "apply_error",
                        serde_json::json!({
                            "error": format!("personal: {e}"),
                            "auto": true,
                        }),
                    );
                }

                // Phase 2: Shared actions on new branch + PR
                if !plan.shared_actions().is_empty() {
                    match execute_shared_with_pr(
                        &conn, &config, &plan, project.as_deref(), true,
                    ) {
                        Ok(shared_result) => {
                            pr_url = shared_result.pr_url;
                        }
                        Err(e) => {
                            if verbose {
                                eprintln!("[verbose] apply shared error: {e}");
                            }
                            let _ = audit_log::append(
                                &audit_path,
                                "apply_error",
                                serde_json::json!({
                                    "error": format!("shared: {e}"),
                                    "auto": true,
                                }),
                            );
                        }
                    }
                }

                // Audit log (best-effort in auto mode)
                let audit_details = serde_json::json!({
                    "actions": plan.actions.len(),
                    "project": project,
                    "global": global,
                    "auto": true,
                    "pr_url": pr_url,
                });
                let _ = audit_log::append(&audit_path, "apply", audit_details);

                if verbose {
                    eprintln!("[verbose] auto-apply complete: {} actions", plan.actions.len());
                }
            }
            Err(e) => {
                if verbose {
                    eprintln!("[verbose] apply plan error: {e}");
                }
                let _ = audit_log::append(
                    &audit_path,
                    "apply_error",
                    serde_json::json!({
                        "error": e.to_string(),
                        "auto": true,
                    }),
                );
            }
        }

        return Ok(());
    }

    // Interactive mode — acquire lockfile (error if locked)
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

    if verbose {
        if let Some(ref p) = project {
            eprintln!("[verbose] project path: {}", p);
        }
    }

    println!(
        "{}",
        "Building apply plan (this may call AI for skill generation)...".cyan()
    );
    println!(
        "  {}",
        "This may take a minute per pattern...".dimmed()
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
        println!(
            "{}",
            "Dry run \u{2014} no files were modified. Run `retro apply` to apply changes."
                .yellow()
                .bold()
        );
        return Ok(());
    }

    // Confirm before writing
    println!();
    print!(
        "{} ",
        format!("Apply {} changes? [y/N]", plan.actions.len())
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

    // Execute in two phases: personal first (current branch), then shared (new branch)
    println!("{}", "Applying changes...".cyan());

    let mut total_files = 0;
    let mut total_patterns = 0;
    let mut pr_url: Option<String> = None;

    // Phase 1: Personal actions (global agents) — write on current branch
    let has_personal = !plan.personal_actions().is_empty();
    if has_personal {
        let result = projection::execute_plan(
            &conn,
            &config,
            &plan,
            project.as_deref(),
            Some(&ApplyTrack::Personal),
        )?;
        total_files += result.files_written;
        total_patterns += result.patterns_activated;
    }

    // Phase 2: Shared actions (skills, CLAUDE.md) — on a new branch if in git repo
    let has_shared = !plan.shared_actions().is_empty();
    if has_shared {
        let shared_result = execute_shared_with_pr(&conn, &config, &plan, project.as_deref(), false)?;
        total_files += shared_result.files_written;
        total_patterns += shared_result.patterns_activated;
        pr_url = shared_result.pr_url;
    }

    // Audit log
    let audit_details = serde_json::json!({
        "files_written": total_files,
        "patterns_activated": total_patterns,
        "project": project,
        "global": global,
        "pr_url": pr_url,
    });
    audit_log::append(&audit_path, "apply", audit_details)?;

    // Summary
    println!();
    println!("{}", "Apply complete!".green().bold());
    println!(
        "  {} {}",
        "Files written:".white(),
        total_files.to_string().green()
    );
    println!(
        "  {} {}",
        "Patterns activated:".white(),
        total_patterns.to_string().green()
    );

    if has_shared {
        println!();
        if let Some(url) = &pr_url {
            println!("  {} {}", "Pull request created:".white(), url.cyan());
        } else if !git::is_in_git_repo() {
            println!(
                "  {}",
                "Not in a git repo \u{2014} shared changes written to disk only.".dimmed()
            );
        } else if !git::is_gh_available() {
            println!(
                "  {}",
                "gh CLI not available \u{2014} create a PR manually from the retro branch."
                    .dimmed()
            );
        }
    }

    Ok(())
}

struct SharedResult {
    files_written: usize,
    patterns_activated: usize,
    pr_url: Option<String>,
}

/// Execute shared actions: create branch from default branch, write files, commit, push, create PR, switch back.
/// When `silent` is true (auto mode), suppress all stdout/stderr output.
fn execute_shared_with_pr(
    conn: &db::Connection,
    config: &Config,
    plan: &ApplyPlan,
    project: Option<&str>,
    silent: bool,
) -> Result<SharedResult> {
    let in_git = git::is_in_git_repo();

    // If not in git, just write files on disk
    if !in_git {
        let result = projection::execute_plan(
            conn,
            config,
            plan,
            project,
            Some(&ApplyTrack::Shared),
        )?;
        return Ok(SharedResult {
            files_written: result.files_written,
            patterns_activated: result.patterns_activated,
            pr_url: None,
        });
    }

    let original_branch = git::current_branch()?;

    // Detect default branch and fetch latest
    let default_branch = match git::default_branch() {
        Ok(b) => b,
        Err(e) => {
            if !silent {
                eprintln!(
                    "  {} detecting default branch: {e}. Writing files on current branch.",
                    "Warning".yellow()
                );
            }
            let result = projection::execute_plan(
                conn,
                config,
                plan,
                project,
                Some(&ApplyTrack::Shared),
            )?;
            return Ok(SharedResult {
                files_written: result.files_written,
                patterns_activated: result.patterns_activated,
                pr_url: None,
            });
        }
    };

    if let Err(e) = git::fetch_branch(&default_branch) {
        if !silent {
            eprintln!("  {} fetching {}: {e}", "Warning".yellow(), default_branch);
        }
    }

    // Stash uncommitted changes before switching branches
    let did_stash = git::stash_push().unwrap_or(false);

    let date = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let branch_name = format!("retro/updates-{date}");
    let start_point = format!("origin/{default_branch}");

    // Create branch from origin/<default>
    if let Err(e) = git::create_branch(&branch_name, Some(&start_point)) {
        if !silent {
            eprintln!(
                "  {} creating branch: {e}. Writing files on current branch.",
                "Warning".yellow()
            );
        }
        // Restore stash before falling back
        if did_stash {
            let _ = git::stash_pop();
        }
        let result = projection::execute_plan(
            conn,
            config,
            plan,
            project,
            Some(&ApplyTrack::Shared),
        )?;
        return Ok(SharedResult {
            files_written: result.files_written,
            patterns_activated: result.patterns_activated,
            pr_url: None,
        });
    }

    // Write shared files on the new branch
    let result = projection::execute_plan(
        conn,
        config,
        plan,
        project,
        Some(&ApplyTrack::Shared),
    )?;

    // Stage and commit
    let shared_files: Vec<&str> = plan
        .shared_actions()
        .iter()
        .map(|a| a.target_path.as_str())
        .collect();

    let commit_msg = format!(
        "retro: update {} shared context items\n\nAuto-generated by retro apply.",
        shared_files.len()
    );

    let pr_url = if let Err(e) = git::commit_files(&shared_files, &commit_msg) {
        if !silent {
            eprintln!("  {} committing: {e}", "Warning".yellow());
        }
        None
    } else if git::is_gh_available() {
        // Push branch to origin before creating PR
        if let Err(e) = git::push_current_branch() {
            if !silent {
                eprintln!("  {} pushing branch: {e}", "Warning".yellow());
                println!(
                    "  {}",
                    format!(
                        "Changes committed to branch `{branch_name}`. Push and create PR manually."
                    )
                    .dimmed()
                );
            }
            None
        } else {
            // Create PR targeting the default branch
            let title = format!("retro: update {} context items", shared_files.len());
            let mut body = "## Retro Auto-Generated Updates\n\n".to_string();
            for action in &plan.shared_actions() {
                let icon = match action.target_type {
                    SuggestedTarget::Skill => "skill",
                    SuggestedTarget::ClaudeMd => "rule",
                    _ => "item",
                };
                body.push_str(&format!("- **[{icon}]** {}\n", action.pattern_description));
            }
            body.push_str("\n---\nGenerated by `retro apply`.");

            match git::create_pr(&title, &body, &default_branch) {
                Ok(url) => Some(url),
                Err(e) => {
                    if !silent {
                        eprintln!("  {} creating PR: {e}", "Warning".yellow());
                        println!(
                            "  {}",
                            format!(
                                "Changes committed to branch `{branch_name}`. Create PR manually."
                            )
                            .dimmed()
                        );
                    }
                    None
                }
            }
        }
    } else {
        if !silent {
            println!(
                "  {}",
                format!("Changes committed to branch `{branch_name}`.").dimmed()
            );
            println!(
                "  {}",
                "Install `gh` CLI to auto-create PRs, or create one manually.".dimmed()
            );
        }
        None
    };

    // Switch back to original branch and restore stashed changes
    let _ = git::checkout_branch(&original_branch);
    if did_stash {
        let _ = git::stash_pop();
    }

    Ok(SharedResult {
        files_written: result.files_written,
        patterns_activated: result.patterns_activated,
        pr_url,
    })
}

/// CLI entry point for `retro apply`.
pub fn run(global: bool, dry_run: bool, auto: bool, verbose: bool) -> Result<()> {
    if dry_run && auto {
        anyhow::bail!("--dry-run and --auto are mutually exclusive");
    }
    run_apply(global, dry_run, auto, DisplayMode::Plan { dry_run }, verbose)
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
            println!("           \u{2192} {}", shorten_path(&action.target_path).dimmed());
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
            println!("           \u{2192} {}", shorten_path(&action.target_path).dimmed());
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
        println!("{} {}", "---".dimmed(), shorten_path(target_path).bold());

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
                shorten_path(&action.target_path).bold(),
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
                shorten_path(&action.target_path).bold(),
                "(new)".green()
            );
            for line in action.content.lines() {
                println!("{} {line}", "+".green());
            }
            println!();
        }
    }
}
