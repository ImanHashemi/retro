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
        // ensure_repo early-returns for EXISTING repos without refreshing the
        // machine-local excludes — upsert them here, or commit_all below
        // re-adds the very files untrack removes (and can newly sweep
        // retro.db/audit.jsonl into a pushed repo). Mirrors runner_v3.
        retro_core::store::git::apply_local_config(&dir)?;
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

    // Environment cleanup: untrack any machine-local files an older binary
    // committed before the ignore rules existed, sweep v1 git hooks from
    // every known project, and remove the v2 launchd runner.
    let known_paths = retro_core::migrate::all_known_project_paths(&dir);
    if dry_run {
        println!("  would untrack any previously-committed machine-local store files");
        println!(
            "  would sweep v1 git hooks from {} known project(s)",
            known_paths.len()
        );
        if cfg!(target_os = "macos") {
            println!("  would remove the v2 launchd runner (com.retro.runner), if present");
        }
    } else {
        if retro_core::migrate::untrack_ignored_entries(&dir)? {
            println!("  untracked previously-committed machine-local store files");
        }
        let mut hooks_removed = 0usize;
        for path in &known_paths {
            hooks_removed += retro_core::migrate::remove_v1_hooks(path).len();
        }
        if hooks_removed > 0 {
            println!(
                "  removed {} v1 git hook(s) across {} known project(s)",
                hooks_removed,
                known_paths.len()
            );
        }
        if cfg!(target_os = "macos") && retro_core::migrate::remove_v2_launchd() {
            println!("  removed the v2 launchd runner (com.retro.runner)");
        }
    }

    // Commit, reindex, and project — same discipline as a normal `retro run`.
    if dry_run {
        println!("  would commit knowledge changes, reindex, and project CLAUDE.md/CLAUDE.local.md");
    } else {
        retro_core::store::git::commit_all(&dir, "retro: migrate v2 knowledge")?;
        retro_core::store::index::build(&store)?;
        let threshold = config.knowledge.confidence_threshold;
        let global_md = config.claude_dir().join("CLAUDE.md");
        let rules = retro_core::projection::local_md::project_global_md(
            &store,
            &global_md,
            threshold,
            Some(&dir.join("backups")),
        )?;
        println!("  projected {rules} rule(s) to {}", global_md.display());
        if let Ok(map) = retro_core::store::projects::PathMap::load(&dir) {
            for (slug, path) in &map.paths {
                let n = retro_core::projection::local_md::project_local_md(
                    &store,
                    slug,
                    std::path::Path::new(path),
                    threshold,
                )?;
                println!("  projected {n} rule(s) to {path}/CLAUDE.local.md");
            }
        }
    }

    if !report.v2_db_missing {
        println!(
            "\n  v2 database preserved at {}/retro.db — safe to delete once you trust the store (rollback: the 2.x binary still reads it)",
            dir.display()
        );
    }

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
