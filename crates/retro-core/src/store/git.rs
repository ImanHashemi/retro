//! Git operations for the store repository (`~/.retro`).
//! All commands run against an explicit root via `git -C <root>`.
//! Commits are local-first; pushing is strictly best-effort.

use std::path::Path;
use std::process::Command;

use crate::errors::CoreError;

/// Outcome of a best-effort push. Failures are data, not errors —
/// callers record them in health, they never abort a pipeline.
#[must_use]
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

/// Apply the store repo's local git config. Safe to call repeatedly.
/// Must also be applied on the clone path (`retro init --from`), which
/// bypasses `ensure_repo`'s create branch.
pub fn apply_local_config(root: &Path) -> Result<(), CoreError> {
    // Local identity fallback: only set if unset in any config scope.
    let email_set = git(root, &["config", "user.email"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !email_set {
        run_checked(root, &["config", "user.email", "retro@localhost"])?;
        run_checked(root, &["config", "user.name", "retro"])?;
    }
    run_checked(root, &["config", "commit.gpgsign", "false"])?;
    run_checked(root, &["config", "core.hooksPath", "/dev/null"])?;
    ensure_machine_excludes(root)?;
    Ok(())
}

/// Upsert the machine-local ignore set into `.git/info/exclude`. Unlike the
/// store's `.gitignore` (written once, then user-owned), this file is never
/// user-edited, so it safely carries new ignore entries to stores created by
/// older binaries — without it, `commit_all`'s `git add -A` would sweep PID
/// files, backups, and v2 artifacts into the knowledge repo.
fn ensure_machine_excludes(root: &Path) -> Result<(), CoreError> {
    let io = |e: std::io::Error| CoreError::Io(e.to_string());
    let info_dir = root.join(".git").join("info");
    if !root.join(".git").exists() {
        return Ok(()); // not a repo yet; ensure_repo calls us again after init
    }
    std::fs::create_dir_all(&info_dir).map_err(io)?;
    let exclude = info_dir.join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    let mut updated = existing.clone();
    for entry in super::IGNORED_ENTRIES {
        if !existing.lines().any(|l| l.trim() == *entry) {
            if !updated.is_empty() && !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str(entry);
            updated.push('\n');
        }
    }
    if updated != existing {
        std::fs::write(&exclude, updated).map_err(io)?;
    }
    Ok(())
}

/// Initialize the store repo if needed. Returns true if newly created.
/// Sets a local identity fallback and disables gpg signing locally so
/// automated commits never depend on the user's global git setup.
pub fn ensure_repo(root: &Path) -> Result<bool, CoreError> {
    if is_repo(root) {
        return Ok(false);
    }
    run_checked(root, &["init"])?;
    apply_local_config(root)?;
    run_checked(root, &["add", "-A"])?;
    run_checked(
        root,
        &["commit", "--allow-empty", "-m", "retro: initialize store"],
    )?;
    Ok(true)
}

pub fn has_remote(root: &Path) -> bool {
    git(root, &["remote"])
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
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

/// True if HEAD has commits its upstream doesn't — or no upstream is set
/// yet (the first `push -u` establishes it), which also warrants a push
/// attempt. Lets the runner sweep up commits made between runs (dashboard
/// writes, manual edits).
pub fn has_unpushed(root: &Path) -> bool {
    match git(root, &["rev-list", "--count", "@{upstream}..HEAD"]) {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim() != "0",
        _ => true,
    }
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
    match git(root, &["push", "-u", "origin", "HEAD"]) {
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
    fn has_remote_false_on_fresh_repo() {
        let tmp = TempDir::new().unwrap();
        ensure_repo(tmp.path()).unwrap();
        assert!(!has_remote(tmp.path()));
    }

    #[test]
    fn has_unpushed_true_without_upstream() {
        let tmp = TempDir::new().unwrap();
        ensure_repo(tmp.path()).unwrap();
        // no upstream configured — a push attempt is warranted
        assert!(has_unpushed(tmp.path()));
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

    #[test]
    fn apply_local_config_is_idempotent_and_standalone() {
        let tmp = TempDir::new().unwrap();
        // idempotency: repeated application after ensure_repo is safe
        assert!(ensure_repo(tmp.path()).unwrap());
        apply_local_config(tmp.path()).unwrap();
        apply_local_config(tmp.path()).unwrap(); // idempotent
        let out = std::process::Command::new("git")
            .args([
                "-C",
                tmp.path().to_str().unwrap(),
                "config",
                "--local",
                "commit.gpgsign",
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "false");
        let out = std::process::Command::new("git")
            .args([
                "-C",
                tmp.path().to_str().unwrap(),
                "config",
                "--local",
                "core.hooksPath",
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/dev/null");
    }

    #[test]
    fn apply_local_config_works_on_raw_git_init_repo() {
        let tmp = TempDir::new().unwrap();
        // clone-path simulation: repo created WITHOUT ensure_repo
        std::process::Command::new("git")
            .args(["-C", tmp.path().to_str().unwrap(), "init"])
            .output()
            .unwrap();
        apply_local_config(tmp.path()).unwrap();
        let out = std::process::Command::new("git")
            .args([
                "-C",
                tmp.path().to_str().unwrap(),
                "config",
                "--local",
                "core.hooksPath",
            ])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/dev/null");
    }
}
