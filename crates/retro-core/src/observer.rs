use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A session file that has been modified since last check.
#[derive(Debug, Clone)]
pub struct ModifiedSession {
    pub path: PathBuf,
    pub mtime: SystemTime,
}

/// Encode a project path to a Claude Code session directory name.
/// Claude replaces `/` with `-`, e.g. `/Users/iman/repos/app` → `-Users-iman-repos-app`.
fn encode_project_path(project_path: &str) -> String {
    project_path.replace('/', "-")
}

/// Scan Claude Code session directories for modified session files.
///
/// If `project_paths` is non-empty, only scans session directories matching
/// those project paths (prevents accessing macOS-protected directories like
/// ~/Downloads or ~/Documents). If empty, scans all session directories.
///
/// If `since` is None, returns all session files.
pub fn find_modified_sessions(
    claude_dir: &Path,
    since: Option<SystemTime>,
    project_paths: &[String],
) -> Vec<ModifiedSession> {
    let projects_dir = claude_dir.join("projects");
    if !projects_dir.exists() {
        return Vec::new();
    }

    let patterns: Vec<String> = if project_paths.is_empty() {
        // Fallback: scan everything (backward compat for single-project mode)
        vec![format!("{}/**/*.jsonl", projects_dir.display())]
    } else {
        // Only scan directories matching registered project paths
        project_paths
            .iter()
            .map(|path| {
                let encoded = encode_project_path(path);
                format!("{}/{}*/**/*.jsonl", projects_dir.display(), encoded)
            })
            .collect()
    };

    let mut results = Vec::new();
    for pattern in &patterns {
        for entry in glob::glob(pattern).unwrap_or_else(|_| glob::glob("").unwrap()) {
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
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_encode_project_path() {
        assert_eq!(
            encode_project_path("/Users/iman/repos/app"),
            "-Users-iman-repos-app"
        );
        assert_eq!(encode_project_path("/tmp/test"), "-tmp-test");
    }

    #[test]
    fn test_find_modified_sessions_detects_new_files() {
        let dir = TempDir::new().unwrap();
        let sessions_dir = dir.path().join("projects").join("-tmp-my-project");
        fs::create_dir_all(&sessions_dir).unwrap();

        let session_file = sessions_dir.join("session1.jsonl");
        fs::write(&session_file, "{}\n").unwrap();

        // With matching project path
        let modified = find_modified_sessions(
            dir.path(),
            None,
            &["/tmp/my-project".to_string()],
        );
        assert_eq!(modified.len(), 1);
        assert!(modified[0].path.ends_with("session1.jsonl"));
    }

    #[test]
    fn test_find_modified_sessions_skips_unregistered() {
        let dir = TempDir::new().unwrap();

        // Create sessions for two projects
        let registered_dir = dir.path().join("projects").join("-tmp-registered");
        fs::create_dir_all(&registered_dir).unwrap();
        fs::write(registered_dir.join("session1.jsonl"), "{}\n").unwrap();

        let unregistered_dir = dir.path().join("projects").join("-Users-iman-Downloads-stuff");
        fs::create_dir_all(&unregistered_dir).unwrap();
        fs::write(unregistered_dir.join("session2.jsonl"), "{}\n").unwrap();

        // Only scan registered project
        let modified = find_modified_sessions(
            dir.path(),
            None,
            &["/tmp/registered".to_string()],
        );
        assert_eq!(modified.len(), 1);
        assert!(modified[0].path.to_str().unwrap().contains("registered"));
    }

    #[test]
    fn test_find_modified_sessions_empty_paths_scans_all() {
        let dir = TempDir::new().unwrap();
        let sessions_dir = dir.path().join("projects").join("any-project");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::write(sessions_dir.join("session1.jsonl"), "{}\n").unwrap();

        let modified = find_modified_sessions(dir.path(), None, &[]);
        assert_eq!(modified.len(), 1);
    }

    #[test]
    fn test_find_modified_sessions_skips_unchanged() {
        let dir = TempDir::new().unwrap();
        let sessions_dir = dir.path().join("projects").join("-tmp-project");
        fs::create_dir_all(&sessions_dir).unwrap();

        let session_file = sessions_dir.join("session1.jsonl");
        fs::write(&session_file, "{}\n").unwrap();

        let mtime = fs::metadata(&session_file).unwrap().modified().unwrap();

        let modified = find_modified_sessions(
            dir.path(),
            Some(mtime),
            &["/tmp/project".to_string()],
        );
        assert_eq!(modified.len(), 0);
    }
}
