use anyhow::Result;
use colored::Colorize;
use retro_core::config::retro_dir;
use retro_core::store::{Store, index};

/// Rebuild the v3 store index (`index.db`) from the markdown files.
/// The index is disposable — this is always safe to run.
pub fn run() -> Result<()> {
    let store = Store::open(retro_dir());
    store.ensure_layout()?;
    let stats = index::build(&store)?;
    for warning in &stats.warnings {
        eprintln!("{} {}", "warning:".yellow(), warning);
    }
    println!(
        "Indexed {} node(s) from {}",
        stats.nodes,
        store.root().join("knowledge").display()
    );
    Ok(())
}
