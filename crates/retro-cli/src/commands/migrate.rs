use anyhow::Result;
use colored::Colorize;
use retro_core::config::{Config, retro_dir};
use retro_core::store::Store;

/// v2 -> v3 migration. Idempotent; retro.db is read-only throughout.
pub fn run(dry_run: bool) -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml"))?;
    // Same discipline as the dashboard write handlers: never interleave
    // store mutations with a runner pass.
    let _lock = if dry_run {
        None
    } else {
        match retro_core::lock::LockFile::try_acquire(&dir.join("run.lock")) {
            Some(l) => Some(l),
            None => anyhow::bail!("a retro run is in progress — retry shortly"),
        }
    };
    let store = Store::open(&dir);
    // A dry run must not mutate the environment either — no store layout,
    // no `git init`. load_all tolerates the dirs not existing yet.
    if !dry_run {
        store.ensure_layout()?;
        retro_core::store::git::ensure_repo(&dir)?;
    }

    let label = if dry_run {
        " (dry run — nothing written)"
    } else {
        ""
    };
    println!("retro migrate{label}");
    let report = retro_core::migrate::migrate_knowledge(&store, &dir, dry_run)?;
    if report.v2_db_missing {
        println!("  no v2 database found — nothing to import");
    } else {
        println!(
            "  knowledge: {} imported, {} already present (deduped), {} skipped (dismissed/archived), {} skipped (invalid)",
            report.imported.to_string().green(),
            report.deduped,
            report.skipped_status,
            report.skipped_invalid
        );
    }

    // Safety-import: rescue any managed-block rules that aren't in the store
    // yet (global CLAUDE.md, then every known project's CLAUDE.md). Guards
    // against the first-projection-wipes-pre-v3-rules failure.
    let mut safety_imported = retro_core::migrate::safety_import(
        &store,
        &config.claude_dir().join("CLAUDE.md"),
        &retro_core::store::Scope::Global,
        &report.imported_bodies,
        dry_run,
    )?;
    if let Ok(map) = retro_core::store::projects::PathMap::load(&dir) {
        for (slug, path) in &map.paths {
            safety_imported += retro_core::migrate::safety_import(
                &store,
                &std::path::Path::new(path).join("CLAUDE.md"),
                &retro_core::store::Scope::Project(slug.clone()),
                &report.imported_bodies,
                dry_run,
            )?;
        }
    }
    println!(
        "  safety-import: {} rule(s) rescued from managed blocks",
        safety_imported.to_string().green()
    );

    // Task 3 extends this function (env cleanup; commit; reindex;
    // projection). Keep this ordering comment until it lands.
    if dry_run {
        println!(
            "\n  rollback note: migrate never modifies retro.db; store writes are git commits in {}",
            dir.display()
        );
        println!(
            "  (reading the db may leave empty retro.db-wal/-shm files behind — standard SQLite artifacts, harmless, cleaned by any 2.x run)"
        );
    }
    Ok(())
}
