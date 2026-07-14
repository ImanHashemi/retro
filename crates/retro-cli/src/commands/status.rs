use anyhow::Result;
use colored::Colorize;
use retro_core::config::{retro_dir, Config};

pub fn run() -> Result<()> {
    let dir = retro_dir();
    if !dir.join("knowledge").exists() {
        anyhow::bail!("retro is not initialized — run `retro init`");
    }
    let config_path = dir.join("config.toml");
    let config = Config::load(&config_path)?;

    print_v3_status(&dir, &config)
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
