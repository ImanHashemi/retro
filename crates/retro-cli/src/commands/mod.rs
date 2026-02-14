pub mod analyze;
pub mod apply;
pub mod diff;
pub mod ingest;
pub mod init;
pub mod patterns;
pub mod status;

use anyhow::{Context, Result};

/// Get git repository root, falling back to current directory.
pub fn git_root_or_cwd() -> Result<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
            Ok(root)
        }
        _ => {
            let cwd = std::env::current_dir().context("getting current directory")?;
            Ok(cwd.to_string_lossy().to_string())
        }
    }
}
