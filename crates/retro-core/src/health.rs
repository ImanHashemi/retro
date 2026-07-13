//! Pipeline health: per-stage status records at `<store>/health.json`.
//! Written by every v3 stage; read by `retro doctor` and the dashboard (Plan 3),
//! and surfaced as warnings in the session briefing.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::errors::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageHealth {
    /// RFC3339 timestamp of the last run of this stage.
    pub at: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Health {
    #[serde(default)]
    pub stages: BTreeMap<String, StageHealth>,
}

impl Health {
    /// Missing or corrupt file loads empty (health is derived, never precious).
    pub fn load(store_root: &Path) -> Result<Self, CoreError> {
        let path = store_root.join("health.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(serde_json::from_str(&content).unwrap_or_default()),
            Err(_) => Ok(Health::default()),
        }
    }

    pub fn save(&self, store_root: &Path) -> Result<(), CoreError> {
        let json =
            serde_json::to_string_pretty(self).map_err(|e| CoreError::Parse(e.to_string()))?;
        std::fs::write(store_root.join("health.json"), json)
            .map_err(|e| CoreError::Io(e.to_string()))
    }

    /// Human-readable warnings for every stage whose last run failed.
    /// Returned in stage-name order (BTreeMap iteration).
    pub fn warnings(&self) -> Vec<String> {
        self.stages
            .iter()
            .filter(|(_, s)| !s.ok)
            .map(|(name, s)| format!("retro {name} failed at {}: {}", s.at, s.detail))
            .collect()
    }
}

/// Record one stage result (load-modify-save; last writer wins, which is fine
/// for a single-user pipeline serialized by the run lockfile).
pub fn record(store_root: &Path, stage: &str, ok: bool, detail: &str) -> Result<(), CoreError> {
    let mut health = Health::load(store_root)?;
    health.stages.insert(
        stage.to_string(),
        StageHealth {
            at: chrono::Utc::now().to_rfc3339(),
            ok,
            detail: detail.to_string(),
        },
    );
    health.save(store_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn record_and_warnings_roundtrip() {
        let tmp = TempDir::new().unwrap();
        record(tmp.path(), "observe", true, "enqueued 1 session").unwrap();
        record(tmp.path(), "analyze", false, "claude CLI exited 1").unwrap();

        let h = Health::load(tmp.path()).unwrap();
        assert_eq!(h.stages.len(), 2);
        assert!(h.stages["observe"].ok);
        assert!(!h.stages["analyze"].ok);

        let warnings = h.warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("analyze"), "got: {warnings:?}");
        assert!(warnings[0].contains("claude CLI exited 1"));
    }

    #[test]
    fn missing_file_loads_empty() {
        let tmp = TempDir::new().unwrap();
        let h = Health::load(tmp.path()).unwrap();
        assert!(h.stages.is_empty());
        assert!(h.warnings().is_empty());
    }
}
