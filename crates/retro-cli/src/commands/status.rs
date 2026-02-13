use anyhow::Result;
use colored::Colorize;
use retro_core::config::{retro_dir, Config};
use retro_core::db;

pub fn run() -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");

    // Check initialization
    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    let is_wal = db::verify_wal_mode(&conn)?;
    let total_ingested =
        db::ingested_session_count(&conn)?;
    let total_analyzed =
        db::analyzed_session_count(&conn)?;
    let last_ingested = db::last_ingested_at(&conn)?;
    let last_analyzed = db::last_analyzed_at(&conn)?;
    let patterns_discovered =
        db::pattern_count_by_status(&conn, "discovered")?;
    let patterns_active =
        db::pattern_count_by_status(&conn, "active")?;
    let projects = db::list_projects(&conn)?;

    println!("{}", "retro status".cyan().bold());
    println!();

    // Database info
    println!("  {} {}", "Database:".white(), db_path.display());
    println!(
        "  {} {}",
        "WAL mode:".white(),
        if is_wal {
            "enabled".green()
        } else {
            "disabled".red()
        }
    );
    println!(
        "  {} {}",
        "Config:".white(),
        config_path.display()
    );
    println!();

    // Session stats
    println!("{}", "Sessions".white().bold());
    println!(
        "  {} {}",
        "Ingested:".white(),
        total_ingested.to_string().cyan()
    );
    println!(
        "  {} {}",
        "Analyzed:".white(),
        total_analyzed.to_string().cyan()
    );
    println!(
        "  {} {}",
        "Last ingested:".white(),
        last_ingested
            .as_deref()
            .unwrap_or("never")
            .to_string()
            .yellow()
    );
    println!(
        "  {} {}",
        "Last analyzed:".white(),
        last_analyzed
            .as_deref()
            .unwrap_or("never")
            .to_string()
            .yellow()
    );
    println!();

    // Pattern stats
    println!("{}", "Patterns".white().bold());
    println!(
        "  {} {}",
        "Discovered:".white(),
        patterns_discovered.to_string().cyan()
    );
    println!(
        "  {} {}",
        "Active:".white(),
        patterns_active.to_string().cyan()
    );
    println!();

    // Projects
    if !projects.is_empty() {
        println!("{}", "Projects".white().bold());
        for project in &projects {
            let count = db::ingested_session_count_for_project(&conn, project)
                ?;
            println!(
                "  {} ({} sessions)",
                project.white(),
                count.to_string().cyan()
            );
        }
    }

    // Config summary
    println!();
    println!("{}", "Configuration".white().bold());
    println!(
        "  {} {} days",
        "Analysis window:".white(),
        config.analysis.window_days.to_string().cyan()
    );
    println!(
        "  {} {}",
        "Confidence threshold:".white(),
        config.analysis.confidence_threshold.to_string().cyan()
    );
    println!(
        "  {} {}",
        "AI backend:".white(),
        config.ai.backend.cyan()
    );

    Ok(())
}
