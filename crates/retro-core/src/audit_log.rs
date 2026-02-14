use crate::errors::CoreError;
use crate::models::AuditEntry;
use chrono::Utc;
use std::fs::OpenOptions;
use std::io::{BufRead, Write};
use std::path::Path;

/// Append an audit entry to the JSONL audit log.
/// Uses O_APPEND for atomic writes on POSIX systems.
pub fn append(
    path: &Path,
    action: &str,
    details: serde_json::Value,
) -> Result<(), CoreError> {
    let entry = AuditEntry {
        timestamp: Utc::now(),
        action: action.to_string(),
        details,
    };

    let mut line =
        serde_json::to_string(&entry).map_err(|e| CoreError::Io(format!("serializing audit entry: {e}")))?;
    line.push('\n');

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| CoreError::Io(format!("opening audit log: {e}")))?;

    file.write_all(line.as_bytes())
        .map_err(|e| CoreError::Io(format!("writing audit log: {e}")))?;

    Ok(())
}

/// Read audit log entries, optionally filtered by time window.
pub fn read_entries(
    path: &Path,
    since: Option<&chrono::DateTime<Utc>>,
) -> Result<Vec<AuditEntry>, CoreError> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = std::fs::File::open(path)
        .map_err(|e| CoreError::Io(format!("opening audit log: {e}")))?;
    let reader = std::io::BufReader::new(file);

    let mut entries = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<AuditEntry>(trimmed) {
            Ok(entry) => {
                if let Some(since) = since {
                    if entry.timestamp >= *since {
                        entries.push(entry);
                    }
                } else {
                    entries.push(entry);
                }
            }
            Err(_) => continue,
        }
    }

    Ok(entries)
}
