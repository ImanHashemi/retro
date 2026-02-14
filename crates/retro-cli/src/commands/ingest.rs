use anyhow::Result;
use colored::Colorize;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::ingest;

use super::git_root_or_cwd;

pub fn run(global: bool) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");

    // Check initialization
    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    let result = if global {
        println!("{}", "Ingesting all projects...".cyan());
        ingest::ingest_all_projects(&conn, &config)?
    } else {
        // Determine current project from git root, falling back to cwd
        let project_path = git_root_or_cwd()?;

        println!(
            "{} {}",
            "Ingesting project:".cyan(),
            project_path.white()
        );
        ingest::ingest_project(&conn, &config, &project_path)
            ?
    };

    // Print results
    println!();
    println!(
        "  {} {}",
        "Sessions found:".white(),
        result.sessions_found.to_string().cyan()
    );
    println!(
        "  {} {}",
        "Sessions ingested:".white(),
        result.sessions_ingested.to_string().green()
    );
    println!(
        "  {} {}",
        "Sessions skipped:".white(),
        result.sessions_skipped.to_string().yellow()
    );

    if !result.errors.is_empty() {
        println!(
            "  {} {}",
            "Errors:".white(),
            result.errors.len().to_string().red()
        );
        for err in &result.errors {
            eprintln!("    {}", err.red());
        }
    }

    Ok(())
}
