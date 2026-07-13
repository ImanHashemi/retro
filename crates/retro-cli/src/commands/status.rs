use anyhow::Result;
use colored::Colorize;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::models::NodeStatus;
use retro_core::util::{shorten_path, shorten_path_buf};

pub fn run() -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");

    let config = Config::load(&config_path)?;
    if config.v3.enabled {
        print_v3_status(&dir, &config)?;
        if !db_path.exists() {
            return Ok(()); // v3-only install: no v2 section to show
        }
        println!();
    }

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    let is_wal = db::verify_wal_mode(&conn)?;
    let total_ingested = db::ingested_session_count(&conn)?;
    let total_analyzed = db::analyzed_session_count(&conn)?;
    let last_ingested = db::last_ingested_at(&conn)?;
    let last_analyzed = db::last_analyzed_at(&conn)?;

    // v2 knowledge stats
    let active_nodes = db::get_nodes_by_status(&conn, &NodeStatus::Active)
        .map(|n| n.len())
        .unwrap_or(0);
    let pending_nodes = db::get_nodes_by_status(&conn, &NodeStatus::PendingReview)
        .map(|n| n.len())
        .unwrap_or(0);

    // Registered projects (v2)
    let registered_projects = db::get_all_projects(&conn).unwrap_or_default();

    // Runner status
    let last_run = retro_core::runner::last_run_time(&conn);
    let (ai_used, ai_max) = retro_core::runner::ai_calls_today(&conn, &config);

    #[cfg(target_os = "macos")]
    let runner_active = crate::launchd::is_loaded();
    #[cfg(not(target_os = "macos"))]
    let runner_active = false;

    println!("{}", "retro status".cyan().bold());
    println!();

    // Database info
    println!("  {} {}", "Database:".white(), shorten_path_buf(&db_path));
    println!(
        "  {} {}",
        "WAL mode:".white(),
        if is_wal { "enabled".green() } else { "disabled".red() }
    );
    println!("  {} {}", "Config:".white(), shorten_path_buf(&config_path));
    println!();

    // Runner status
    println!("{}", "Runner".white().bold());
    println!(
        "  {} {}",
        "Status:".white(),
        if runner_active { "Active".green() } else { "Stopped".red() }
    );
    println!(
        "  {} {}",
        "Last run:".white(),
        last_run
            .map(|dt| {
                let diff = chrono::Utc::now() - dt;
                if diff.num_hours() > 0 { format!("{}h ago", diff.num_hours()) }
                else if diff.num_minutes() > 0 { format!("{}m ago", diff.num_minutes()) }
                else { "just now".to_string() }
            })
            .unwrap_or_else(|| "never".to_string())
            .yellow()
    );
    println!(
        "  {} {}/{}",
        "AI calls today:".white(),
        ai_used.to_string().cyan(),
        ai_max.to_string().dimmed()
    );
    println!();

    // Session stats
    println!("{}", "Sessions".white().bold());
    println!("  {} {}", "Ingested:".white(), total_ingested.to_string().cyan());
    println!("  {} {}", "Analyzed:".white(), total_analyzed.to_string().cyan());
    println!(
        "  {} {}",
        "Last ingested:".white(),
        last_ingested.as_deref().unwrap_or("never").to_string().yellow()
    );
    println!(
        "  {} {}",
        "Last analyzed:".white(),
        last_analyzed.as_deref().unwrap_or("never").to_string().yellow()
    );
    println!();

    // Knowledge stats (v2)
    println!("{}", "Knowledge".white().bold());
    println!("  {} {}", "Active:".white(), active_nodes.to_string().green());
    println!(
        "  {} {}",
        "Pending review:".white(),
        if pending_nodes > 0 {
            pending_nodes.to_string().yellow()
        } else {
            "0".dimmed()
        }
    );
    println!();

    // Registered projects
    if !registered_projects.is_empty() {
        println!("{}", "Projects".white().bold());
        for project in &registered_projects {
            let count = db::ingested_session_count_for_project(&conn, &project.path)?;
            println!(
                "  {} ({} sessions)",
                shorten_path(&project.path).white(),
                count.to_string().cyan()
            );
        }
        println!();
    }

    // Config summary
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
        "Trust mode:".white(),
        config.trust.mode.cyan()
    );
    println!(
        "  {} {}",
        "Runner interval:".white(),
        format!("{}s", config.runner.interval_seconds).cyan()
    );

    Ok(())
}

fn print_v3_status(dir: &std::path::Path, config: &Config) -> Result<()> {
    use retro_core::store::{queue, state::RunnerState, Store};

    let store = Store::open(dir);
    let loaded = store.load_all()?;
    let active = loaded.nodes.iter().filter(|(_, n)| n.is_active()).count();
    let invalidated = loaded.nodes.len() - active;
    let global = loaded
        .nodes
        .iter()
        .filter(|(_, n)| n.is_active() && n.scope == retro_core::store::Scope::Global)
        .count();
    let queue_len = queue::list(dir).map(|q| q.len()).unwrap_or(0);
    let state = RunnerState::load(dir)?;
    let today = chrono::Utc::now().date_naive().to_string();
    let budget_left = state.budget_remaining(&today, config.runner.max_ai_calls_per_day);

    println!("{}", "v3 knowledge store".bold());
    println!(
        "  nodes:   {active} active ({global} global, {} project), {invalidated} invalidated",
        active - global
    );
    println!("  queue:   {queue_len} pending session(s)");
    println!(
        "  budget:  {budget_left}/{} AI call(s) left today",
        config.runner.max_ai_calls_per_day
    );
    if let Ok(health) = retro_core::health::Health::load(dir) {
        let warnings = health.warnings();
        if warnings.is_empty() {
            println!("  health:  {}", "ok".green());
        } else {
            for w in warnings {
                println!("  health:  {} {}", "⚠".yellow(), w);
            }
        }
    }
    println!("  hint:    retro ui — dashboard; retro doctor — full checks");
    Ok(())
}
