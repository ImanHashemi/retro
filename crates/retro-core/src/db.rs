use crate::errors::CoreError;
use crate::models::{IngestedSession, Pattern, PatternStatus, PatternType, Projection, SuggestedTarget};
use chrono::{DateTime, Utc};
pub use rusqlite::Connection;
use rusqlite::params;
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

/// Get ingested sessions that haven't been analyzed yet, within the time window.
pub fn get_sessions_for_analysis(
    conn: &Connection,
    project: Option<&str>,
    since: &DateTime<Utc>,
) -> Result<Vec<IngestedSession>, CoreError> {
    let since_str = since.to_rfc3339();

    let (query, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match project {
        Some(proj) => {
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
        None => {
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
        "INSERT INTO projections (id, pattern_id, target_type, target_path, content, applied_at, pr_url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            proj.id,
            proj.pattern_id,
            proj.target_type,
            proj.target_path,
            proj.content,
            proj.applied_at.to_rfc3339(),
            proj.pr_url,
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
        "SELECT p.id, p.pattern_id, p.target_type, p.target_path, p.content, p.applied_at, p.pr_url
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
            Ok(Projection {
                id: row.get(0)?,
                pattern_id: row.get(1)?,
                target_type: row.get(2)?,
                target_path: row.get(3)?,
                content: row.get(4)?,
                applied_at,
                pr_url: row.get(6)?,
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

        // It should appear in sessions for analysis
        let since = Utc::now() - chrono::Duration::days(14);
        let pending = get_sessions_for_analysis(&conn, None, &since).unwrap();
        assert_eq!(pending.len(), 1);

        // After marking as analyzed, it should not appear
        record_analyzed_session(&conn, "sess-1", "/test").unwrap();
        let pending = get_sessions_for_analysis(&conn, None, &since).unwrap();
        assert_eq!(pending.len(), 0);
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
}
