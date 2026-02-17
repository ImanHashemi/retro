use anyhow::Result;
use colored::Colorize;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::ingest;
use retro_core::lock::LockFile;

use retro_core::util::shorten_path;

use super::{git_root_or_cwd, within_cooldown};

pub fn run(global: bool, auto: bool, verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
    let lock_path = dir.join("retro.lock");

    // Check initialization
    if !db_path.exists() {
        if auto {
            return Ok(());
        }
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    // In auto mode: acquire lockfile silently, check cooldown
    if auto {
        let _lock = match LockFile::try_acquire(&lock_path) {
            Some(lock) => lock,
            None => {
                if verbose {
                    eprintln!("[verbose] skipping ingest: another process holds the lock");
                }
                return Ok(());
            }
        };

        // Check cooldown: skip if ingested within ingest_cooldown_minutes
        if let Ok(Some(ref last)) = db::last_ingested_at(&conn) {
            if within_cooldown(last, config.hooks.ingest_cooldown_minutes) {
                if verbose {
                    eprintln!(
                        "[verbose] skipping ingest: within cooldown ({}m)",
                        config.hooks.ingest_cooldown_minutes
                    );
                }
                return Ok(());
            }
        }

        // Run ingestion silently — any error exits quietly
        let result = if global {
            ingest::ingest_all_projects(&conn, &config)
        } else {
            let project_path = git_root_or_cwd()?;
            ingest::ingest_project(&conn, &config, &project_path)
        };

        match result {
            Ok(r) => {
                if verbose {
                    eprintln!(
                        "[verbose] ingested {} sessions ({} skipped)",
                        r.sessions_ingested, r.sessions_skipped
                    );
                }
            }
            Err(e) => {
                if verbose {
                    eprintln!("[verbose] ingest error: {e}");
                }
            }
        }

        return Ok(());
    }

    // Interactive mode
    let result = if global {
        println!("{}", "Ingesting all projects...".cyan());
        ingest::ingest_all_projects(&conn, &config)?
    } else {
        let project_path = git_root_or_cwd()?;
        if verbose {
            eprintln!("[verbose] project path: {}", project_path);
        }
        println!(
            "{} {}",
            "Ingesting project:".cyan(),
            shorten_path(&project_path).white()
        );
        ingest::ingest_project(&conn, &config, &project_path)?
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
