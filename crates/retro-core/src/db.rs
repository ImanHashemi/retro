use crate::errors::CoreError;
use crate::models::{
    IngestedSession, KnowledgeEdge, KnowledgeNode, KnowledgeProject,
    EdgeType, GraphOperation, NodeScope, NodeStatus, NodeType,
    Pattern, PatternStatus, PatternType, Projection, ProjectionStatus, SuggestedTarget,
};
use chrono::{DateTime, Utc};
pub use rusqlite::Connection;
use rusqlite::params;
use rusqlite::OptionalExtension;
use std::path::Path;

const SCHEMA_VERSION: u32 = 5;

/// Open (or create) the retro database with WAL mode enabled.
pub fn open_db(path: &Path) -> Result<Connection, CoreError> {
    let conn = Connection::open(path)?;

    // Enable WAL mode for concurrent access
    conn.pragma_update(None, "journal_mode", "WAL")?;

    // Run migrations
    migrate(&conn)?;

    Ok(conn)
}

/// Initialize schema on an existing connection (for testing with in-memory DBs).
pub fn init_db(conn: &Connection) -> Result<(), CoreError> {
    migrate(conn)
}

fn migrate(conn: &Connection) -> Result<(), CoreError> {
    let current_version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if current_version < 1 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS patterns (
                id TEXT PRIMARY KEY,
                pattern_type TEXT NOT NULL,
                description TEXT NOT NULL,
                confidence REAL NOT NULL,
                times_seen INTEGER NOT NULL DEFAULT 1,
                first_seen TEXT NOT NULL,
                last_seen TEXT NOT NULL,
                last_projected TEXT,
                status TEXT NOT NULL DEFAULT 'discovered',
                source_sessions TEXT NOT NULL,
                related_files TEXT NOT NULL,
                suggested_content TEXT NOT NULL,
                suggested_target TEXT NOT NULL,
                project TEXT,
                generation_failed INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS projections (
                id TEXT PRIMARY KEY,
                pattern_id TEXT NOT NULL REFERENCES patterns(id),
                target_type TEXT NOT NULL,
                target_path TEXT NOT NULL,
                content TEXT NOT NULL,
                applied_at TEXT NOT NULL,
                pr_url TEXT,
                nudged INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS analyzed_sessions (
                session_id TEXT PRIMARY KEY,
                project TEXT NOT NULL,
                analyzed_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS ingested_sessions (
                session_id TEXT PRIMARY KEY,
                project TEXT NOT NULL,
                session_path TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                file_mtime TEXT NOT NULL,
                ingested_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_patterns_status ON patterns(status);
            CREATE INDEX IF NOT EXISTS idx_patterns_type ON patterns(pattern_type);
            CREATE INDEX IF NOT EXISTS idx_patterns_target ON patterns(suggested_target);
            CREATE INDEX IF NOT EXISTS idx_patterns_project ON patterns(project);
            CREATE INDEX IF NOT EXISTS idx_projections_pattern ON projections(pattern_id);
            ",
        )?;

        conn.pragma_update(None, "user_version", 1)?;
    }

    if current_version < 2 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        conn.pragma_update(None, "user_version", 2)?;
    }

    if current_version < 3 {
        conn.execute_batch(
            "ALTER TABLE projections ADD COLUMN status TEXT NOT NULL DEFAULT 'applied';",
        )?;
        conn.pragma_update(None, "user_version", 3)?;
    }

    if current_version < 4 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS nodes (
                id TEXT PRIMARY KEY,
                type TEXT NOT NULL,
                scope TEXT NOT NULL,
                project_id TEXT,
                content TEXT NOT NULL,
                confidence REAL NOT NULL,
                status TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                projected_at TEXT,
                pr_url TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_nodes_scope_project ON nodes(scope, project_id, status);
            CREATE INDEX IF NOT EXISTS idx_nodes_type_status ON nodes(type, status);

            CREATE TABLE IF NOT EXISTS edges (
                source_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                type TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (source_id, target_id, type)
            );

            CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id, type);
            CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id, type);

            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                remote_url TEXT,
                agent_type TEXT NOT NULL DEFAULT 'claude_code',
                last_seen TEXT NOT NULL
            );
            ",
        )?;

        // Migrate existing v1 patterns to v2 nodes
        migrate_patterns_to_nodes(conn)?;

        conn.pragma_update(None, "user_version", 4)?;
    }

    if current_version < 5 {
        // For databases upgraded from v4, we need to add the new columns.
        // For fresh installs, v4 CREATE TABLE already includes them.
        let has_projected_at: bool = conn
            .prepare("SELECT projected_at FROM nodes LIMIT 0")
            .is_ok();
        if !has_projected_at {
            conn.execute_batch(
                "ALTER TABLE nodes ADD COLUMN projected_at TEXT;
                 ALTER TABLE nodes ADD COLUMN pr_url TEXT;"
            )?;
        }
        conn.pragma_update(None, "user_version", 5)?;
    }

    if current_version < 6 {
        // v6: Clean up v1 leftovers now that v2 pipeline is primary.
        // - Archive all v1 patterns (they've been migrated to nodes in v4)
        // - Delete pending_review projections (v2 uses nodes table for review)
        // - Remove bogus project entries with path "/" (from sessions ingested without project context)
        conn.execute_batch(
            "UPDATE patterns SET status = 'archived' WHERE status IN ('discovered', 'active');
             DELETE FROM projections WHERE status = 'pending_review';
             DELETE FROM projects WHERE path = '/';
             DELETE FROM ingested_sessions WHERE project = '/';"
        )?;
        conn.pragma_update(None, "user_version", 6)?;
    }

    Ok(())
}

/// Check if a session has already been ingested and is up-to-date.
pub fn is_session_ingested(
    conn: &Connection,
    session_id: &str,
    file_size: u64,
    file_mtime: &str,
) -> Result<bool, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT file_size, file_mtime FROM ingested_sessions WHERE session_id = ?1",
    )?;

    let result = stmt.query_row(params![session_id], |row| {
        let size: u64 = row.get(0)?;
        let mtime: String = row.get(1)?;
        Ok((size, mtime))
    });

    match result {
        Ok((size, mtime)) => Ok(size == file_size && mtime == file_mtime),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(e) => Err(CoreError::Database(e.to_string())),
    }
}

/// Record a session as ingested.
pub fn record_ingested_session(
    conn: &Connection,
    session: &IngestedSession,
) -> Result<(), CoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO ingested_sessions (session_id, project, session_path, file_size, file_mtime, ingested_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            session.session_id,
            session.project,
            session.session_path,
            session.file_size,
            session.file_mtime,
            session.ingested_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Get the count of ingested sessions.
pub fn ingested_session_count(conn: &Connection) -> Result<u64, CoreError> {
    let count: u64 =
        conn.query_row("SELECT COUNT(*) FROM ingested_sessions", [], |row| {
            row.get(0)
        })?;
    Ok(count)
}

/// Get the count of ingested sessions for a specific project.
pub fn ingested_session_count_for_project(
    conn: &Connection,
    project: &str,
) -> Result<u64, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM ingested_sessions WHERE project = ?1",
        params![project],
        |row| row.get(0),
    )?;
    Ok(count)
}

/// Get the count of analyzed sessions.
pub fn analyzed_session_count(conn: &Connection) -> Result<u64, CoreError> {
    let count: u64 =
        conn.query_row("SELECT COUNT(*) FROM analyzed_sessions", [], |row| {
            row.get(0)
        })?;
    Ok(count)
}

/// Get the count of patterns by status.
pub fn pattern_count_by_status(conn: &Connection, status: &str) -> Result<u64, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM patterns WHERE status = ?1",
        params![status],
        |row| row.get(0),
    )?;
    Ok(count)
}

/// Get the most recent ingestion timestamp.
pub fn last_ingested_at(conn: &Connection) -> Result<Option<String>, CoreError> {
    let result = conn.query_row(
        "SELECT MAX(ingested_at) FROM ingested_sessions",
        [],
        |row| row.get::<_, Option<String>>(0),
    )?;
    Ok(result)
}

/// Get the most recent analysis timestamp.
pub fn last_analyzed_at(conn: &Connection) -> Result<Option<String>, CoreError> {
    let result = conn.query_row(
        "SELECT MAX(analyzed_at) FROM analyzed_sessions",
        [],
        |row| row.get::<_, Option<String>>(0),
    )?;
    Ok(result)
}

/// Get the most recent projection (apply) timestamp.
pub fn last_applied_at(conn: &Connection) -> Result<Option<String>, CoreError> {
    let result = conn.query_row(
        "SELECT MAX(applied_at) FROM projections",
        [],
        |row| row.get::<_, Option<String>>(0),
    )?;
    Ok(result)
}

/// Check if there are ingested sessions that haven't been analyzed yet.
pub fn has_unanalyzed_sessions(conn: &Connection) -> Result<bool, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM ingested_sessions i
         LEFT JOIN analyzed_sessions a ON i.session_id = a.session_id
         WHERE a.session_id IS NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Count ingested sessions that haven't been analyzed yet.
pub fn unanalyzed_session_count(conn: &Connection) -> Result<u64, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM ingested_sessions i
         LEFT JOIN analyzed_sessions a ON i.session_id = a.session_id
         WHERE a.session_id IS NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(count)
}

/// Check if there are patterns eligible for projection that haven't been projected yet.
/// Mirrors the gating logic in `get_qualifying_patterns()`: excludes patterns with
/// generation_failed=true, suggested_target='db_only', or confidence below threshold.
/// The confidence threshold is the sole quality gate (no times_seen requirement).
pub fn has_unprojected_patterns(conn: &Connection, confidence_threshold: f64) -> Result<bool, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM patterns p
         LEFT JOIN projections pr ON p.id = pr.pattern_id
         WHERE pr.id IS NULL
         AND p.status IN ('discovered', 'active')
         AND p.generation_failed = 0
         AND p.suggested_target != 'db_only'
         AND p.confidence >= ?1",
        [confidence_threshold],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Get the last nudge timestamp from metadata.
pub fn get_last_nudge_at(conn: &Connection) -> Result<Option<DateTime<Utc>>, CoreError> {
    let result: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'last_nudge_at'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    match result {
        Some(s) => match DateTime::parse_from_rfc3339(&s) {
            Ok(dt) => Ok(Some(dt.with_timezone(&Utc))),
            Err(_) => Ok(None),
        },
        None => Ok(None),
    }
}

/// Set the last nudge timestamp in metadata.
pub fn set_last_nudge_at(conn: &Connection, timestamp: &DateTime<Utc>) -> Result<(), CoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('last_nudge_at', ?1)",
        params![timestamp.to_rfc3339()],
    )?;
    Ok(())
}

/// Get a value from the metadata table by key.
pub fn get_metadata(conn: &Connection, key: &str) -> Result<Option<String>, CoreError> {
    let result: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    Ok(result)
}

/// Set a value in the metadata table (insert or replace).
pub fn set_metadata(conn: &Connection, key: &str, value: &str) -> Result<(), CoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

/// Verify the database is using WAL mode.
pub fn verify_wal_mode(conn: &Connection) -> Result<bool, CoreError> {
    let mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    Ok(mode.to_lowercase() == "wal")
}

/// Get all distinct projects from ingested sessions.
pub fn list_projects(conn: &Connection) -> Result<Vec<String>, CoreError> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT project FROM ingested_sessions ORDER BY project")?;
    let projects = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(projects)
}

// ── Pattern operations ──

const PATTERN_COLUMNS: &str = "id, pattern_type, description, confidence, times_seen, first_seen, last_seen, last_projected, status, source_sessions, related_files, suggested_content, suggested_target, project, generation_failed";

/// Insert a new pattern into the database.
pub fn insert_pattern(conn: &Connection, pattern: &Pattern) -> Result<(), CoreError> {
    let source_sessions =
        serde_json::to_string(&pattern.source_sessions).unwrap_or_else(|_| "[]".to_string());
    let related_files =
        serde_json::to_string(&pattern.related_files).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        "INSERT INTO patterns (id, pattern_type, description, confidence, times_seen, first_seen, last_seen, last_projected, status, source_sessions, related_files, suggested_content, suggested_target, project, generation_failed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        params![
            pattern.id,
            pattern.pattern_type.to_string(),
            pattern.description,
            pattern.confidence,
            pattern.times_seen,
            pattern.first_seen.to_rfc3339(),
            pattern.last_seen.to_rfc3339(),
            pattern.last_projected.map(|t| t.to_rfc3339()),
            pattern.status.to_string(),
            source_sessions,
            related_files,
            pattern.suggested_content,
            pattern.suggested_target.to_string(),
            pattern.project,
            pattern.generation_failed as i32,
        ],
    )?;
    Ok(())
}

/// Update an existing pattern with new evidence (merge).
pub fn update_pattern_merge(
    conn: &Connection,
    id: &str,
    new_sessions: &[String],
    new_confidence: f64,
    new_last_seen: DateTime<Utc>,
    additional_times_seen: i64,
) -> Result<(), CoreError> {
    // Load existing source_sessions and merge
    let existing_sessions: String = conn.query_row(
        "SELECT source_sessions FROM patterns WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;

    let mut sessions: Vec<String> =
        serde_json::from_str(&existing_sessions).unwrap_or_default();
    for s in new_sessions {
        if !sessions.contains(s) {
            sessions.push(s.clone());
        }
    }
    let merged_sessions = serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_string());

    conn.execute(
        "UPDATE patterns SET
            confidence = MAX(confidence, ?2),
            times_seen = times_seen + ?3,
            last_seen = ?4,
            source_sessions = ?5
         WHERE id = ?1",
        params![
            id,
            new_confidence,
            additional_times_seen,
            new_last_seen.to_rfc3339(),
            merged_sessions,
        ],
    )?;
    Ok(())
}

/// Get patterns filtered by status and optionally by project.
pub fn get_patterns(
    conn: &Connection,
    statuses: &[&str],
    project: Option<&str>,
) -> Result<Vec<Pattern>, CoreError> {
    if statuses.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders: Vec<String> = statuses.iter().enumerate().map(|(i, _)| format!("?{}", i + 1)).collect();
    let status_clause = placeholders.join(", ");

    let (query, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match project {
        Some(proj) => {
            let q = format!(
                "SELECT {PATTERN_COLUMNS}
                 FROM patterns WHERE status IN ({}) AND (project = ?{} OR project IS NULL)
                 ORDER BY confidence DESC",
                status_clause,
                statuses.len() + 1
            );
            let mut p: Vec<Box<dyn rusqlite::types::ToSql>> = statuses.iter().map(|s| Box::new(s.to_string()) as Box<dyn rusqlite::types::ToSql>).collect();
            p.push(Box::new(proj.to_string()));
            (q, p)
        }
        None => {
            let q = format!(
                "SELECT {PATTERN_COLUMNS}
                 FROM patterns WHERE status IN ({})
                 ORDER BY confidence DESC",
                status_clause
            );
            let p: Vec<Box<dyn rusqlite::types::ToSql>> = statuses.iter().map(|s| Box::new(s.to_string()) as Box<dyn rusqlite::types::ToSql>).collect();
            (q, p)
        }
    };

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&query)?;
    let patterns = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(read_pattern_row(row))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(patterns)
}

/// Get all patterns, optionally filtered by project.
pub fn get_all_patterns(conn: &Connection, project: Option<&str>) -> Result<Vec<Pattern>, CoreError> {
    let (query, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match project {
        Some(proj) => {
            let q = format!(
                "SELECT {PATTERN_COLUMNS}
                 FROM patterns WHERE project = ?1 OR project IS NULL
                 ORDER BY confidence DESC"
            );
            (q, vec![Box::new(proj.to_string()) as Box<dyn rusqlite::types::ToSql>])
        }
        None => {
            let q = format!(
                "SELECT {PATTERN_COLUMNS}
                 FROM patterns ORDER BY confidence DESC"
            );
            (q, vec![])
        }
    };

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&query)?;
    let patterns = stmt
        .query_map(params_refs.as_slice(), |row| Ok(read_pattern_row(row)))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(patterns)
}

fn read_pattern_row(row: &rusqlite::Row<'_>) -> Pattern {
    let source_sessions_str: String = row.get(9).unwrap_or_default();
    let related_files_str: String = row.get(10).unwrap_or_default();
    let first_seen_str: String = row.get(5).unwrap_or_default();
    let last_seen_str: String = row.get(6).unwrap_or_default();
    let last_projected_str: Option<String> = row.get(7).unwrap_or(None);
    let gen_failed: i32 = row.get(14).unwrap_or(0);

    Pattern {
        id: row.get(0).unwrap_or_default(),
        pattern_type: PatternType::from_str(&row.get::<_, String>(1).unwrap_or_default()),
        description: row.get(2).unwrap_or_default(),
        confidence: row.get(3).unwrap_or(0.0),
        times_seen: row.get(4).unwrap_or(1),
        first_seen: DateTime::parse_from_rfc3339(&first_seen_str)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        last_seen: DateTime::parse_from_rfc3339(&last_seen_str)
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        last_projected: last_projected_str
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
        status: PatternStatus::from_str(&row.get::<_, String>(8).unwrap_or_default()),
        source_sessions: serde_json::from_str(&source_sessions_str).unwrap_or_default(),
        related_files: serde_json::from_str(&related_files_str).unwrap_or_default(),
        suggested_content: row.get(11).unwrap_or_default(),
        suggested_target: SuggestedTarget::from_str(&row.get::<_, String>(12).unwrap_or_default()),
        project: row.get(13).unwrap_or(None),
        generation_failed: gen_failed != 0,
    }
}

// ── Analyzed session tracking ──

/// Record a session as analyzed.
pub fn record_analyzed_session(
    conn: &Connection,
    session_id: &str,
    project: &str,
) -> Result<(), CoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO analyzed_sessions (session_id, project, analyzed_at)
         VALUES (?1, ?2, ?3)",
        params![session_id, project, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Check if a session has been analyzed.
pub fn is_session_analyzed(conn: &Connection, session_id: &str) -> Result<bool, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM analyzed_sessions WHERE session_id = ?1",
        params![session_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Get ingested sessions for analysis within the time window.
/// When `rolling_window` is true, returns ALL sessions in the window (re-analyzes everything).
/// When false, only returns sessions not yet in `analyzed_sessions` (analyze-once).
pub fn get_sessions_for_analysis(
    conn: &Connection,
    project: Option<&str>,
    since: &DateTime<Utc>,
    rolling_window: bool,
) -> Result<Vec<IngestedSession>, CoreError> {
    let since_str = since.to_rfc3339();

    let (query, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match (project, rolling_window) {
        (Some(proj), true) => {
            let q = "SELECT i.session_id, i.project, i.session_path, i.file_size, i.file_mtime, i.ingested_at
                     FROM ingested_sessions i
                     WHERE i.project = ?1 AND i.ingested_at >= ?2
                     ORDER BY i.ingested_at".to_string();
            (q, vec![
                Box::new(proj.to_string()) as Box<dyn rusqlite::types::ToSql>,
                Box::new(since_str) as Box<dyn rusqlite::types::ToSql>,
            ])
        }
        (Some(proj), false) => {
            let q = "SELECT i.session_id, i.project, i.session_path, i.file_size, i.file_mtime, i.ingested_at
                     FROM ingested_sessions i
                     LEFT JOIN analyzed_sessions a ON i.session_id = a.session_id
                     WHERE a.session_id IS NULL AND i.project = ?1 AND i.ingested_at >= ?2
                     ORDER BY i.ingested_at".to_string();
            (q, vec![
                Box::new(proj.to_string()) as Box<dyn rusqlite::types::ToSql>,
                Box::new(since_str) as Box<dyn rusqlite::types::ToSql>,
            ])
        }
        (None, true) => {
            let q = "SELECT i.session_id, i.project, i.session_path, i.file_size, i.file_mtime, i.ingested_at
                     FROM ingested_sessions i
                     WHERE i.ingested_at >= ?1
                     ORDER BY i.ingested_at".to_string();
            (q, vec![Box::new(since_str) as Box<dyn rusqlite::types::ToSql>])
        }
        (None, false) => {
            let q = "SELECT i.session_id, i.project, i.session_path, i.file_size, i.file_mtime, i.ingested_at
                     FROM ingested_sessions i
                     LEFT JOIN analyzed_sessions a ON i.session_id = a.session_id
                     WHERE a.session_id IS NULL AND i.ingested_at >= ?1
                     ORDER BY i.ingested_at".to_string();
            (q, vec![Box::new(since_str) as Box<dyn rusqlite::types::ToSql>])
        }
    };

    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&query)?;
    let sessions = stmt
        .query_map(params_refs.as_slice(), |row| {
            let ingested_at_str: String = row.get(5)?;
            let ingested_at = DateTime::parse_from_rfc3339(&ingested_at_str)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            Ok(IngestedSession {
                session_id: row.get(0)?,
                project: row.get(1)?,
                session_path: row.get(2)?,
                file_size: row.get(3)?,
                file_mtime: row.get(4)?,
                ingested_at,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(sessions)
}

// ── Projection operations ──

/// Insert a new projection record.
pub fn insert_projection(conn: &Connection, proj: &Projection) -> Result<(), CoreError> {
    conn.execute(
        "INSERT INTO projections (id, pattern_id, target_type, target_path, content, applied_at, pr_url, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            proj.id,
            proj.pattern_id,
            proj.target_type,
            proj.target_path,
            proj.content,
            proj.applied_at.to_rfc3339(),
            proj.pr_url,
            proj.status.to_string(),
        ],
    )?;
    Ok(())
}

/// Check if a pattern already has an active projection.
pub fn has_projection_for_pattern(conn: &Connection, pattern_id: &str) -> Result<bool, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM projections WHERE pattern_id = ?1",
        params![pattern_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Get the set of all pattern IDs that already have projections.
pub fn get_projected_pattern_ids(
    conn: &Connection,
) -> Result<std::collections::HashSet<String>, CoreError> {
    let mut stmt = conn.prepare("SELECT DISTINCT pattern_id FROM projections")?;
    let ids = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(ids)
}

/// Update a pattern's status.
pub fn update_pattern_status(
    conn: &Connection,
    id: &str,
    status: &PatternStatus,
) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE patterns SET status = ?2 WHERE id = ?1",
        params![id, status.to_string()],
    )?;
    Ok(())
}

/// Set or clear the generation_failed flag on a pattern.
pub fn set_generation_failed(
    conn: &Connection,
    id: &str,
    failed: bool,
) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE patterns SET generation_failed = ?2 WHERE id = ?1",
        params![id, failed as i32],
    )?;
    Ok(())
}

/// Get all projections for active patterns (for staleness detection).
pub fn get_projections_for_active_patterns(
    conn: &Connection,
) -> Result<Vec<Projection>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.pattern_id, p.target_type, p.target_path, p.content, p.applied_at, p.pr_url, p.status
         FROM projections p
         INNER JOIN patterns pat ON p.pattern_id = pat.id
         WHERE pat.status = 'active'",
    )?;

    let projections = stmt
        .query_map([], |row| {
            let applied_at_str: String = row.get(5)?;
            let applied_at = DateTime::parse_from_rfc3339(&applied_at_str)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let status_str: String = row.get(7)?;
            let status = ProjectionStatus::from_str(&status_str)
                .unwrap_or(ProjectionStatus::Applied);
            Ok(Projection {
                id: row.get(0)?,
                pattern_id: row.get(1)?,
                target_type: row.get(2)?,
                target_path: row.get(3)?,
                content: row.get(4)?,
                applied_at,
                pr_url: row.get(6)?,
                status,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(projections)
}

/// Update a pattern's last_projected timestamp to now.
pub fn update_pattern_last_projected(conn: &Connection, id: &str) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE patterns SET last_projected = ?2 WHERE id = ?1",
        params![id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Get all projections with pending_review status.
pub fn get_pending_review_projections(conn: &Connection) -> Result<Vec<Projection>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.pattern_id, p.target_type, p.target_path, p.content, p.applied_at, p.pr_url, p.status
         FROM projections p
         WHERE p.status = 'pending_review'
         ORDER BY p.applied_at ASC",
    )?;

    let projections = stmt
        .query_map([], |row| {
            let applied_at_str: String = row.get(5)?;
            let applied_at = DateTime::parse_from_rfc3339(&applied_at_str)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let status_str: String = row.get(7)?;
            let status = ProjectionStatus::from_str(&status_str)
                .unwrap_or(ProjectionStatus::PendingReview);
            Ok(Projection {
                id: row.get(0)?,
                pattern_id: row.get(1)?,
                target_type: row.get(2)?,
                target_path: row.get(3)?,
                content: row.get(4)?,
                applied_at,
                pr_url: row.get(6)?,
                status,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(projections)
}

/// Update a projection's status.
pub fn update_projection_status(
    conn: &Connection,
    projection_id: &str,
    status: &ProjectionStatus,
) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE projections SET status = ?2 WHERE id = ?1",
        params![projection_id, status.to_string()],
    )?;
    Ok(())
}

/// Delete a projection record.
pub fn delete_projection(conn: &Connection, projection_id: &str) -> Result<(), CoreError> {
    conn.execute("DELETE FROM projections WHERE id = ?1", params![projection_id])?;
    Ok(())
}

/// Get applied projections that have a PR URL (for sync).
pub fn get_applied_projections_with_pr(conn: &Connection) -> Result<Vec<Projection>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.pattern_id, p.target_type, p.target_path, p.content, p.applied_at, p.pr_url, p.status
         FROM projections p
         WHERE p.status = 'applied' AND p.pr_url IS NOT NULL",
    )?;

    let projections = stmt
        .query_map([], |row| {
            let applied_at_str: String = row.get(5)?;
            let applied_at = DateTime::parse_from_rfc3339(&applied_at_str)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let status_str: String = row.get(7)?;
            let status = ProjectionStatus::from_str(&status_str)
                .unwrap_or(ProjectionStatus::Applied);
            Ok(Projection {
                id: row.get(0)?,
                pattern_id: row.get(1)?,
                target_type: row.get(2)?,
                target_path: row.get(3)?,
                content: row.get(4)?,
                applied_at,
                pr_url: row.get(6)?,
                status,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(projections)
}

/// Get pattern IDs that have projections with specific statuses.
pub fn get_projected_pattern_ids_by_status(
    conn: &Connection,
    statuses: &[ProjectionStatus],
) -> Result<std::collections::HashSet<String>, CoreError> {
    if statuses.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let placeholders: Vec<String> = statuses.iter().enumerate().map(|(i, _)| format!("?{}", i + 1)).collect();
    let sql = format!(
        "SELECT DISTINCT pattern_id FROM projections WHERE status IN ({})",
        placeholders.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<String> = statuses.iter().map(|s| s.to_string()).collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
    let ids = stmt
        .query_map(param_refs.as_slice(), |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(ids)
}

/// Update a projection's PR URL.
pub fn update_projection_pr_url(
    conn: &Connection,
    projection_id: &str,
    pr_url: &str,
) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE projections SET pr_url = ?2 WHERE id = ?1",
        params![projection_id, pr_url],
    )?;
    Ok(())
}

// ── Knowledge graph: Migration ──

/// Migrate v1 patterns to v2 knowledge nodes. Returns number of nodes created.
/// Safe to call multiple times — skips patterns that already have corresponding nodes.
pub fn migrate_patterns_to_nodes(conn: &Connection) -> Result<usize, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, pattern_type, description, confidence, status, suggested_content, suggested_target, project, first_seen, last_seen
         FROM patterns",
    )?;
    let patterns: Vec<(String, String, String, f64, String, String, String, Option<String>, String, String)> = stmt.query_map([], |row| {
        Ok((
            row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
            row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?,
            row.get(8)?, row.get(9)?,
        ))
    })?.filter_map(|r| r.ok()).collect();

    let mut count = 0;
    for (id, pattern_type, description, confidence, _status, suggested_content, suggested_target, project, first_seen, last_seen) in &patterns {
        let node_id = format!("migrated-{id}");

        // Skip if already migrated
        if get_node(conn, &node_id)?.is_some() {
            continue;
        }

        // Determine node type using the spec's deterministic mapping
        let content_lower = suggested_content.to_lowercase();
        let has_directive_keyword = content_lower.contains("always") || content_lower.contains("never");
        let node_type = if *confidence >= 0.85 && has_directive_keyword {
            NodeType::Directive
        } else {
            match (pattern_type.as_str(), suggested_target.as_str()) {
                ("repetitive_instruction", "claude_md") => NodeType::Rule,
                ("repetitive_instruction", "skill") => NodeType::Directive,
                ("recurring_mistake", _) => NodeType::Pattern,
                ("workflow_pattern", "skill") => NodeType::Skill,
                ("workflow_pattern", "claude_md") => NodeType::Rule,
                ("stale_context", _) => NodeType::Memory,
                ("redundant_context", _) => NodeType::Memory,
                _ => NodeType::Pattern,
            }
        };

        let created_at = DateTime::parse_from_rfc3339(first_seen)
            .unwrap_or_default()
            .with_timezone(&Utc);
        let updated_at = DateTime::parse_from_rfc3339(last_seen)
            .unwrap_or_default()
            .with_timezone(&Utc);

        let node = KnowledgeNode {
            id: node_id,
            node_type,
            scope: NodeScope::Project,
            project_id: project.clone(),
            content: description.clone(),
            confidence: *confidence,
            status: NodeStatus::Active,
            created_at,
            updated_at,
            projected_at: None,
            pr_url: None,
        };
        insert_node(conn, &node)?;
        count += 1;
    }
    Ok(count)
}

// ── Knowledge graph: Node operations ──

pub fn insert_node(conn: &Connection, node: &KnowledgeNode) -> Result<(), CoreError> {
    conn.execute(
        "INSERT INTO nodes (id, type, scope, project_id, content, confidence, status, created_at, updated_at, projected_at, pr_url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            node.id,
            node.node_type.to_string(),
            node.scope.to_string(),
            node.project_id,
            node.content,
            node.confidence,
            node.status.to_string(),
            node.created_at.to_rfc3339(),
            node.updated_at.to_rfc3339(),
            node.projected_at,
            node.pr_url,
        ],
    )?;
    Ok(())
}

pub fn get_node(conn: &Connection, id: &str) -> Result<Option<KnowledgeNode>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, type, scope, project_id, content, confidence, status, created_at, updated_at, projected_at, pr_url
         FROM nodes WHERE id = ?1",
    )?;
    let result = stmt.query_row(params![id], |row| {
        Ok(KnowledgeNode {
            id: row.get(0)?,
            node_type: NodeType::from_str(&row.get::<_, String>(1)?),
            scope: NodeScope::from_str(&row.get::<_, String>(2)?),
            project_id: row.get(3)?,
            content: row.get(4)?,
            confidence: row.get(5)?,
            status: NodeStatus::from_str(&row.get::<_, String>(6)?),
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(8)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
            projected_at: row.get(9)?,
            pr_url: row.get(10)?,
        })
    }).optional()?;
    Ok(result)
}

pub fn get_nodes_by_scope(
    conn: &Connection,
    scope: &NodeScope,
    project_id: Option<&str>,
    statuses: &[NodeStatus],
) -> Result<Vec<KnowledgeNode>, CoreError> {
    if statuses.is_empty() {
        return Ok(Vec::new());
    }
    let status_placeholders: Vec<String> = statuses.iter().enumerate().map(|(i, _)| format!("?{}", i + 3)).collect();
    let sql = format!(
        "SELECT id, type, scope, project_id, content, confidence, status, created_at, updated_at, projected_at, pr_url
         FROM nodes WHERE scope = ?1 AND (?2 IS NULL OR project_id = ?2) AND status IN ({})
         ORDER BY confidence DESC",
        status_placeholders.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
        Box::new(scope.to_string()),
        Box::new(project_id.map(|s| s.to_string())),
    ];
    for s in statuses {
        params_vec.push(Box::new(s.to_string()));
    }
    let rows = stmt.query_map(rusqlite::params_from_iter(params_vec.iter().map(|p| p.as_ref())), |row| {
        Ok(KnowledgeNode {
            id: row.get(0)?,
            node_type: NodeType::from_str(&row.get::<_, String>(1)?),
            scope: NodeScope::from_str(&row.get::<_, String>(2)?),
            project_id: row.get(3)?,
            content: row.get(4)?,
            confidence: row.get(5)?,
            status: NodeStatus::from_str(&row.get::<_, String>(6)?),
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(8)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
            projected_at: row.get(9)?,
            pr_url: row.get(10)?,
        })
    })?;
    let mut nodes = Vec::new();
    for row in rows {
        nodes.push(row?);
    }
    Ok(nodes)
}

/// Get all nodes with a given status, ordered by confidence DESC.
pub fn get_nodes_by_status(
    conn: &Connection,
    status: &NodeStatus,
) -> Result<Vec<KnowledgeNode>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, type, scope, project_id, content, confidence, status, created_at, updated_at, projected_at, pr_url
         FROM nodes WHERE status = ?1
         ORDER BY confidence DESC",
    )?;
    let rows = stmt.query_map(params![status.to_string()], |row| {
        Ok(KnowledgeNode {
            id: row.get(0)?,
            node_type: NodeType::from_str(&row.get::<_, String>(1)?),
            scope: NodeScope::from_str(&row.get::<_, String>(2)?),
            project_id: row.get(3)?,
            content: row.get(4)?,
            confidence: row.get(5)?,
            status: NodeStatus::from_str(&row.get::<_, String>(6)?),
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(8)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
            projected_at: row.get(9)?,
            pr_url: row.get(10)?,
        })
    })?;
    let mut nodes = Vec::new();
    for row in rows {
        nodes.push(row?);
    }
    Ok(nodes)
}

pub fn update_node_confidence(conn: &Connection, id: &str, confidence: f64) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE nodes SET confidence = ?1, updated_at = ?2 WHERE id = ?3",
        params![confidence, Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

pub fn update_node_status(conn: &Connection, id: &str, status: &NodeStatus) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE nodes SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status.to_string(), Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

pub fn update_node_content(conn: &Connection, id: &str, content: &str) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE nodes SET content = ?1, updated_at = ?2 WHERE id = ?3",
        params![content, Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

// ── Knowledge graph: Edge operations ──

pub fn insert_edge(conn: &Connection, edge: &KnowledgeEdge) -> Result<(), CoreError> {
    conn.execute(
        "INSERT OR IGNORE INTO edges (source_id, target_id, type, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            edge.source_id,
            edge.target_id,
            edge.edge_type.to_string(),
            edge.created_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn get_edges_from(conn: &Connection, source_id: &str) -> Result<Vec<KnowledgeEdge>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT source_id, target_id, type, created_at FROM edges WHERE source_id = ?1",
    )?;
    let rows = stmt.query_map(params![source_id], |row| {
        Ok(KnowledgeEdge {
            source_id: row.get(0)?,
            target_id: row.get(1)?,
            edge_type: EdgeType::from_str(&row.get::<_, String>(2)?).unwrap_or(EdgeType::Supports),
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
        })
    })?;
    let mut edges = Vec::new();
    for row in rows {
        edges.push(row?);
    }
    Ok(edges)
}

pub fn get_edges_to(conn: &Connection, target_id: &str) -> Result<Vec<KnowledgeEdge>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT source_id, target_id, type, created_at FROM edges WHERE target_id = ?1",
    )?;
    let rows = stmt.query_map(params![target_id], |row| {
        Ok(KnowledgeEdge {
            source_id: row.get(0)?,
            target_id: row.get(1)?,
            edge_type: EdgeType::from_str(&row.get::<_, String>(2)?).unwrap_or(EdgeType::Supports),
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(3)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
        })
    })?;
    let mut edges = Vec::new();
    for row in rows {
        edges.push(row?);
    }
    Ok(edges)
}

pub fn delete_edge(conn: &Connection, source_id: &str, target_id: &str, edge_type: &EdgeType) -> Result<(), CoreError> {
    conn.execute(
        "DELETE FROM edges WHERE source_id = ?1 AND target_id = ?2 AND type = ?3",
        params![source_id, target_id, edge_type.to_string()],
    )?;
    Ok(())
}

/// Mark new_id as superseding old_id: archives old node and creates supersedes edge.
pub fn supersede_node(conn: &Connection, new_id: &str, old_id: &str) -> Result<(), CoreError> {
    update_node_status(conn, old_id, &NodeStatus::Archived)?;
    let edge = KnowledgeEdge {
        source_id: new_id.to_string(),
        target_id: old_id.to_string(),
        edge_type: EdgeType::Supersedes,
        created_at: Utc::now(),
    };
    insert_edge(conn, &edge)?;
    Ok(())
}

/// Result of applying a batch of graph operations.
#[derive(Debug, Clone, Default)]
pub struct ApplyGraphResult {
    pub nodes_created: usize,
    pub nodes_updated: usize,
    pub edges_created: usize,
    pub nodes_merged: usize,
}

/// Apply a batch of graph operations to the database.
pub fn apply_graph_operations(conn: &Connection, ops: &[GraphOperation]) -> Result<ApplyGraphResult, CoreError> {
    let mut result = ApplyGraphResult::default();

    for op in ops {
        match op {
            GraphOperation::CreateNode { node_type, scope, project_id, content, confidence } => {
                let node = KnowledgeNode {
                    id: uuid::Uuid::new_v4().to_string(),
                    node_type: node_type.clone(),
                    scope: scope.clone(),
                    project_id: project_id.clone(),
                    content: content.clone(),
                    confidence: *confidence,
                    status: NodeStatus::Active,
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    projected_at: None,
                    pr_url: None,
                };
                insert_node(conn, &node)?;
                result.nodes_created += 1;
            }
            GraphOperation::UpdateNode { id, confidence, content } => {
                if let Some(conf) = confidence {
                    update_node_confidence(conn, id, *conf)?;
                }
                if let Some(cont) = content {
                    update_node_content(conn, id, cont)?;
                }
                result.nodes_updated += 1;
            }
            GraphOperation::CreateEdge { source_id, target_id, edge_type } => {
                let edge = KnowledgeEdge {
                    source_id: source_id.clone(),
                    target_id: target_id.clone(),
                    edge_type: edge_type.clone(),
                    created_at: Utc::now(),
                };
                insert_edge(conn, &edge)?;
                result.edges_created += 1;
            }
            GraphOperation::MergeNodes { keep_id, remove_id } => {
                supersede_node(conn, keep_id, remove_id)?;
                result.nodes_merged += 1;
            }
        }
    }

    Ok(result)
}

// ── Knowledge graph: Project operations ──

/// Generate a human-readable slug from a repository path.
pub fn generate_project_slug(repo_path: &str) -> String {
    let name = std::path::Path::new(repo_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed-project");

    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "unnamed-project".to_string()
    } else {
        // Collapse consecutive hyphens
        let mut result = String::new();
        let mut prev_hyphen = false;
        for c in slug.chars() {
            if c == '-' {
                if !prev_hyphen {
                    result.push(c);
                }
                prev_hyphen = true;
            } else {
                result.push(c);
                prev_hyphen = false;
            }
        }
        result
    }
}

/// Generate a unique project slug, appending -2, -3, etc. if needed.
pub fn generate_unique_project_slug(conn: &Connection, repo_path: &str) -> Result<String, CoreError> {
    let base = generate_project_slug(repo_path);
    if get_project(conn, &base)?.is_none() {
        return Ok(base);
    }
    for i in 2..100 {
        let candidate = format!("{base}-{i}");
        if get_project(conn, &candidate)?.is_none() {
            return Ok(candidate);
        }
    }
    Ok(format!("{base}-{}", &uuid::Uuid::new_v4().to_string()[..8]))
}

pub fn upsert_project(conn: &Connection, project: &KnowledgeProject) -> Result<(), CoreError> {
    conn.execute(
        "INSERT INTO projects (id, path, remote_url, agent_type, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
             path = excluded.path,
             remote_url = COALESCE(excluded.remote_url, projects.remote_url),
             last_seen = excluded.last_seen",
        params![
            project.id,
            project.path,
            project.remote_url,
            project.agent_type,
            project.last_seen.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn get_project(conn: &Connection, id: &str) -> Result<Option<KnowledgeProject>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, path, remote_url, agent_type, last_seen FROM projects WHERE id = ?1",
    )?;
    let result = stmt.query_row(params![id], |row| {
        Ok(KnowledgeProject {
            id: row.get(0)?,
            path: row.get(1)?,
            remote_url: row.get(2)?,
            agent_type: row.get(3)?,
            last_seen: DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
        })
    }).optional()?;
    Ok(result)
}

pub fn get_project_by_remote_url(conn: &Connection, remote_url: &str) -> Result<Option<KnowledgeProject>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, path, remote_url, agent_type, last_seen FROM projects WHERE remote_url = ?1",
    )?;
    let result = stmt.query_row(params![remote_url], |row| {
        Ok(KnowledgeProject {
            id: row.get(0)?,
            path: row.get(1)?,
            remote_url: row.get(2)?,
            agent_type: row.get(3)?,
            last_seen: DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
        })
    }).optional()?;
    Ok(result)
}

/// Get active nodes that haven't been projected yet, ordered by confidence DESC.
pub fn get_unprojected_nodes(conn: &Connection) -> Result<Vec<KnowledgeNode>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, type, scope, project_id, content, confidence, status, created_at, updated_at, projected_at, pr_url
         FROM nodes WHERE status = 'active' AND projected_at IS NULL
         ORDER BY confidence DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(KnowledgeNode {
            id: row.get(0)?,
            node_type: NodeType::from_str(&row.get::<_, String>(1)?),
            scope: NodeScope::from_str(&row.get::<_, String>(2)?),
            project_id: row.get(3)?,
            content: row.get(4)?,
            confidence: row.get(5)?,
            status: NodeStatus::from_str(&row.get::<_, String>(6)?),
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(8)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
            projected_at: row.get(9)?,
            pr_url: row.get(10)?,
        })
    })?;
    let mut nodes = Vec::new();
    for row in rows {
        nodes.push(row?);
    }
    Ok(nodes)
}

/// Mark a node as projected (direct write, no PR).
pub fn mark_node_projected(conn: &Connection, id: &str) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE nodes SET projected_at = ?1 WHERE id = ?2",
        params![Utc::now().to_rfc3339(), id],
    )?;
    Ok(())
}

/// Mark a node as projected via PR.
pub fn mark_node_projected_with_pr(conn: &Connection, id: &str, pr_url: &str) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE nodes SET projected_at = ?1, pr_url = ?2 WHERE id = ?3",
        params![Utc::now().to_rfc3339(), pr_url, id],
    )?;
    Ok(())
}

/// Get all nodes with an associated PR URL.
pub fn get_nodes_with_pr(conn: &Connection) -> Result<Vec<KnowledgeNode>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, type, scope, project_id, content, confidence, status, created_at, updated_at, projected_at, pr_url
         FROM nodes WHERE pr_url IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(KnowledgeNode {
            id: row.get(0)?,
            node_type: NodeType::from_str(&row.get::<_, String>(1)?),
            scope: NodeScope::from_str(&row.get::<_, String>(2)?),
            project_id: row.get(3)?,
            content: row.get(4)?,
            confidence: row.get(5)?,
            status: NodeStatus::from_str(&row.get::<_, String>(6)?),
            created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(7)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(8)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
            projected_at: row.get(9)?,
            pr_url: row.get(10)?,
        })
    })?;
    let mut nodes = Vec::new();
    for row in rows {
        nodes.push(row?);
    }
    Ok(nodes)
}

/// Clear PR URL from nodes after merge.
pub fn clear_node_pr(conn: &Connection, pr_url: &str) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE nodes SET pr_url = NULL WHERE pr_url = ?1",
        params![pr_url],
    )?;
    Ok(())
}

/// Dismiss all nodes for a closed PR.
pub fn dismiss_nodes_for_pr(conn: &Connection, pr_url: &str) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE nodes SET status = 'dismissed', pr_url = NULL WHERE pr_url = ?1",
        params![pr_url],
    )?;
    Ok(())
}

pub fn get_all_projects(conn: &Connection) -> Result<Vec<KnowledgeProject>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, path, remote_url, agent_type, last_seen FROM projects ORDER BY last_seen DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(KnowledgeProject {
            id: row.get(0)?,
            path: row.get(1)?,
            remote_url: row.get(2)?,
            agent_type: row.get(3)?,
            last_seen: DateTime::parse_from_rfc3339(&row.get::<_, String>(4)?)
                .unwrap_or_default()
                .with_timezone(&Utc),
        })
    })?;
    let mut projects = Vec::new();
    for row in rows {
        projects.push(row?);
    }
    Ok(projects)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();
        conn
    }

    fn test_pattern(id: &str, description: &str) -> Pattern {
        Pattern {
            id: id.to_string(),
            pattern_type: PatternType::RepetitiveInstruction,
            description: description.to_string(),
            confidence: 0.85,
            times_seen: 1,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            last_projected: None,
            status: PatternStatus::Discovered,
            source_sessions: vec!["sess-1".to_string()],
            related_files: vec![],
            suggested_content: "Always do X".to_string(),
            suggested_target: SuggestedTarget::ClaudeMd,
            project: Some("/test/project".to_string()),
            generation_failed: false,
        }
    }

    #[test]
    fn test_insert_and_get_pattern() {
        let conn = test_db();
        let pattern = test_pattern("pat-1", "Use uv for Python packages");
        insert_pattern(&conn, &pattern).unwrap();

        let patterns = get_all_patterns(&conn, None).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].id, "pat-1");
        assert_eq!(patterns[0].description, "Use uv for Python packages");
        assert!((patterns[0].confidence - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_pattern_merge_update() {
        let conn = test_db();
        let pattern = test_pattern("pat-1", "Use uv for Python packages");
        insert_pattern(&conn, &pattern).unwrap();

        update_pattern_merge(
            &conn,
            "pat-1",
            &["sess-2".to_string(), "sess-3".to_string()],
            0.92,
            Utc::now(),
            2,
        )
        .unwrap();

        let patterns = get_all_patterns(&conn, None).unwrap();
        assert_eq!(patterns[0].times_seen, 3);
        assert!((patterns[0].confidence - 0.92).abs() < f64::EPSILON);
        assert_eq!(patterns[0].source_sessions.len(), 3);
    }

    #[test]
    fn test_get_patterns_by_status() {
        let conn = test_db();
        let p1 = test_pattern("pat-1", "Pattern one");
        let mut p2 = test_pattern("pat-2", "Pattern two");
        p2.status = PatternStatus::Active;
        insert_pattern(&conn, &p1).unwrap();
        insert_pattern(&conn, &p2).unwrap();

        let discovered = get_patterns(&conn, &["discovered"], None).unwrap();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].id, "pat-1");

        let active = get_patterns(&conn, &["active"], None).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "pat-2");

        let both = get_patterns(&conn, &["discovered", "active"], None).unwrap();
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn test_analyzed_session_tracking() {
        let conn = test_db();
        assert!(!is_session_analyzed(&conn, "sess-1").unwrap());

        record_analyzed_session(&conn, "sess-1", "/test").unwrap();
        assert!(is_session_analyzed(&conn, "sess-1").unwrap());
        assert!(!is_session_analyzed(&conn, "sess-2").unwrap());
    }

    #[test]
    fn test_sessions_for_analysis() {
        let conn = test_db();

        // Record an ingested session
        let session = IngestedSession {
            session_id: "sess-1".to_string(),
            project: "/test".to_string(),
            session_path: "/tmp/test.jsonl".to_string(),
            file_size: 100,
            file_mtime: "2026-01-01T00:00:00Z".to_string(),
            ingested_at: Utc::now(),
        };
        record_ingested_session(&conn, &session).unwrap();

        // It should appear in sessions for analysis (non-rolling)
        let since = Utc::now() - chrono::Duration::days(14);
        let pending = get_sessions_for_analysis(&conn, None, &since, false).unwrap();
        assert_eq!(pending.len(), 1);

        // After marking as analyzed, it should not appear in non-rolling mode
        record_analyzed_session(&conn, "sess-1", "/test").unwrap();
        let pending = get_sessions_for_analysis(&conn, None, &since, false).unwrap();
        assert_eq!(pending.len(), 0);

        // But it SHOULD still appear in rolling window mode
        let pending = get_sessions_for_analysis(&conn, None, &since, true).unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn test_insert_and_check_projection() {
        let conn = test_db();
        let pattern = test_pattern("pat-1", "Use uv");
        insert_pattern(&conn, &pattern).unwrap();

        assert!(!has_projection_for_pattern(&conn, "pat-1").unwrap());

        let proj = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "claude_md".to_string(),
            target_path: "/test/CLAUDE.md".to_string(),
            content: "Always use uv".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::Applied,
        };
        insert_projection(&conn, &proj).unwrap();

        assert!(has_projection_for_pattern(&conn, "pat-1").unwrap());
        assert!(!has_projection_for_pattern(&conn, "pat-2").unwrap());
    }

    #[test]
    fn test_update_pattern_status() {
        let conn = test_db();
        let pattern = test_pattern("pat-1", "Test pattern");
        insert_pattern(&conn, &pattern).unwrap();

        update_pattern_status(&conn, "pat-1", &PatternStatus::Active).unwrap();
        let patterns = get_patterns(&conn, &["active"], None).unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].id, "pat-1");
    }

    #[test]
    fn test_set_generation_failed() {
        let conn = test_db();
        let pattern = test_pattern("pat-1", "Test pattern");
        insert_pattern(&conn, &pattern).unwrap();

        assert!(!get_all_patterns(&conn, None).unwrap()[0].generation_failed);

        set_generation_failed(&conn, "pat-1", true).unwrap();
        assert!(get_all_patterns(&conn, None).unwrap()[0].generation_failed);

        set_generation_failed(&conn, "pat-1", false).unwrap();
        assert!(!get_all_patterns(&conn, None).unwrap()[0].generation_failed);
    }

    #[test]
    fn test_projections_nudged_column_defaults_to_zero() {
        let conn = test_db();

        // Verify the nudged column exists by preparing a statement that references it
        conn.prepare("SELECT nudged FROM projections").unwrap();

        // Insert a projection without specifying nudged — should default to 0
        let pattern = test_pattern("pat-1", "Test pattern");
        insert_pattern(&conn, &pattern).unwrap();

        let proj = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "claude_md".to_string(),
            target_path: "/test/CLAUDE.md".to_string(),
            content: "Always use uv".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::Applied,
        };
        insert_projection(&conn, &proj).unwrap();

        let nudged: i64 = conn
            .query_row(
                "SELECT nudged FROM projections WHERE id = 'proj-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(nudged, 0, "nudged column should default to 0");
    }

    // ── Tests for auto-apply pipeline DB functions ──

    #[test]
    fn test_last_applied_at_empty() {
        let conn = test_db();
        let result = last_applied_at(&conn).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_last_applied_at_returns_max() {
        let conn = test_db();

        // Insert two patterns to serve as FK targets
        let p1 = test_pattern("pat-1", "Pattern one");
        let p2 = test_pattern("pat-2", "Pattern two");
        insert_pattern(&conn, &p1).unwrap();
        insert_pattern(&conn, &p2).unwrap();

        // Insert projections with different timestamps
        let earlier = chrono::DateTime::parse_from_rfc3339("2026-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let later = chrono::DateTime::parse_from_rfc3339("2026-02-15T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let proj1 = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "Skill".to_string(),
            target_path: "/path/a".to_string(),
            content: "content a".to_string(),
            applied_at: earlier,
            pr_url: None,
            status: ProjectionStatus::Applied,
        };
        let proj2 = Projection {
            id: "proj-2".to_string(),
            pattern_id: "pat-2".to_string(),
            target_type: "Skill".to_string(),
            target_path: "/path/b".to_string(),
            content: "content b".to_string(),
            applied_at: later,
            pr_url: None,
            status: ProjectionStatus::Applied,
        };
        insert_projection(&conn, &proj1).unwrap();
        insert_projection(&conn, &proj2).unwrap();

        let result = last_applied_at(&conn).unwrap();
        assert!(result.is_some());
        // The max should be the later timestamp
        let max_ts = result.unwrap();
        assert!(max_ts.contains("2026-02-15"), "Expected later timestamp, got: {}", max_ts);
    }

    #[test]
    fn test_has_unanalyzed_sessions_empty() {
        let conn = test_db();
        assert!(!has_unanalyzed_sessions(&conn).unwrap());
    }

    #[test]
    fn test_has_unanalyzed_sessions_with_new_session() {
        let conn = test_db();

        let session = IngestedSession {
            session_id: "sess-1".to_string(),
            project: "/test".to_string(),
            session_path: "/tmp/test.jsonl".to_string(),
            file_size: 100,
            file_mtime: "2026-01-01T00:00:00Z".to_string(),
            ingested_at: Utc::now(),
        };
        record_ingested_session(&conn, &session).unwrap();

        assert!(has_unanalyzed_sessions(&conn).unwrap());
    }

    #[test]
    fn test_has_unanalyzed_sessions_after_analysis() {
        let conn = test_db();

        let session = IngestedSession {
            session_id: "sess-1".to_string(),
            project: "/test".to_string(),
            session_path: "/tmp/test.jsonl".to_string(),
            file_size: 100,
            file_mtime: "2026-01-01T00:00:00Z".to_string(),
            ingested_at: Utc::now(),
        };
        record_ingested_session(&conn, &session).unwrap();
        record_analyzed_session(&conn, "sess-1", "/test").unwrap();

        assert!(!has_unanalyzed_sessions(&conn).unwrap());
    }

    #[test]
    fn test_has_unprojected_patterns_empty() {
        let conn = test_db();
        assert!(!has_unprojected_patterns(&conn, 0.0).unwrap());
    }

    #[test]
    fn test_has_unprojected_patterns_with_discovered() {
        let conn = test_db();

        let pattern = test_pattern("pat-1", "Use uv for Python");
        insert_pattern(&conn, &pattern).unwrap();

        assert!(has_unprojected_patterns(&conn, 0.0).unwrap());
    }

    #[test]
    fn test_has_unprojected_patterns_after_projection() {
        let conn = test_db();

        let pattern = test_pattern("pat-1", "Use uv for Python");
        insert_pattern(&conn, &pattern).unwrap();

        let proj = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "Skill".to_string(),
            target_path: "/path".to_string(),
            content: "content".to_string(),
            applied_at: Utc::now(),
            pr_url: Some("https://github.com/test/pull/1".to_string()),
            status: ProjectionStatus::Applied,
        };
        insert_projection(&conn, &proj).unwrap();

        assert!(!has_unprojected_patterns(&conn, 0.0).unwrap());
    }

    #[test]
    fn test_has_unprojected_patterns_excludes_generation_failed() {
        let conn = test_db();

        let pattern = test_pattern("pat-1", "Use uv for Python");
        insert_pattern(&conn, &pattern).unwrap();
        set_generation_failed(&conn, "pat-1", true).unwrap();

        assert!(!has_unprojected_patterns(&conn, 0.0).unwrap());
    }

    #[test]
    fn test_has_unprojected_patterns_excludes_dbonly() {
        let conn = test_db();

        let mut pattern = test_pattern("pat-1", "Internal tracking only");
        pattern.suggested_target = SuggestedTarget::DbOnly;
        insert_pattern(&conn, &pattern).unwrap();

        assert!(!has_unprojected_patterns(&conn, 0.0).unwrap());
    }

    #[test]
    fn test_auto_apply_data_triggers_full_flow() {
        let conn = test_db();

        // Initially: no data, no triggers
        assert!(!has_unanalyzed_sessions(&conn).unwrap());
        assert!(!has_unprojected_patterns(&conn, 0.0).unwrap());

        // Step 1: Ingest creates sessions → triggers analyze
        let session = IngestedSession {
            session_id: "sess-1".to_string(),
            project: "/proj".to_string(),
            session_path: "/path/sess".to_string(),
            file_size: 100,
            file_mtime: "2025-01-01T00:00:00Z".to_string(),
            ingested_at: Utc::now(),
        };
        record_ingested_session(&conn, &session).unwrap();
        assert!(has_unanalyzed_sessions(&conn).unwrap());

        // Step 2: After analysis → sessions marked, patterns created → triggers apply
        record_analyzed_session(&conn, "sess-1", "/proj").unwrap();
        assert!(!has_unanalyzed_sessions(&conn).unwrap());

        let p = test_pattern("pat-1", "Always use cargo fmt");
        insert_pattern(&conn, &p).unwrap();
        assert!(has_unprojected_patterns(&conn, 0.0).unwrap());

        // Step 3: After apply → projection created with PR URL
        let proj = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "Skill".to_string(),
            target_path: "/skills/cargo-fmt.md".to_string(),
            content: "skill content".to_string(),
            applied_at: Utc::now(),
            pr_url: Some("https://github.com/test/pull/42".to_string()),
            status: ProjectionStatus::Applied,
        };
        insert_projection(&conn, &proj).unwrap();
        assert!(!has_unprojected_patterns(&conn, 0.0).unwrap());
    }

    #[test]
    fn test_get_last_nudge_at_empty() {
        let conn = test_db();
        assert!(get_last_nudge_at(&conn).unwrap().is_none());
    }

    #[test]
    fn test_unanalyzed_session_count() {
        let conn = test_db();
        assert_eq!(unanalyzed_session_count(&conn).unwrap(), 0);

        // Add 3 sessions
        for i in 1..=3 {
            let session = IngestedSession {
                session_id: format!("sess-{i}"),
                project: "/proj".to_string(),
                session_path: format!("/path/sess-{i}"),
                file_size: 100,
                file_mtime: "2025-01-01T00:00:00Z".to_string(),
                ingested_at: Utc::now(),
            };
            record_ingested_session(&conn, &session).unwrap();
        }
        assert_eq!(unanalyzed_session_count(&conn).unwrap(), 3);

        // Analyze one
        record_analyzed_session(&conn, "sess-1", "/proj").unwrap();
        assert_eq!(unanalyzed_session_count(&conn).unwrap(), 2);
    }

    #[test]
    fn test_set_and_get_last_nudge_at() {
        let conn = test_db();
        let now = Utc::now();
        set_last_nudge_at(&conn, &now).unwrap();
        let result = get_last_nudge_at(&conn).unwrap().unwrap();
        // Compare to second precision (DB stores RFC 3339)
        assert_eq!(
            result.format("%Y-%m-%dT%H:%M:%S").to_string(),
            now.format("%Y-%m-%dT%H:%M:%S").to_string()
        );
    }

    #[test]
    fn test_projection_status_column_exists() {
        let conn = test_db();
        let pattern = test_pattern("pat-1", "Test");
        insert_pattern(&conn, &pattern).unwrap();

        let proj = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "skill".to_string(),
            target_path: "/test/skill.md".to_string(),
            content: "content".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::PendingReview,
        };
        insert_projection(&conn, &proj).unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM projections WHERE id = 'proj-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(status, "pending_review");
    }

    #[test]
    fn test_existing_projections_default_to_applied() {
        // Simulate a v2 database with existing projections
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();

        // Create v1 schema manually
        conn.execute_batch(
            "CREATE TABLE patterns (
                id TEXT PRIMARY KEY, pattern_type TEXT NOT NULL, description TEXT NOT NULL,
                confidence REAL NOT NULL, times_seen INTEGER NOT NULL DEFAULT 1,
                first_seen TEXT NOT NULL, last_seen TEXT NOT NULL, last_projected TEXT,
                status TEXT NOT NULL DEFAULT 'discovered', source_sessions TEXT NOT NULL,
                related_files TEXT NOT NULL, suggested_content TEXT NOT NULL,
                suggested_target TEXT NOT NULL, project TEXT,
                generation_failed INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE projections (
                id TEXT PRIMARY KEY, pattern_id TEXT NOT NULL REFERENCES patterns(id),
                target_type TEXT NOT NULL, target_path TEXT NOT NULL, content TEXT NOT NULL,
                applied_at TEXT NOT NULL, pr_url TEXT, nudged INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE analyzed_sessions (session_id TEXT PRIMARY KEY, project TEXT NOT NULL, analyzed_at TEXT NOT NULL);
            CREATE TABLE ingested_sessions (session_id TEXT PRIMARY KEY, project TEXT NOT NULL, session_path TEXT NOT NULL, file_size INTEGER NOT NULL, file_mtime TEXT NOT NULL, ingested_at TEXT NOT NULL);
            PRAGMA user_version = 1;",
        ).unwrap();

        // Insert a pattern first (FK target)
        conn.execute(
            "INSERT INTO patterns (id, pattern_type, description, confidence, first_seen, last_seen, status, source_sessions, related_files, suggested_content, suggested_target)
             VALUES ('pat-1', 'workflow_pattern', 'Test', 0.8, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'discovered', '[]', '[]', 'content', 'skill')",
            [],
        ).unwrap();

        // Insert an old-style projection (no status column)
        conn.execute(
            "INSERT INTO projections (id, pattern_id, target_type, target_path, content, applied_at)
             VALUES ('proj-old', 'pat-1', 'skill', '/path', 'content', '2026-01-01T00:00:00Z')",
            [],
        ).unwrap();

        // Now run migration (open_db equivalent)
        migrate(&conn).unwrap();

        // Old projection should have status = 'applied'
        let status: String = conn
            .query_row("SELECT status FROM projections WHERE id = 'proj-old'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "applied");
    }

    #[test]
    fn test_get_pending_review_projections() {
        let conn = test_db();
        let p1 = test_pattern("pat-1", "Pattern one");
        let p2 = test_pattern("pat-2", "Pattern two");
        insert_pattern(&conn, &p1).unwrap();
        insert_pattern(&conn, &p2).unwrap();

        // One pending, one applied
        let proj1 = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "skill".to_string(),
            target_path: "/test/a.md".to_string(),
            content: "content a".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::PendingReview,
        };
        let proj2 = Projection {
            id: "proj-2".to_string(),
            pattern_id: "pat-2".to_string(),
            target_type: "skill".to_string(),
            target_path: "/test/b.md".to_string(),
            content: "content b".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::Applied,
        };
        insert_projection(&conn, &proj1).unwrap();
        insert_projection(&conn, &proj2).unwrap();

        let pending = get_pending_review_projections(&conn).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "proj-1");
    }

    #[test]
    fn test_update_projection_status() {
        let conn = test_db();
        let p = test_pattern("pat-1", "Pattern");
        insert_pattern(&conn, &p).unwrap();

        let proj = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "skill".to_string(),
            target_path: "/test.md".to_string(),
            content: "content".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::PendingReview,
        };
        insert_projection(&conn, &proj).unwrap();

        update_projection_status(&conn, "proj-1", &ProjectionStatus::Applied).unwrap();

        let status: String = conn
            .query_row("SELECT status FROM projections WHERE id = 'proj-1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(status, "applied");
    }

    #[test]
    fn test_delete_projection() {
        let conn = test_db();
        let p = test_pattern("pat-1", "Pattern");
        insert_pattern(&conn, &p).unwrap();

        let proj = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "skill".to_string(),
            target_path: "/test.md".to_string(),
            content: "content".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::PendingReview,
        };
        insert_projection(&conn, &proj).unwrap();
        assert!(has_projection_for_pattern(&conn, "pat-1").unwrap());

        delete_projection(&conn, "proj-1").unwrap();
        assert!(!has_projection_for_pattern(&conn, "pat-1").unwrap());
    }

    #[test]
    fn test_get_projections_with_pr_url() {
        let conn = test_db();
        let p1 = test_pattern("pat-1", "Pattern one");
        let p2 = test_pattern("pat-2", "Pattern two");
        insert_pattern(&conn, &p1).unwrap();
        insert_pattern(&conn, &p2).unwrap();

        let proj1 = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "skill".to_string(),
            target_path: "/a.md".to_string(),
            content: "a".to_string(),
            applied_at: Utc::now(),
            pr_url: Some("https://github.com/test/pull/1".to_string()),
            status: ProjectionStatus::Applied,
        };
        let proj2 = Projection {
            id: "proj-2".to_string(),
            pattern_id: "pat-2".to_string(),
            target_type: "skill".to_string(),
            target_path: "/b.md".to_string(),
            content: "b".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::Applied,
        };
        insert_projection(&conn, &proj1).unwrap();
        insert_projection(&conn, &proj2).unwrap();

        let with_pr = get_applied_projections_with_pr(&conn).unwrap();
        assert_eq!(with_pr.len(), 1);
        assert_eq!(with_pr[0].pr_url, Some("https://github.com/test/pull/1".to_string()));
    }

    #[test]
    fn test_get_projected_pattern_ids_by_status() {
        let conn = test_db();
        let p1 = test_pattern("pat-1", "Pattern one");
        let p2 = test_pattern("pat-2", "Pattern two");
        insert_pattern(&conn, &p1).unwrap();
        insert_pattern(&conn, &p2).unwrap();

        let proj1 = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "skill".to_string(),
            target_path: "/a.md".to_string(),
            content: "a".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::Applied,
        };
        let proj2 = Projection {
            id: "proj-2".to_string(),
            pattern_id: "pat-2".to_string(),
            target_type: "skill".to_string(),
            target_path: "/b.md".to_string(),
            content: "b".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::PendingReview,
        };
        insert_projection(&conn, &proj1).unwrap();
        insert_projection(&conn, &proj2).unwrap();

        let ids = get_projected_pattern_ids_by_status(&conn, &[ProjectionStatus::Applied, ProjectionStatus::PendingReview]).unwrap();
        assert_eq!(ids.len(), 2);

        let ids_applied_only = get_projected_pattern_ids_by_status(&conn, &[ProjectionStatus::Applied]).unwrap();
        assert_eq!(ids_applied_only.len(), 1);
        assert!(ids_applied_only.contains("pat-1"));
    }

    #[test]
    fn test_has_unprojected_patterns_excludes_dismissed() {
        let conn = test_db();

        let mut pattern = test_pattern("pat-1", "Dismissed pattern");
        pattern.status = PatternStatus::Dismissed;
        insert_pattern(&conn, &pattern).unwrap();

        assert!(!has_unprojected_patterns(&conn, 0.0).unwrap());
    }

    #[test]
    fn test_has_unprojected_patterns_excludes_pending_review() {
        let conn = test_db();

        let pattern = test_pattern("pat-1", "Pattern with pending review");
        insert_pattern(&conn, &pattern).unwrap();

        // Create a pending_review projection
        let proj = Projection {
            id: "proj-1".to_string(),
            pattern_id: "pat-1".to_string(),
            target_type: "skill".to_string(),
            target_path: "/test.md".to_string(),
            content: "content".to_string(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::PendingReview,
        };
        insert_projection(&conn, &proj).unwrap();

        // Pattern already has a pending_review projection — should NOT be "unprojected"
        assert!(!has_unprojected_patterns(&conn, 0.0).unwrap());
    }

    #[test]
    fn test_v4_migration_creates_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 5);

        // Verify nodes table exists with correct columns
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE 1=0", [], |row| row.get(0)
        ).unwrap();
        assert_eq!(count, 0);

        // Verify edges table exists
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE 1=0", [], |row| row.get(0)
        ).unwrap();
        assert_eq!(count, 0);

        // Verify projects table exists
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM projects WHERE 1=0", [], |row| row.get(0)
        ).unwrap();
        assert_eq!(count, 0);
    }

    // ── Node CRUD tests ──

    #[test]
    fn test_insert_and_get_node() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let node = KnowledgeNode {
            id: "node-1".to_string(),
            node_type: NodeType::Rule,
            scope: NodeScope::Project,
            project_id: Some("my-app".to_string()),
            content: "Always run tests".to_string(),
            confidence: 0.85,
            status: NodeStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        };

        insert_node(&conn, &node).unwrap();
        let retrieved = get_node(&conn, "node-1").unwrap().unwrap();
        assert_eq!(retrieved.content, "Always run tests");
        assert_eq!(retrieved.node_type, NodeType::Rule);
        assert_eq!(retrieved.scope, NodeScope::Project);
        assert_eq!(retrieved.confidence, 0.85);
    }

    #[test]
    fn test_get_nodes_by_scope_and_status() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let now = Utc::now();
        for (i, scope) in [NodeScope::Global, NodeScope::Project, NodeScope::Global].iter().enumerate() {
            let node = KnowledgeNode {
                id: format!("node-{i}"),
                node_type: NodeType::Rule,
                scope: scope.clone(),
                project_id: if *scope == NodeScope::Project { Some("my-app".to_string()) } else { None },
                content: format!("Rule {i}"),
                confidence: 0.8,
                status: NodeStatus::Active,
                created_at: now,
                updated_at: now,
                projected_at: None,
                pr_url: None,
            };
            insert_node(&conn, &node).unwrap();
        }

        let global_nodes = get_nodes_by_scope(&conn, &NodeScope::Global, None, &[NodeStatus::Active]).unwrap();
        assert_eq!(global_nodes.len(), 2);

        let project_nodes = get_nodes_by_scope(&conn, &NodeScope::Project, Some("my-app"), &[NodeStatus::Active]).unwrap();
        assert_eq!(project_nodes.len(), 1);
    }

    #[test]
    fn test_update_node_confidence() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let node = KnowledgeNode {
            id: "node-1".to_string(),
            node_type: NodeType::Pattern,
            scope: NodeScope::Project,
            project_id: Some("my-app".to_string()),
            content: "Forgets tests".to_string(),
            confidence: 0.5,
            status: NodeStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        };
        insert_node(&conn, &node).unwrap();

        update_node_confidence(&conn, "node-1", 0.75).unwrap();
        let updated = get_node(&conn, "node-1").unwrap().unwrap();
        assert_eq!(updated.confidence, 0.75);
    }

    #[test]
    fn test_update_node_status() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let node = KnowledgeNode {
            id: "node-1".to_string(),
            node_type: NodeType::Rule,
            scope: NodeScope::Global,
            project_id: None,
            content: "Use snake_case".to_string(),
            confidence: 0.9,
            status: NodeStatus::PendingReview,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        };
        insert_node(&conn, &node).unwrap();

        update_node_status(&conn, "node-1", &NodeStatus::Active).unwrap();
        let updated = get_node(&conn, "node-1").unwrap().unwrap();
        assert_eq!(updated.status, NodeStatus::Active);
    }

    #[test]
    fn test_v4_migration_from_v3() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();

        // Manually create a v3-state database (v1+v2+v3 tables, user_version=3)
        conn.execute_batch("
            CREATE TABLE patterns (id TEXT PRIMARY KEY, pattern_type TEXT NOT NULL, description TEXT NOT NULL, confidence REAL NOT NULL, times_seen INTEGER NOT NULL DEFAULT 1, first_seen TEXT NOT NULL, last_seen TEXT NOT NULL, last_projected TEXT, status TEXT NOT NULL DEFAULT 'discovered', source_sessions TEXT NOT NULL, related_files TEXT NOT NULL, suggested_content TEXT NOT NULL, suggested_target TEXT NOT NULL, project TEXT, generation_failed INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE projections (id TEXT PRIMARY KEY, pattern_id TEXT NOT NULL, target_type TEXT NOT NULL, target_path TEXT NOT NULL, content TEXT NOT NULL, applied_at TEXT NOT NULL, pr_url TEXT, nudged INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'applied');
            CREATE TABLE analyzed_sessions (session_id TEXT PRIMARY KEY, project TEXT NOT NULL, analyzed_at TEXT NOT NULL);
            CREATE TABLE ingested_sessions (session_id TEXT PRIMARY KEY, project TEXT NOT NULL, session_path TEXT NOT NULL, file_size INTEGER NOT NULL, file_mtime TEXT NOT NULL, ingested_at TEXT NOT NULL);
            CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        ").unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();

        // Now run migrate — should only add v4 tables
        migrate(&conn).unwrap();

        let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 5);

        // v4 tables exist
        conn.query_row("SELECT COUNT(*) FROM nodes WHERE 1=0", [], |row| row.get::<_, i64>(0)).unwrap();
        conn.query_row("SELECT COUNT(*) FROM edges WHERE 1=0", [], |row| row.get::<_, i64>(0)).unwrap();
        conn.query_row("SELECT COUNT(*) FROM projects WHERE 1=0", [], |row| row.get::<_, i64>(0)).unwrap();

        // Old tables still exist
        conn.query_row("SELECT COUNT(*) FROM patterns WHERE 1=0", [], |row| row.get::<_, i64>(0)).unwrap();
    }

    // ── Edge CRUD tests ──

    #[test]
    fn test_insert_and_get_edges() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let now = Utc::now();
        let node1 = KnowledgeNode {
            id: "node-1".to_string(), node_type: NodeType::Pattern,
            scope: NodeScope::Project, project_id: Some("app".to_string()),
            content: "Pattern A".to_string(), confidence: 0.5,
            status: NodeStatus::Active, created_at: now, updated_at: now,
            projected_at: None, pr_url: None,
        };
        let node2 = KnowledgeNode {
            id: "node-2".to_string(), node_type: NodeType::Rule,
            scope: NodeScope::Project, project_id: Some("app".to_string()),
            content: "Rule B".to_string(), confidence: 0.8,
            status: NodeStatus::Active, created_at: now, updated_at: now,
            projected_at: None, pr_url: None,
        };
        insert_node(&conn, &node1).unwrap();
        insert_node(&conn, &node2).unwrap();

        let edge = KnowledgeEdge {
            source_id: "node-1".to_string(),
            target_id: "node-2".to_string(),
            edge_type: EdgeType::DerivedFrom,
            created_at: now,
        };
        insert_edge(&conn, &edge).unwrap();

        let edges = get_edges_from(&conn, "node-1").unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].target_id, "node-2");
        assert_eq!(edges[0].edge_type, EdgeType::DerivedFrom);

        let edges_to = get_edges_to(&conn, "node-2").unwrap();
        assert_eq!(edges_to.len(), 1);
        assert_eq!(edges_to[0].source_id, "node-1");
    }

    #[test]
    fn test_supersede_node_archives_old() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let now = Utc::now();
        let old_node = KnowledgeNode {
            id: "old".to_string(), node_type: NodeType::Rule,
            scope: NodeScope::Global, project_id: None,
            content: "Old rule".to_string(), confidence: 0.8,
            status: NodeStatus::Active, created_at: now, updated_at: now,
            projected_at: None, pr_url: None,
        };
        let new_node = KnowledgeNode {
            id: "new".to_string(), node_type: NodeType::Rule,
            scope: NodeScope::Global, project_id: None,
            content: "New rule".to_string(), confidence: 0.85,
            status: NodeStatus::Active, created_at: now, updated_at: now,
            projected_at: None, pr_url: None,
        };
        insert_node(&conn, &old_node).unwrap();
        insert_node(&conn, &new_node).unwrap();

        supersede_node(&conn, "new", "old").unwrap();

        let old = get_node(&conn, "old").unwrap().unwrap();
        assert_eq!(old.status, NodeStatus::Archived);

        let edges = get_edges_from(&conn, "new").unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, EdgeType::Supersedes);
        assert_eq!(edges[0].target_id, "old");
    }

    // ── Project CRUD tests ──

    #[test]
    fn test_upsert_and_get_project() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let project = KnowledgeProject {
            id: "my-app".to_string(),
            path: "/home/user/my-app".to_string(),
            remote_url: Some("git@github.com:user/my-app.git".to_string()),
            agent_type: "claude_code".to_string(),
            last_seen: Utc::now(),
        };
        upsert_project(&conn, &project).unwrap();

        let retrieved = get_project(&conn, "my-app").unwrap().unwrap();
        assert_eq!(retrieved.path, "/home/user/my-app");
        assert_eq!(retrieved.remote_url.unwrap(), "git@github.com:user/my-app.git");
    }

    #[test]
    fn test_get_project_by_remote_url() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let project = KnowledgeProject {
            id: "my-app".to_string(),
            path: "/old/path".to_string(),
            remote_url: Some("git@github.com:user/my-app.git".to_string()),
            agent_type: "claude_code".to_string(),
            last_seen: Utc::now(),
        };
        upsert_project(&conn, &project).unwrap();

        let found = get_project_by_remote_url(&conn, "git@github.com:user/my-app.git").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "my-app");
    }

    #[test]
    fn test_get_all_projects() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        for name in ["app-1", "app-2"] {
            let project = KnowledgeProject {
                id: name.to_string(),
                path: format!("/home/{name}"),
                remote_url: None,
                agent_type: "claude_code".to_string(),
                last_seen: Utc::now(),
            };
            upsert_project(&conn, &project).unwrap();
        }

        let projects = get_all_projects(&conn).unwrap();
        assert_eq!(projects.len(), 2);
    }

    #[test]
    fn test_generate_project_slug() {
        assert_eq!(generate_project_slug("/home/user/my-rust-app"), "my-rust-app");
        assert_eq!(generate_project_slug("/home/user/My App"), "my-app");
        assert_eq!(generate_project_slug("/"), "unnamed-project");
    }

    #[test]
    fn test_migrate_patterns_to_nodes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        // Insert v1 patterns
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO patterns (id, pattern_type, description, confidence, times_seen, first_seen, last_seen, status, source_sessions, related_files, suggested_content, suggested_target, project, generation_failed)
             VALUES (?1, ?2, ?3, ?4, 2, ?5, ?5, 'active', '[]', '[]', 'content', ?6, ?7, 0)",
            params!["p1", "repetitive_instruction", "Always run tests", 0.85, &now, "claude_md", "my-app"],
        ).unwrap();
        conn.execute(
            "INSERT INTO patterns (id, pattern_type, description, confidence, times_seen, first_seen, last_seen, status, source_sessions, related_files, suggested_content, suggested_target, project, generation_failed)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5, 'discovered', '[]', '[]', 'content', ?6, ?7, 0)",
            params!["p2", "recurring_mistake", "Forgets imports", 0.6, &now, "skill", "my-app"],
        ).unwrap();
        conn.execute(
            "INSERT INTO patterns (id, pattern_type, description, confidence, times_seen, first_seen, last_seen, status, source_sessions, related_files, suggested_content, suggested_target, project, generation_failed)
             VALUES (?1, ?2, ?3, ?4, 3, ?5, ?5, 'active', '[]', '[]', 'Always use snake_case', ?6, ?7, 0)",
            params!["p3", "repetitive_instruction", "Always use snake_case", 0.9, &now, "claude_md", "my-app"],
        ).unwrap();

        let count = migrate_patterns_to_nodes(&conn).unwrap();
        assert_eq!(count, 3);

        // p1: RepetitiveInstruction + ClaudeMd -> rule
        let node1 = get_node(&conn, "migrated-p1").unwrap().unwrap();
        assert_eq!(node1.node_type, NodeType::Rule);
        assert_eq!(node1.scope, NodeScope::Project);

        // p2: RecurringMistake -> pattern
        let node2 = get_node(&conn, "migrated-p2").unwrap().unwrap();
        assert_eq!(node2.node_type, NodeType::Pattern);

        // p3: confidence >= 0.85 + "always" in content -> directive (override)
        let node3 = get_node(&conn, "migrated-p3").unwrap().unwrap();
        assert_eq!(node3.node_type, NodeType::Directive);
    }

    #[test]
    fn test_apply_graph_operations() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let ops = vec![
            GraphOperation::CreateNode {
                node_type: NodeType::Rule,
                scope: NodeScope::Project,
                project_id: Some("my-app".to_string()),
                content: "Always run tests".to_string(),
                confidence: 0.85,
            },
            GraphOperation::CreateNode {
                node_type: NodeType::Pattern,
                scope: NodeScope::Global,
                project_id: None,
                content: "Prefers TDD".to_string(),
                confidence: 0.6,
            },
        ];

        let result = apply_graph_operations(&conn, &ops).unwrap();
        assert_eq!(result.nodes_created, 2);

        let nodes = get_nodes_by_scope(&conn, &NodeScope::Project, Some("my-app"), &[NodeStatus::Active]).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].content, "Always run tests");
    }

    #[test]
    fn test_apply_graph_operations_update() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        let node = KnowledgeNode {
            id: "node-1".to_string(),
            node_type: NodeType::Pattern,
            scope: NodeScope::Project,
            project_id: Some("app".to_string()),
            content: "Old content".to_string(),
            confidence: 0.5,
            status: NodeStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        };
        insert_node(&conn, &node).unwrap();

        let ops = vec![
            GraphOperation::UpdateNode {
                id: "node-1".to_string(),
                confidence: Some(0.8),
                content: Some("Updated content".to_string()),
            },
        ];

        let result = apply_graph_operations(&conn, &ops).unwrap();
        assert_eq!(result.nodes_updated, 1);

        let updated = get_node(&conn, "node-1").unwrap().unwrap();
        assert_eq!(updated.confidence, 0.8);
        assert_eq!(updated.content, "Updated content");
    }

    #[test]
    fn test_get_nodes_by_status() {
        let conn = test_db();

        let active_node = KnowledgeNode {
            id: "n1".to_string(),
            node_type: NodeType::Rule,
            scope: NodeScope::Project,
            project_id: Some("my-app".to_string()),
            content: "Always run tests".to_string(),
            confidence: 0.85,
            status: NodeStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        };
        let pending_node = KnowledgeNode {
            id: "n2".to_string(),
            node_type: NodeType::Directive,
            scope: NodeScope::Global,
            project_id: None,
            content: "Use snake_case".to_string(),
            confidence: 0.9,
            status: NodeStatus::PendingReview,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        };
        let dismissed_node = KnowledgeNode {
            id: "n3".to_string(),
            node_type: NodeType::Pattern,
            scope: NodeScope::Project,
            project_id: Some("my-app".to_string()),
            content: "Old pattern".to_string(),
            confidence: 0.5,
            status: NodeStatus::Dismissed,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        };

        insert_node(&conn, &active_node).unwrap();
        insert_node(&conn, &pending_node).unwrap();
        insert_node(&conn, &dismissed_node).unwrap();

        let pending = get_nodes_by_status(&conn, &NodeStatus::PendingReview).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, "n2");

        let active = get_nodes_by_status(&conn, &NodeStatus::Active).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "n1");

        let pending2 = KnowledgeNode {
            id: "n4".to_string(),
            node_type: NodeType::Rule,
            scope: NodeScope::Project,
            project_id: Some("other".to_string()),
            content: "Second pending".to_string(),
            confidence: 0.95,
            status: NodeStatus::PendingReview,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        };
        insert_node(&conn, &pending2).unwrap();
        let pending_all = get_nodes_by_status(&conn, &NodeStatus::PendingReview).unwrap();
        assert_eq!(pending_all.len(), 2);
        assert_eq!(pending_all[0].id, "n4"); // Higher confidence first
    }

    #[test]
    fn test_migrate_v4_to_v5_adds_projection_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        migrate(&conn).unwrap();

        // Insert a node — should support new columns
        let node = KnowledgeNode {
            id: "test-1".to_string(),
            node_type: NodeType::Rule,
            scope: NodeScope::Global,
            project_id: None,
            content: "test rule".to_string(),
            confidence: 0.8,
            status: NodeStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        };
        insert_node(&conn, &node).unwrap();

        let retrieved = get_node(&conn, "test-1").unwrap().unwrap();
        assert!(retrieved.projected_at.is_none());
        assert!(retrieved.pr_url.is_none());

        // Verify schema version
        let version: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0)).unwrap();
        assert_eq!(version, 5);
    }

    fn test_node(id: &str, status: NodeStatus, projected_at: Option<String>, pr_url: Option<String>) -> KnowledgeNode {
        KnowledgeNode {
            id: id.to_string(),
            node_type: NodeType::Rule,
            scope: NodeScope::Global,
            project_id: None,
            content: format!("Content for {}", id),
            confidence: 0.8,
            status,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at,
            pr_url,
        }
    }

    #[test]
    fn test_get_unprojected_nodes() {
        let conn = test_db();

        // Active node with no projected_at — should be returned
        let active_unprojected = test_node("n1", NodeStatus::Active, None, None);
        // Active node with projected_at set — should NOT be returned
        let active_projected = test_node("n2", NodeStatus::Active, Some(Utc::now().to_rfc3339()), None);
        // PendingReview node with no projected_at — should NOT be returned (wrong status)
        let pending = test_node("n3", NodeStatus::PendingReview, None, None);

        insert_node(&conn, &active_unprojected).unwrap();
        insert_node(&conn, &active_projected).unwrap();
        insert_node(&conn, &pending).unwrap();

        let nodes = get_unprojected_nodes(&conn).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, "n1");
    }

    #[test]
    fn test_mark_node_projected() {
        let conn = test_db();

        let node = test_node("n1", NodeStatus::Active, None, None);
        insert_node(&conn, &node).unwrap();

        mark_node_projected(&conn, "n1").unwrap();

        let retrieved = get_node(&conn, "n1").unwrap().unwrap();
        assert!(retrieved.projected_at.is_some());
        assert!(retrieved.pr_url.is_none());
    }

    #[test]
    fn test_mark_node_projected_with_pr() {
        let conn = test_db();

        let node = test_node("n1", NodeStatus::Active, None, None);
        insert_node(&conn, &node).unwrap();

        mark_node_projected_with_pr(&conn, "n1", "https://github.com/test/pull/42").unwrap();

        let retrieved = get_node(&conn, "n1").unwrap().unwrap();
        assert!(retrieved.projected_at.is_some());
        assert_eq!(retrieved.pr_url, Some("https://github.com/test/pull/42".to_string()));
    }

    #[test]
    fn test_dismiss_nodes_for_pr() {
        let conn = test_db();

        let pr_url = "https://github.com/test/pull/99";
        let node1 = test_node("n1", NodeStatus::Active, Some(Utc::now().to_rfc3339()), Some(pr_url.to_string()));
        let node2 = test_node("n2", NodeStatus::Active, Some(Utc::now().to_rfc3339()), Some(pr_url.to_string()));

        insert_node(&conn, &node1).unwrap();
        insert_node(&conn, &node2).unwrap();

        dismiss_nodes_for_pr(&conn, pr_url).unwrap();

        let n1 = get_node(&conn, "n1").unwrap().unwrap();
        let n2 = get_node(&conn, "n2").unwrap().unwrap();

        assert_eq!(n1.status, NodeStatus::Dismissed);
        assert!(n1.pr_url.is_none());
        assert_eq!(n2.status, NodeStatus::Dismissed);
        assert!(n2.pr_url.is_none());
    }

    #[test]
    fn test_clear_node_pr() {
        let conn = test_db();

        let pr_url = "https://github.com/test/pull/7";
        let node = test_node("n1", NodeStatus::Active, Some(Utc::now().to_rfc3339()), Some(pr_url.to_string()));
        insert_node(&conn, &node).unwrap();

        clear_node_pr(&conn, pr_url).unwrap();

        let retrieved = get_node(&conn, "n1").unwrap().unwrap();
        assert!(retrieved.pr_url.is_none());
        assert_eq!(retrieved.status, NodeStatus::Active);
    }
}
