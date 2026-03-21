use crate::config::Config;
use crate::db;
use crate::errors::CoreError;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::path::Path;

const MAX_LOG_SIZE: u64 = 1_000_000; // 1 MB

/// Rotate runner.log if it exceeds 1 MB.
pub fn rotate_log_if_needed(retro_dir: &Path) -> Result<(), CoreError> {
    let log_path = retro_dir.join("runner.log");
    if !log_path.exists() {
        return Ok(());
    }
    let metadata = std::fs::metadata(&log_path)
        .map_err(|e| CoreError::Io(format!("reading runner.log metadata: {e}")))?;
    if metadata.len() < MAX_LOG_SIZE {
        return Ok(());
    }
    let backup_path = retro_dir.join("runner.log.1");
    std::fs::rename(&log_path, &backup_path)
        .map_err(|e| CoreError::Io(format!("rotating runner.log: {e}")))?;
    std::fs::write(&log_path, "")
        .map_err(|e| CoreError::Io(format!("creating fresh runner.log: {e}")))?;
    Ok(())
}

/// Get the last run timestamp from the metadata table.
pub fn last_run_time(conn: &Connection) -> Option<DateTime<Utc>> {
    db::get_metadata(conn, "last_run_at")
        .ok()
        .flatten()
        .and_then(|ts| {
            DateTime::parse_from_rfc3339(&ts)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        })
}

/// Get AI calls used today and the configured max. Resets count on new day.
pub fn ai_calls_today(conn: &Connection, config: &Config) -> (u32, u32) {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let date = db::get_metadata(conn, "ai_calls_date")
        .ok()
        .flatten()
        .unwrap_or_default();
    let count = db::get_metadata(conn, "ai_calls_today")
        .ok()
        .flatten()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let used = if date == today { count } else { 0 };
    (used, config.runner.max_ai_calls_per_day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_rotate_log_skips_small_file() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("runner.log");
        fs::write(&log_path, "small content").unwrap();
        rotate_log_if_needed(dir.path()).unwrap();
        assert!(log_path.exists());
        assert!(!dir.path().join("runner.log.1").exists());
        assert_eq!(fs::read_to_string(&log_path).unwrap(), "small content");
    }

    #[test]
    fn test_rotate_log_rotates_large_file() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("runner.log");
        let content = "x".repeat(1_100_000);
        fs::write(&log_path, &content).unwrap();
        rotate_log_if_needed(dir.path()).unwrap();
        assert!(dir.path().join("runner.log.1").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("runner.log.1")).unwrap(),
            content
        );
        assert!(log_path.exists());
        assert_eq!(fs::read_to_string(&log_path).unwrap(), "");
    }

    #[test]
    fn test_rotate_log_overwrites_old_backup() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("runner.log");
        let backup_path = dir.path().join("runner.log.1");
        fs::write(&backup_path, "old backup").unwrap();
        let content = "y".repeat(1_100_000);
        fs::write(&log_path, &content).unwrap();
        rotate_log_if_needed(dir.path()).unwrap();
        assert_eq!(fs::read_to_string(&backup_path).unwrap(), content);
    }

    #[test]
    fn test_rotate_log_no_file_ok() {
        let dir = TempDir::new().unwrap();
        rotate_log_if_needed(dir.path()).unwrap();
    }

    #[test]
    fn test_last_run_time_none_when_not_set() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        db::init_db(&conn).unwrap();
        assert!(last_run_time(&conn).is_none());
    }

    #[test]
    fn test_last_run_time_returns_timestamp() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        db::init_db(&conn).unwrap();
        let now = Utc::now();
        db::set_metadata(&conn, "last_run_at", &now.to_rfc3339()).unwrap();
        let result = last_run_time(&conn);
        assert!(result.is_some());
        assert!((result.unwrap() - now).num_seconds().abs() < 1);
    }

    #[test]
    fn test_ai_calls_today_new_day_resets() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        db::init_db(&conn).unwrap();
        let config = Config::default();
        db::set_metadata(&conn, "ai_calls_date", "2020-01-01").unwrap();
        db::set_metadata(&conn, "ai_calls_today", "5").unwrap();
        let (used, max) = ai_calls_today(&conn, &config);
        assert_eq!(used, 0);
        assert_eq!(max, config.runner.max_ai_calls_per_day);
    }

    #[test]
    fn test_ai_calls_today_same_day() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        db::init_db(&conn).unwrap();
        let config = Config::default();
        let today = Utc::now().format("%Y-%m-%d").to_string();
        db::set_metadata(&conn, "ai_calls_date", &today).unwrap();
        db::set_metadata(&conn, "ai_calls_today", "3").unwrap();
        let (used, max) = ai_calls_today(&conn, &config);
        assert_eq!(used, 3);
        assert_eq!(max, config.runner.max_ai_calls_per_day);
    }
}
