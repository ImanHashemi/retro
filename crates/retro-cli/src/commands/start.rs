use anyhow::Result;
use colored::Colorize;
use retro_core::config::{retro_dir, Config};

pub fn run(_verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    if cfg!(not(target_os = "macos")) {
        anyhow::bail!("retro start is currently only supported on macOS. Linux (systemd) support coming soon.");
    }

    let config = Config::load(&config_path)?;
    crate::launchd::install_and_load(&config)?;

    let interval = config.runner.interval_seconds;
    println!("{} scheduled runner (every {}s)", "Started".green().bold(), interval);
    println!("  Plist: {}", crate::launchd::plist_path().display().to_string().dimmed());
    println!("  Log:   {}", dir.join("runner.log").display().to_string().dimmed());
    println!();
    println!("  Use {} to stop, {} to check status", "retro stop".cyan(), "retro status".cyan());

    Ok(())
}
