use anyhow::Result;
use colored::Colorize;
use retro_core::config::{Config, retro_dir};
use retro_core::lint;
use retro_core::store::{Store, state::RunnerState};

/// Free lint pass (no AI calls). Without --dry-run, findings are also pushed
/// as briefing notifications (capped) so they surface in the next session.
pub fn run(dry_run: bool) -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml"))?;
    if !config.v3.enabled {
        anyhow::bail!("v3 is disabled — run `retro init --v3` first");
    }
    let store = Store::open(&dir);
    let report = lint::run_lint(&store, &config)?;
    println!(
        "Scanned {} active node(s): {} finding(s)",
        report.nodes_scanned,
        report.findings.len()
    );
    for f in &report.findings {
        println!("  {} {}", format!("[{}]", f.kind).yellow(), f.detail);
    }
    if !dry_run && !report.findings.is_empty() {
        let mut state = RunnerState::load(&dir)?;
        for f in report.findings.iter().take(3) {
            let msg = format!("Lint: {}", f.detail);
            // Findings are recurring state (unlike one-shot registration events):
            // dedup so repeated lint runs don't stack identical briefing lines.
            if !state.notifications.contains(&msg) {
                state.notifications.push(msg);
            }
        }
        state.save(&dir)?;
        println!("\n(Top findings queued for your next session briefing.)");
    }
    Ok(())
}
