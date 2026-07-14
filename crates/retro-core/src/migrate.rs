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
    pub skipped_status: usize,  // dismissed/archived
    pub skipped_invalid: usize, // unknown type/scope/slug
    pub v2_db_missing: bool,
    /// Bodies accepted this run (also on dry-run) — lets a following
    /// safety_import dedup against what the import would have written,
    /// keeping dry-run counts identical to a real run's.
    pub imported_bodies: Vec<(Scope, String)>,
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
    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
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
                id: r.get(0)?,
                node_type: r.get(1)?,
                scope: r.get(2)?,
                project_id: r.get(3)?,
                content: r.get(4)?,
                confidence: r.get(5)?,
                status: r.get(6)?,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
            })
        })
        .map_err(|e| CoreError::Io(format!("reading v2 nodes: {e}")))?
        .filter_map(|row| match row {
            Ok(n) => Some(n),
            Err(_) => {
                // Rows failing column extraction (manual db surgery) must not
                // vanish silently — the report accounts for every source row.
                report.skipped_invalid += 1;
                None
            }
        })
        .collect();

    // Existing v3 bodies per scope, for dedup (and rerun idempotency).
    let existing = store.load_all()?;
    let mut bodies: Vec<(Scope, String)> = existing
        .nodes
        .iter()
        .map(|(_, n)| (n.scope.clone(), n.body.clone()))
        .collect();

    for v2 in v2_nodes {
        match v2.status.as_str() {
            "active" | "pending_review" => {}
            _ => {
                report.skipped_status += 1;
                continue;
            }
        }
        let node_type = match v2.node_type.as_str() {
            "rule" | "directive" => NodeType::Rule,
            "preference" => NodeType::Preference,
            "pattern" | "skill" => NodeType::Pattern,
            "memory" => NodeType::Memory,
            _ => {
                report.skipped_invalid += 1;
                continue;
            }
        };
        let scope = match v2.scope.as_str() {
            "global" => Scope::Global,
            "project" => match &v2.project_id {
                Some(p) if store::is_valid_slug(p) => Scope::Project(p.clone()),
                Some(p) => {
                    let s = store::slugify(p);
                    if store::is_valid_slug(&s) {
                        Scope::Project(s)
                    } else {
                        report.skipped_invalid += 1;
                        continue;
                    }
                }
                None => {
                    report.skipped_invalid += 1;
                    continue;
                }
            },
            _ => {
                report.skipped_invalid += 1;
                continue;
            }
        };
        // Dedup compares against ALL store nodes including invalidated ones —
        // deliberately, so migration never resurrects knowledge the user
        // already killed in v3.
        let is_dup = bodies
            .iter()
            .any(|(s, b)| *s == scope && normalized_similarity(b, &v2.content) > 0.8);
        if is_dup {
            report.deduped += 1;
            continue;
        }

        report.imported += 1;
        if !dry_run {
            let base: String = v2
                .content
                .split_whitespace()
                .take(8)
                .collect::<Vec<_>>()
                .join(" ");
            let id = store.unique_slug(&store::slugify(&base), &scope);
            let node = Node {
                id,
                scope: scope.clone(),
                node_type,
                confidence: v2.confidence.clamp(0.0, 1.0),
                sources: vec![format!("v2:{}", v2.id)],
                created: date_of(&v2.created_at),
                updated: date_of(&v2.updated_at),
                invalidated_by: None,
                body: v2.content.clone(),
            };
            store.write_node(&node)?;
        }
        report.imported_bodies.push((scope.clone(), v2.content.clone()));
        bodies.push((scope, v2.content));
    }
    Ok(report)
}

/// Import managed-block bullets that exist in a CLAUDE.md but not in the
/// store, as rule nodes at 0.8 (the v2 reconcile-import convention). This is
/// the guard against the "first projection wipes pre-v3 rules" failure.
pub fn safety_import(
    store: &Store,
    claude_md: &Path,
    scope: &Scope,
    seed_bodies: &[(Scope, String)],
    dry_run: bool,
) -> Result<usize, CoreError> {
    let Ok(content) = std::fs::read_to_string(claude_md) else {
        return Ok(0);
    };
    let Some(rules) = crate::projection::claude_md::read_managed_section(&content) else {
        return Ok(0);
    };
    // Dedup includes invalidated nodes — deliberately, so the rescue never
    // resurrects knowledge the user already killed in v3.
    let existing = store.load_all()?;
    let mut bodies: Vec<String> = existing
        .nodes
        .iter()
        .filter(|(_, n)| n.scope == *scope)
        .map(|(_, n)| n.body.clone())
        .collect();
    // Seed with bodies a preceding (possibly dry-run) knowledge import
    // accepted, so dry-run previews report the same count a real run would.
    bodies.extend(
        seed_bodies
            .iter()
            .filter(|(s, _)| s == scope)
            .map(|(_, b)| b.clone()),
    );
    let today = chrono::Utc::now().date_naive();
    let mut imported = 0;
    for rule in rules {
        if bodies.iter().any(|b| normalized_similarity(b, &rule) > 0.8) {
            continue;
        }
        imported += 1;
        // In-loop push: near-identical bullets within ONE managed block must
        // not all import (hand-edited blocks contain such pairs).
        bodies.push(rule.clone());
        if !dry_run {
            let base: String = rule.split_whitespace().take(8).collect::<Vec<_>>().join(" ");
            let id = store.unique_slug(&store::slugify(&base), scope);
            store.write_node(&Node {
                id,
                scope: scope.clone(),
                node_type: NodeType::Rule,
                confidence: 0.8,
                sources: vec!["managed-import".to_string()],
                created: today,
                updated: today,
                invalidated_by: None,
                body: rule,
            })?;
        }
    }
    Ok(imported)
}

const HOOK_MARKER: &str = "# retro hook - do not remove";

/// Strip v1 retro hook lines (marker + the following line) from a repo's
/// post-commit/post-merge hooks. Returns which hooks were modified. A hook
/// left with only a shebang/blank lines is deleted outright.
pub fn remove_v1_hooks(repo_root: &str) -> Vec<String> {
    let mut removed = Vec::new();
    for name in ["post-commit", "post-merge"] {
        let path = Path::new(repo_root).join(".git/hooks").join(name);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if !content.contains(HOOK_MARKER) {
            continue;
        }
        let mut out: Vec<&str> = Vec::new();
        let mut skip_next = false;
        for line in content.lines() {
            if skip_next {
                skip_next = false;
                continue;
            }
            if line.trim() == HOOK_MARKER {
                skip_next = true;
                continue;
            }
            out.push(line);
        }
        let remaining = out.join("\n");
        let only_boilerplate = out
            .iter()
            .all(|l| l.trim().is_empty() || l.starts_with("#!"));
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
        let pathspec = entry.trim_end_matches('/');
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(store_root)
            .args(["rm", "-r", "--cached", "--ignore-unmatch", pathspec])
            .output()
            .map_err(|e| CoreError::Io(e.to_string()))?;
        if !out.status.success() {
            continue; // pathspec oddity — non-fatal, entry stays for next run
        }
        // rm prints one "rm '<path>'" line per real removal — detect those
        // directly, so unrelated pre-staged changes can't flip this flag.
        if !out.stdout.is_empty() {
            any = true;
        }
    }
    if any {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(store_root)
            .args(["commit", "-m", "retro: untrack machine-local files (migrate)"])
            .output()
            .map_err(|e| CoreError::Io(e.to_string()))?;
        if !out.status.success() {
            return Err(CoreError::Io(format!(
                "committing untrack: {}",
                String::from_utf8_lossy(&out.stderr)
            )));
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
    let Ok(home) = std::env::var("HOME") else {
        return false;
    };
    let plist = Path::new(&home).join("Library/LaunchAgents/com.retro.runner.plist");
    std::fs::remove_file(&plist).is_ok()
}

/// Registered project paths from BOTH generations, for the v1 hook sweep:
/// the v2 projects table plus the v3 path map. Missing db/table tolerated.
pub fn all_known_project_paths(retro_dir: &Path) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let db_path = retro_dir.join("retro.db");
    if db_path.exists() {
        if let Ok(conn) = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NodeType, Scope, Store};
    use tempfile::TempDir;

    fn fixture_v2_db(dir: &std::path::Path) -> rusqlite::Connection {
        fixture_v2_db_with(dir, false)
    }

    fn fixture_v2_db_with(dir: &std::path::Path, wal: bool) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(dir.join("retro.db")).unwrap();
        if wal {
            // The real user db is WAL-mode — the WAL test exercises this.
            let mode: String = conn
                .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
                .unwrap();
            assert_eq!(mode.to_lowercase(), "wal");
        }
        conn.execute_batch(
            "CREATE TABLE nodes (id TEXT PRIMARY KEY, type TEXT, scope TEXT, project_id TEXT,
                content TEXT, confidence REAL, status TEXT, created_at TEXT, updated_at TEXT,
                projected_at TEXT, pr_url TEXT);
             CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT, remote_url TEXT,
                agent_type TEXT DEFAULT 'claude_code', last_seen TEXT);",
        )
        .unwrap();
        conn.execute("INSERT INTO projects VALUES ('my-app', '/tmp/my-app', NULL, 'claude_code', '2026-01-01T00:00:00Z')", []).unwrap();
        let rows: &[(&str, &str, &str, Option<&str>, &str, f64, &str)] = &[
            (
                "n1",
                "rule",
                "global",
                None,
                "Always run smoke tests before full runs",
                0.8,
                "active",
            ),
            (
                "n2",
                "directive",
                "global",
                None,
                "Never commit secrets",
                0.85,
                "active",
            ),
            (
                "n3",
                "skill",
                "global",
                None,
                "Use uv for python scripts",
                0.75,
                "active",
            ),
            (
                "n4",
                "pattern",
                "project",
                Some("my-app"),
                "Deploys go through staging first",
                0.6,
                "pending_review",
            ),
            (
                "n5",
                "rule",
                "global",
                None,
                "A dismissed rule",
                0.9,
                "dismissed",
            ),
            (
                "n6",
                "memory",
                "global",
                None,
                "Context-only memory item",
                0.7,
                "active",
            ),
            (
                "n7",
                "wizardry",
                "global",
                None,
                "Unknown type must be skipped visibly",
                0.7,
                "active",
            ),
            (
                "n8",
                "rule",
                "project",
                None,
                "Project rule with no project id",
                0.7,
                "active",
            ),
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
        assert_eq!(report.skipped_status, 1); // n5 dismissed
        assert_eq!(report.skipped_invalid, 2); // n7 unknown type, n8 null project_id
        let all = store.load_all().unwrap().nodes;
        let types: Vec<_> = all
            .iter()
            .map(|(_, n)| (n.body.clone(), n.node_type))
            .collect();
        assert!(
            types
                .iter()
                .any(|(b, t)| b.contains("Never commit secrets") && *t == NodeType::Rule)
        ); // directive -> rule
        assert!(
            types
                .iter()
                .any(|(b, t)| b.contains("uv for python") && *t == NodeType::Pattern)
        ); // skill -> pattern
        assert!(
            all.iter()
                .any(|(_, n)| matches!(&n.scope, Scope::Project(s) if s == "my-app"))
        );
        // provenance + dates carried over
        let n1 = all
            .iter()
            .find(|(_, n)| n.body.contains("smoke tests"))
            .unwrap();
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
    fn untrack_survives_commit_all_on_stale_ignore_store() {
        // Simulates a store whose last-run binary predates the current
        // IGNORED_ENTRIES: tracked machine-local file AND stale ignore rules.
        // After untrack, refreshed excludes must keep commit_all's `add -A`
        // from re-adding the file (the migrate CLI calls apply_local_config
        // for exactly this reason).
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        crate::store::git::ensure_repo(tmp.path()).unwrap();
        // stale-ify both ignore mechanisms
        std::fs::write(tmp.path().join(".gitignore"), "").unwrap();
        std::fs::write(tmp.path().join(".git/info/exclude"), "").unwrap();
        std::fs::write(tmp.path().join("health.json"), "{}").unwrap();
        let force = std::process::Command::new("git")
            .args(["-C", tmp.path().to_str().unwrap(), "add", "-f", "health.json"])
            .output()
            .unwrap();
        assert!(force.status.success());
        std::process::Command::new("git")
            .args(["-C", tmp.path().to_str().unwrap(), "commit", "-m", "poisoned"])
            .output()
            .unwrap();

        assert!(untrack_ignored_entries(tmp.path()).unwrap());
        crate::store::git::apply_local_config(tmp.path()).unwrap();
        // commit_all's outcome doesn't matter — the tracking state after
        // its `add -A` is what the refreshed excludes must protect.
        let _ = crate::store::git::commit_all(tmp.path(), "sweep").unwrap();
        let ls = std::process::Command::new("git")
            .args(["-C", tmp.path().to_str().unwrap(), "ls-files"])
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&ls.stdout).contains("health.json"),
            "refreshed excludes must prevent re-adding untracked machine-local files"
        );
    }

    #[test]
    fn wal_mode_v2_db_reads_fine_and_bytes_unchanged() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        crate::store::git::ensure_repo(tmp.path()).unwrap();
        let conn = fixture_v2_db_with(tmp.path(), true);
        drop(conn); // writer close checkpoints and removes -wal/-shm
        let before = std::fs::read(tmp.path().join("retro.db")).unwrap();

        let report = migrate_knowledge(&store, tmp.path(), false).unwrap();
        assert_eq!(report.imported, 5);
        let after = std::fs::read(tmp.path().join("retro.db")).unwrap();
        assert_eq!(before, after, "v2 db bytes must be untouched");
        // SQLite may recreate retro.db-wal/-shm wal-index artifacts on a
        // read-only open (documented ≥3.22 behavior). They hold no data, are
        // in IGNORED_ENTRIES, and any 2.x open removes them — tolerated here.
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
        let imported =
            safety_import(&store, &claude.path().join("CLAUDE.md"), &Scope::Global, &[], false).unwrap();
        assert_eq!(imported, 2);
        // idempotent: rerun imports nothing
        let again =
            safety_import(&store, &claude.path().join("CLAUDE.md"), &Scope::Global, &[], false).unwrap();
        assert_eq!(again, 0);
        let nodes = store.load_all().unwrap().nodes;
        assert_eq!(nodes.len(), 2);
        assert!(
            nodes
                .iter()
                .all(|(_, n)| n.node_type == NodeType::Rule && (n.confidence - 0.8).abs() < 1e-9)
        );
    }

    #[test]
    fn safety_import_dedups_within_block_seeds_and_dry_runs() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        // two near-identical bullets in ONE block -> only one may import
        std::fs::write(
            claude.path().join("CLAUDE.md"),
            "<!-- retro:managed:start -->\n- Always run the smoke tests first\n- Always run the smoke tests first!\n<!-- retro:managed:end -->\n",
        )
        .unwrap();
        let path = claude.path().join("CLAUDE.md");

        // seeded with a matching body (as a dry-run knowledge import would
        // produce) -> nothing to rescue
        let seed = vec![(Scope::Global, "Always run the smoke tests first".to_string())];
        assert_eq!(
            safety_import(&store, &path, &Scope::Global, &seed, true).unwrap(),
            0
        );

        // dry run: counted once (within-block dedup), nothing written
        assert_eq!(
            safety_import(&store, &path, &Scope::Global, &[], true).unwrap(),
            1
        );
        assert!(store.load_all().unwrap().nodes.is_empty());

        // real run: one node, correct shape
        assert_eq!(
            safety_import(&store, &path, &Scope::Global, &[], false).unwrap(),
            1
        );
        let nodes = store.load_all().unwrap().nodes;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].1.scope, Scope::Global);
        assert_eq!(nodes[0].1.sources, vec!["managed-import".to_string()]);
    }

    #[test]
    fn safety_import_noop_without_managed_section() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        std::fs::write(claude.path().join("CLAUDE.md"), "# my own file\n").unwrap();
        assert_eq!(
            safety_import(&store, &claude.path().join("CLAUDE.md"), &Scope::Global, &[], false).unwrap(),
            0
        );
        assert_eq!(
            safety_import(&store, &tmp.path().join("nope.md"), &Scope::Global, &[], false).unwrap(),
            0
        );
    }

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
}
