//! Pending-session queue: `<store>/queue/<session_id>.json` (gitignored).
//! Populated by `retro observe` (SessionEnd hook) and the `retro brief`
//! catch-up scan; drained by the v3 pipeline in `runner_v3`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEntry {
    pub session_id: String,
    /// Absolute path to the session JSONL transcript.
    pub transcript_path: String,
    /// Working directory of the session, if known at enqueue time
    /// (SessionEnd hook provides it; catch-up scan recovers it later from the transcript).
    #[serde(default)]
    pub cwd: Option<String>,
    /// RFC3339. Drain order.
    pub enqueued_at: String,
}

fn queue_dir(store_root: &Path) -> PathBuf {
    store_root.join("queue")
}

/// Session ids must be safe file names; anything else is rejected.
fn entry_path(store_root: &Path, session_id: &str) -> Result<PathBuf, CoreError> {
    let safe = session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if session_id.is_empty() || !safe {
        return Err(CoreError::Parse(format!(
            "invalid session id: {session_id:?}"
        )));
    }
    Ok(queue_dir(store_root).join(format!("{session_id}.json")))
}

/// Idempotent by session id: re-enqueueing overwrites the entry.
pub fn enqueue(store_root: &Path, entry: &QueueEntry) -> Result<(), CoreError> {
    let io = |e: std::io::Error| CoreError::Io(e.to_string());
    let path = entry_path(store_root, &entry.session_id)?;
    std::fs::create_dir_all(queue_dir(store_root)).map_err(io)?;
    let json = serde_json::to_string_pretty(entry).map_err(|e| CoreError::Parse(e.to_string()))?;
    std::fs::write(&path, json).map_err(io)
}

/// All entries, oldest first. Unparseable files are skipped (prune_stale removes them).
pub fn list(store_root: &Path) -> Result<Vec<QueueEntry>, CoreError> {
    let dir = queue_dir(store_root);
    let mut entries = Vec::new();
    if !dir.is_dir() {
        return Ok(entries);
    }
    let read = std::fs::read_dir(&dir).map_err(|e| CoreError::Io(e.to_string()))?;
    for item in read.flatten() {
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(entry) = serde_json::from_str::<QueueEntry>(&content) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by(|a, b| a.enqueued_at.cmp(&b.enqueued_at));
    Ok(entries)
}

pub fn remove(store_root: &Path, session_id: &str) -> Result<(), CoreError> {
    let path = entry_path(store_root, session_id)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| CoreError::Io(e.to_string()))?;
    }
    Ok(())
}

/// Remove entries whose transcript no longer exists, plus unparseable entry
/// files. Returns the removed session ids (callers record them in health —
/// visible, never silently retried forever).
pub fn prune_stale(store_root: &Path) -> Result<Vec<String>, CoreError> {
    let dir = queue_dir(store_root);
    let mut pruned = Vec::new();
    if !dir.is_dir() {
        return Ok(pruned);
    }
    let read = std::fs::read_dir(&dir).map_err(|e| CoreError::Io(e.to_string()))?;
    for item in read.flatten() {
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let stale = match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<QueueEntry>(&content) {
                Ok(entry) => !Path::new(&entry.transcript_path).exists(),
                Err(_) => true, // corrupt entry
            },
            Err(_) => true,
        };
        if stale {
            std::fs::remove_file(&path).map_err(|e| CoreError::Io(e.to_string()))?;
            pruned.push(stem);
        }
    }
    pruned.sort();
    Ok(pruned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(id: &str, transcript: &Path) -> QueueEntry {
        QueueEntry {
            session_id: id.to_string(),
            transcript_path: transcript.display().to_string(),
            cwd: Some("/tmp/some-project".to_string()),
            enqueued_at: "2026-07-06T10:00:00Z".to_string(),
        }
    }

    #[test]
    fn enqueue_list_remove_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let transcript = tmp.path().join("s1.jsonl");
        std::fs::write(&transcript, "{}").unwrap();

        enqueue(tmp.path(), &entry("s1", &transcript)).unwrap();
        enqueue(tmp.path(), &entry("s1", &transcript)).unwrap(); // idempotent
        let entries = list(tmp.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "s1");

        remove(tmp.path(), "s1").unwrap();
        assert!(list(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn list_is_sorted_by_enqueued_at() {
        let tmp = TempDir::new().unwrap();
        let t = tmp.path().join("t.jsonl");
        std::fs::write(&t, "{}").unwrap();
        let mut b = entry("b", &t);
        b.enqueued_at = "2026-07-06T09:00:00Z".to_string();
        let mut a = entry("a", &t);
        a.enqueued_at = "2026-07-06T11:00:00Z".to_string();
        enqueue(tmp.path(), &a).unwrap();
        enqueue(tmp.path(), &b).unwrap();
        let ids: Vec<String> = list(tmp.path())
            .unwrap()
            .into_iter()
            .map(|e| e.session_id)
            .collect();
        assert_eq!(ids, vec!["b".to_string(), "a".to_string()]);
    }

    #[test]
    fn prune_stale_removes_entries_with_missing_transcripts() {
        let tmp = TempDir::new().unwrap();
        let alive = tmp.path().join("alive.jsonl");
        std::fs::write(&alive, "{}").unwrap();
        enqueue(tmp.path(), &entry("alive", &alive)).unwrap();
        enqueue(tmp.path(), &entry("gone", &tmp.path().join("gone.jsonl"))).unwrap();

        let pruned = prune_stale(tmp.path()).unwrap();
        assert_eq!(pruned, vec!["gone".to_string()]);
        let remaining = list(tmp.path()).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].session_id, "alive");
    }

    #[test]
    fn corrupt_queue_entry_is_pruned() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("queue")).unwrap();
        std::fs::write(tmp.path().join("queue/junk.json"), "{not json").unwrap();
        let pruned = prune_stale(tmp.path()).unwrap();
        assert_eq!(pruned, vec!["junk".to_string()]);
        assert!(list(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn invalid_session_ids_are_rejected() {
        let tmp = TempDir::new().unwrap();
        let t = tmp.path().join("t.jsonl");
        std::fs::write(&t, "{}").unwrap();
        for bad in ["", "../escape", "has space", "a/b"] {
            let mut e = entry("x", &t);
            e.session_id = bad.to_string();
            assert!(enqueue(tmp.path(), &e).is_err(), "should reject: {bad:?}");
        }
    }
}
