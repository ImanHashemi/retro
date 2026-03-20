use anyhow::Result;
use chrono::Utc;
use colored::Colorize;
use retro_core::analysis;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::ingest;
use retro_core::observer;

use super::git_root_or_cwd;

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
    let project_path = git_root_or_cwd()?;

    // Step 1: Observe — find modified sessions
    println!("{}", "Step 1/4: Observing session changes...".cyan());
    let claude_dir = config.claude_dir();
    let last_run = db::get_metadata(&conn, "last_run_at")?;
    let since = last_run.as_ref().and_then(|ts| {
        chrono::DateTime::parse_from_rfc3339(ts)
            .ok()
            .map(|dt| {
                std::time::SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_secs(dt.timestamp() as u64)
            })
    });

    let modified = observer::find_modified_sessions(&claude_dir, since);
    println!(
        "  {} modified session file{}",
        modified.len().to_string().cyan(),
        if modified.len() == 1 { "" } else { "s" }
    );

    if modified.is_empty() && !has_pending_work(&conn, &config) {
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
    println!("{}", "Step 2/4: Ingesting sessions...".cyan());
    if dry_run {
        println!(
            "  {} {} modified session files to ingest",
            "[dry-run]".yellow(),
            modified.len()
        );
    } else {
        let ingest_result = ingest::ingest_project(&conn, &config, &project_path)?;
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
    println!("{}", "Step 3/4: Checking analysis trigger...".cyan());

    // Check AI call budget
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let (ai_calls_today, ai_calls_date) = get_ai_call_count(&conn)?;
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
        update_last_run(&conn)?;
        return Ok(());
    }

    if verbose {
        eprintln!(
            "[verbose] AI budget: {}/{} calls remaining today",
            budget_remaining, config.runner.max_ai_calls_per_day
        );
    }

    // Check if analysis is needed based on trigger
    let should_analyze = should_trigger_analysis(&conn, &config)?;

    if !should_analyze {
        println!(
            "  {} analysis threshold not met (need {} unanalyzed sessions)",
            "Skipping:".dimmed(),
            config.runner.analysis_threshold
        );
    } else {
        println!(
            "  {} analysis threshold met",
            "Triggering:".green()
        );

        if dry_run {
            let unanalyzed = db::unanalyzed_session_count(&conn)?;
            println!(
                "  {} would analyze {} unanalyzed sessions",
                "[dry-run]".yellow(),
                unanalyzed
            );
        } else {
            // Run analysis (v1 for now)
            println!("  {}", "Running AI-powered analysis...".dimmed());
            let window_days = config.analysis.window_days;
            let result = analysis::analyze(
                &conn,
                &config,
                Some(project_path.as_str()),
                window_days,
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
                "  {} sessions analyzed, {} new patterns, {} updated",
                result.sessions_analyzed.to_string().cyan(),
                result.new_patterns.to_string().green(),
                result.updated_patterns.to_string().yellow()
            );

            if verbose {
                eprintln!(
                    "[verbose] tokens: {} in / {} out",
                    result.input_tokens, result.output_tokens
                );
            }

            // Track AI calls
            let calls_used = result.batch_details.len() as u32;
            increment_ai_calls(&conn, calls_used)?;
        }
    }

    // Step 4: Summary
    println!("{}", "Step 4/4: Pipeline complete.".cyan());

    if dry_run {
        println!();
        println!("{}", "Dry run -- no changes made.".yellow().bold());
    } else {
        // Update last_run_at
        update_last_run(&conn)?;

        // Show next steps
        let unanalyzed = db::unanalyzed_session_count(&conn).unwrap_or(0);
        let has_unprojected = db::has_unprojected_patterns(&conn, config.analysis.confidence_threshold)
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

/// Check if there is pending work (unanalyzed sessions or unprojected patterns).
fn has_pending_work(conn: &db::Connection, config: &Config) -> bool {
    let has_unanalyzed = db::has_unanalyzed_sessions(conn).unwrap_or(false);
    let has_unprojected = db::has_unprojected_patterns(conn, config.analysis.confidence_threshold)
        .unwrap_or(false);
    has_unanalyzed || has_unprojected
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
