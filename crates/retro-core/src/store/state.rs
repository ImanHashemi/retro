//! Machine-local runner state: observe watermark, AI budget, notifications.
//! Lives at `<store>/state/state.json` — gitignored, disposable-ish (losing it
//! causes a catch-up rescan and budget reset, never data loss).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunnerState {
    /// Unix seconds of the newest session mtime already enqueued (observe watermark).
    #[serde(default)]
    pub last_observed_unix: u64,
    /// Day the AI-call counter refers to (YYYY-MM-DD).
    #[serde(default)]
    pub ai_calls_date: String,
    #[serde(default)]
    pub ai_calls_today: u32,
    /// Messages for the next session briefing (new registrations, learned nodes).
    #[serde(default)]
    pub notifications: Vec<String>,
    /// session_id -> transcript mtime (unix secs) at the time it was last
    /// processed by the pipeline. Used to skip re-enqueueing unchanged
    /// sessions that fall inside the catch-up safety margin.
    /// Written by the pipeline (`runner_v3`) via `record_processed()` after
    /// each drained session; read by `retro brief`'s catch-up dedup.
    #[serde(default)]
    pub processed: std::collections::BTreeMap<String, u64>,
}

fn state_path(store_root: &Path) -> PathBuf {
    store_root.join("state").join("state.json")
}

impl RunnerState {
    /// Load state; a missing or corrupt file yields defaults (never an error —
    /// state is machine-local and safe to reset).
    pub fn load(store_root: &Path) -> Result<Self, CoreError> {
        let path = state_path(store_root);
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(serde_json::from_str(&content).unwrap_or_default()),
            Err(_) => Ok(RunnerState::default()),
        }
    }

    /// Load-modify-save; callers relying on atomicity must hold the run lockfile.
    pub fn save(&mut self, store_root: &Path) -> Result<(), CoreError> {
        // Briefings drain these, but a machine with no new sessions can pile
        // them up forever — keep only the newest 50.
        const MAX_NOTIFICATIONS: usize = 50;
        if self.notifications.len() > MAX_NOTIFICATIONS {
            let excess = self.notifications.len() - MAX_NOTIFICATIONS;
            self.notifications.drain(..excess);
        }
        let io = |e: std::io::Error| CoreError::Io(e.to_string());
        let path = state_path(store_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let json =
            serde_json::to_string_pretty(self).map_err(|e| CoreError::Parse(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)
    }

    /// Remaining AI calls for `today` (YYYY-MM-DD) under `max_per_day`.
    /// A stored date != today means the counter is stale: full budget.
    pub fn budget_remaining(&self, today: &str, max_per_day: u32) -> u32 {
        if self.ai_calls_date == today {
            max_per_day.saturating_sub(self.ai_calls_today)
        } else {
            max_per_day
        }
    }

    /// Record `calls` AI calls made on `today`, resetting on day change.
    pub fn record_ai_calls(&mut self, today: &str, calls: u32) {
        if self.ai_calls_date != today {
            self.ai_calls_date = today.to_string();
            self.ai_calls_today = 0;
        }
        self.ai_calls_today += calls;
    }

    pub fn drain_notifications(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notifications)
    }

    /// Record a session as processed at the given transcript mtime, pruning
    /// the map to the newest 1000 entries to bound growth.
    pub fn record_processed(&mut self, session_id: &str, mtime_unix: u64) {
        self.processed.insert(session_id.to_string(), mtime_unix);
        if self.processed.len() > 1000 {
            let mut by_mtime: Vec<(String, u64)> = self
                .processed
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            by_mtime.sort_by_key(|(_, m)| std::cmp::Reverse(*m));
            self.processed = by_mtime.into_iter().take(1000).collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn state_roundtrips_and_defaults() {
        let tmp = TempDir::new().unwrap();
        let s = RunnerState::load(tmp.path()).unwrap();
        assert_eq!(s.last_observed_unix, 0);
        assert_eq!(s.ai_calls_today, 0);
        assert!(s.notifications.is_empty());

        let mut s = s;
        s.last_observed_unix = 1234;
        s.notifications
            .push("retro is now watching my-proj".to_string());
        s.save(tmp.path()).unwrap();

        let loaded = RunnerState::load(tmp.path()).unwrap();
        assert_eq!(loaded.last_observed_unix, 1234);
        assert_eq!(loaded.notifications.len(), 1);
    }

    #[test]
    fn budget_resets_on_new_day_and_counts_within_day() {
        let tmp = TempDir::new().unwrap();
        let mut s = RunnerState::load(tmp.path()).unwrap();
        assert!(s.budget_remaining("2026-07-06", 3) == 3);
        s.record_ai_calls("2026-07-06", 2);
        assert_eq!(s.budget_remaining("2026-07-06", 3), 1);
        // new day resets
        assert_eq!(s.budget_remaining("2026-07-07", 3), 3);
        s.record_ai_calls("2026-07-07", 1);
        assert_eq!(s.ai_calls_today, 1);
        assert_eq!(s.ai_calls_date, "2026-07-07");
    }

    #[test]
    fn drain_notifications_empties_the_list() {
        let tmp = TempDir::new().unwrap();
        let mut s = RunnerState::load(tmp.path()).unwrap();
        s.notifications.push("a".to_string());
        s.notifications.push("b".to_string());
        let drained = s.drain_notifications();
        assert_eq!(drained, vec!["a".to_string(), "b".to_string()]);
        assert!(s.notifications.is_empty());
    }

    #[test]
    fn corrupt_state_file_resets_to_default() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("state")).unwrap();
        std::fs::write(tmp.path().join("state/state.json"), "{corrupt").unwrap();
        let s = RunnerState::load(tmp.path()).unwrap();
        assert_eq!(s.last_observed_unix, 0);
    }

    #[test]
    fn record_processed_tracks_and_prunes() {
        let tmp = TempDir::new().unwrap();
        let mut s = RunnerState::load(tmp.path()).unwrap();
        s.record_processed("a", 100);
        assert_eq!(s.processed["a"], 100);
        for i in 0..1100 {
            s.record_processed(&format!("s{i}"), 1000 + i as u64);
        }
        assert!(s.processed.len() <= 1000);
        assert!(s.processed.contains_key("s1099"), "newest kept");
        assert!(!s.processed.contains_key("a"), "oldest pruned");
    }

    #[test]
    fn notifications_capped_at_50_keeping_newest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut st = RunnerState::default();
        for i in 0..60 {
            st.notifications.push(format!("note {i}"));
        }
        st.save(tmp.path()).unwrap();
        let loaded = RunnerState::load(tmp.path()).unwrap();
        assert_eq!(loaded.notifications.len(), 50);
        assert_eq!(loaded.notifications.first().unwrap(), "note 10"); // oldest 10 dropped
        assert_eq!(loaded.notifications.last().unwrap(), "note 59");
    }
}
