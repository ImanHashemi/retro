use anyhow::Result;
use colored::Colorize;

pub fn run(_verbose: bool) -> Result<()> {
    if cfg!(not(target_os = "macos")) {
        anyhow::bail!("retro stop is currently only supported on macOS.");
    }

    crate::launchd::unload()?;

    println!("{} scheduled runner", "Stopped".green().bold());
    println!("  Plist preserved at {}", crate::launchd::plist_path().display().to_string().dimmed());
    println!("  Use {} to restart", "retro start".cyan());

    Ok(())
}
