use std::path::Path;

use anyhow::Result;
use chrono::{Duration, Utc};
use colored::Colorize;
use retro_core::analysis;
use retro_core::audit_log;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::ingest;
use retro_core::ingest::session;
use retro_core::lock::LockFile;
use retro_core::util;

use super::{git_root_or_cwd, within_cooldown};

pub fn run(global: bool, since_days: Option<u32>, auto: bool, dry_run: bool, verbose: bool) -> Result<()> {
    if dry_run && auto {
        anyhow::bail!("--dry-run and --auto cannot be used together");
    }

    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
    let audit_path = dir.join("audit.jsonl");
    let lock_path = dir.join("retro.lock");

    // Check initialization
    if !db_path.exists() {
        if auto {
            return Ok(());
        }
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    // In auto mode: acquire lockfile silently, check cooldown
    if auto {
        let _lock = match LockFile::try_acquire(&lock_path) {
            Some(lock) => lock,
            None => {
                if verbose {
                    eprintln!("[verbose] skipping analyze: another process holds the lock");
                }
                return Ok(());
            }
        };

        // Check cooldown: skip if analyzed within auto_cooldown_minutes
        if let Ok(Some(ref last)) = db::last_analyzed_at(&conn) {
            if within_cooldown(last, config.hooks.auto_cooldown_minutes) {
                if verbose {
                    eprintln!(
                        "[verbose] skipping analyze: within cooldown ({}m)",
                        config.hooks.auto_cooldown_minutes
                    );
                }
                return Ok(());
            }
        }

        let project = if global {
            None
        } else {
            Some(git_root_or_cwd()?)
        };

        let window_days = since_days.unwrap_or(config.analysis.window_days);

        // Run ingestion silently
        let ingest_result = if global {
            ingest::ingest_all_projects(&conn, &config)
        } else {
            ingest::ingest_project(&conn, &config, project.as_deref().unwrap())
        };
        if let Err(e) = &ingest_result {
            if verbose {
                eprintln!("[verbose] ingest error (continuing to analyze): {e}");
            }
        }

        // Run analysis silently
        match analysis::analyze(&conn, &config, project.as_deref(), window_days) {
            Ok(result) => {
                if result.sessions_analyzed > 0 {
                    // Record audit log even in auto mode
                    let audit_details = serde_json::json!({
                        "sessions_analyzed": result.sessions_analyzed,
                        "new_patterns": result.new_patterns,
                        "updated_patterns": result.updated_patterns,
                        "total_patterns": result.total_patterns,
                        "ai_cost_usd": result.ai_cost,
                        "window_days": window_days,
                        "global": global,
                        "project": project,
                        "auto": true,
                    });
                    let _ = audit_log::append(&audit_path, "analyze", audit_details);
                }
                if verbose {
                    eprintln!(
                        "[verbose] analyzed {} sessions, {} new patterns, {} updated",
                        result.sessions_analyzed, result.new_patterns, result.updated_patterns
                    );
                }
            }
            Err(e) => {
                if verbose {
                    eprintln!("[verbose] analyze error: {e}");
                }
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

    let window_days = since_days.unwrap_or(config.analysis.window_days);

    if verbose {
        if let Some(ref p) = project {
            eprintln!("[verbose] project path: {}", p);
        }
        eprintln!("[verbose] window: {} days", window_days);
    }

    // Step 1: Run ingestion first
    println!("{}", "Step 1/3: Ingesting new sessions...".cyan());
    let ingest_result = if global {
        ingest::ingest_all_projects(&conn, &config)?
    } else {
        ingest::ingest_project(&conn, &config, project.as_deref().unwrap())?
    };

    if ingest_result.sessions_ingested > 0 {
        println!(
            "  {} new sessions ingested",
            ingest_result.sessions_ingested.to_string().green()
        );
    }

    // Dry-run: show preview of what would be analyzed, then return
    if dry_run {
        return print_dry_run_preview(&conn, project.as_deref(), window_days, verbose);
    }

    // Step 2: Run analysis
    println!(
        "{}",
        format!("Step 2/3: Analyzing sessions (window: {}d)...", window_days).cyan()
    );
    println!(
        "  {}",
        "This may take a minute (AI-powered analysis)...".dimmed()
    );

    let result = analysis::analyze(&conn, &config, project.as_deref(), window_days)?;

    if result.sessions_analyzed == 0 {
        println!(
            "  {}",
            "No new sessions to analyze within the time window.".yellow()
        );
        return Ok(());
    }

    // Step 3: Audit log
    println!("{}", "Step 3/3: Recording audit log...".cyan());
    let audit_details = serde_json::json!({
        "sessions_analyzed": result.sessions_analyzed,
        "new_patterns": result.new_patterns,
        "updated_patterns": result.updated_patterns,
        "total_patterns": result.total_patterns,
        "ai_cost_usd": result.ai_cost,
        "window_days": window_days,
        "global": global,
        "project": project,
    });
    audit_log::append(&audit_path, "analyze", audit_details)?;

    // Print results
    println!();
    println!("{}", "Analysis complete!".green().bold());
    println!(
        "  {} {}",
        "Sessions analyzed:".white(),
        result.sessions_analyzed.to_string().cyan()
    );
    println!(
        "  {} {}",
        "New patterns:".white(),
        result.new_patterns.to_string().green()
    );
    println!(
        "  {} {}",
        "Updated patterns:".white(),
        result.updated_patterns.to_string().yellow()
    );
    println!(
        "  {} {}",
        "Total patterns:".white(),
        result.total_patterns.to_string().cyan()
    );
    println!(
        "  {} ${:.4}",
        "AI cost:".white(),
        result.ai_cost
    );

    if result.new_patterns > 0 || result.updated_patterns > 0 {
        println!();
        println!(
            "Run {} to see discovered patterns.",
            "retro patterns".cyan()
        );
    }

    Ok(())
}

fn print_dry_run_preview(
    conn: &retro_core::db::Connection,
    project: Option<&str>,
    window_days: u32,
    verbose: bool,
) -> Result<()> {
    let since = Utc::now() - Duration::days(window_days as i64);
    let sessions_to_analyze = db::get_sessions_for_analysis(conn, project, &since)?;

    if sessions_to_analyze.is_empty() {
        println!();
        println!(
            "  {}",
            "No new sessions to analyze within the time window.".yellow()
        );
        println!();
        println!("{}", "Dry run — no AI calls made.".yellow().bold());
        return Ok(());
    }

    // Re-parse sessions from disk to get message/error counts
    println!();
    println!("{}", "Sessions to analyze:".white().bold());
    let mut total_user_msgs = 0;
    let mut total_assistant_msgs = 0;
    let mut total_errors = 0;

    for ingested in &sessions_to_analyze {
        let path = Path::new(&ingested.session_path);
        if !path.exists() {
            println!(
                "  {} {} {}",
                "-".dimmed(),
                ingested.session_id.cyan(),
                "(file missing)".red()
            );
            continue;
        }

        match session::parse_session_file(path, &ingested.session_id, &ingested.project) {
            Ok(s) => {
                let user_count = s.user_messages.len();
                let assistant_count = s.assistant_messages.len();
                let error_count = s.errors.len();
                total_user_msgs += user_count;
                total_assistant_msgs += assistant_count;
                total_errors += error_count;

                let project_label = &ingested.project;
                let detail = format!(
                    "{} user, {} assistant msgs{}",
                    user_count,
                    assistant_count,
                    if error_count > 0 {
                        format!(", {} errors", error_count)
                    } else {
                        String::new()
                    }
                );
                println!(
                    "  {} {} {} ({})",
                    "-".dimmed(),
                    util::truncate_str(&ingested.session_id, 8).cyan(),
                    project_label.dimmed(),
                    detail.dimmed()
                );

                if verbose {
                    eprintln!(
                        "[verbose]   path: {}, size: {} bytes",
                        ingested.session_path, ingested.file_size
                    );
                }
            }
            Err(e) => {
                println!(
                    "  {} {} {}",
                    "-".dimmed(),
                    ingested.session_id.cyan(),
                    format!("(parse error: {e})").red()
                );
            }
        }
    }

    // Existing patterns
    let existing = db::get_patterns(conn, &["discovered", "active"], project)?;
    let batch_count =
        (sessions_to_analyze.len() + analysis::BATCH_SIZE - 1) / analysis::BATCH_SIZE;

    println!();
    println!("{}", "Summary:".white().bold());
    println!(
        "  {} {}",
        "Sessions:".white(),
        sessions_to_analyze.len().to_string().cyan()
    );
    println!(
        "  {} {} user, {} assistant",
        "Messages:".white(),
        total_user_msgs.to_string().cyan(),
        total_assistant_msgs.to_string().cyan()
    );
    if total_errors > 0 {
        println!(
            "  {} {}",
            "Errors:".white(),
            total_errors.to_string().yellow()
        );
    }
    println!(
        "  {} {}",
        "Existing patterns:".white(),
        existing.len().to_string().cyan()
    );
    println!(
        "  {} {} (batch size: {})",
        "AI calls:".white(),
        batch_count.to_string().cyan(),
        analysis::BATCH_SIZE
    );

    println!();
    println!("{}", "Dry run — no AI calls made.".yellow().bold());
    Ok(())
}
