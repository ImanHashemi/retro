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
}
