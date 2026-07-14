pub mod doctor;
pub mod init;
pub mod lint;
pub mod migrate;
pub mod observe;
pub mod brief;
pub mod reindex;
pub mod run;
pub mod status;
pub mod ui;
pub mod uninstall;

/// Check for v3 pipeline health/queue issues and display a status block.
/// Silently does nothing if the store isn't initialized or any error occurs.
pub fn check_and_display_nudge() {
    let dir = retro_core::config::retro_dir();
    if !dir.join("knowledge").exists() {
        return;
    }

    if let Ok(health) = retro_core::health::Health::load(&dir) {
        use colored::Colorize;
        for w in health.warnings() {
            eprintln!("  {} {}", "retro:".yellow(), w);
        }
    }
    if let Ok(entries) = retro_core::store::queue::list(&dir) {
        if !entries.is_empty() {
            // enqueued_at is RFC3339; oldest entry first (list is sorted)
            let oldest = &entries[0].enqueued_at;
            let stale = chrono::DateTime::parse_from_rfc3339(oldest)
                .map(|t| {
                    chrono::Utc::now().signed_duration_since(t) > chrono::Duration::hours(24)
                })
                .unwrap_or(false);
            if stale {
                use colored::Colorize;
                eprintln!(
                    "  {} {} session(s) queued (oldest > 24h) — run `retro run` or `retro doctor`",
                    "retro:".yellow(),
                    entries.len()
                );
            }
        }
    }
}
