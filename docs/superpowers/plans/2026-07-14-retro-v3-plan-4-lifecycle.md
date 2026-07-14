# Retro v3 Plan 4 — Lifecycle (migrate, uninstall, v1/v2 deletion, 3.0.0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship retro 3.0.0: a `retro migrate` bridge from v2 (SQLite + launchd + git hooks) to v3 (markdown store + Claude Code hooks), a clean v3 uninstall, deletion of every v1/v2 command and module, v3 scenario tests, and the release.

**Architecture:** Migrate is fully self-contained (its own read-only SQLite queries, its own launchd/git-hook removal helpers) so the later deletion waves cannot break it. Deletion proceeds compiler-driven in two waves (CLI first, core second), guided by the exact inventories below. The `[v3] enabled` gate is removed entirely — in 3.0.0, v3 is the only pipeline.

**Tech Stack:** Rust (sync, no tokio), rusqlite (read-only for migrate), chrono, existing v3 store modules.

**Spec:** `docs/superpowers/specs/2026-07-06-retro-v3-personal-redesign-design.md` (§10 migration/lifecycle; §4–§9 already shipped in Plans 1–3).

---

## Context for implementers (read first)

- **Conventions:** `CoreError` (thiserror) in retro-core, `anyhow` + bare `?` in retro-cli. NEVER `cargo fmt` — per-file `rustfmt --edition 2024` on NEW files only; manual style in shared files. Mandatory preflight every dispatch: `cd <repo>/.claude/worktrees/v3-lifecycle && git branch --show-current` must print `v3-lifecycle`.
- **SAFETY (absolute):** never run `retro init` (any form), `retro start`, `retro stop`, `retro migrate`, or `retro uninstall` against the real environment — they write launchd plists, `~/.claude/settings.json` hooks, and CLAUDE.md files OUTSIDE the RETRO_HOME sandbox unless isolated. Live checks only via an isolated `RETRO_HOME` **plus** a config whose `[paths] claude_dir` points at a temp dir. Tests use TempDir only. Never bare `git stash` (shared stack). Never stage `Cargo.lock` except where a task explicitly says so.
- **Existing API you build on (verified against merged main):**
  - v3 store: `store::{Store, Node, NodeType, Scope, slugify, is_valid_slug}`; `Store::{open, ensure_layout, load_all, get, write_node, invalidate, unique_slug, node_path}`; `Node{id, scope, node_type, confidence, sources, created, updated, invalidated_by, body}`; `Node::to_markdown` writes confidence as `{:.2}`; v3 `NodeType` = `Rule | Preference | Pattern | Memory`
  - `store::git::{ensure_repo, apply_local_config, commit_all, push_best_effort, has_remote, has_unpushed}`; `store::index::{build, open, query}`; `store::projects::PathMap`; `store::state::RunnerState`; `store::queue`
  - `projection::claude_md::{read_managed_section, build_managed_section, update_claude_md_content, has_managed_section}` (markers: `<!-- retro:managed:start -->` / `<!-- retro:managed:end -->`)
  - `projection::local_md::{project_global_md, project_local_md}`
  - `claude_settings::ensure_hook`; `lock::LockFile::try_acquire`; `util::backup_file`; `health::record`
  - `analysis::merge::normalized_similarity(a, b) -> f64` (lowercases internally; > 0.8 = near-duplicate) — **moved to `util.rs` in Task 1**
  - v2 SQLite (read via raw rusqlite in migrate, NOT via db.rs): DB at `retro_dir()/retro.db`. `nodes(id TEXT PK, type TEXT, scope TEXT, project_id TEXT NULL, content TEXT, confidence REAL, status TEXT, created_at TEXT, updated_at TEXT, projected_at TEXT NULL, pr_url TEXT NULL)`; `projects(id TEXT PK, path TEXT, remote_url TEXT NULL, agent_type TEXT, last_seen TEXT)`. v2 `NodeType` strings: `preference|pattern|rule|skill|memory|directive`; v2 `NodeStatus` strings: `active|pending_review|dismissed|archived`; v2 scope strings: `global|project`. **Verify the exact serialized strings against `models.rs` `as_str()`/Display impls before relying on them; adapt the match arms if they differ.**
  - v1 git hook format (root `git.rs`): marker line `# retro hook - do not remove` followed by one command line, in `.git/hooks/post-commit` (and legacy `post-merge`)
  - launchd artifact: `~/Library/LaunchAgents/com.retro.runner.plist`, label `com.retro.runner`, removal = `launchctl bootout gui/{uid}/com.retro.runner` (tolerate "No such process") + delete plist file
- **Branch:** create worktree + branch `v3-lifecycle` from origin/main at execution start (superpowers:using-git-worktrees).
- **Baseline:** 404 tests passing on main. Roughly 210 of them are v1/v2 and die in Tasks 6–8; each deletion task states its expected fallout.
- **Rollback story (user-critical):** migrate never mutates `retro.db` (read-only); every store write is a git commit in `~/.retro` (revertable); environment cleanup (plist, git hooks) is re-creatable by the old 2.x binary. Re-running migrate is a documented no-op (dedup). State this in `--dry-run` output.

## File structure (created/modified)

- Create: `crates/retro-core/src/migrate.rs` — all migrate logic (v2 reader, import, dedup, safety-import wiring, env cleanup)
- Create: `crates/retro-cli/src/commands/migrate.rs`, `crates/retro-cli/src/commands/uninstall.rs`
- Modify: `crates/retro-core/src/util.rs` (+similarity fns), `claude_settings.rs` (+remove_hook), `projection/claude_md.rs` (+strip_managed_section), `store/state.rs` (notification cap), `commands/init.rs` (v3-only + safety-import), `main.rs`, `commands/mod.rs`, `config.rs`, `lib.rs`
- Delete (Task 6): `commands/{analyze,apply,audit,clean,curate,dash,diff,hooks,ingest,log,patterns,review,start,stop,sync}.rs`, `launchd.rs`, `tui/`
- Delete (Tasks 7–8): `db.rs`, root `git.rs`, `reconcile.rs`, `curator.rs`, `audit_log.rs`, `runner.rs`, `trust.rs`, `analysis/merge.rs`, `ingest/{context,history}.rs`, `projection/{skill,global_agent}.rs`, slim `models.rs`/`analysis/mod.rs`/`analysis/prompts.rs`/`projection/mod.rs`/`briefing.rs`/`ingest/mod.rs`/`projection/claude_md.rs`/`config.rs`, remove `crates/retro-projectors/`
- Rewrite: `scenarios/*.md`, `README.md`, `CLAUDE.md`

---

### Task 1: Migrate core — v2 nodes → v3 store

**Files:**
- Modify: `crates/retro-core/src/util.rs` (move `normalized_similarity` + `levenshtein` here from `analysis/merge.rs`)
- Modify: `crates/retro-core/src/analysis/merge.rs` (replace the moved fns with `pub use crate::util::{levenshtein, normalized_similarity};`)
- Modify: `crates/retro-core/src/lint.rs` (its `crate::analysis::merge::normalized_similarity` call becomes `crate::util::normalized_similarity`)
- Create: `crates/retro-core/src/migrate.rs`
- Modify: `crates/retro-core/src/lib.rs` (`pub mod migrate;`)
- Create: `crates/retro-cli/src/commands/migrate.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs`, `crates/retro-cli/src/main.rs`

- [ ] **Step 1: Move similarity helpers to util.rs.** Cut `levenshtein` and `normalized_similarity` (with their tests) from `analysis/merge.rs` into `util.rs` verbatim; leave `pub use crate::util::{levenshtein, normalized_similarity};` in merge.rs so v2 callers keep compiling; change `crates/retro-core/src/lint.rs` to `use crate::util::normalized_similarity` (it currently calls `crate::analysis::merge::normalized_similarity`). Run `cargo test -p retro-core lint` and `cargo test -p retro-core util` — all pass.

- [ ] **Step 2: Write the failing tests for the v2 reader + import.** In `migrate.rs`, tests build a REAL fixture v2 db with raw rusqlite (no db.rs):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NodeType, Scope, Store};
    use tempfile::TempDir;

    fn fixture_v2_db(dir: &std::path::Path) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(dir.join("retro.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE nodes (id TEXT PRIMARY KEY, type TEXT, scope TEXT, project_id TEXT,
                content TEXT, confidence REAL, status TEXT, created_at TEXT, updated_at TEXT,
                projected_at TEXT, pr_url TEXT);
             CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT, remote_url TEXT,
                agent_type TEXT DEFAULT 'claude_code', last_seen TEXT);",
        ).unwrap();
        conn.execute("INSERT INTO projects VALUES ('my-app', '/tmp/my-app', NULL, 'claude_code', '2026-01-01T00:00:00Z')", []).unwrap();
        let rows: &[(&str, &str, &str, Option<&str>, &str, f64, &str)] = &[
            ("n1", "rule", "global", None, "Always run smoke tests before full runs", 0.8, "active"),
            ("n2", "directive", "global", None, "Never commit secrets", 0.85, "active"),
            ("n3", "skill", "global", None, "Use uv for python scripts", 0.75, "active"),
            ("n4", "pattern", "project", Some("my-app"), "Deploys go through staging first", 0.6, "pending_review"),
            ("n5", "rule", "global", None, "A dismissed rule", 0.9, "dismissed"),
            ("n6", "memory", "global", None, "Context-only memory item", 0.7, "active"),
        ];
        for (id, t, s, p, c, conf, st) in rows {
            conn.execute(
                "INSERT INTO nodes VALUES (?1,?2,?3,?4,?5,?6,?7,'2026-05-01T10:00:00Z','2026-06-01T10:00:00Z',NULL,NULL)",
                rusqlite::params![id, t, s, p, c, conf, st],
            ).unwrap();
        }
        conn
    }

    #[test]
    fn imports_active_and_pending_nodes_with_type_mapping() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        crate::store::git::ensure_repo(tmp.path()).unwrap();
        fixture_v2_db(tmp.path());

        let report = migrate_knowledge(&store, tmp.path(), false).unwrap();
        assert_eq!(report.imported, 5, "{report:?}"); // n1,n2,n3,n4,n6
        assert_eq!(report.skipped_status, 1);         // n5 dismissed
        let all = store.load_all().unwrap().nodes;
        let types: Vec<_> = all.iter().map(|(_, n)| (n.body.clone(), n.node_type)).collect();
        assert!(types.iter().any(|(b, t)| b.contains("Never commit secrets") && *t == NodeType::Rule)); // directive -> rule
        assert!(types.iter().any(|(b, t)| b.contains("uv for python") && *t == NodeType::Pattern));      // skill -> pattern
        assert!(all.iter().any(|(_, n)| matches!(&n.scope, Scope::Project(s) if s == "my-app")));
        // provenance + dates carried over
        let n1 = all.iter().find(|(_, n)| n.body.contains("smoke tests")).unwrap();
        assert!(n1.1.sources.iter().any(|s| s.starts_with("v2:")));
        assert_eq!(n1.1.created.to_string(), "2026-05-01");
    }

    #[test]
    fn rerun_is_idempotent_and_dedups_against_existing() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        crate::store::git::ensure_repo(tmp.path()).unwrap();
        fixture_v2_db(tmp.path());
        let first = migrate_knowledge(&store, tmp.path(), false).unwrap();
        assert_eq!(first.imported, 5);
        let second = migrate_knowledge(&store, tmp.path(), false).unwrap();
        assert_eq!(second.imported, 0, "{second:?}");
        assert_eq!(second.deduped, 5);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        crate::store::git::ensure_repo(tmp.path()).unwrap();
        fixture_v2_db(tmp.path());
        let report = migrate_knowledge(&store, tmp.path(), true).unwrap();
        assert_eq!(report.imported, 5); // counted, not written
        assert!(store.load_all().unwrap().nodes.is_empty());
    }

    #[test]
    fn missing_v2_db_is_a_clean_noop() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        let report = migrate_knowledge(&store, tmp.path(), false).unwrap();
        assert_eq!(report.imported, 0);
        assert!(report.v2_db_missing);
    }
}
```

- [ ] **Step 3: Run tests, verify they fail** (`migrate_knowledge` undefined): `cargo test -p retro-core migrate` → compile error.

- [ ] **Step 4: Implement `migrate.rs`.**

```rust
//! v2 -> v3 migration: read the v2 SQLite knowledge (read-only), import it
//! into the markdown store, and clean up v1/v2 environment artifacts.
//! Fully self-contained (raw rusqlite, own hook/launchd helpers) so deleting
//! the v2 modules cannot break it. retro.db is NEVER modified — rollback is
//! "keep using the 2.x binary".

use std::path::Path;

use crate::errors::CoreError;
use crate::store::{self, Node, NodeType, Scope, Store};
use crate::util::normalized_similarity;

#[derive(Debug, Default)]
pub struct MigrateReport {
    pub imported: usize,
    pub deduped: usize,
    pub skipped_status: usize,   // dismissed/archived
    pub skipped_invalid: usize,  // unknown type/scope/slug
    pub v2_db_missing: bool,
}

struct V2Node {
    id: String,
    node_type: String,
    scope: String,
    project_id: Option<String>,
    content: String,
    confidence: f64,
    status: String,
    created_at: String,
    updated_at: String,
}

fn date_of(rfc3339: &str) -> chrono::NaiveDate {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|d| d.date_naive())
        .unwrap_or_else(|_| chrono::Utc::now().date_naive())
}

/// Import v2 nodes into the v3 store. `dry_run` counts without writing.
pub fn migrate_knowledge(
    store: &Store,
    retro_dir: &Path,
    dry_run: bool,
) -> Result<MigrateReport, CoreError> {
    let mut report = MigrateReport::default();
    let db_path = retro_dir.join("retro.db");
    if !db_path.exists() {
        report.v2_db_missing = true;
        return Ok(report);
    }
    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| CoreError::Io(format!("opening v2 db read-only: {e}")))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, type, scope, project_id, content, confidence, status, created_at, updated_at
             FROM nodes",
        )
        .map_err(|e| CoreError::Io(format!("querying v2 nodes: {e}")))?;
    let v2_nodes: Vec<V2Node> = stmt
        .query_map([], |r| {
            Ok(V2Node {
                id: r.get(0)?, node_type: r.get(1)?, scope: r.get(2)?,
                project_id: r.get(3)?, content: r.get(4)?, confidence: r.get(5)?,
                status: r.get(6)?, created_at: r.get(7)?, updated_at: r.get(8)?,
            })
        })
        .map_err(|e| CoreError::Io(format!("reading v2 nodes: {e}")))?
        .filter_map(Result::ok)
        .collect();

    // Existing v3 bodies per scope, for dedup (and rerun idempotency).
    let existing = store.load_all()?;
    let mut bodies: Vec<(Scope, String)> = existing
        .nodes.iter().map(|(_, n)| (n.scope.clone(), n.body.clone())).collect();

    for v2 in v2_nodes {
        match v2.status.as_str() {
            "active" | "pending_review" => {}
            _ => { report.skipped_status += 1; continue; }
        }
        let node_type = match v2.node_type.as_str() {
            "rule" | "directive" => NodeType::Rule,
            "preference" => NodeType::Preference,
            "pattern" | "skill" => NodeType::Pattern,
            "memory" => NodeType::Memory,
            _ => { report.skipped_invalid += 1; continue; }
        };
        let scope = match v2.scope.as_str() {
            "global" => Scope::Global,
            "project" => match &v2.project_id {
                Some(p) if store::is_valid_slug(p) => Scope::Project(p.clone()),
                Some(p) => {
                    let s = store::slugify(p);
                    if store::is_valid_slug(&s) { Scope::Project(s) }
                    else { report.skipped_invalid += 1; continue; }
                }
                None => { report.skipped_invalid += 1; continue; }
            },
            _ => { report.skipped_invalid += 1; continue; }
        };
        let is_dup = bodies.iter().any(|(s, b)| {
            *s == scope && normalized_similarity(b, &v2.content) > 0.8
        });
        if is_dup { report.deduped += 1; continue; }

        report.imported += 1;
        if !dry_run {
            let base: String = v2.content.split_whitespace().take(8).collect::<Vec<_>>().join(" ");
            let id = store.unique_slug(&store::slugify(&base), &scope);
            let node = Node {
                id, scope: scope.clone(), node_type,
                confidence: v2.confidence.clamp(0.0, 1.0),
                sources: vec![format!("v2:{}", v2.id)],
                created: date_of(&v2.created_at),
                updated: date_of(&v2.updated_at),
                invalidated_by: None,
                body: v2.content.clone(),
            };
            store.write_node(&node)?;
        }
        bodies.push((scope, v2.content));
    }
    Ok(report)
}

/// Registered project paths from BOTH generations, for the v1 hook sweep:
/// the v2 projects table plus the v3 path map. Missing db/table tolerated.
pub fn all_known_project_paths(retro_dir: &Path) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let db_path = retro_dir.join("retro.db");
    if db_path.exists() {
        if let Ok(conn) = rusqlite::Connection::open_with_flags(
            &db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) {
            if let Ok(mut stmt) = conn.prepare("SELECT path FROM projects") {
                if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                    paths.extend(rows.filter_map(Result::ok));
                }
            }
        }
    }
    if let Ok(map) = crate::store::projects::PathMap::load(retro_dir) {
        paths.extend(map.paths.values().cloned());
    }
    paths.sort();
    paths.dedup();
    paths
}
```

(One unfiltered query; status filtering happens in Rust so the skipped counts stay visible.)

- [ ] **Step 5: Verify the actual v2 enum strings.** Open `crates/retro-core/src/models.rs`, find the v2 `NodeType`/`NodeStatus`/`NodeScope` serialization (`as_str()`/Display/serde rename). If any serialized form differs from the match arms above (e.g. `PendingReview` serializes as `"pending-review"`), fix BOTH the match arms and the fixture inserts to the real strings. Note the finding in your report.

- [ ] **Step 6: Run tests until green:** `cargo test -p retro-core migrate` → 4 passed. `rustfmt --edition 2024 crates/retro-core/src/migrate.rs`.

- [ ] **Step 7: CLI command.** `crates/retro-cli/src/commands/migrate.rs`:

```rust
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
    store.ensure_layout()?;
    retro_core::store::git::ensure_repo(&dir)?;

    let label = if dry_run { " (dry run — nothing written)" } else { "" };
    println!("retro migrate{label}");
    let report = retro_core::migrate::migrate_knowledge(&store, &dir, dry_run)?;
    if report.v2_db_missing {
        println!("  no v2 database found — nothing to import");
    } else {
        println!(
            "  knowledge: {} imported, {} already present (deduped), {} skipped (dismissed/archived), {} skipped (invalid)",
            report.imported.to_string().green(), report.deduped,
            report.skipped_status, report.skipped_invalid
        );
    }
    // Tasks 2 and 3 extend this function (safety-import; env cleanup; commit;
    // reindex; projection). Keep this ordering comment until they land.
    if dry_run {
        println!("\n  rollback note: migrate never modifies retro.db; store writes are git commits in {}", dir.display());
    }
    let _ = config; // used from Task 3 on
    Ok(())
}
```

Register in `commands/mod.rs` (`pub mod migrate;`) and `main.rs`:

```rust
    /// Migrate v2 knowledge and environment to v3 (idempotent, v2 db untouched)
    Migrate {
        /// Preview without writing
        #[arg(long)]
        dry_run: bool,
    },
```
arm: `Commands::Migrate { dry_run } => commands::migrate::run(dry_run),`

- [ ] **Step 8: Full test + commit:**

```bash
cargo test && git add crates/retro-core/src/migrate.rs crates/retro-core/src/lib.rs crates/retro-core/src/util.rs crates/retro-core/src/analysis/merge.rs crates/retro-core/src/lint.rs crates/retro-cli/src/commands/migrate.rs crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs && git commit -m "feat(v3): retro migrate — v2 knowledge import (idempotent, read-only source)"
```
Expected: 404 baseline + 4 new = 408 passed.

---

### Task 2: Managed-block safety-import (migrate + init)

The 2026-07-13 dogfood incident: enabling v3 without importing existing knowledge let the first projection WIPE 41 v2-era rules from the global CLAUDE.md (recovered from backups). This task makes that impossible from either entry point.

**Files:**
- Modify: `crates/retro-core/src/migrate.rs` (add `import_managed_rules` + wire into `migrate_knowledge` flow via a new `pub fn safety_import`)
- Modify: `crates/retro-cli/src/commands/migrate.rs` (call it)
- Modify: `crates/retro-cli/src/commands/init.rs` (call it in `init_v3`, after `ensure_layout`/`index::build`, before enabling anything)

- [ ] **Step 1: Failing tests** (in `migrate.rs` tests):

```rust
    #[test]
    fn safety_import_rescues_managed_rules_not_in_store() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        std::fs::write(
            claude.path().join("CLAUDE.md"),
            "<!-- retro:managed:start -->\n## Retro-Discovered Patterns\n\n- Rule one from the old days\n- Rule two survives too\n\n<!-- retro:managed:end -->\n",
        ).unwrap();
        let imported = safety_import(&store, &claude.path().join("CLAUDE.md"), &Scope::Global, false).unwrap();
        assert_eq!(imported, 2);
        // idempotent: rerun imports nothing
        let again = safety_import(&store, &claude.path().join("CLAUDE.md"), &Scope::Global, false).unwrap();
        assert_eq!(again, 0);
        let nodes = store.load_all().unwrap().nodes;
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().all(|(_, n)| n.node_type == NodeType::Rule && (n.confidence - 0.8).abs() < 1e-9));
    }

    #[test]
    fn safety_import_noop_without_managed_section() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        std::fs::write(claude.path().join("CLAUDE.md"), "# my own file\n").unwrap();
        assert_eq!(safety_import(&store, &claude.path().join("CLAUDE.md"), &Scope::Global, false).unwrap(), 0);
        assert_eq!(safety_import(&store, &tmp.path().join("nope.md"), &Scope::Global, false).unwrap(), 0);
    }
```

- [ ] **Step 2: Run, verify fail** (`safety_import` undefined).

- [ ] **Step 3: Implement** in `migrate.rs`:

```rust
/// Import managed-block bullets that exist in a CLAUDE.md but not in the
/// store, as rule nodes at 0.8 (the v2 reconcile-import convention). This is
/// the guard against the "first projection wipes pre-v3 rules" failure.
pub fn safety_import(
    store: &Store,
    claude_md: &Path,
    scope: &Scope,
    dry_run: bool,
) -> Result<usize, CoreError> {
    let Ok(content) = std::fs::read_to_string(claude_md) else { return Ok(0) };
    let Some(rules) = crate::projection::claude_md::read_managed_section(&content) else {
        return Ok(0);
    };
    let existing = store.load_all()?;
    let bodies: Vec<String> = existing
        .nodes.iter()
        .filter(|(_, n)| n.scope == *scope)
        .map(|(_, n)| n.body.clone())
        .collect();
    let today = chrono::Utc::now().date_naive();
    let mut imported = 0;
    for rule in rules {
        if bodies.iter().any(|b| normalized_similarity(b, &rule) > 0.8) {
            continue;
        }
        imported += 1;
        if !dry_run {
            let base: String = rule.split_whitespace().take(8).collect::<Vec<_>>().join(" ");
            let id = store.unique_slug(&store::slugify(&base), scope);
            store.write_node(&Node {
                id, scope: scope.clone(), node_type: NodeType::Rule,
                confidence: 0.8, sources: vec!["managed-import".to_string()],
                created: today, updated: today, invalidated_by: None, body: rule,
            })?;
        }
    }
    Ok(imported)
}
```

- [ ] **Step 4: Tests green:** `cargo test -p retro-core migrate` → 6 passed.

- [ ] **Step 5: Wire into the migrate command** (after the knowledge line in `commands/migrate.rs`): global CLAUDE.md via `config.claude_dir().join("CLAUDE.md")` with `Scope::Global`; then each `PathMap` entry's `<path>/CLAUDE.md` with `Scope::Project(slug)`. Print `  safety-import: {n} rule(s) rescued from managed blocks`.

- [ ] **Step 6: Wire into `init_v3`** (`commands/init.rs`, after `index::build(...)`, before the config-enable block): same global call, print only when n > 0: `  Imported {n} existing rule(s) from your CLAUDE.md managed section`. **Verify while there:** `init_v3` writes hooks to `config.claude_dir().join("settings.json")` (config-driven), NOT a hardcoded `~/.claude`. If it is hardcoded, change it to `config.claude_dir()` — scenario isolation (Task 9) depends on this. Report which you found.

- [ ] **Step 7: Full test + commit:**

```bash
cargo test && git add crates/retro-core/src/migrate.rs crates/retro-cli/src/commands/migrate.rs crates/retro-cli/src/commands/init.rs && git commit -m "feat(v3): managed-block safety-import in migrate and init"
```
Expected: 410 passed.

---

### Task 3: Migrate environment cleanup + finish the flow

**Files:**
- Modify: `crates/retro-core/src/migrate.rs` (env-cleanup helpers)
- Modify: `crates/retro-cli/src/commands/migrate.rs` (cleanup + commit + reindex + projection)

- [ ] **Step 1: Failing tests** for the two testable helpers (launchd removal is macOS-external; keep it thin and untested):

```rust
    #[test]
    fn v1_hook_sweep_strips_marker_pairs_and_leaves_user_lines() {
        let repo = TempDir::new().unwrap();
        let hooks = repo.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("post-commit"),
            "#!/bin/sh\necho mine\n# retro hook - do not remove\nretro ingest --auto 2>>~/.retro/hook-stderr.log &\n").unwrap();
        std::fs::write(hooks.join("post-merge"),
            "#!/bin/sh\n# retro hook - do not remove\nretro analyze --auto &\n").unwrap();
        let removed = remove_v1_hooks(repo.path().to_str().unwrap());
        assert_eq!(removed, vec!["post-commit".to_string(), "post-merge".to_string()]);
        let pc = std::fs::read_to_string(hooks.join("post-commit")).unwrap();
        assert!(pc.contains("echo mine") && !pc.contains("retro hook"));
        assert!(!hooks.join("post-merge").exists(), "shebang-only hook is deleted");
        // idempotent
        assert!(remove_v1_hooks(repo.path().to_str().unwrap()).is_empty());
    }

    #[test]
    fn untrack_ignored_entries_removes_previously_committed_state() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        crate::store::git::ensure_repo(tmp.path()).unwrap();
        // simulate a poisoned store: force-track a machine-local file
        std::fs::write(tmp.path().join("health.json"), "{}").unwrap();
        let force = std::process::Command::new("git")
            .args(["-C", tmp.path().to_str().unwrap(), "add", "-f", "health.json"])
            .output().unwrap();
        assert!(force.status.success());
        std::process::Command::new("git")
            .args(["-C", tmp.path().to_str().unwrap(), "commit", "-m", "poisoned"])
            .output().unwrap();

        let untracked = untrack_ignored_entries(tmp.path()).unwrap();
        assert!(untracked, "should have removed at least one tracked entry");
        let ls = std::process::Command::new("git")
            .args(["-C", tmp.path().to_str().unwrap(), "ls-files"])
            .output().unwrap();
        assert!(!String::from_utf8_lossy(&ls.stdout).contains("health.json"));
        assert!(tmp.path().join("health.json").exists(), "--cached keeps the file on disk");
        assert!(!untrack_ignored_entries(tmp.path()).unwrap(), "idempotent");
    }
```

(Note the fixture uses `git add -f` deliberately — it simulates a pre-fix store; this is a test-only exception to the no-force-add rule.)

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement in `migrate.rs`:**

```rust
const HOOK_MARKER: &str = "# retro hook - do not remove";

/// Strip v1 retro hook lines (marker + the following line) from a repo's
/// post-commit/post-merge hooks. Returns which hooks were modified. A hook
/// left with only a shebang/blank lines is deleted outright.
pub fn remove_v1_hooks(repo_root: &str) -> Vec<String> {
    let mut removed = Vec::new();
    for name in ["post-commit", "post-merge"] {
        let path = Path::new(repo_root).join(".git/hooks").join(name);
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        if !content.contains(HOOK_MARKER) { continue; }
        let mut out: Vec<&str> = Vec::new();
        let mut skip_next = false;
        for line in content.lines() {
            if skip_next { skip_next = false; continue; }
            if line.trim() == HOOK_MARKER { skip_next = true; continue; }
            out.push(line);
        }
        let remaining = out.join("\n");
        let only_boilerplate = out.iter().all(|l| l.trim().is_empty() || l.starts_with("#!"));
        if only_boilerplate {
            let _ = std::fs::remove_file(&path);
        } else {
            let _ = std::fs::write(&path, remaining + "\n");
        }
        removed.push(name.to_string());
    }
    removed
}

/// `git rm -r --cached` every IGNORED_ENTRIES path that an older binary may
/// have committed before the ignore rules existed. Returns true if anything
/// was untracked (caller commits). `--ignore-unmatch` makes it idempotent.
pub fn untrack_ignored_entries(store_root: &Path) -> Result<bool, CoreError> {
    let mut any = false;
    for entry in crate::store::IGNORED_ENTRIES {
        let e = entry.trim_end_matches('/');
        let out = std::process::Command::new("git")
            .arg("-C").arg(store_root)
            .args(["rm", "-r", "--cached", "--ignore-unmatch", "--quiet", e])
            .output()
            .map_err(|e| CoreError::Io(e.to_string()))?;
        if !out.status.success() {
            continue; // pathspec oddity — non-fatal, entry stays for next run
        }
        // rm --cached stages deletions; detect via diff --cached
        let staged = std::process::Command::new("git")
            .arg("-C").arg(store_root)
            .args(["diff", "--cached", "--quiet"])
            .status()
            .map_err(|e| CoreError::Io(e.to_string()))?;
        if !staged.success() { any = true; }
    }
    if any {
        let out = std::process::Command::new("git")
            .arg("-C").arg(store_root)
            .args(["commit", "-m", "retro: untrack machine-local files (migrate)"])
            .output()
            .map_err(|e| CoreError::Io(e.to_string()))?;
        if !out.status.success() {
            return Err(CoreError::Io(format!(
                "committing untrack: {}", String::from_utf8_lossy(&out.stderr))));
        }
    }
    Ok(any)
}

/// Boot out + delete the v2 launchd runner. Both steps tolerate absence.
/// Returns true if the plist file was removed.
pub fn remove_v2_launchd() -> bool {
    let uid = unsafe { libc::getuid() };
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/com.retro.runner")])
        .output();
    let Ok(home) = std::env::var("HOME") else { return false };
    let plist = Path::new(&home).join("Library/LaunchAgents/com.retro.runner.plist");
    std::fs::remove_file(&plist).is_ok()
}
```

`IGNORED_ENTRIES` is currently `pub(crate)` in `store/mod.rs` — that is visible to `migrate.rs` (same crate); no change needed.

- [ ] **Step 4: Tests green:** `cargo test -p retro-core migrate` → 8 passed.

- [ ] **Step 5: Finish the migrate command flow** in `commands/migrate.rs`, replacing the ordering comment. Final order (skip everything mutating when `dry_run`):
  1. knowledge import (Task 1)
  2. safety-import global + projects (Task 2)
  3. `untrack_ignored_entries(&dir)` → print if cleaned
  4. v1 hook sweep: `for p in retro_core::migrate::all_known_project_paths(&dir) { let r = retro_core::migrate::remove_v1_hooks(&p); ... }` → print total hooks removed across repos
  5. `remove_v2_launchd()` → print if removed (macOS only — guard with `#[cfg(target_os = "macos")]` or `cfg!(target_os = "macos")`)
  6. `store::git::commit_all(&dir, "retro: migrate v2 knowledge")` → `index::build(&store)` → project: `projection::local_md::project_global_md(&store, &config.claude_dir().join("CLAUDE.md"), config.knowledge.confidence_threshold, Some(&dir.join("backups")))` and `project_local_md` for each PathMap project
  7. Final summary block, ending with: `  v2 database preserved at {dir}/retro.db — safe to delete once you trust the store (rollback: the 2.x binary still reads it)`
  In dry-run, print what WOULD happen for steps 3–6 (counts from the pure fns where cheap: hook sweep can report per-repo marker presence by reading files without writing — acceptable to just print "would sweep N repos").

- [ ] **Step 6: Full test + commit:**

```bash
cargo test && git add crates/retro-core/src/migrate.rs crates/retro-cli/src/commands/migrate.rs && git commit -m "feat(v3): migrate environment cleanup — launchd, v1 hooks, poisoned-store untrack"
```
Expected: 412 passed.

---

### Task 4: Notification cap

29 notifications piled up on the real machine (they only drain when a session starts). Cap the vec, newest-first retention.

**Files:**
- Modify: `crates/retro-core/src/store/state.rs`

- [ ] **Step 1: Failing test:**

```rust
    #[test]
    fn notifications_capped_at_50_keeping_newest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut st = RunnerState::default();
        for i in 0..60 { st.notifications.push(format!("note {i}")); }
        st.save(tmp.path()).unwrap();
        let loaded = RunnerState::load(tmp.path()).unwrap();
        assert_eq!(loaded.notifications.len(), 50);
        assert_eq!(loaded.notifications.first().unwrap(), "note 10"); // oldest 10 dropped
        assert_eq!(loaded.notifications.last().unwrap(), "note 59");
    }
```

- [ ] **Step 2: Run, verify fail** (60 survive today).

- [ ] **Step 3: Implement** in `RunnerState::save`, before serialization:

```rust
        // Briefings drain these, but a machine with no new sessions can pile
        // them up forever — keep only the newest 50.
        const MAX_NOTIFICATIONS: usize = 50;
        if self.notifications.len() > MAX_NOTIFICATIONS {
            let drop = self.notifications.len() - MAX_NOTIFICATIONS;
            self.notifications.drain(..drop);
        }
```
`save` currently takes `&self` — change to `&mut self` and fix the (few) call sites, OR clone-truncate locally without mutating. Prefer `&mut self` (call sites: runner_v3, observe, brief, lint, api — compiler lists them; all already own `mut state`). Verify each call site still compiles.

- [ ] **Step 4: Tests green, full suite, commit:**

```bash
cargo test && git add crates/retro-core/src/store/state.rs <any touched call sites> && git commit -m "fix(v3): cap briefing notifications at 50, newest kept"
```
Expected: 413 passed.

---

### Task 5: `retro uninstall`

**Files:**
- Modify: `crates/retro-core/src/claude_settings.rs` (+`remove_hook`)
- Modify: `crates/retro-core/src/projection/claude_md.rs` (+`strip_managed_section`)
- Create: `crates/retro-cli/src/commands/uninstall.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs`, `main.rs`

- [ ] **Step 1: Failing tests.** In `claude_settings.rs` (mirror `ensure_hook`'s test style):

```rust
    #[test]
    fn remove_hook_strips_matching_entries_and_leaves_others() {
        let mut settings = serde_json::json!({});
        ensure_hook(&mut settings, "SessionEnd", "/some/retro observe");
        ensure_hook(&mut settings, "SessionEnd", "~/.masko-desktop/hooks/hook-sender.sh");
        assert!(remove_hook(&mut settings, "SessionEnd", "retro observe"));
        let rendered = settings.to_string();
        assert!(!rendered.contains("retro observe"));
        assert!(rendered.contains("hook-sender.sh"), "unrelated hooks preserved");
        assert!(!remove_hook(&mut settings, "SessionEnd", "retro observe"), "idempotent");
    }
```

In `projection/claude_md.rs`:

```rust
    #[test]
    fn strip_managed_section_removes_block_keeps_user_content() {
        let content = "# Mine\n\n<!-- retro:managed:start -->\n- a rule\n<!-- retro:managed:end -->\n\n## Also mine\n";
        let out = strip_managed_section(content);
        assert!(out.contains("# Mine") && out.contains("## Also mine"));
        assert!(!out.contains("retro:managed") && !out.contains("a rule"));
        assert_eq!(strip_managed_section("no block here\n"), "no block here\n");
    }
```

- [ ] **Step 2: Run, verify fail.**

- [ ] **Step 3: Implement.** In `claude_settings.rs` (verify the group shape against what `ensure_hook` writes — `hooks.<event>` is an array of groups, each with a `hooks` array of `{type, command}` objects — and adapt if it differs):

```rust
/// Remove every hook whose command contains `needle` from the event's
/// groups. Emptied groups (and an emptied event key) are dropped so the
/// settings stay tidy. Returns true if anything was removed.
pub fn remove_hook(settings: &mut serde_json::Value, event: &str, needle: &str) -> bool {
    let Some(groups) = settings
        .get_mut("hooks")
        .and_then(|h| h.get_mut(event))
        .and_then(|e| e.as_array_mut())
    else {
        return false;
    };
    let mut removed = false;
    for group in groups.iter_mut() {
        if let Some(hooks) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            let before = hooks.len();
            hooks.retain(|h| {
                !h.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.contains(needle))
            });
            removed |= hooks.len() != before;
        }
    }
    groups.retain(|g| {
        g.get("hooks").and_then(|h| h.as_array()).is_none_or(|h| !h.is_empty())
    });
    if groups.is_empty() {
        if let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) {
            hooks.remove(event);
        }
    }
    removed
}
```

In `projection/claude_md.rs`:

```rust
/// Remove the managed block (markers inclusive) plus one adjacent trailing
/// blank line; user content around it is untouched. No block -> unchanged.
pub fn strip_managed_section(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim() == MANAGED_START) else {
        return content.to_string();
    };
    let Some(end_rel) = lines[start..].iter().position(|l| l.trim() == MANAGED_END) else {
        return content.to_string();
    };
    let mut end = start + end_rel;
    if lines.get(end + 1).map(|l| l.trim().is_empty()).unwrap_or(false) {
        end += 1;
    }
    let mut out: Vec<&str> = Vec::new();
    out.extend(&lines[..start]);
    out.extend(&lines[end + 1..]);
    let mut s = out.join("\n");
    if content.ends_with('\n') && !s.ends_with('\n') {
        s.push('\n');
    }
    s
}
```

- [ ] **Step 4: Tests green.**

- [ ] **Step 5: The command.** `commands/uninstall.rs`:

```rust
use anyhow::Result;
use retro_core::config::{Config, retro_dir};

/// Remove retro from the machine: Claude Code hooks, projected content,
/// launchd remnants. The store survives unless --purge (with confirmation).
pub fn run(purge: bool) -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml"))?;
    let claude_dir = config.claude_dir();
    // Projection files race the runner — same lock discipline as migrate.
    let Some(_lock) = retro_core::lock::LockFile::try_acquire(&dir.join("run.lock")) else {
        anyhow::bail!("a retro run is in progress — retry shortly");
    };

    // 1. Claude Code hooks out of settings.json (backup first)
    let settings_path = claude_dir.join("settings.json");
    if settings_path.exists() {
        retro_core::util::backup_file(&settings_path, &dir.join("backups"))?;
        let raw = std::fs::read_to_string(&settings_path)?;
        let mut settings: serde_json::Value = serde_json::from_str(&raw)?;
        let a = retro_core::claude_settings::remove_hook(&mut settings, "SessionEnd", "retro observe");
        let b = retro_core::claude_settings::remove_hook(&mut settings, "SessionStart", "retro brief");
        if a || b {
            std::fs::write(&settings_path, serde_json::to_string_pretty(&settings)?)?;
            println!("  removed SessionEnd/SessionStart hooks from {}", settings_path.display());
        }
    }

    // 2. Projected content: strip global managed block, delete CLAUDE.local.md files
    let global_md = claude_dir.join("CLAUDE.md");
    if global_md.exists() {
        retro_core::util::backup_file(&global_md, &dir.join("backups"))?;
        let content = std::fs::read_to_string(&global_md)?;
        let stripped = retro_core::projection::claude_md::strip_managed_section(&content);
        if stripped != content {
            std::fs::write(&global_md, stripped)?;
            println!("  removed managed section from {}", global_md.display());
        }
    }
    if let Ok(map) = retro_core::store::projects::PathMap::load(&dir) {
        for (slug, path) in &map.paths {
            let local = std::path::Path::new(path).join("CLAUDE.local.md");
            if local.exists() && std::fs::remove_file(&local).is_ok() {
                println!("  removed {} ({slug})", local.display());
            }
        }
    }

    // 3. v1/v2 remnants (idempotent, tolerate absence)
    if cfg!(target_os = "macos") && retro_core::migrate::remove_v2_launchd() {
        println!("  removed v2 launchd runner");
    }
    for p in retro_core::migrate::all_known_project_paths(&dir) {
        for h in retro_core::migrate::remove_v1_hooks(&p) {
            println!("  removed v1 {h} hook in {p}");
        }
    }

    // 4. The store itself — only with --purge, only with explicit consent
    if purge {
        println!("\n  --purge deletes {} INCLUDING its git history (unpushed knowledge is unrecoverable).", dir.display());
        print!("  Type 'yes' to confirm: ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim() == "yes" {
            std::fs::remove_dir_all(&dir)?;
            println!("  removed {}", dir.display());
        } else {
            println!("  purge cancelled");
        }
    } else {
        println!("\n  store kept at {} (use --purge to delete it)", dir.display());
    }
    println!("\nretro uninstalled.");
    Ok(())
}
```

`main.rs` variant:

```rust
    /// Remove retro (hooks, projections, launchd). Store kept unless --purge
    Uninstall {
        /// Also delete ~/.retro (asks for confirmation)
        #[arg(long)]
        purge: bool,
    },
```
arm: `Commands::Uninstall { purge } => commands::uninstall::run(purge),`

- [ ] **Step 6: Full test + commit:**

```bash
cargo test && git add crates/retro-core/src/claude_settings.rs crates/retro-core/src/projection/claude_md.rs crates/retro-cli/src/commands/uninstall.rs crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs && git commit -m "feat(v3): retro uninstall — hooks, projections, remnants; store kept unless --purge"
```
Expected: 415 passed.

---

### Task 6: Deletion wave 1 — v1/v2 CLI surface

Compiler-driven. After this task the CLI is: `init [--from] | migrate | run | observe | brief | reindex | status | doctor | lint | ui | uninstall`.

**Files:**
- Delete: `crates/retro-cli/src/commands/{analyze,apply,audit,clean,curate,dash,diff,hooks,ingest,log,patterns,review,start,stop,sync}.rs`, `crates/retro-cli/src/launchd.rs`, `crates/retro-cli/src/tui/` (whole dir)
- Delete: `crates/retro-cli/tests/{dash_fixture.rs,run_pipeline.rs,v2_types_accessible.rs}`
- Modify: `main.rs`, `commands/mod.rs`, `commands/{run,status,init}.rs`

- [ ] **Step 1: Delete the files listed above** (`git rm`).
- [ ] **Step 2: `main.rs`:** remove the deleted variants + `HooksAction`; remove `mod launchd;`/`mod tui;` if declared; `Init` loses `--uninstall`, `--purge`, `--v3` (keep `--from`); `Run` keeps `--dry-run --background`, drops nothing else; `Ingest/Analyze/Apply` auto-flag `is_auto` logic shrinks to `Observe | Brief | Run{background:true}`.
- [ ] **Step 3: `commands/mod.rs`:** remove the deleted `pub mod` lines; slim `check_and_display_nudge` to ONLY the v3 block (health warnings + stale-queue) — delete `AutoRunSummary`, `aggregate_auto_runs`, `format_time_ago`, `display_auto_run`, the db/audit tail, `warn_auto_deprecated`, `warn_command_deprecated`, `within_cooldown`. Keep `git_root_or_cwd` only if something still calls it (grep; `observe.rs` is the likely caller).
- [ ] **Step 4: `commands/run.rs`:** delete the entire v2 path (`run_for_project` and every helper below the v3 `return Ok(())`); the function becomes: load config → backend → `run_v3` → print summary. Remove the `config.v3.enabled` check itself — v3 is unconditional now (config cleanup happens in Task 8; using `config.v3.enabled` must be GONE from this file).
- [ ] **Step 5: `commands/status.rs`:** delete the v2 branch (db reads, `runner::` calls); `print_v3_status` becomes the whole body; drop the enabled check. **`commands/{observe,brief,lint,ui,doctor}.rs`:** remove their `config.v3.enabled` bail/skip guards. Where a missing store would now panic or error confusingly, add a friendly early exit: observe/brief → silent `Ok(())` when `!dir.join("knowledge").exists()` (hooks may fire on machines that never ran init); doctor/lint/ui/status → `bail!("retro is not initialized — run `retro init`")`.
- [ ] **Step 6: `commands/init.rs`:** delete the v2 body (`install_hooks`, launchd install, `install_briefing_hook`, db registration, v2 reconcile) and `run_uninstall` (replaced by Task 5); `run(from)` = the `init_v3` flow (+ Task 2's safety-import). Keep settings backup and `--from`.
- [ ] **Step 7: Iterate `cargo check -p retro-cli` until clean, then `cargo test`.** Expected fallout: retro-cli loses launchd(6) + log(5) + run v2(5) + tui(7) unit tests and all 6 integration tests → retro-cli ≈ 34 remaining (33 + ui/mod 1 + new cmd tests − deleted; report the exact number). retro-core untouched this task.
- [ ] **Step 8: Commit:**

```bash
cargo test && git add -A crates/retro-cli && git commit -m "feat(v3)!: delete v1/v2 CLI — init/run/status are v3-only"
```

---

### Task 7: Deletion wave 2 — retro-core v2 modules

**Files:**
- Delete: `crates/retro-core/src/{db.rs,git.rs,reconcile.rs,curator.rs,audit_log.rs,runner.rs,trust.rs}`, `analysis/merge.rs`, `ingest/{context.rs,history.rs}`, `projection/{skill.rs,global_agent.rs}`
- Modify: `lib.rs`, `ingest/mod.rs`, `projection/mod.rs`, `briefing.rs`, `analysis/mod.rs` (imports only this wave)

- [ ] **Step 1: Delete files; fix `lib.rs` module decls.** `analysis/merge.rs` is deletable because Task 1 moved the similarity fns to `util.rs` (grep for `analysis::merge` first — the only remaining users are v2 files also dying in this wave).
- [ ] **Step 2: `ingest/mod.rs`:** keep `pub mod session;`, `encode_project_path`, and whatever `runner_v3`/`ui/api` actually import (grep `retro_core::ingest::` and `crate::ingest::`); delete `ingest_project`, `ingest_all_projects`, `recover_project_path`, db-coupled helpers, `pub mod context/history`.
- [ ] **Step 3: `projection/mod.rs`:** reduce to `pub mod claude_md; pub mod local_md;` (apply-plan machinery deleted).
- [ ] **Step 4: `briefing.rs`:** keep `build_v3_briefing` (+ its tests); delete `generate_briefing`/`write_briefing`/`read_briefing` and their tests.
- [ ] **Step 5: Iterate `cargo check` workspace-wide until clean; run `cargo test`.** Expected core fallout this wave: db(56) + git(10) + reconcile(8) + curator(1) + runner(8) + trust(3) + merge(0 — tests moved in Task 1) + context(7) + skill(31) + global_agent(5) + projection/mod(16) + ingest/mod(~2) + briefing(3) ≈ **−150**; suite lands ≈ 265. Report exact.
- [ ] **Step 6: Commit:**

```bash
cargo test && git add -A crates/retro-core && git commit -m "feat(v3)!: delete v2 core — db, PR flow, reconcile, curator, skill projection"
```

---

### Task 8: Deletion wave 3 — models/analysis/config slim, gate removal, projectors crate

**Files:**
- Modify: `crates/retro-core/src/{models.rs,config.rs}`, `analysis/{mod.rs,prompts.rs}`, `projection/claude_md.rs`, `doctor.rs`
- Delete: `crates/retro-projectors/` (verify first), root `Cargo.toml` member entry

- [ ] **Step 1: `models.rs` slim.** KEEP (v3 uses them — verify each with grep before assuming): `Session`, `SessionMetadata`, `ParsedUserMessage`, the session-entry parse types (`SessionEntry`, `ContentBlock`, `UserEntry`, `AssistantEntry`, `ToolResultContent`), `GraphOperation`, `GraphAnalysisResponse`, `GraphOperationResponse`, v2 `NodeType`/`NodeScope`/`KnowledgeNode` (the analysis-prompt shim used by `analysis/v3.rs` + `parse_graph_response`), `EdgeType` if the graph response references it, `ClaudeCliOutput`/`CliUsage`. DELETE: `Pattern`, `PatternStatus`, `Projection`, `ProjectionStatus`, `ApplyAction/ApplyTrack/ApplyPlan`, `AnalysisResponse`/`PatternUpdate`, `ClaudeMdEdit` + edit enums, skill/agent draft types, v2 `NodeStatus`, `KnowledgeEdge`, `KnowledgeProject`, `AuditEntry` if audit_log is gone. Delete their tests.
- [ ] **Step 2: `analysis/mod.rs` + `prompts.rs` slim.** KEEP: `parse_graph_response`, `parse_graph_response_full`, `GRAPH_ANALYSIS_RESPONSE_SCHEMA`, `BATCH_SIZE`, `backend.rs`, `claude_cli.rs`, `v3.rs`, `build_graph_analysis_prompt`, `to_compact_session`, `MAX_USER_MSG_LEN`. DELETE: `analyze`, `analyze_v2`, `ANALYSIS_RESPONSE_SCHEMA`, `full_management_analysis_schema`, `build_analysis_prompt`, `build_context_summary`, `build_curate_prompt`, and their tests.
- [ ] **Step 3: `projection/claude_md.rs` slim.** KEEP: markers, `read_managed_section`, `build_managed_section`, `update_claude_md_content`, `has_managed_section`, `strip_managed_section` (Task 5). DELETE: `apply_edit`/`apply_edits`, `dissolve_managed_section`, their tests.
- [ ] **Step 4: `config.rs` slim + gate removal.** DELETE sections/structs: `[hooks]` (`HooksConfig`), `[trust]` (`TrustConfig` + nested), `[claude_md]` (`ClaudeMdConfig`), `[v3]` (`V3Config`) — serde ignores unknown keys in existing user configs, so old config.toml files still load. Grep `v3.enabled` workspace-wide: every remaining reference must go (run/observe/brief/lint/ui/doctor were done in Task 6 — this catches stragglers like `doctor.rs`'s v3-enabled check → replace with a "store initialized" check (`knowledge/` dir exists) and `commands/mod.rs`'s nudge guard → gate on store-dir existence instead). Also delete `[analysis]` fields only used by v2 (`rolling_window`; keep `window_days`, `staleness_days`, `confidence_threshold` — lint and runner_v3 use them; verify by grep) and `[hooks]`-era `runner` fields nothing reads anymore (grep each `runner.` field; keep `max_ai_calls_per_day`, `analysis_trigger`/`analysis_threshold` if runner_v3 reads them, `interval_seconds` DIES with launchd).
- [ ] **Step 5: retro-projectors.** `grep -r "retro-projectors\|retro_projectors" crates/ Cargo.toml` — expect zero non-self references. Delete `crates/retro-projectors/`, remove from workspace `members`, `cargo update --workspace` to sync the lock. (This task IS sanctioned to stage Cargo.lock.)
- [ ] **Step 6: Iterate `cargo check`/`cargo test` until green.** Expected fallout: models(~10) + analysis v2 tests(~6) + prompts(~5) + claude_md(~6) + config(~4) + projectors(4) ≈ −35; suite ≈ 230 (plus Tasks 1–5 additions ≈ +11 already counted). Report exact.
- [ ] **Step 7: Commit:**

```bash
cargo test && git add -A && git commit -m "feat(v3)!: remove v2 models, schemas, config sections, and the [v3] gate — v3 is the only pipeline"
```

---

### Task 9: Scenario tests rewrite

**Files:**
- Delete: `scenarios/{v2-backward-compat.md,v2-init-and-lifecycle.md,v2-pipeline-dry-run.md}`
- Create: `scenarios/{v3-init-and-lifecycle.md,v3-pipeline-dry-run.md,v3-migrate.md}`
- Modify: `scenarios/README.md` (isolation preamble)

Every scenario MUST begin with this isolation preamble in its Setup (add it to README.md as a hard rule too):

```
Setup (MANDATORY isolation — never touch the real environment):
  export RETRO_HOME=$(mktemp -d)
  export FAKE_CLAUDE=$(mktemp -d)
  cat > "$RETRO_HOME/config.toml" <<EOF
  [paths]
  claude_dir = "$FAKE_CLAUDE"
  EOF
  Use ./target/release/retro (or debug) — NEVER a PATH binary.
  NEVER run any retro command without RETRO_HOME set in this scenario.
```

- [ ] **Step 1: Write `scenarios/v3-init-and-lifecycle.md`.** Steps: `retro init` → expect store dirs (`knowledge/global`, `knowledge/projects`), git repo with initial commit, `$FAKE_CLAUDE/settings.json` containing `retro observe` (SessionEnd) + `retro brief` (SessionStart), settings backup in `$RETRO_HOME/backups/` when a settings.json pre-existed, config saved. Pre-seed `$FAKE_CLAUDE/CLAUDE.md` with a managed block of 2 bullets → expect init reports importing 2 rules (safety-import). Then `retro status` shows the v3 block; `retro doctor` exits 0 apart from claude-cli (may be absent in sandbox — Expected section should tolerate that check either way). Then `retro uninstall` → hooks gone from settings.json (other hooks preserved), managed section stripped, store still present; `retro uninstall --purge` fed `yes` on stdin → `$RETRO_HOME` gone. Not Expected: any write outside `$RETRO_HOME`/`$FAKE_CLAUDE`; any launchd/plist output.
- [ ] **Step 2: Write `scenarios/v3-pipeline-dry-run.md`.** Setup additionally seeds one global node file (copy the exact frontmatter format from `store/node.rs` docs) and runs `retro reindex`. Steps: `retro run --dry-run` → v3 summary line mentioning 0 sessions/0 AI calls, no store commit made (git log unchanged); `retro lint --dry-run` → scan count, no state.json write; `retro doctor` → store-repo/index checks pass; `retro ui --no-open` in background + `curl /api/ping` + `curl /api/nodes?scope=global` shows the seeded node + kill ONLY that PID. Not Expected: AI/claude invocations, real-path leakage.
- [ ] **Step 3: Write `scenarios/v3-migrate.md`.** Setup builds a fixture v2 `retro.db` via `sqlite3` heredoc (same schema/rows as Task 1's fixture — write the exact SQL in the scenario), plus a `$FAKE_CLAUDE/CLAUDE.md` managed block with 1 novel bullet. Steps: `retro migrate --dry-run` → counts shown, store still empty, retro.db untouched; `retro migrate` → N imported + 1 safety-imported, store commit exists, `retro.db` file byte-identical (compare checksum before/after); second `retro migrate` → 0 imported (idempotent). Not Expected: retro.db modified/deleted; duplicate nodes after rerun.
- [ ] **Step 4: Update `scenarios/README.md`:** isolation preamble as a hard rule; note the removed v2 scenarios; the run-scenarios skill drives these.
- [ ] **Step 5: Execute all three scenarios with the run-scenarios skill against a release build; fix whatever they surface; paste results.**
- [ ] **Step 6: Commit:**

```bash
git add scenarios/ && git commit -m "test(v3): scenario suite rewritten for v3 lifecycle, pipeline, and migrate"
```

---

### Task 10: README + CLAUDE.md rewrite

**Files:**
- Rewrite: `README.md`
- Rewrite: `CLAUDE.md` (v3-only)

- [ ] **Step 1: README.** Replace wholesale. Structure (write real prose, keep ~140 lines, no company names, no local paths):
  - `# retro` — tagline: personal context curator for Claude Code; watches your sessions via hooks, learns your rules, keeps CLAUDE.md/CLAUDE.local.md current. Files-as-truth markdown knowledge store, git-backed.
  - `## Quick Start` — `cargo install retro-cli` → `retro init` → work normally → `retro ui`. Mention `retro migrate` for 2.x users.
  - `## How It Works` — hooks (SessionEnd observe → queue; SessionStart brief → catch-up + briefing), budget-gated analysis via `claude -p`, one-way projection (global CLAUDE.md managed block + per-project CLAUDE.local.md via info/exclude), store layout (`~/.retro/knowledge/**.md`, frontmatter sample), git history as audit log.
  - `## Dashboard` — `retro ui`: X-ray, knowledge browse/search/invalidate, health, history; localhost-only.
  - `## Commands` — table of the eleven 3.0.0 commands with one-line purposes.
  - `## Configuration` — the surviving config sections with defaults (copy from Task 8's final config.rs).
  - `## Migrating from 2.x` — `retro migrate` (idempotent, retro.db read-only, what gets cleaned up), rollback note.
  - `## Requirements / Installation / License` — Rust, macOS/Linux (launchd no longer needed at all), claude CLI.
- [ ] **Step 2: CLAUDE.md.** Delete: the v2 five-layer architecture block, v1 three-stage block, the entire v1/v2 command rows, `--auto` note, launchd/TUI/pattern-discovery/apply-review-PR/full-CLAUDE.md-management sections, v1/v2 Implementation Status, v2-era Key Design Decisions that describe deleted code (launchd, TUI, review queue, PR flow, pattern accumulation, session-cap hooks). Keep/rewrite: repo structure (2 crates now), build/test, v3 architecture (hooks → queue → analysis → store → projection → surfaces), the 3.0.0 command table, AI-backend CLI quirks (still true), store design decisions (markdown nodes, index, machine-local state, budget), coding conventions (all still true — drop hook-format and PR-creation-flow bullets), testing section, Implementation Status: v3 Plans 1–4 DONE + release process. Update the "Clean install testing" line to `retro uninstall --purge && cargo build --release && ./target/release/retro init` **with a loud note that this must only ever be run with RETRO_HOME isolation during development**.
- [ ] **Step 3: Sweep for stragglers:** `grep -rn "retro dash\|retro start\|retro stop\|retro apply\|retro review\|launchd\|--auto" README.md CLAUDE.md docs/ scenarios/ crates/ --include="*.md" --include="*.rs" | grep -v superpowers/plans | grep -v superpowers/specs` — plans/specs are historical records (leave them); everything else must be current. Fix hits.
- [ ] **Step 4: Commit:**

```bash
git add README.md CLAUDE.md && git commit -m "docs: 3.0.0 — v3-only README and CLAUDE.md"
```

---

### Task 11: Version 3.0.0 + release prep

**Files:**
- Modify: `crates/retro-core/Cargo.toml`, `crates/retro-cli/Cargo.toml`, `Cargo.lock`

- [ ] **Step 1:** Set `version = "3.0.0"` in both crates; retro-cli's dep becomes `retro-core = { version = "3.0.0", path = "../retro-core" }` (match the existing dep syntax). `cargo update --workspace` to sync the lock (sanctioned Cargo.lock staging).
- [ ] **Step 2:** `cargo test` (all green — record final count) and `cargo build --release`; `./target/release/retro --help` → exactly the eleven commands; run `retro doctor` + `retro migrate --dry-run` against an ISOLATED RETRO_HOME (fixture from Task 9's migrate scenario) and paste output.
- [ ] **Step 3:** Commit:

```bash
git add crates/retro-core/Cargo.toml crates/retro-cli/Cargo.toml Cargo.lock && git commit -m "release: 3.0.0"
```

- [ ] **Step 4 (post-merge, manual — NOT in this branch):** after the PR merges: `git tag v3.0.0 && git push origin v3.0.0` — publish.yml verifies the tag matches both crate versions, tests, publishes retro-core then retro-cli, creates the GitHub release. Then the real-machine rollout: rebuild release, `cargo install --path`, **run `retro migrate` on the real machine** (this is the sanctioned exception to the never-run rule: user-approved, after dry-run output is reviewed), `retro doctor`.

---

### Final: whole-branch review

- [ ] Dispatch a final reviewer over `origin/main...HEAD`: company-name scan (blocking), plan-conformance sweep, v2-remnant grep (`db.rs`, `launchd`, `retro dash`, `pattern`, `--auto` in non-historical files), migrate idempotency + retro.db read-only guarantees re-verified, scenario isolation audit (no real-env writes), test suite + release build. Fix findings; then push + PR per the standard flow.

## Out of scope (backlog / Plan 5)

- Dashboard: node editing UI, one-click history revert, seen-but-unwatched coverage list, confidence filter
- AI-assisted lint stage; Linux hook parity audit; crates.io retro-projectors (crate deleted instead)
