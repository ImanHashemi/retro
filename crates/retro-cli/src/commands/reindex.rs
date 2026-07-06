use anyhow::Result;
use colored::Colorize;
use retro_core::config::retro_dir;
use retro_core::store::{Store, index};

/// Rebuild the v3 store index (`index.db`) from the markdown files.
/// The index is disposable — this is always safe to run.
///
/// Exit 0 even with warnings: the index was built; warnings identify
/// individual skipped files.
///
/// Creates the store layout if missing (bootstrap convenience for
/// clone-then-reindex flows). TODO(Plan 2): `retro init` owns real
/// initialization; revisit whether reindex should require it.
pub fn run() -> Result<()> {
    let store = Store::open(retro_dir());
    store.ensure_layout()?;
    let stats = index::build(&store)?;
    for warning in &stats.warnings {
        eprintln!("{} {}", "warning:".yellow(), warning);
    }
    let warn_suffix = if stats.warnings.is_empty() {
        String::new()
    } else {
        format!(
            " ({} file(s) skipped — see warnings above)",
            stats.warnings.len()
        )
    };
    println!(
        "Indexed {} node(s) from {}{}",
        stats.nodes,
        store.knowledge_dir().display(),
        warn_suffix
    );
    Ok(())
}
