use anyhow::Result;
use colored::Colorize;
use retro_core::config::retro_dir;
use retro_core::db;
use retro_core::util::shorten_path;

pub fn run(status_filter: Option<String>) -> Result<()> {
    let dir = retro_dir();
    let db_path = dir.join("retro.db");

    // Check initialization
    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let conn = db::open_db(&db_path)?;

    let patterns = if let Some(ref status) = status_filter {
        db::get_patterns(&conn, &[status.as_str()], None)?
    } else {
        db::get_all_patterns(&conn, None)?
    };

    if patterns.is_empty() {
        println!(
            "{}",
            "No patterns found. Run `retro analyze` to discover patterns.".yellow()
        );
        return Ok(());
    }

    // Count by status
    let discovered = patterns.iter().filter(|p| p.status.to_string() == "discovered").count();
    let active = patterns.iter().filter(|p| p.status.to_string() == "active").count();
    let archived = patterns.iter().filter(|p| p.status.to_string() == "archived").count();

    println!(
        "{} ({} discovered, {} active, {} archived)",
        "Patterns".white().bold(),
        discovered.to_string().cyan(),
        active.to_string().green(),
        archived.to_string().yellow(),
    );
    println!();

    for pattern in &patterns {
        let status_colored = match pattern.status.to_string().as_str() {
            "discovered" => format!("[{}]", "discovered").cyan(),
            "active" => format!("[{}]", "active").green(),
            "archived" => format!("[{}]", "archived").yellow(),
            "dismissed" => format!("[{}]", "dismissed").red(),
            s => format!("[{}]", s).white(),
        };

        let type_str = pattern.pattern_type.to_string();
        let confidence_str = format!("{:.0}%", pattern.confidence * 100.0);
        let seen_str = format!("{}x", pattern.times_seen);

        println!(
            "  {} {} (confidence: {}, seen: {})",
            status_colored,
            type_str.white(),
            confidence_str.cyan(),
            seen_str.yellow(),
        );
        println!("    \"{}\"", pattern.description);
        println!(
            "    {} {}",
            "→".white(),
            pattern.suggested_target.to_string().cyan()
        );

        if pattern.generation_failed {
            println!("    {}", "⚠ skill generation failed".red());
        }

        if let Some(ref proj) = pattern.project {
            println!("    project: {}", shorten_path(proj).white());
        }

        println!();
    }

    Ok(())
}
