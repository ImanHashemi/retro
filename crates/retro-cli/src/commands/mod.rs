pub mod analyze;
pub mod apply;
pub mod audit;
pub mod clean;
pub mod diff;
pub mod hooks;
pub mod ingest;
pub mod init;
pub mod log;
pub mod patterns;
pub mod status;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

/// Get git repository root, falling back to current directory.
pub fn git_root_or_cwd() -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Ok(root)
        }
        _ => {
            let cwd = std::env::current_dir().context("getting current directory")?;
            Ok(cwd.to_string_lossy().to_string())
        }
    }
}

/// Check for auto-created PRs and display a one-time nudge.
/// Silently does nothing if DB doesn't exist or any error occurs.
pub fn check_and_display_nudge() {
    let dir = retro_core::config::retro_dir();
    let db_path = dir.join("retro.db");
    if !db_path.exists() {
        return;
    }

    let conn = match retro_core::db::open_db(&db_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let urls = match retro_core::db::get_unnudged_pr_urls(&conn) {
        Ok(u) => u,
        Err(_) => return,
    };

    if urls.is_empty() {
        return;
    }

    use colored::Colorize;
    for url in &urls {
        println!(
            "  {} {}",
            "retro auto-created a PR:".yellow(),
            url.cyan().underline()
        );
    }
    println!();

    // Mark as nudged so we don't show again
    let _ = retro_core::db::mark_projections_nudged(&conn);
}

/// Check if a timestamp (RFC 3339) is within the cooldown window.
/// Returns true if the action should be skipped (i.e., within cooldown).
pub fn within_cooldown(last_rfc3339: &str, cooldown_minutes: u32) -> bool {
    if let Ok(last_time) = DateTime::parse_from_rfc3339(last_rfc3339) {
        let last_utc = last_time.with_timezone(&Utc);
        let cooldown = chrono::Duration::minutes(cooldown_minutes as i64);
        Utc::now() - last_utc < cooldown
    } else {
        false
    }
}
