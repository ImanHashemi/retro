use anyhow::{Context, Result};
use colored::Colorize;
use retro_core::config::{retro_dir, Config};
use retro_core::db;

pub fn run() -> Result<()> {
    let dir = retro_dir();

    // Create ~/.retro/ directory structure
    std::fs::create_dir_all(&dir).context("creating ~/.retro/")?;
    std::fs::create_dir_all(dir.join("backups")).context("creating ~/.retro/backups/")?;

    // Create config.toml if it doesn't exist
    let config_path = dir.join("config.toml");
    if !config_path.exists() {
        let config = Config::default();
        config.save(&config_path)?;
        println!("  {} {}", "Created".green(), config_path.display());
    } else {
        println!("  {} {}", "Exists".yellow(), config_path.display());
    }

    // Initialize database with WAL mode
    let db_path = dir.join("retro.db");
    let db_existed = db_path.exists();
    let conn = db::open_db(&db_path)?;

    let is_wal = db::verify_wal_mode(&conn)?;
    let label = if db_existed { "Exists" } else { "Created" };
    let color_label = if db_existed {
        label.yellow()
    } else {
        label.green()
    };
    if is_wal {
        println!("  {} {} (WAL mode)", color_label, db_path.display());
    } else {
        println!(
            "  {} {} (warning: WAL mode not enabled)",
            label.yellow(),
            db_path.display()
        );
    }

    println!();
    println!("{}", "retro initialized successfully".green().bold());
    println!(
        "  Run {} to parse Claude Code sessions",
        "retro ingest".cyan()
    );

    Ok(())
}
