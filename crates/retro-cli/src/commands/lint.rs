use anyhow::Result;
use colored::Colorize;
use retro_core::config::{Config, retro_dir};
use retro_core::lint;
use retro_core::store::{Store, state::RunnerState};

/// Free lint pass (no AI calls). Without --dry-run, findings are also pushed
/// as briefing notifications (capped) so they surface in the next session.
pub fn run(dry_run: bool) -> Result<()> {
    let dir = retro_dir();
    if !dir.join("knowledge").exists() {
        anyhow::bail!("retro is not initialized — run `retro init`");
    }
    let config = Config::load(&dir.join("config.toml"))?;
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
        // state.json writes require the run lock (same discipline as the
        // dashboard write handlers) — a load-modify-save racing a runner
        // pass could clobber budget/processed fields.
        let Some(_lock) = retro_core::lock::LockFile::try_acquire(&dir.join("run.lock")) else {
            println!("\n(a retro run is in progress — findings not queued; rerun `retro lint` shortly)");
            return Ok(());
        };
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
