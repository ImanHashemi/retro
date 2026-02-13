pub mod context;
pub mod history;
pub mod session;

use crate::config::Config;
use crate::db;
use crate::errors::CoreError;
use crate::models::IngestedSession;
use chrono::Utc;
use rusqlite::Connection;

/// Result of an ingestion run.
#[derive(Debug)]
pub struct IngestResult {
    pub sessions_found: usize,
    pub sessions_ingested: usize,
    pub sessions_skipped: usize,
    pub errors: Vec<String>,
}

/// Run ingestion for a specific project path.
pub fn ingest_project(
    conn: &Connection,
    config: &Config,
    project_path: &str,
) -> Result<IngestResult, CoreError> {
    // Check if project is excluded
    if config.privacy.exclude_projects.iter().any(|excl| project_path.contains(excl.as_str())) {
        return Ok(IngestResult {
            sessions_found: 0,
            sessions_ingested: 0,
            sessions_skipped: 0,
            errors: Vec::new(),
        });
    }

    let claude_dir = config.claude_dir();
    let encoded_path = encode_project_path(project_path);
    let sessions_dir = claude_dir.join("projects").join(&encoded_path);

    let mut result = IngestResult {
        sessions_found: 0,
        sessions_ingested: 0,
        sessions_skipped: 0,
        errors: Vec::new(),
    };

    if !sessions_dir.exists() {
        return Ok(result);
    }

    // Find all session JSONL files (direct children, not subagent files)
    let pattern = sessions_dir.join("*.jsonl");
    let pattern_str = pattern.to_string_lossy();

    let paths: Vec<_> = glob::glob(&pattern_str)
        .map_err(|e| CoreError::Parse(format!("glob pattern error: {e}")))?
        .filter_map(|r| r.ok())
        .collect();

    result.sessions_found = paths.len();

    for path in paths {
        let session_id = match path.file_stem().and_then(|s| s.to_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        // Get file metadata for change detection
        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                result
                    .errors
                    .push(format!("metadata error for {}: {e}", path.display()));
                continue;
            }
        };

        let file_size = metadata.len();
        let file_mtime = match metadata.modified() {
            Ok(t) => {
                let dt: chrono::DateTime<Utc> = t.into();
                dt.to_rfc3339()
            }
            Err(_) => Utc::now().to_rfc3339(),
        };

        // Check if already ingested and unchanged
        if db::is_session_ingested(conn, &session_id, file_size, &file_mtime)? {
            result.sessions_skipped += 1;
            continue;
        }

        // Parse the session
        match session::parse_session_file(&path, &session_id, project_path) {
            Ok(_session) => {
                // Also parse subagent files if the session directory exists
                let subagent_dir = sessions_dir.join(&session_id).join("subagents");
                let _subagent_sessions = if subagent_dir.exists() {
                    match session::parse_subagent_dir(&subagent_dir, &session_id, project_path) {
                        Ok(subs) => subs,
                        Err(e) => {
                            result.errors.push(format!(
                                "subagent parse error for {}: {e}",
                                session_id
                            ));
                            Vec::new()
                        }
                    }
                } else {
                    Vec::new()
                };

                // Record the ingested session
                let ingested = IngestedSession {
                    session_id: session_id.clone(),
                    project: project_path.to_string(),
                    session_path: path.to_string_lossy().to_string(),
                    file_size,
                    file_mtime,
                    ingested_at: Utc::now(),
                };
                db::record_ingested_session(conn, &ingested)?;
                result.sessions_ingested += 1;
            }
            Err(e) => {
                result
                    .errors
                    .push(format!("parse error for {}: {e}", session_id));
            }
        }
    }

    Ok(result)
}

/// Ingest all projects found in the claude projects directory.
pub fn ingest_all_projects(
    conn: &Connection,
    config: &Config,
) -> Result<IngestResult, CoreError> {
    let claude_dir = config.claude_dir();
    let projects_dir = claude_dir.join("projects");

    let mut total = IngestResult {
        sessions_found: 0,
        sessions_ingested: 0,
        sessions_skipped: 0,
        errors: Vec::new(),
    };

    if !projects_dir.exists() {
        return Ok(total);
    }

    let entries = std::fs::read_dir(&projects_dir)
        .map_err(|e| CoreError::Io(format!("reading projects dir: {e}")))?;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry.path().is_dir() {
            continue;
        }

        let dir_name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Check if project is excluded
        if config.privacy.exclude_projects.iter().any(|excl| dir_name.contains(&encode_project_path(excl))) {
            continue;
        }

        let sessions_dir = entry.path();
        let project_path = recover_project_path(&sessions_dir, &dir_name);

        let result = ingest_project(conn, config, &project_path)?;
        total.sessions_found += result.sessions_found;
        total.sessions_ingested += result.sessions_ingested;
        total.sessions_skipped += result.sessions_skipped;
        total.errors.extend(result.errors);
    }

    Ok(total)
}

/// Encode a project path for use as a directory name.
/// /home/user/project → -home-user-project
pub fn encode_project_path(path: &str) -> String {
    path.replace('/', "-")
}

/// Recover the project path from an encoded directory name by reading `cwd`
/// from the first session file inside it. Falls back to the encoded name
/// if no sessions exist (which means the directory has no data anyway).
pub fn recover_project_path(sessions_dir: &std::path::Path, encoded: &str) -> String {
    // Try to read the cwd field from the first session file
    let pattern = sessions_dir.join("*.jsonl");
    if let Ok(paths) = glob::glob(&pattern.to_string_lossy()) {
        for path in paths.filter_map(|r| r.ok()) {
            if let Ok(file) = std::fs::File::open(&path) {
                let reader = std::io::BufReader::new(file);
                use std::io::BufRead;
                for line in reader.lines().take(5) {
                    if let Ok(line) = line {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                            if let Some(cwd) = val.get("cwd").and_then(|c| c.as_str()) {
                                return cwd.to_string();
                            }
                        }
                    }
                }
            }
        }
    }
    // Fallback: naive decode (works for paths without hyphens)
    naive_decode_project_path(encoded)
}

/// Naive decode — only correct for paths without hyphens in components.
fn naive_decode_project_path(encoded: &str) -> String {
    if encoded.starts_with('-') {
        encoded.replacen('-', "/", 1).replace('-', "/")
    } else {
        encoded.replace('-', "/")
    }
}

/// Find the encoded project directory for a given project path.
pub fn find_project_dir(config: &Config, project_path: &str) -> Option<std::path::PathBuf> {
    let claude_dir = config.claude_dir();
    let encoded = encode_project_path(project_path);
    let dir = claude_dir.join("projects").join(&encoded);
    if dir.exists() {
        Some(dir)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_project_path() {
        assert_eq!(
            encode_project_path("/home/claude/repositories/retro"),
            "-home-claude-repositories-retro"
        );
    }

    #[test]
    fn test_naive_decode_project_path() {
        assert_eq!(
            naive_decode_project_path("-home-claude-repositories-retro"),
            "/home/claude/repositories/retro"
        );
    }
}
