use anyhow::Result;
use colored::Colorize;
use retro_core::audit_log;
use retro_core::config::{retro_dir, Config};
use retro_core::curator;
use retro_core::db;
use retro_core::lock::LockFile;

pub fn run(dry_run: bool, verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
    let audit_path = dir.join("audit.jsonl");
    let lock_path = dir.join("retro.lock");

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    let _lock = LockFile::acquire(&lock_path)
        .map_err(|e| anyhow::anyhow!("could not acquire lock: {e}"))?;

    println!(
        "{}",
        format!(
            "Checking for stale items (threshold: {} days)...",
            config.analysis.staleness_days
        )
        .cyan()
    );

    if verbose {
        println!("[verbose] staleness threshold: {} days", config.analysis.staleness_days);
    }

    let stale_items = curator::detect_stale(&conn, &config)?;

    if stale_items.is_empty() {
        println!("{}", "No stale items found.".green());
        return Ok(());
    }

    println!();
    println!(
        "Found {} stale items:",
        stale_items.len().to_string().yellow()
    );

    for item in &stale_items {
        let icon = match item.projection.target_type.as_str() {
            "skill" => "skill",
            "claude_md" => "rule ",
            "global_agent" => "agent",
            _ => "     ",
        };
        println!();
        println!(
            "  {} [{}] {}",
            "x".red(),
            icon.dimmed(),
            item.pattern.description.white()
        );
        println!("         {} {}", "path:".dimmed(), item.projection.target_path.dimmed());
        println!("         {} {}", "reason:".dimmed(), item.reason.dimmed());
    }

    if dry_run {
        println!();
        println!(
            "{}",
            "Dry run — no changes made. Run `retro clean` to archive stale items."
                .yellow()
                .bold()
        );
        return Ok(());
    }

    // Confirm
    println!();
    print!(
        "{} ",
        format!("Archive {} stale items? [y/N]", stale_items.len())
            .yellow()
            .bold()
    );
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    if !input.trim().eq_ignore_ascii_case("y") {
        println!("{}", "Aborted.".dimmed());
        return Ok(());
    }

    let result = curator::archive_stale_items(&conn, &config, &stale_items)?;

    // Audit log
    let audit_details = serde_json::json!({
        "archived_count": result.archived_count,
        "skills_removed": result.skills_removed,
        "claude_md_rules_removed": result.claude_md_rules_removed,
        "agents_removed": result.agents_removed,
    });
    audit_log::append(&audit_path, "clean", audit_details)?;

    println!();
    println!("{}", "Clean complete!".green().bold());
    println!(
        "  {} {}",
        "Patterns archived:".white(),
        result.archived_count.to_string().green()
    );
    if result.skills_removed > 0 {
        println!(
            "  {} {}",
            "Skills removed:".white(),
            result.skills_removed.to_string().green()
        );
    }
    if result.claude_md_rules_removed > 0 {
        println!(
            "  {} {}",
            "CLAUDE.md rules removed:".white(),
            result.claude_md_rules_removed.to_string().green()
        );
    }
    if result.agents_removed > 0 {
        println!(
            "  {} {}",
            "Agents removed:".white(),
            result.agents_removed.to_string().green()
        );
    }
    println!(
        "  {}",
        "All files backed up to ~/.retro/backups/".dimmed()
    );

    Ok(())
}
