use anyhow::Result;
use colored::Colorize;
use retro_core::config::{Config, retro_dir};
use retro_core::doctor;

/// End-to-end v3 health verification. Read-only; the claude CLI probe is
/// a --version subprocess (no tokens).
pub fn run() -> Result<()> {
    let dir = retro_dir();
    if !dir.join("knowledge").exists() {
        anyhow::bail!("retro is not initialized — run `retro init`");
    }
    let config = Config::load(&dir.join("config.toml"))?;
    let report = doctor::run_checks(&dir, &config, true);
    for check in &report.checks {
        let mark = if check.ok { "✓".green() } else { "✗".red() };
        println!("  {} {:<12} {}", mark, check.name, check.detail);
    }
    if report.all_ok() {
        println!("\n{}", "All checks passed.".green());
        Ok(())
    } else {
        println!("\n{}", "Some checks failed — see above.".yellow());
        std::process::exit(1);
    }
}
