//! Disposable SQLite index over the file store.
//! `build()` fully rebuilds `index.db` from the markdown files.
//! No state lives here that is not derivable from the files.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::Store;
use crate::errors::CoreError;

/// Result of an index build.
pub struct IndexStats {
    pub nodes: usize,
    pub warnings: Vec<String>,
}

/// Query filter; all fields are AND-combined. Default = everything.
#[derive(Default)]
pub struct NodeFilter {
    pub scope: Option<String>,
    pub node_type: Option<String>,
    pub active_only: bool,
    pub text: Option<String>,
}

/// One row from the index (denormalized for surfaces).
pub struct NodeRow {
    pub id: String,
    pub scope: String,
    pub node_type: String,
    pub confidence: f64,
    pub active: bool,
    pub created: String,
    pub updated: String,
    pub invalidated_by: Option<String>,
    pub body: String,
    pub path: String,
    pub sources: Vec<String>,
}

pub fn index_path(store_root: &Path) -> PathBuf {
    store_root.join("index.db")
}

/// Open the index for querying. Errors with NotInitialized if the index
/// has never been built — callers should run `build()` (or `retro reindex`).
pub fn open(store_root: &Path) -> Result<Connection, CoreError> {
    let path = index_path(store_root);
    if !path.exists() {
        return Err(CoreError::NotInitialized(
            "index not built — run `retro reindex`".to_string(),
        ));
    }
    let conn = Connection::open(&path)?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 0 {
        return Err(CoreError::NotInitialized(
            "index not built — run `retro reindex`".to_string(),
        ));
    }
    Ok(conn)
}

/// Full rebuild: delete index.db, recreate schema, insert every node.
/// Builds are atomic: user_version is set only after all rows are committed, so a failed build is indistinguishable from "not built".
pub fn build(store: &Store) -> Result<IndexStats, CoreError> {
    let db = index_path(store.root());
    for suffix in ["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{}", db.display(), suffix));
        if p.exists() {
            std::fs::remove_file(&p).map_err(|e| CoreError::Io(e.to_string()))?;
        }
    }
    let mut conn = Connection::open(&db)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE nodes (
             id TEXT NOT NULL,
             scope TEXT NOT NULL,
             type TEXT NOT NULL,
             confidence REAL NOT NULL,
             active INTEGER NOT NULL,
             created TEXT NOT NULL,
             updated TEXT NOT NULL,
             invalidated_by TEXT,
             body TEXT NOT NULL,
             path TEXT NOT NULL,
             PRIMARY KEY (scope, id)
         );
         CREATE TABLE node_sources (
             scope TEXT NOT NULL,
             node_id TEXT NOT NULL,
             source TEXT NOT NULL
         );
         CREATE VIRTUAL TABLE nodes_fts USING fts5(id, scope, body);",
    )?;

    let loaded = store.load_all()?;
    let tx = conn.transaction()?;
    for (path, node) in &loaded.nodes {
        let scope = node.scope.to_string();
        tx.execute(
            "INSERT INTO nodes (id, scope, type, confidence, active, created, updated, invalidated_by, body, path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                node.id,
                scope,
                node.node_type.as_str(),
                node.confidence,
                node.is_active() as i64,
                node.created.to_string(),
                node.updated.to_string(),
                node.invalidated_by,
                node.body,
                path.display().to_string(),
            ],
        )?;
        for source in &node.sources {
            tx.execute(
                "INSERT INTO node_sources (scope, node_id, source) VALUES (?1, ?2, ?3)",
                rusqlite::params![scope, node.id, source],
            )?;
        }
        tx.execute(
            "INSERT INTO nodes_fts (id, scope, body) VALUES (?1, ?2, ?3)",
            rusqlite::params![node.id, scope, node.body],
        )?;
    }
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('fingerprint', ?1)",
        rusqlite::params![fingerprint_of(&loaded.nodes)?],
    )?;
    tx.commit()?;
    conn.pragma_update(None, "user_version", 1)?;
    Ok(IndexStats {
        nodes: loaded.nodes.len(),
        warnings: loaded.warnings,
    })
}

pub fn query(conn: &Connection, filter: &NodeFilter) -> Result<Vec<NodeRow>, CoreError> {
    let mut sql = String::from(
        "SELECT id, scope, type, confidence, active, created, updated, invalidated_by, body, path
         FROM nodes WHERE 1=1",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(scope) = &filter.scope {
        sql.push_str(" AND scope = ?");
        params.push(Box::new(scope.clone()));
    }
    if let Some(t) = &filter.node_type {
        sql.push_str(" AND type = ?");
        params.push(Box::new(t.clone()));
    }
    if filter.active_only {
        sql.push_str(" AND active = 1");
    }
    if let Some(text) = &filter.text {
        if !text.trim().is_empty() {
            sql.push_str(
                " AND (scope || '/' || id) IN (SELECT scope || '/' || id FROM nodes_fts WHERE nodes_fts MATCH ?)",
            );
            params.push(Box::new(fts_escape(text)));
        }
    }
    sql.push_str(" ORDER BY scope, id");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut rows_iter = stmt.query(param_refs.as_slice())?;
    let mut rows = Vec::new();
    while let Some(row) = rows_iter.next()? {
        rows.push(NodeRow {
            id: row.get(0)?,
            scope: row.get(1)?,
            node_type: row.get(2)?,
            confidence: row.get(3)?,
            active: row.get::<_, i64>(4)? != 0,
            created: row.get(5)?,
            updated: row.get(6)?,
            invalidated_by: row.get(7)?,
            body: row.get(8)?,
            path: row.get(9)?,
            sources: Vec::new(),
        });
    }
    let mut sources_stmt = conn.prepare(
        "SELECT source FROM node_sources WHERE scope = ?1 AND node_id = ?2 ORDER BY source",
    )?;
    for r in &mut rows {
        let sources = sources_stmt
            .query_map(rusqlite::params![r.scope, r.id], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        r.sources = sources;
    }
    Ok(rows)
}

/// Wrap each whitespace-separated term in double quotes so hostile
/// input can't hit FTS5 query-syntax errors. Quotes inside terms are stripped.
fn fts_escape(input: &str) -> String {
    input
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Cheap store fingerprint: sorted `path|mtime|len` lines.
/// String comparison — no hashing dependency needed.
pub fn fingerprint(store: &Store) -> Result<String, CoreError> {
    fingerprint_of(&store.load_all()?.nodes)
}

fn fingerprint_of(nodes: &[(PathBuf, super::Node)]) -> Result<String, CoreError> {
    let mut lines: Vec<String> = Vec::with_capacity(nodes.len());
    for (path, _) in nodes {
        let meta = std::fs::metadata(path).map_err(|e| CoreError::Io(e.to_string()))?;
        let mtime = meta
            .modified()
            .map_err(|e| CoreError::Io(e.to_string()))?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        lines.push(format!("{}|{}|{}", path.display(), mtime, meta.len()));
    }
    lines.sort();
    Ok(lines.join("\n"))
}

/// True if the index was built from the store's current file state.
pub fn is_fresh(store: &Store, conn: &Connection) -> Result<bool, CoreError> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'fingerprint'",
            [],
            |row| row.get(0),
        )
        .ok();
    match stored {
        Some(fp) => Ok(fp == fingerprint(store)?),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Node, NodeType, Scope};
    use chrono::NaiveDate;
    use tempfile::TempDir;

    fn seeded_store() -> (TempDir, Store) {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        let mk = |id: &str, scope: Scope, t: NodeType, inv: Option<&str>, body: &str| Node {
            id: id.to_string(),
            scope,
            node_type: t,
            confidence: 0.8,
            sources: vec![format!("session:src-{id}")],
            created: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            updated: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            invalidated_by: inv.map(String::from),
            body: body.to_string(),
        };
        store
            .write_node(&mk(
                "g-rule",
                Scope::Global,
                NodeType::Rule,
                None,
                "always run smoke tests first",
            ))
            .unwrap();
        store
            .write_node(&mk(
                "p-pattern",
                Scope::Project("my-proj".to_string()),
                NodeType::Pattern,
                None,
                "paired observations for experiments",
            ))
            .unwrap();
        store
            .write_node(&mk(
                "dead-rule",
                Scope::Global,
                NodeType::Rule,
                Some("g-rule"),
                "obsolete advice",
            ))
            .unwrap();
        (tmp, store)
    }

    #[test]
    fn build_indexes_all_nodes_and_is_rebuildable() {
        let (_tmp, store) = seeded_store();
        let stats = build(&store).unwrap();
        assert_eq!(stats.nodes, 3);
        assert!(stats.warnings.is_empty());
        // rebuild from scratch works (delete + recreate)
        let stats = build(&store).unwrap();
        assert_eq!(stats.nodes, 3);
    }

    #[test]
    fn query_filters_by_scope_type_and_active() {
        let (_tmp, store) = seeded_store();
        build(&store).unwrap();
        let conn = open(store.root()).unwrap();

        let all = query(&conn, &NodeFilter::default()).unwrap();
        assert_eq!(all.len(), 3);

        let active_only = query(
            &conn,
            &NodeFilter {
                active_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(active_only.len(), 2);

        let global = query(
            &conn,
            &NodeFilter {
                scope: Some("global".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(global.len(), 2);

        let patterns = query(
            &conn,
            &NodeFilter {
                node_type: Some("pattern".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].id, "p-pattern");
        assert_eq!(
            patterns[0].sources,
            vec!["session:src-p-pattern".to_string()]
        );
    }

    #[test]
    fn query_full_text_search() {
        let (_tmp, store) = seeded_store();
        build(&store).unwrap();
        let conn = open(store.root()).unwrap();
        let hits = query(
            &conn,
            &NodeFilter {
                text: Some("paired observations".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "p-pattern");
        // hostile input must not cause an FTS syntax error
        let hits = query(
            &conn,
            &NodeFilter {
                text: Some("\"unbalanced -NOT (".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn query_combined_filters_and_blank_text() {
        let (_tmp, store) = seeded_store();
        build(&store).unwrap();
        let conn = open(store.root()).unwrap();
        // scope + text combined (dashboard's primary pattern)
        let hits = query(
            &conn,
            &NodeFilter {
                scope: Some("global".to_string()),
                text: Some("smoke tests".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "g-rule");
        // blank text is ignored, not an FTS error
        let hits = query(
            &conn,
            &NodeFilter {
                text: Some("   ".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn open_before_build_errors_not_initialized() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        let err = open(store.root()).unwrap_err();
        assert!(err.to_string().contains("not built"), "got: {err}");
    }

    #[test]
    fn build_with_copied_duplicate_file_warns_and_succeeds() {
        let (tmp, store) = seeded_store();
        std::fs::copy(
            tmp.path().join("knowledge/global/g-rule.md"),
            tmp.path().join("knowledge/global/g-rule-copy.md"),
        )
        .unwrap();
        let stats = build(&store).unwrap();
        assert_eq!(stats.nodes, 3);
        assert_eq!(stats.warnings.len(), 1);
        let conn = open(store.root()).unwrap();
        assert_eq!(query(&conn, &NodeFilter::default()).unwrap().len(), 3);
    }

    #[test]
    fn freshness_detects_file_changes() {
        let (_tmp, store) = seeded_store();
        build(&store).unwrap();
        let conn = open(store.root()).unwrap();
        assert!(is_fresh(&store, &conn).unwrap());
        // adding a node makes the index stale
        store
            .write_node(&Node {
                id: "new-rule".to_string(),
                scope: Scope::Global,
                node_type: NodeType::Rule,
                confidence: 0.7,
                sources: vec![],
                created: NaiveDate::from_ymd_opt(2026, 7, 2).unwrap(),
                updated: NaiveDate::from_ymd_opt(2026, 7, 2).unwrap(),
                invalidated_by: None,
                body: "fresh".to_string(),
            })
            .unwrap();
        assert!(!is_fresh(&store, &conn).unwrap());
    }
}
