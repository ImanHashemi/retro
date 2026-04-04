use anyhow::Result;
use chrono::Utc;
use colored::Colorize;
use retro_core::analysis;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::ingest;
use retro_core::observer;

/// Run the full v2 pipeline: observe -> ingest -> analyze -> project -> apply.
pub fn run(verbose: bool, dry_run: bool) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");

    // Check initialization
    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    // Rotate log if needed
    let _ = retro_core::runner::rotate_log_if_needed(&dir);

    // Check schema version
    let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version < 4 {
        anyhow::bail!("database schema v{version} detected — run `retro init` to migrate to v2");
    }

    match retro_core::git::git_root() {
        Ok(path) => {
            // Single project mode (manual invocation from inside a git repo)
            // Ensure project is registered (catches projects initialized before this fix)
            if !dry_run {
                let _ = db::ensure_project_registered(&conn, &path);
            }
            run_for_project(&conn, &config, &path, verbose, dry_run)?;
        }
        Err(_) => {
            // Global mode (launchd invocation, or outside any repo)
            let projects = db::get_all_projects(&conn)?;
            if projects.is_empty() {
                if verbose {
                    println!("{}", "No known projects.".dimmed());
                }
                return Ok(());
            }
            for project in &projects {
                if verbose {
                    println!("  {} {}", "Project:".white(), project.id.cyan());
                }
                if let Err(e) = run_for_project(&conn, &config, &project.path, verbose, dry_run) {
                    eprintln!("  {} {}: {e}", "Error".red(), project.id);
                }
            }
        }
    }

    if !dry_run {
        update_last_run(&conn)?;
    }

    Ok(())
}

/// Run the pipeline for a single project.
fn run_for_project(
    conn: &retro_core::db::Connection,
    config: &Config,
    project_path: &str,
    verbose: bool,
    dry_run: bool,
) -> Result<()> {
    // Step 1: Observe — find modified sessions
    println!("{}", "Step 1/6: Observing session changes...".cyan());
    let claude_dir = config.claude_dir();
    let last_run = db::get_metadata(conn, "last_run_at")?;
    let since = last_run.as_ref().and_then(|ts| {
        chrono::DateTime::parse_from_rfc3339(ts)
            .ok()
            .map(|dt| {
                std::time::SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_secs(dt.timestamp() as u64)
            })
    });

    let modified = observer::find_modified_sessions(&claude_dir, since, &[project_path.to_string()]);
    println!(
        "  {} modified session file{}",
        modified.len().to_string().cyan(),
        if modified.len() == 1 { "" } else { "s" }
    );

    if modified.is_empty() && !has_pending_work(conn, config) {
        println!();
        println!("{}", "Nothing to do — no modified sessions and no pending work.".dimmed());
        return Ok(());
    }

    if verbose {
        for m in &modified {
            eprintln!("[verbose]   {}", m.path.display());
        }
    }

    // Step 2: Ingest
    println!("{}", "Step 2/6: Ingesting sessions...".cyan());
    if dry_run {
        println!(
            "  {} {} modified session files to ingest",
            "[dry-run]".yellow(),
            modified.len()
        );
    } else {
        let ingest_result = ingest::ingest_project(conn, config, project_path)?;
        println!(
            "  {} ingested, {} skipped",
            ingest_result.sessions_ingested.to_string().green(),
            ingest_result.sessions_skipped.to_string().dimmed()
        );
        if !ingest_result.errors.is_empty() {
            for err in &ingest_result.errors {
                eprintln!("  {}", err.red());
            }
        }
    }

    // Step 3: Check analysis trigger
    println!("{}", "Step 3/6: Checking analysis trigger...".cyan());

    // Check AI call budget
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let (ai_calls_today, ai_calls_date) = get_ai_call_count(conn)?;
    let budget_remaining = if ai_calls_date == today {
        config.runner.max_ai_calls_per_day.saturating_sub(ai_calls_today)
    } else {
        config.runner.max_ai_calls_per_day // New day, full budget
    };

    if budget_remaining == 0 {
        println!(
            "  {} AI call budget exhausted for today ({}/{})",
            "Skipping:".yellow(),
            ai_calls_today,
            config.runner.max_ai_calls_per_day
        );
        // Don't return early — projection and sync steps below don't need AI calls
    }

    // Check minimum interval between analysis runs
    let interval_ok = if config.runner.min_analysis_interval_minutes > 0 && budget_remaining > 0 {
        let last_analysis = db::get_metadata(conn, "last_analysis_at")?;
        match last_analysis.and_then(|ts| chrono::DateTime::parse_from_rfc3339(&ts).ok()) {
            Some(last) => {
                let elapsed = Utc::now().signed_duration_since(last);
                let min_interval = chrono::Duration::minutes(config.runner.min_analysis_interval_minutes as i64);
                if elapsed < min_interval {
                    let remaining_mins = (min_interval - elapsed).num_minutes();
                    println!(
                        "  {} analysis cooldown — next in ~{} min ({}/{})",
                        "Skipping:".yellow(),
                        remaining_mins,
                        ai_calls_today,
                        config.runner.max_ai_calls_per_day
                    );
                    false
                } else {
                    true
                }
            }
            None => true, // No previous analysis, proceed
        }
    } else {
        true // No interval configured or budget exhausted
    };

    if verbose {
        eprintln!(
            "[verbose] AI budget: {}/{} calls remaining today",
            budget_remaining, config.runner.max_ai_calls_per_day
        );
    }

    // Check if analysis is needed based on trigger
    let should_analyze = should_trigger_analysis(conn, config)?;

    if !should_analyze || budget_remaining == 0 || !interval_ok {
        if budget_remaining == 0 || !interval_ok {
            // Already printed budget/interval message above
        } else {
            println!(
                "  {} analysis threshold not met (need {} unanalyzed sessions)",
                "Skipping:".dimmed(),
                config.runner.analysis_threshold
            );
        }
    } else {
        println!(
            "  {} analysis threshold met",
            "Triggering:".green()
        );

        if dry_run {
            let unanalyzed = db::unanalyzed_session_count(conn)?;
            println!(
                "  {} would analyze {} unanalyzed sessions",
                "[dry-run]".yellow(),
                unanalyzed
            );
        } else {
            println!("  {}", "Running AI-powered analysis...".dimmed());
            let window_days = config.analysis.window_days;
            let max_batches = batches_within_budget(usize::MAX, budget_remaining);
            let result = analysis::analyze_v2(
                conn,
                config,
                Some(project_path),
                window_days,
                max_batches,
                |idx, total, sessions, chars| {
                    println!(
                        "    {} batch {}/{} ({} sessions, ~{}K chars)...",
                        "Processing".dimmed(),
                        idx + 1,
                        total,
                        sessions,
                        chars / 1000
                    );
                },
            )?;

            println!(
                "  {} sessions analyzed, {} nodes created, {} updated",
                result.sessions_analyzed.to_string().cyan(),
                result.nodes_created.to_string().green(),
                result.nodes_updated.to_string().yellow()
            );

            if verbose {
                eprintln!(
                    "[verbose] tokens: {} in / {} out",
                    result.input_tokens, result.output_tokens
                );
            }

            // Track AI calls and last analysis time
            let calls_used = result.batch_count as u32;
            increment_ai_calls(conn, calls_used)?;
            db::set_metadata(conn, "last_analysis_at", &Utc::now().to_rfc3339())?;
        }
    }

    // Step 4/6: Project approved changes
    println!("{}", "Step 4/6: Projecting approved changes...".cyan());

    let unprojected = db::get_unprojected_nodes(conn)?;
    if unprojected.is_empty() {
        println!("  {} no unprojected nodes", "Skipping:".dimmed());
    } else if dry_run {
        println!("  {} {} nodes to project", "[dry-run]".yellow(), unprojected.len());
    } else {
        let mut projected_count = 0usize;

        // Group by scope
        let global_rules: Vec<&retro_core::models::KnowledgeNode> = unprojected.iter()
            .filter(|n| n.scope == retro_core::models::NodeScope::Global && n.project_id.is_none())
            .filter(|n| matches!(n.node_type, retro_core::models::NodeType::Rule | retro_core::models::NodeType::Directive | retro_core::models::NodeType::Preference))
            .collect();
        // Filter project-scoped nodes to ONLY this project (by slug match)
        let current_project_slug = db::generate_project_slug(project_path);
        let project_nodes: Vec<&retro_core::models::KnowledgeNode> = unprojected.iter()
            .filter(|n| n.scope == retro_core::models::NodeScope::Project)
            .filter(|n| n.project_id.as_deref() == Some(current_project_slug.as_str()))
            .filter(|n| matches!(n.node_type, retro_core::models::NodeType::Rule | retro_core::models::NodeType::Directive | retro_core::models::NodeType::Preference | retro_core::models::NodeType::Pattern))
            .collect();

        // Global rules → ~/.claude/CLAUDE.md
        if !global_rules.is_empty() {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            let claude_md_path = std::path::PathBuf::from(&home).join(".claude").join("CLAUDE.md");
            for node in &global_rules {
                if retro_core::projection::claude_md::project_rule_to_claude_md(&claude_md_path, &node.content).is_ok() {
                    db::mark_node_projected(conn, &node.id)?;
                    projected_count += 1;
                }
            }
            if verbose {
                println!("  {} {} global rules to ~/.claude/CLAUDE.md", "Projected".green(), global_rules.len());
            }
        }

        // Project-scoped nodes → CLAUDE.md on a PR branch
        if !project_nodes.is_empty() {
            let new_rules: Vec<String> = project_nodes.iter()
                .map(|n| n.content.clone())
                .collect();

            // Read existing managed section rules and APPEND new ones (don't replace)
            let claude_md_path = format!("{}/CLAUDE.md", project_path);
            let existing = std::fs::read_to_string(&claude_md_path).unwrap_or_default();
            let mut all_rules = retro_core::projection::claude_md::read_managed_section(&existing)
                .unwrap_or_default();
            for rule in &new_rules {
                if !all_rules.iter().any(|r| r == rule) {
                    all_rules.push(rule.clone());
                }
            }
            let updated = retro_core::projection::claude_md::update_claude_md_content(&existing, &all_rules);

            match retro_core::git::create_retro_pr(
                project_path,
                &[("CLAUDE.md", &updated)],
                "retro: update CLAUDE.md with discovered rules",
                "retro: update CLAUDE.md rules",
                &format!("Retro discovered {} rule(s) from your sessions.\n\nApproved via `retro dash`.", new_rules.len()),
            ) {
                Ok(Some(url)) => {
                    for node in &project_nodes {
                        if let Err(e) = db::mark_node_projected_with_pr(conn, &node.id, &url) {
                            eprintln!("  {} marking node projected: {e}", "Warning".yellow());
                        }
                    }
                    projected_count += project_nodes.len();
                    println!("  {} PR: {}", "Created".green(), url.cyan());
                }
                Ok(None) => {
                    for node in &project_nodes {
                        if let Err(e) = db::mark_node_projected(conn, &node.id) {
                            eprintln!("  {} marking node projected: {e}", "Warning".yellow());
                        }
                    }
                    projected_count += project_nodes.len();
                    println!("  {} committed to branch (no gh for PR)", "Projected".yellow());
                }
                Err(e) => {
                    eprintln!("  {} PR creation: {e}", "Error".red());
                }
            }
        }

        if projected_count > 0 {
            println!("  {} {} nodes projected", "Done:".green(), projected_count);
        }

        // Regenerate briefing for this project
        if projected_count > 0 {
            let project_slug = retro_core::db::generate_project_slug(project_path);
            let applied: Vec<String> = unprojected.iter()
                .filter(|n| {
                    // Include nodes that were just projected (check db for updated state)
                    db::get_node(conn, &n.id)
                        .ok()
                        .flatten()
                        .map(|updated| updated.projected_at.is_some())
                        .unwrap_or(false)
                })
                .map(|n| format!("{}: {}", n.node_type, retro_core::util::truncate_str(&n.content, 80)))
                .collect();
            let pending_count = db::get_nodes_by_status(conn, &retro_core::models::NodeStatus::PendingReview)
                .map(|nodes| nodes.iter()
                    .filter(|n| n.project_id.as_deref() == Some(project_slug.as_str()) || n.project_id.is_none())
                    .count())
                .unwrap_or(0);
            let briefing_content = retro_core::briefing::generate_briefing(
                &project_slug, &applied, &[], pending_count,
            );
            let _ = retro_core::briefing::write_briefing(&project_slug, &briefing_content);
            if verbose {
                println!("  {} briefing for {}", "Updated".green(), project_slug);
            }
        }
    }

    // Step 5/6: Syncing PR state
    println!("{}", "Step 5/6: Syncing PR state...".cyan());

    let nodes_with_pr = db::get_nodes_with_pr(conn)?;
    if nodes_with_pr.is_empty() {
        if verbose {
            println!("  {} no open PRs to check", "Skipping:".dimmed());
        }
    } else if dry_run {
        let unique_prs: std::collections::HashSet<&str> = nodes_with_pr.iter()
            .filter_map(|n| n.pr_url.as_deref())
            .collect();
        println!("  {} {} PRs to check", "[dry-run]".yellow(), unique_prs.len());
    } else {
        let unique_prs: std::collections::HashSet<String> = nodes_with_pr.iter()
            .filter_map(|n| n.pr_url.clone())
            .collect();

        for pr_url in &unique_prs {
            match retro_core::git::pr_state(pr_url) {
                Ok(state) => {
                    match state.as_str() {
                        "MERGED" => {
                            db::clear_node_pr(conn, pr_url)?;
                            if verbose {
                                println!("  {} PR merged: {}", "Cleared".green(), pr_url);
                            }
                        }
                        "CLOSED" => {
                            db::dismiss_nodes_for_pr(conn, pr_url)?;
                            println!("  {} PR closed — nodes dismissed: {}", "Dismissed".yellow(), pr_url);
                        }
                        _ => {
                            if verbose {
                                println!("  {} PR open: {}", "Waiting".dimmed(), pr_url);
                            }
                        }
                    }
                }
                Err(e) => {
                    if verbose {
                        eprintln!("  {} checking PR {}: {e}", "Warning".yellow(), pr_url);
                    }
                }
            }
        }
    }

    // Step 6: Summary
    println!("{}", "Step 6/6: Pipeline complete.".cyan());

    if dry_run {
        println!();
        println!("{}", "Dry run -- no changes made.".yellow().bold());
    } else {
        // Show next steps
        let unanalyzed = db::unanalyzed_session_count(conn).unwrap_or(0);
        let has_unprojected = db::has_unprojected_patterns(conn, config.analysis.confidence_threshold)
            .unwrap_or(false);

        println!();
        if unanalyzed > 0 {
            println!(
                "  {} {} unanalyzed sessions remaining",
                "Note:".dimmed(),
                unanalyzed
            );
        }
        if has_unprojected {
            println!(
                "  {} Run {} to generate projections, then {} to review",
                "Next:".white(),
                "retro apply".cyan(),
                "retro review".cyan()
            );
        }
    }

    Ok(())
}

/// Check if there is pending work (unanalyzed sessions or unprojected patterns/nodes).
fn has_pending_work(conn: &db::Connection, config: &Config) -> bool {
    let has_unanalyzed = db::has_unanalyzed_sessions(conn).unwrap_or(false);
    let has_unprojected_v1 = db::has_unprojected_patterns(conn, config.analysis.confidence_threshold)
        .unwrap_or(false);
    let has_unprojected_v2 = db::get_unprojected_nodes(conn)
        .map(|nodes| !nodes.is_empty())
        .unwrap_or(false);
    has_unanalyzed || has_unprojected_v1 || has_unprojected_v2
}

/// Check whether analysis should be triggered based on the configured trigger and threshold.
fn should_trigger_analysis(conn: &db::Connection, config: &Config) -> Result<bool> {
    let trigger = config.runner.analysis_trigger.as_str();
    let threshold = config.runner.analysis_threshold as u64;

    match trigger {
        "sessions" => {
            let unanalyzed = db::unanalyzed_session_count(conn)?;
            Ok(unanalyzed >= threshold)
        }
        "always" => Ok(true),
        _ => {
            // Default to sessions-based trigger
            let unanalyzed = db::unanalyzed_session_count(conn)?;
            Ok(unanalyzed >= threshold)
        }
    }
}

/// Get the AI call count for today from the metadata table.
fn get_ai_call_count(conn: &db::Connection) -> Result<(u32, String)> {
    let date = db::get_metadata(conn, "ai_calls_date")?
        .unwrap_or_default();
    let count = db::get_metadata(conn, "ai_calls_today")?
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    Ok((count, date))
}

/// Increment the AI call count for today.
fn increment_ai_calls(conn: &db::Connection, calls: u32) -> Result<()> {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let (current, date) = get_ai_call_count(conn)?;

    let new_count = if date == today {
        current + calls
    } else {
        calls // New day, reset
    };

    db::set_metadata(conn, "ai_calls_date", &today)?;
    db::set_metadata(conn, "ai_calls_today", &new_count.to_string())?;
    Ok(())
}

/// Update the last_run_at timestamp in metadata.
fn update_last_run(conn: &db::Connection) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    db::set_metadata(conn, "last_run_at", &now)?;
    Ok(())
}

/// Calculate how many AI call batches can run given the remaining budget.
/// Returns 0 if budget is exhausted.
fn batches_within_budget(total_batches: usize, budget_remaining: u32) -> usize {
    if budget_remaining == 0 {
        return 0;
    }
    total_batches.min(budget_remaining as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batches_within_budget_zero_budget() {
        // Bug: when budget is 0, pipeline should still run projection/sync
        // but analysis should get 0 batches
        assert_eq!(batches_within_budget(4, 0), 0);
    }

    #[test]
    fn test_batches_within_budget_sufficient() {
        // Budget covers all batches
        assert_eq!(batches_within_budget(4, 10), 4);
    }

    #[test]
    fn test_batches_within_budget_limited() {
        // Bug: 4 batches but only 2 budget remaining should cap at 2, not run all 4
        assert_eq!(batches_within_budget(4, 2), 2);
    }

    #[test]
    fn test_batches_within_budget_exact() {
        assert_eq!(batches_within_budget(3, 3), 3);
    }

    #[test]
    fn test_batches_within_budget_one_remaining() {
        assert_eq!(batches_within_budget(5, 1), 1);
    }
}
