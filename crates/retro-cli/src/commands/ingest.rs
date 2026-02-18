use anyhow::Result;
use colored::Colorize;
use retro_core::analysis;
use retro_core::audit_log;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::ingest;
use retro_core::lock::LockFile;

use retro_core::util::shorten_path;

use super::{git_root_or_cwd, within_cooldown};

pub fn run(global: bool, auto: bool, verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
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
        let audit_path = dir.join("audit.jsonl");

        // Scope the lock so it's released after ingest completes,
        // before orchestrating analyze and apply (which acquire their own locks).
        {
            let _lock = match LockFile::try_acquire(&lock_path) {
                Some(lock) => lock,
                None => {
                    if verbose {
                        eprintln!("[verbose] skipping ingest: another process holds the lock");
                    }
                    return Ok(());
                }
            };

            // Check cooldown: skip if ingested within ingest_cooldown_minutes
            if let Ok(Some(ref last)) = db::last_ingested_at(&conn) {
                if within_cooldown(last, config.hooks.ingest_cooldown_minutes) {
                    if verbose {
                        eprintln!(
                            "[verbose] skipping ingest: within cooldown ({}m)",
                            config.hooks.ingest_cooldown_minutes
                        );
                    }
                    return Ok(());
                }
            }

            // Run ingestion silently — any error exits quietly
            let result = if global {
                ingest::ingest_all_projects(&conn, &config)
            } else {
                let project_path = git_root_or_cwd()?;
                ingest::ingest_project(&conn, &config, &project_path)
            };

            match result {
                Ok(r) => {
                    if verbose {
                        eprintln!(
                            "[verbose] ingested {} sessions ({} skipped)",
                            r.sessions_ingested, r.sessions_skipped
                        );
                    }
                    // Audit: ingest success
                    let _ = audit_log::append(
                        &audit_path,
                        "ingest",
                        serde_json::json!({
                            "sessions_ingested": r.sessions_ingested,
                            "sessions_skipped": r.sessions_skipped,
                            "auto": true,
                        }),
                    );
                }
                Err(e) => {
                    if verbose {
                        eprintln!("[verbose] ingest error: {e}");
                    }
                }
            }
        } // _lock dropped here — released after ingest

        // --- Orchestration: chain analyze and apply if auto_apply enabled ---
        if config.hooks.auto_apply {
            // Re-acquire lock for the orchestration phase (analyze + apply).
            // This prevents concurrent orchestration from two rapid commits.
            let _orch_lock = match LockFile::try_acquire(&lock_path) {
                Some(lock) => lock,
                None => {
                    if verbose {
                        eprintln!("[verbose] orchestrator: another process holds the lock, skipping");
                    }
                    return Ok(());
                }
            };

            let project = if global {
                None
            } else {
                match git_root_or_cwd() {
                    Ok(p) => Some(p),
                    Err(e) => {
                        if verbose {
                            eprintln!("[verbose] orchestrator: could not resolve project path: {e}");
                        }
                        return Ok(());
                    }
                }
            };

            // Check analyze conditions: un-analyzed sessions + cooldown elapsed
            let should_analyze = db::has_unanalyzed_sessions(&conn).unwrap_or(false)
                && match db::last_analyzed_at(&conn) {
                    Ok(Some(ref last)) => {
                        !within_cooldown(last, config.hooks.analyze_cooldown_minutes)
                    }
                    Ok(None) => true, // never analyzed before
                    Err(_) => false,
                };

            if should_analyze {
                // Check session cap for auto mode
                let unanalyzed_count = db::unanalyzed_session_count(&conn).unwrap_or(0);
                let cap = config.hooks.auto_analyze_max_sessions;

                if unanalyzed_count > cap as u64 {
                    if verbose {
                        eprintln!(
                            "[verbose] orchestrator: skipping analyze — {} unanalyzed sessions exceeds auto limit ({})",
                            unanalyzed_count, cap
                        );
                    }
                    let _ = audit_log::append(
                        &audit_path,
                        "analyze_skipped",
                        serde_json::json!({
                            "reason": "session_cap",
                            "unanalyzed_count": unanalyzed_count,
                            "cap": cap,
                            "auto": true,
                        }),
                    );
                } else {
                    if verbose {
                        eprintln!("[verbose] orchestrator: running analyze");
                    }

                    let window_days = config.analysis.window_days;

                    match analysis::analyze(&conn, &config, project.as_deref(), window_days) {
                        Ok(result) => {
                            if verbose {
                                eprintln!(
                                    "[verbose] analyze complete: {} patterns ({} new, {} updated)",
                                    result.total_patterns, result.new_patterns, result.updated_patterns
                                );
                            }
                            // Record audit log for analyze (best-effort)
                            if result.sessions_analyzed > 0 {
                                let audit_details = serde_json::json!({
                                    "sessions_analyzed": result.sessions_analyzed,
                                    "new_patterns": result.new_patterns,
                                    "updated_patterns": result.updated_patterns,
                                    "total_patterns": result.total_patterns,
                                    "input_tokens": result.input_tokens,
                                    "output_tokens": result.output_tokens,
                                    "window_days": window_days,
                                    "global": global,
                                    "project": &project,
                                    "auto": true,
                                    "orchestrated": true,
                                });
                                let _ = audit_log::append(&audit_path, "analyze", audit_details);
                            }
                        }
                        Err(e) => {
                            if verbose {
                                eprintln!("[verbose] analyze error: {e}");
                            }
                            let _ = audit_log::append(
                                &audit_path,
                                "analyze_error",
                                serde_json::json!({
                                    "error": e.to_string(),
                                    "auto": true,
                                }),
                            );
                        }
                    }
                }
            } else {
                if verbose {
                    eprintln!("[verbose] orchestrator: skipping analyze (no unanalyzed sessions or within cooldown)");
                }
                let _ = audit_log::append(
                    &audit_path,
                    "analyze_skipped",
                    serde_json::json!({
                        "reason": "cooldown_or_no_data",
                        "auto": true,
                    }),
                );
            }

            // Check apply conditions: un-projected patterns + cooldown elapsed
            let should_apply = db::has_unprojected_patterns(&conn, config.analysis.confidence_threshold).unwrap_or(false)
                && match db::last_applied_at(&conn) {
                    Ok(Some(ref last)) => {
                        !within_cooldown(last, config.hooks.apply_cooldown_minutes)
                    }
                    Ok(None) => true, // never applied before
                    Err(_) => false,
                };

            if should_apply {
                if verbose {
                    eprintln!("[verbose] orchestrator: running apply");
                }
                // Drop orchestration lock before calling apply (which acquires its own lock)
                drop(_orch_lock);

                match super::apply::run_apply(
                    global,
                    false,
                    true,
                    super::apply::DisplayMode::Plan { dry_run: false },
                    verbose,
                ) {
                    Ok(()) => {}
                    Err(e) => {
                        if verbose {
                            eprintln!("[verbose] apply error: {e}");
                        }
                    }
                }
            } else {
                if verbose {
                    eprintln!("[verbose] orchestrator: skipping apply (no unprojected patterns or within cooldown)");
                }
                let _ = audit_log::append(
                    &audit_path,
                    "apply_skipped",
                    serde_json::json!({
                        "reason": "no_qualifying_patterns",
                        "auto": true,
                    }),
                );
            }
        } else if verbose {
            eprintln!("[verbose] orchestrator: auto_apply not enabled");
        }

        return Ok(());
    }

    // Interactive mode
    let result = if global {
        println!("{}", "Ingesting all projects...".cyan());
        ingest::ingest_all_projects(&conn, &config)?
    } else {
        let project_path = git_root_or_cwd()?;
        if verbose {
            eprintln!("[verbose] project path: {}", project_path);
        }
        println!(
            "{} {}",
            "Ingesting project:".cyan(),
            shorten_path(&project_path).white()
        );
        ingest::ingest_project(&conn, &config, &project_path)?
    };

    // Print results
    println!();
    println!(
        "  {} {}",
        "Sessions found:".white(),
        result.sessions_found.to_string().cyan()
    );
    println!(
        "  {} {}",
        "Sessions ingested:".white(),
        result.sessions_ingested.to_string().green()
    );
    println!(
        "  {} {}",
        "Sessions skipped:".white(),
        result.sessions_skipped.to_string().yellow()
    );

    if !result.errors.is_empty() {
        println!(
            "  {} {}",
            "Errors:".white(),
            result.errors.len().to_string().red()
        );
        for err in &result.errors {
            eprintln!("    {}", err.red());
        }
    }

    Ok(())
}
