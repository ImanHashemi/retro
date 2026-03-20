use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A session file that has been modified since last check.
#[derive(Debug, Clone)]
pub struct ModifiedSession {
    pub path: PathBuf,
    pub mtime: SystemTime,
}

/// Scan the Claude Code projects directory for session files modified since `since`.
/// If `since` is None, returns all session files.
pub fn find_modified_sessions(claude_dir: &Path, since: Option<SystemTime>) -> Vec<ModifiedSession> {
    let projects_dir = claude_dir.join("projects");
    if !projects_dir.exists() {
        return Vec::new();
    }

    let mut results = Vec::new();
    let pattern = format!("{}/**/*.jsonl", projects_dir.display());
    for entry in glob::glob(&pattern).unwrap_or_else(|_| glob::glob("").unwrap()) {
        if let Ok(path) = entry {
            if let Ok(metadata) = std::fs::metadata(&path) {
                if let Ok(mtime) = metadata.modified() {
                    let dominated = match since {
                        Some(since_time) => mtime > since_time,
                        None => true,
                    };
                    if dominated {
                        results.push(ModifiedSession { path, mtime });
                    }
                }
            }
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn test_find_modified_sessions_detects_new_files() {
        let dir = TempDir::new().unwrap();
        let sessions_dir = dir.path().join("projects").join("test-project");
        fs::create_dir_all(&sessions_dir).unwrap();

        let session_file = sessions_dir.join("session1.jsonl");
        fs::write(&session_file, "{}\n").unwrap();

        let modified = find_modified_sessions(dir.path(), None);
        assert_eq!(modified.len(), 1);
        assert!(modified[0].path.ends_with("session1.jsonl"));
    }

    #[test]
    fn test_find_modified_sessions_skips_unchanged() {
        let dir = TempDir::new().unwrap();
        let sessions_dir = dir.path().join("projects").join("test-project");
        fs::create_dir_all(&sessions_dir).unwrap();

        let session_file = sessions_dir.join("session1.jsonl");
        fs::write(&session_file, "{}\n").unwrap();

        let mtime = fs::metadata(&session_file).unwrap()
            .modified().unwrap();

        let modified = find_modified_sessions(dir.path(), Some(mtime));
        assert_eq!(modified.len(), 0);
    }
}
