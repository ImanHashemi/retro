use crate::errors::CoreError;
use crate::models::IngestedSession;
use rusqlite::{params, Connection};
use std::path::Path;

const SCHEMA_VERSION: u32 = 1;

/// Open (or create) the retro database with WAL mode enabled.
pub fn open_db(path: &Path) -> Result<Connection, CoreError> {
    let conn = Connection::open(path)?;

    // Enable WAL mode for concurrent access
    conn.pragma_update(None, "journal_mode", "WAL")?;

    // Run migrations
    migrate(&conn)?;

    Ok(conn)
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
                pr_url TEXT
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

        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
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

/// Verify the database is using WAL mode.
pub fn verify_wal_mode(conn: &Connection) -> Result<bool, CoreError> {
    let mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    Ok(mode.to_lowercase() == "wal")
}

/// Get all distinct projects from ingested sessions.
pub fn list_projects(conn: &Connection) -> Result<Vec<String>, CoreError> {
    let mut stmt = conn.prepare("SELECT DISTINCT project FROM ingested_sessions ORDER BY project")?;
    let projects = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(projects)
}
