//! Store-format marker (`~/.retro/meta.toml`).
//!
//! Lets a future binary refuse to operate on a store written by a newer one
//! instead of silently degrading. Note the limitation: released 3.1.x
//! binaries do not look for this file, so for the 3.1.x -> 3.2.0 window the
//! emit-only-when-set rule in `Node::to_markdown` is the real protection.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::errors::CoreError;

/// Store format this binary reads and writes.
pub const STORE_FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoreMeta {
    pub store_format: u32,
}

fn meta_path(root: &Path) -> std::path::PathBuf {
    root.join("meta.toml")
}

/// Read the marker. `Ok(None)` means a pre-marker store, which is valid.
pub fn read(root: &Path) -> Result<Option<StoreMeta>, CoreError> {
    let path = meta_path(root);
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| CoreError::Io(format!("reading store meta: {e}")))?;
    let meta: StoreMeta =
        toml::from_str(&contents).map_err(|e| CoreError::Config(e.to_string()))?;
    Ok(Some(meta))
}

/// Write this binary's format marker. Idempotent.
///
/// `ensure_layout` calls this on every `retro observe` (i.e. every session
/// end), so a crash mid-write must not leave a truncated marker — an
/// unparseable marker is a hard `CoreError::Config` in `read`/
/// `check_compatible`, which would be worse than the absent-marker case
/// `read` treats as fine. Tmp-sibling + rename, matching the convention
/// already used for settings.json and the CLAUDE.md family.
pub fn write(root: &Path) -> Result<(), CoreError> {
    let meta = StoreMeta {
        store_format: STORE_FORMAT,
    };
    let contents = toml::to_string(&meta).map_err(|e| CoreError::Config(e.to_string()))?;
    let path = meta_path(root);
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, contents)
        .map_err(|e| CoreError::Io(format!("writing store meta: {e}")))?;
    std::fs::rename(&tmp, &path).map_err(|e| CoreError::Io(format!("writing store meta: {e}")))
}

/// Refuse a store written by a newer binary. A missing marker is fine.
pub fn check_compatible(root: &Path) -> Result<(), CoreError> {
    match read(root)? {
        Some(meta) if meta.store_format > STORE_FORMAT => Err(CoreError::Config(format!(
            "store format {} is newer than this binary supports ({STORE_FORMAT}) — upgrade retro",
            meta.store_format
        ))),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn writes_then_reads_current_format() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path()).unwrap();
        assert_eq!(
            read(tmp.path()).unwrap().unwrap().store_format,
            STORE_FORMAT
        );
    }

    #[test]
    fn absent_marker_is_ok_and_none() {
        let tmp = TempDir::new().unwrap();
        assert!(read(tmp.path()).unwrap().is_none());
        // A pre-marker store must be usable, not an error.
        assert!(check_compatible(tmp.path()).is_ok());
    }

    #[test]
    fn newer_format_is_refused() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("meta.toml"),
            format!("store_format = {}\n", STORE_FORMAT + 1),
        )
        .unwrap();
        let err = check_compatible(tmp.path()).unwrap_err();
        assert!(
            err.to_string().contains("newer"),
            "error should name the cause: {err}"
        );
    }

    #[test]
    fn write_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path()).unwrap();
        let first = std::fs::read(tmp.path().join("meta.toml")).unwrap();
        write(tmp.path()).unwrap();
        let second = std::fs::read(tmp.path().join("meta.toml")).unwrap();
        assert_eq!(
            first, second,
            "rewriting the marker must not churn the file"
        );
    }

    #[test]
    fn meta_toml_is_not_gitignored() {
        // meta.toml is shared state (committed with the store), not
        // machine-local — a future edit to IGNORED_ENTRIES that accidentally
        // excludes it must fail loudly here rather than silently breaking
        // cross-version sync.
        let content = crate::store::gitignore_content();
        assert!(
            !content.lines().any(|l| l == "meta.toml"),
            "meta.toml must not be gitignored: {content}"
        );
    }

    #[test]
    fn malformed_toml_errors_not_panics() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("meta.toml"), "not valid toml at all {{{").unwrap();
        let err = check_compatible(tmp.path()).unwrap_err();
        // Just confirm it's a clear error, not a panic.
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn missing_store_format_key_errors() {
        let tmp = TempDir::new().unwrap();
        // Syntactically valid TOML, but no store_format key. Must not
        // default to 0 (which would compare as older-than-supported and
        // silently pass check_compatible).
        std::fs::write(tmp.path().join("meta.toml"), "some_other_key = 1\n").unwrap();
        let err = check_compatible(tmp.path()).unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}
