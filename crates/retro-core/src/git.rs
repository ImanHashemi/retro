use crate::errors::CoreError;
use std::path::Path;
use std::process::Command;

const HOOK_MARKER: &str = "# retro hook - do not remove";

/// Check if we are inside a git repository.
pub fn is_in_git_repo() -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Check if the `gh` CLI is available on PATH.
pub fn is_gh_available() -> bool {
    Command::new("gh")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Get the git repository root directory.
pub fn git_root() -> Result<String, CoreError> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| CoreError::Io(format!("running git: {e}")))?;

    if !output.status.success() {
        return Err(CoreError::Io("not inside a git repository".to_string()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the current git branch name.
pub fn current_branch() -> Result<String, CoreError> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .map_err(|e| CoreError::Io(format!("getting current branch: {e}")))?;

    if !output.status.success() {
        return Err(CoreError::Io("failed to get current branch".to_string()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Create and checkout a new git branch.
pub fn create_branch(name: &str) -> Result<(), CoreError> {
    let output = Command::new("git")
        .args(["checkout", "-b", name])
        .output()
        .map_err(|e| CoreError::Io(format!("creating branch: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::Io(format!("git checkout -b failed: {stderr}")));
    }

    Ok(())
}

/// Switch back to a branch.
pub fn checkout_branch(name: &str) -> Result<(), CoreError> {
    let output = Command::new("git")
        .args(["checkout", name])
        .output()
        .map_err(|e| CoreError::Io(format!("checking out branch: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::Io(format!("git checkout failed: {stderr}")));
    }

    Ok(())
}

/// Stage specific files and commit.
pub fn commit_files(files: &[&str], message: &str) -> Result<(), CoreError> {
    // Stage files
    let mut args = vec!["add", "--"];
    args.extend(files);

    let output = Command::new("git")
        .args(&args)
        .output()
        .map_err(|e| CoreError::Io(format!("git add: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::Io(format!("git add failed: {stderr}")));
    }

    // Commit
    let output = Command::new("git")
        .args(["commit", "-m", message])
        .output()
        .map_err(|e| CoreError::Io(format!("git commit: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::Io(format!("git commit failed: {stderr}")));
    }

    Ok(())
}

/// Create a PR using `gh pr create`. Returns the PR URL on success.
pub fn create_pr(title: &str, body: &str) -> Result<String, CoreError> {
    let output = Command::new("gh")
        .args(["pr", "create", "--title", title, "--body", body])
        .output()
        .map_err(|e| CoreError::Io(format!("gh pr create: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::Io(format!("gh pr create failed: {stderr}")));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Install retro git hooks (post-commit, post-merge) into the repository.
pub fn install_hooks(repo_root: &str) -> Result<Vec<String>, CoreError> {
    let hooks_dir = Path::new(repo_root).join(".git").join("hooks");
    if !hooks_dir.exists() {
        return Err(CoreError::Io(format!(
            "git hooks directory not found: {}",
            hooks_dir.display()
        )));
    }

    let mut installed = Vec::new();

    // post-commit hook: fast ingest
    let post_commit_path = hooks_dir.join("post-commit");
    if install_hook_lines(
        &post_commit_path,
        &format!("{HOOK_MARKER}\nretro ingest --auto 2>/dev/null &\n"),
    )? {
        installed.push("post-commit".to_string());
    }

    // post-merge hook: full analyze
    let post_merge_path = hooks_dir.join("post-merge");
    if install_hook_lines(
        &post_merge_path,
        &format!("{HOOK_MARKER}\nretro analyze --auto 2>/dev/null &\n"),
    )? {
        installed.push("post-merge".to_string());
    }

    Ok(installed)
}

/// Install hook lines into a hook file. Returns true if lines were added.
fn install_hook_lines(hook_path: &Path, lines: &str) -> Result<bool, CoreError> {
    let existing = if hook_path.exists() {
        std::fs::read_to_string(hook_path)
            .map_err(|e| CoreError::Io(format!("reading hook {}: {e}", hook_path.display())))?
    } else {
        String::new()
    };

    // Already installed
    if existing.contains(HOOK_MARKER) {
        return Ok(false);
    }

    let mut content = if existing.is_empty() {
        "#!/bin/sh\n".to_string()
    } else {
        let mut s = existing;
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s
    };

    content.push_str(lines);

    std::fs::write(hook_path, &content)
        .map_err(|e| CoreError::Io(format!("writing hook {}: {e}", hook_path.display())))?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(hook_path, perms)
            .map_err(|e| CoreError::Io(format!("chmod hook: {e}")))?;
    }

    Ok(true)
}

/// Remove retro hook lines from git hooks in the given repository.
/// Returns the list of hooks that were modified.
pub fn remove_hooks(repo_root: &str) -> Result<Vec<String>, CoreError> {
    let hooks_dir = Path::new(repo_root).join(".git").join("hooks");
    if !hooks_dir.exists() {
        return Ok(Vec::new());
    }

    let mut modified = Vec::new();

    for hook_name in &["post-commit", "post-merge"] {
        let hook_path = hooks_dir.join(hook_name);
        if !hook_path.exists() {
            continue;
        }

        let content = std::fs::read_to_string(&hook_path)
            .map_err(|e| CoreError::Io(format!("reading hook: {e}")))?;

        if !content.contains(HOOK_MARKER) {
            continue;
        }

        let cleaned = remove_hook_lines(&content);

        // If only the shebang remains (or empty), remove the file
        let trimmed = cleaned.trim();
        if trimmed.is_empty() || trimmed == "#!/bin/sh" || trimmed == "#!/bin/bash" {
            std::fs::remove_file(&hook_path)
                .map_err(|e| CoreError::Io(format!("removing hook file: {e}")))?;
        } else {
            std::fs::write(&hook_path, &cleaned)
                .map_err(|e| CoreError::Io(format!("writing cleaned hook: {e}")))?;
        }

        modified.push(hook_name.to_string());
    }

    Ok(modified)
}

/// Remove retro hook lines from hook content.
/// Removes the marker line and the command line immediately after it.
fn remove_hook_lines(content: &str) -> String {
    let mut result = Vec::new();
    let mut skip_next = false;

    for line in content.lines() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if line.trim() == HOOK_MARKER {
            skip_next = true;
            continue;
        }
        result.push(line);
    }

    let mut output = result.join("\n");
    if !output.is_empty() && content.ends_with('\n') {
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_hook_lines_basic() {
        let content = "#!/bin/sh\n# retro hook - do not remove\nretro ingest 2>/dev/null &\n";
        let result = remove_hook_lines(content);
        assert_eq!(result, "#!/bin/sh\n");
    }

    #[test]
    fn test_remove_hook_lines_preserves_other_hooks() {
        let content = "#!/bin/sh\nsome-other-tool run\n# retro hook - do not remove\nretro ingest 2>/dev/null &\nanother-command\n";
        let result = remove_hook_lines(content);
        assert_eq!(result, "#!/bin/sh\nsome-other-tool run\nanother-command\n");
    }

    #[test]
    fn test_remove_hook_lines_no_marker() {
        let content = "#!/bin/sh\nsome-command\n";
        let result = remove_hook_lines(content);
        assert_eq!(result, "#!/bin/sh\nsome-command\n");
    }

    #[test]
    fn test_remove_hook_lines_multiple_markers() {
        let content = "#!/bin/sh\n# retro hook - do not remove\nretro ingest 2>/dev/null &\n# retro hook - do not remove\nretro analyze --auto 2>/dev/null &\n";
        let result = remove_hook_lines(content);
        assert_eq!(result, "#!/bin/sh\n");
    }
}
