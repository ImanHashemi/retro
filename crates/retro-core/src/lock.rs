use crate::errors::CoreError;
use std::fs;
use std::path::{Path, PathBuf};

/// A PID-based lockfile for mutual exclusion.
pub struct LockFile {
    path: PathBuf,
}

impl LockFile {
    /// Try to acquire the lockfile. Returns error if already locked by a running process.
    pub fn acquire(path: &Path) -> Result<Self, CoreError> {
        if path.exists() {
            // Check if the holding process is still alive
            let contents = fs::read_to_string(path)
                .map_err(|e| CoreError::Lock(format!("reading lockfile: {e}")))?;
            if let Ok(pid) = contents.trim().parse::<i32>() {
                if is_process_alive(pid) {
                    return Err(CoreError::Lock(format!(
                        "another retro process is running (PID {pid})"
                    )));
                }
            }
            // Stale lockfile — remove it
            let _ = fs::remove_file(path);
        }

        let pid = std::process::id();
        fs::write(path, pid.to_string())
            .map_err(|e| CoreError::Lock(format!("writing lockfile: {e}")))?;

        Ok(LockFile {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Check if a process is alive using kill(pid, 0) — portable across Linux and macOS.
fn is_process_alive(pid: i32) -> bool {
    // kill with signal 0 checks process existence without sending a signal.
    // Returns 0 if process exists, -1 with ESRCH if it doesn't.
    unsafe { libc::kill(pid, 0) == 0 }
}
