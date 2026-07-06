//! Git operations for the store repository (`~/.retro`).
//! All commands run against an explicit root via `git -C <root>`.
//! Commits are local-first; pushing is strictly best-effort.

use std::path::Path;
use std::process::Command;

use crate::errors::CoreError;

/// Outcome of a best-effort push. Failures are data, not errors —
/// callers record them in health, they never abort a pipeline.
#[derive(Debug)]
pub enum PushOutcome {
    Pushed,
    NoRemote,
    Failed(String),
}

fn git(root: &Path, args: &[&str]) -> Result<std::process::Output, CoreError> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| CoreError::Io(format!("failed to run git: {e}")))
}

pub fn is_repo(root: &Path) -> bool {
    root.join(".git").exists()
}

pub fn head_exists(root: &Path) -> bool {
    git(root, &["rev-parse", "--verify", "HEAD"])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Initialize the store repo if needed. Returns true if newly created.
/// Sets a local identity fallback and disables gpg signing locally so
/// automated commits never depend on the user's global git setup.
pub fn ensure_repo(root: &Path) -> Result<bool, CoreError> {
    if is_repo(root) {
        return Ok(false);
    }
    run_checked(root, &["init"])?;
    // Local identity fallback: only set if unset globally.
    let email_set = git(root, &["config", "user.email"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !email_set {
        run_checked(root, &["config", "user.email", "retro@localhost"])?;
        run_checked(root, &["config", "user.name", "retro"])?;
    }
    run_checked(root, &["config", "commit.gpgsign", "false"])?;
    run_checked(root, &["add", "-A"])?;
    run_checked(
        root,
        &["commit", "--allow-empty", "-m", "retro: initialize store"],
    )?;
    Ok(true)
}

pub fn has_changes(root: &Path) -> Result<bool, CoreError> {
    let out = git(root, &["status", "--porcelain"])?;
    if !out.status.success() {
        return Err(CoreError::Io(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(!out.stdout.is_empty())
}

/// Stage everything and commit. Returns true if a commit was made,
/// false if the tree was clean. Never errors on "nothing to commit".
pub fn commit_all(root: &Path, message: &str) -> Result<bool, CoreError> {
    if !has_changes(root)? {
        return Ok(false);
    }
    run_checked(root, &["add", "-A"])?;
    run_checked(root, &["commit", "-m", message])?;
    Ok(true)
}

/// Push to origin if a remote exists. Never fails the caller.
pub fn push_best_effort(root: &Path) -> PushOutcome {
    let remotes = match git(root, &["remote"]) {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => return PushOutcome::Failed(String::from_utf8_lossy(&o.stderr).to_string()),
        Err(e) => return PushOutcome::Failed(e.to_string()),
    };
    if remotes.is_empty() {
        return PushOutcome::NoRemote;
    }
    match git(root, &["push", "origin", "HEAD"]) {
        Ok(o) if o.status.success() => PushOutcome::Pushed,
        Ok(o) => PushOutcome::Failed(String::from_utf8_lossy(&o.stderr).to_string()),
        Err(e) => PushOutcome::Failed(e.to_string()),
    }
}

fn run_checked(root: &Path, args: &[&str]) -> Result<(), CoreError> {
    let out = git(root, args)?;
    if !out.status.success() {
        return Err(CoreError::Io(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn ensure_repo_initializes_once() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_repo(tmp.path()));
        let created = ensure_repo(tmp.path()).unwrap();
        assert!(created);
        assert!(is_repo(tmp.path()));
        // idempotent
        let created_again = ensure_repo(tmp.path()).unwrap();
        assert!(!created_again);
        // has an initial commit (HEAD resolves)
        assert!(head_exists(tmp.path()));
    }

    #[test]
    fn commit_all_commits_changes_and_reports_clean() {
        let tmp = TempDir::new().unwrap();
        ensure_repo(tmp.path()).unwrap();
        // clean repo → no commit
        assert!(!commit_all(tmp.path(), "retro: noop").unwrap());
        // new file → commit
        std::fs::write(tmp.path().join("note.md"), "hello").unwrap();
        assert!(has_changes(tmp.path()).unwrap());
        assert!(commit_all(tmp.path(), "retro: learn note").unwrap());
        assert!(!has_changes(tmp.path()).unwrap());
        // modification → commit
        std::fs::write(tmp.path().join("note.md"), "hello again").unwrap();
        assert!(commit_all(tmp.path(), "user: edit note").unwrap());
    }

    #[test]
    fn push_without_remote_reports_no_remote() {
        let tmp = TempDir::new().unwrap();
        ensure_repo(tmp.path()).unwrap();
        assert!(matches!(
            push_best_effort(tmp.path()),
            PushOutcome::NoRemote
        ));
    }
}
