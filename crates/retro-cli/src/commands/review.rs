use anyhow::Result;
use colored::Colorize;
use retro_core::audit_log;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::models::{ApplyAction, ApplyPlan, ApplyTrack, PatternStatus, ProjectionStatus, SuggestedTarget};
use retro_core::projection;
use retro_core::util::shorten_path;

use super::git_root_or_cwd;

/// User's decision for a pending review item.
#[derive(Debug, Clone, PartialEq)]
enum ReviewAction {
    Apply,
    Skip,
    Dismiss,
}

pub fn run(global: bool, dry_run: bool, verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
    let audit_path = dir.join("audit.jsonl");

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    // Run sync first to clean up closed PRs
    let _ = super::sync::run_sync(&conn, &audit_path, verbose);

    // Get pending review items
    let pending = db::get_pending_review_projections(&conn)?;

    if pending.is_empty() {
        println!("{}", "No items pending review.".dimmed());
        return Ok(());
    }

    // Also fetch the patterns for display
    let all_patterns = db::get_all_patterns(&conn, None)?;
    let pattern_map: std::collections::HashMap<String, _> = all_patterns
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect();

    // Display numbered list
    println!();
    println!(
        "Pending review ({} items):",
        pending.len().to_string().cyan()
    );
    println!();

    for (i, proj) in pending.iter().enumerate() {
        let num = format!("  {}.", i + 1);
        let target_label = match proj.target_type.as_str() {
            "skill" => "[skill]".to_string(),
            "claude_md" => {
                if projection::is_edit_action(&proj.content) {
                    if let Some(edit) = projection::parse_edit(&proj.content) {
                        match edit.edit_type {
                            retro_core::models::ClaudeMdEditType::Add => "[rule+]".to_string(),
                            retro_core::models::ClaudeMdEditType::Remove => "[rule-]".to_string(),
                            retro_core::models::ClaudeMdEditType::Reword => "[rule~]".to_string(),
                            retro_core::models::ClaudeMdEditType::Move => "[rule>]".to_string(),
                        }
                    } else {
                        "[rule] ".to_string()
                    }
                } else {
                    "[rule+]".to_string()
                }
            }
            "global_agent" => "[agent]".to_string(),
            _ => "[item] ".to_string(),
        };

        let description = pattern_map
            .get(&proj.pattern_id)
            .map(|p| p.description.as_str())
            .unwrap_or("(unknown pattern)");

        let confidence = pattern_map
            .get(&proj.pattern_id)
            .map(|p| p.confidence)
            .unwrap_or(0.0);

        let times_seen = pattern_map
            .get(&proj.pattern_id)
            .map(|p| p.times_seen)
            .unwrap_or(0);

        println!(
            "{} {} {}",
            num.white().bold(),
            target_label.dimmed(),
            description.white()
        );
        println!(
            "     Target: {}",
            shorten_path(&proj.target_path).dimmed()
        );
        println!(
            "     Seen {} times (confidence: {:.2})",
            times_seen.to_string().cyan(),
            confidence
        );
        println!();
    }

    if dry_run {
        println!(
            "{}",
            "Dry run — no actions taken. Run `retro review` to make decisions.".yellow().bold()
        );
        return Ok(());
    }

    // Parse user input
    println!(
        "{}",
        "Actions: apply (a), skip (s), dismiss (d), preview (p)".dimmed()
    );
    print!(
        "{} ",
        "Enter selections (e.g., \"1a 2a 3d\" or \"all:a\"):".yellow().bold()
    );
    use std::io::Write;
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim().to_string();

    if input.is_empty() {
        println!("{}", "No selections made.".dimmed());
        return Ok(());
    }

    // Handle preview requests first
    let tokens: Vec<&str> = input.split_whitespace().collect();
    for token in &tokens {
        if token.ends_with('p') || token.ends_with('P') {
            let num_str = &token[..token.len() - 1];
            if let Ok(num) = num_str.parse::<usize>() {
                if num >= 1 && num <= pending.len() {
                    let proj = &pending[num - 1];
                    println!();
                    println!("{}", format!("--- Preview: item {} ---", num).cyan().bold());
                    println!("{}", &proj.content);
                    println!("{}", "--- End preview ---".cyan());
                    println!();
                }
            }
        }
    }

    // Re-prompt after preview if only previews were requested
    let has_non_preview = tokens.iter().any(|t| {
        let last = t.chars().last().unwrap_or(' ');
        matches!(last, 'a' | 'A' | 's' | 'S' | 'd' | 'D')
    });

    let final_input = if !has_non_preview {
        // Only previews were requested — re-prompt
        print!(
            "{} ",
            "Enter selections (e.g., \"1a 2a 3d\" or \"all:a\"):".yellow().bold()
        );
        std::io::stdout().flush()?;
        let mut new_input = String::new();
        std::io::stdin().read_line(&mut new_input)?;
        let trimmed = new_input.trim().to_string();
        if trimmed.is_empty() {
            println!("{}", "No selections made.".dimmed());
            return Ok(());
        }
        trimmed
    } else {
        input
    };

    // Parse actions
    let mut decisions: Vec<(usize, ReviewAction)> = Vec::new();

    for token in final_input.split_whitespace() {
        if token.starts_with("all:") {
            let action_char = token.chars().last().unwrap_or(' ');
            let action = match action_char {
                'a' | 'A' => ReviewAction::Apply,
                's' | 'S' => ReviewAction::Skip,
                'd' | 'D' => ReviewAction::Dismiss,
                _ => continue,
            };
            for i in 0..pending.len() {
                decisions.push((i, action.clone()));
            }
            break;
        }

        if token.len() < 2 {
            continue;
        }

        let action_char = token.chars().last().unwrap_or(' ');
        let num_str = &token[..token.len() - 1];

        let action = match action_char {
            'a' | 'A' => ReviewAction::Apply,
            's' | 'S' => ReviewAction::Skip,
            'd' | 'D' => ReviewAction::Dismiss,
            'p' | 'P' => continue, // Already handled previews
            _ => continue,
        };

        if let Ok(num) = num_str.parse::<usize>() {
            if num >= 1 && num <= pending.len() {
                decisions.push((num - 1, action));
            }
        }
    }

    if decisions.is_empty() {
        println!("{}", "No valid selections.".dimmed());
        return Ok(());
    }

    // Execute decisions
    let project = if global {
        None
    } else {
        Some(git_root_or_cwd()?)
    };

    let mut applied_projections = Vec::new();
    let mut dismissed_patterns = Vec::new();
    let mut skipped = 0;

    for (idx, action) in &decisions {
        let proj = &pending[*idx];

        match action {
            ReviewAction::Apply => {
                applied_projections.push(proj.clone());
            }
            ReviewAction::Skip => {
                skipped += 1;
            }
            ReviewAction::Dismiss => {
                // Delete projection and mark pattern as Dismissed
                db::delete_projection(&conn, &proj.id)?;
                db::update_pattern_status(&conn, &proj.pattern_id, &PatternStatus::Dismissed)?;
                dismissed_patterns.push(proj.pattern_id.clone());
            }
        }
    }

    // Execute approved items
    if !applied_projections.is_empty() {
        // Build an ApplyPlan from the approved projections
        let actions: Vec<ApplyAction> = applied_projections
            .iter()
            .map(|proj| {
                let target_type = SuggestedTarget::from_str(&proj.target_type);
                let track = match target_type {
                    SuggestedTarget::GlobalAgent => ApplyTrack::Personal,
                    _ => ApplyTrack::Shared,
                };
                let description = pattern_map
                    .get(&proj.pattern_id)
                    .map(|p| p.description.clone())
                    .unwrap_or_default();
                ApplyAction {
                    pattern_id: proj.pattern_id.clone(),
                    pattern_description: description,
                    target_type,
                    target_path: proj.target_path.clone(),
                    content: proj.content.clone(),
                    track,
                }
            })
            .collect();

        let plan = ApplyPlan { actions };

        let mut total_files = 0;
        let mut total_patterns = 0;
        let mut pr_url: Option<String> = None;

        // Phase 1: Personal actions
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

        // Phase 2: Shared actions with PR
        let has_shared = !plan.shared_actions().is_empty();
        if has_shared {
            let shared_result = super::apply::execute_shared_with_pr(
                &conn, &config, &plan, project.as_deref(), false,
            )?;
            total_files += shared_result.files_written;
            total_patterns += shared_result.patterns_activated;
            pr_url = shared_result.pr_url;
        }

        // Update the pending_review projections to applied
        for proj in &applied_projections {
            db::update_projection_status(&conn, &proj.id, &ProjectionStatus::Applied)?;
            if let Some(ref url) = pr_url {
                // Update pr_url on shared projections
                let target_type = proj.target_type.as_str();
                if target_type == "skill" || target_type == "claude_md" {
                    db::update_projection_pr_url(&conn, &proj.id, url)?;
                }
            }
        }

        // Audit
        let applied_pattern_ids: Vec<_> = applied_projections.iter().map(|p| p.pattern_id.clone()).collect();
        audit_log::append(
            &audit_path,
            "review_applied",
            serde_json::json!({
                "patterns": applied_pattern_ids,
                "files_written": total_files,
                "patterns_activated": total_patterns,
                "pr_url": pr_url,
            }),
        )?;

        println!();
        println!("{}", "Review complete!".green().bold());
        println!("  {} {}", "Applied:".white(), applied_projections.len().to_string().green());
        if total_files > 0 {
            println!("  {} {}", "Files written:".white(), total_files.to_string().green());
        }
        if let Some(url) = &pr_url {
            println!("  {} {}", "Pull request:".white(), url.cyan());
        }
    }

    if !dismissed_patterns.is_empty() {
        audit_log::append(
            &audit_path,
            "review_dismissed",
            serde_json::json!({ "patterns": dismissed_patterns }),
        )?;
        println!("  {} {}", "Dismissed:".white(), dismissed_patterns.len().to_string().yellow());
    }

    if skipped > 0 {
        println!("  {} {}", "Skipped:".white(), skipped.to_string().dimmed());
    }

    Ok(())
}
