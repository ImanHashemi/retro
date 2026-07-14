use anyhow::Result;
use retro_core::config::{retro_dir, Config};

/// Run the v3 pipeline: drain queue -> analyze -> project -> commit -> push.
pub fn run(_verbose: bool, dry_run: bool, background: bool) -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml"))?;
    let backend = retro_core::analysis::claude_cli::ClaudeCliBackend::new(&config.ai);
    let summary = retro_core::runner_v3::run_v3(&dir, &config, &backend, dry_run)?;
    match summary {
        None => {
            if !background {
                println!("Another retro run is in progress — skipped.");
            }
        }
        Some(s) => {
            if !background {
                if dry_run {
                    let stale = if s.sessions_stale > 0 {
                        format!(", {} stale (would prune)", s.sessions_stale)
                    } else {
                        String::new()
                    };
                    println!(
                        "v3 dry run: {} session(s) pending, {} skipped{stale} — no AI calls, no writes",
                        s.sessions_pending, s.sessions_skipped
                    );
                } else {
                    println!(
                        "v3 run: {} session(s) analyzed ({} AI call(s)) — +{} nodes, {} updated, {} merged, {} invalidated; {} global rule(s) projected{}{}",
                        s.sessions_processed, s.ai_calls, s.nodes_created, s.nodes_updated,
                        s.nodes_merged, s.nodes_invalidated, s.rules_projected_global,
                        if s.sessions_pending > 0 { format!("; {} pending (budget)", s.sessions_pending) } else { String::new() },
                        if s.ops_skipped > 0 { format!("; {} op(s) skipped", s.ops_skipped) } else { String::new() },
                    );
                }
            }
        }
    }
    Ok(())
}
