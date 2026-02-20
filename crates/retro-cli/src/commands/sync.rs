use anyhow::Result;
use colored::Colorize;
use retro_core::audit_log;
use retro_core::config::retro_dir;
use retro_core::db;
use retro_core::git;
use retro_core::models::PatternStatus;

/// Run sync: check PR status for applied projections and reset patterns from closed PRs.
pub fn run(verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let db_path = dir.join("retro.db");
    let audit_path = dir.join("audit.jsonl");

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let conn = db::open_db(&db_path)?;

    let reset_count = run_sync(&conn, &audit_path, verbose)?;

    if reset_count > 0 {
        println!(
            "{}",
            format!("Reset {} pattern(s) from closed PRs back to discoverable.", reset_count)
                .green()
        );
    } else if verbose {
        println!("{}", "No closed PRs found — nothing to sync.".dimmed());
    }

    Ok(())
}

/// Core sync logic — callable from other commands (review, apply).
/// Returns the number of patterns reset.
pub fn run_sync(
    conn: &db::Connection,
    audit_path: &std::path::Path,
    verbose: bool,
) -> Result<usize> {
    if !git::is_gh_available() {
        if verbose {
            eprintln!("[verbose] sync: gh CLI not available, skipping");
        }
        return Ok(0);
    }

    let projections = db::get_applied_projections_with_pr(conn)?;
    if projections.is_empty() {
        return Ok(0);
    }

    // Dedupe by PR URL
    let mut seen_urls = std::collections::HashSet::new();
    let mut pr_urls: Vec<String> = Vec::new();
    for proj in &projections {
        if let Some(ref url) = proj.pr_url {
            if seen_urls.insert(url.clone()) {
                pr_urls.push(url.clone());
            }
        }
    }

    let mut reset_count = 0;

    for url in &pr_urls {
        let state = match git::pr_state(url) {
            Ok(s) => s,
            Err(e) => {
                if verbose {
                    eprintln!("[verbose] sync: failed to check PR {url}: {e}");
                }
                continue;
            }
        };

        if state == "CLOSED" {
            // Find all projections for this PR URL
            let affected: Vec<_> = projections
                .iter()
                .filter(|p| p.pr_url.as_deref() == Some(url.as_str()))
                .collect();

            let mut pattern_ids = Vec::new();
            for proj in &affected {
                // Delete the projection
                db::delete_projection(conn, &proj.id)?;
                // Reset pattern to Discovered
                db::update_pattern_status(conn, &proj.pattern_id, &PatternStatus::Discovered)?;
                pattern_ids.push(proj.pattern_id.clone());
                reset_count += 1;
            }

            // Audit log
            let _ = audit_log::append(
                audit_path,
                "sync_reset",
                serde_json::json!({
                    "patterns": pattern_ids,
                    "pr_url": url,
                }),
            );

            if verbose {
                eprintln!("[verbose] sync: reset {} patterns from closed PR {url}", affected.len());
            }
        }
    }

    Ok(reset_count)
}
