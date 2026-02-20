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
pub mod sync;

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

/// Summary of one auto-mode run (entries within 60s window).
struct AutoRunSummary {
    timestamp: DateTime<Utc>,
    sessions_ingested: Option<u64>,
    sessions_skipped: Option<u64>,
    sessions_analyzed: Option<u64>,
    new_patterns: Option<u64>,
    actions_applied: Option<u64>,
    pr_url: Option<String>,
    analyze_skipped_reason: Option<String>,
    unanalyzed_count: Option<u64>,
    session_cap: Option<u64>,
    errors: Vec<String>,
}

/// Group audit entries into auto-run summaries (entries within 60s = one run).
fn aggregate_auto_runs(entries: &[retro_core::models::AuditEntry]) -> Vec<AutoRunSummary> {
    let auto_entries: Vec<&retro_core::models::AuditEntry> = entries
        .iter()
        .filter(|e| {
            e.details
                .get("auto")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .collect();

    if auto_entries.is_empty() {
        return Vec::new();
    }

    let mut runs: Vec<AutoRunSummary> = Vec::new();
    let mut current = AutoRunSummary {
        timestamp: auto_entries[0].timestamp,
        sessions_ingested: None,
        sessions_skipped: None,
        sessions_analyzed: None,
        new_patterns: None,
        actions_applied: None,
        pr_url: None,
        analyze_skipped_reason: None,
        unanalyzed_count: None,
        session_cap: None,
        errors: Vec::new(),
    };
    let mut current_start = auto_entries[0].timestamp;

    for entry in &auto_entries {
        // New run if gap > 60s
        if (entry.timestamp - current_start).num_seconds() > 60 {
            runs.push(current);
            current = AutoRunSummary {
                timestamp: entry.timestamp,
                sessions_ingested: None,
                sessions_skipped: None,
                sessions_analyzed: None,
                new_patterns: None,
                actions_applied: None,
                pr_url: None,
                analyze_skipped_reason: None,
                unanalyzed_count: None,
                session_cap: None,
                errors: Vec::new(),
            };
            current_start = entry.timestamp;
        }
        current.timestamp = entry.timestamp;

        match entry.action.as_str() {
            "ingest" => {
                current.sessions_ingested = entry
                    .details
                    .get("sessions_ingested")
                    .and_then(|v| v.as_u64());
                current.sessions_skipped = entry
                    .details
                    .get("sessions_skipped")
                    .and_then(|v| v.as_u64());
            }
            "analyze" => {
                current.sessions_analyzed = entry
                    .details
                    .get("sessions_analyzed")
                    .and_then(|v| v.as_u64());
                current.new_patterns =
                    entry.details.get("new_patterns").and_then(|v| v.as_u64());
            }
            "apply" => {
                current.actions_applied =
                    entry.details.get("actions").and_then(|v| v.as_u64());
                current.pr_url = entry
                    .details
                    .get("pr_url")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            "analyze_skipped" => {
                current.analyze_skipped_reason = entry
                    .details
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                current.unanalyzed_count = entry
                    .details
                    .get("unanalyzed_count")
                    .and_then(|v| v.as_u64());
                current.session_cap =
                    entry.details.get("cap").and_then(|v| v.as_u64());
            }
            "analyze_error" | "apply_error" => {
                if let Some(err) = entry.details.get("error").and_then(|v| v.as_str()) {
                    current.errors.push(format!("{}: {}", entry.action, err));
                }
            }
            _ => {}
        }
    }
    runs.push(current);

    // Filter out runs that only have cooldown skips (nothing interesting to show)
    runs.into_iter()
        .filter(|r| {
            r.sessions_ingested.is_some()
                || r.sessions_analyzed.is_some()
                || r.actions_applied.is_some()
                || r.analyze_skipped_reason.is_some()
                || !r.errors.is_empty()
        })
        .collect()
}

/// Format a timestamp as a human-readable "time ago" string.
fn format_time_ago(timestamp: DateTime<Utc>) -> String {
    let diff = Utc::now() - timestamp;
    if diff.num_hours() > 0 {
        format!("{}h ago", diff.num_hours())
    } else if diff.num_minutes() > 0 {
        format!("{}m ago", diff.num_minutes())
    } else {
        "just now".to_string()
    }
}

/// Display a single auto-run summary as a colored status block.
fn display_auto_run(run: &AutoRunSummary) {
    use colored::Colorize;
    let ago = format_time_ago(run.timestamp);

    println!("  {}", format!("--- retro auto-run ({ago}) ---").dimmed());

    if let Some(n) = run.sessions_ingested {
        println!(
            "  {}  {} sessions",
            "Ingested:".white(),
            n.to_string().cyan()
        );
    }

    if let Some(n) = run.sessions_analyzed {
        let pattern_info = match run.new_patterns {
            Some(p) if p > 0 => format!(" -> {} new patterns", p),
            _ => String::new(),
        };
        println!(
            "  {} {} sessions{}",
            "Analyzed:".white(),
            n.to_string().cyan(),
            pattern_info.green()
        );
    }

    if let Some(ref reason) = run.analyze_skipped_reason {
        if reason == "session_cap" {
            let count = run.unanalyzed_count.unwrap_or(0);
            let cap = run.session_cap.unwrap_or(0);
            println!(
                "  {} {} unanalyzed sessions exceeds auto limit ({})",
                "! Skipped:".yellow(),
                count.to_string().yellow(),
                cap
            );
            println!("  {}", "  Run `retro analyze` to process them.".dimmed());
        }
    }

    if let Some(n) = run.actions_applied {
        if n > 0 {
            println!(
                "  {}  {} actions",
                "Applied:".white(),
                n.to_string().green()
            );
        }
    }

    if let Some(ref url) = run.pr_url {
        println!("  {} {}", "PR created:".white(), url.cyan().underline());
    }

    for err in &run.errors {
        println!("  {} {}", "Error:".red(), err);
    }

    println!();
}

/// Check for recent auto-run activity and display a one-time status block.
/// Reads audit entries since the last nudge, groups them into auto-run
/// summaries, and shows a multi-line colored status block.
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

    // Get last nudge timestamp
    let since = match retro_core::db::get_last_nudge_at(&conn) {
        Ok(Some(ts)) => Some(ts),
        _ => None,
    };

    // Read audit entries since last nudge
    let audit_path = dir.join("audit.jsonl");
    let entries = match retro_core::audit_log::read_entries(&audit_path, since.as_ref()) {
        Ok(e) => e,
        Err(_) => return,
    };

    let runs = aggregate_auto_runs(&entries);
    if runs.is_empty() {
        return;
    }

    for run in &runs {
        display_auto_run(run);
    }

    // Update last nudge timestamp
    let _ = retro_core::db::set_last_nudge_at(&conn, &Utc::now());
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
