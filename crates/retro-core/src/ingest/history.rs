use crate::errors::CoreError;
use crate::models::HistoryEntry;
use crate::util::log_parse_warning;
use std::io::BufRead;
use std::path::Path;

/// Parse the global history.jsonl file.
/// Returns entries filtered by optional project and time window.
pub fn parse_history(
    path: &Path,
    project_filter: Option<&str>,
    since_ms: Option<u64>,
) -> Result<Vec<HistoryEntry>, CoreError> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = std::fs::File::open(path)
        .map_err(|e| CoreError::Io(format!("opening history: {e}")))?;
    let reader = std::io::BufReader::new(file);

    let mut entries = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                log_parse_warning(&format!("history.jsonl line {}: read error: {e}", line_num + 1));
                continue;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<HistoryEntry>(trimmed) {
            Ok(entry) => {
                // Filter by project if specified
                if let Some(filter) = project_filter {
                    if entry.project.as_deref() != Some(filter) {
                        continue;
                    }
                }

                // Filter by time window if specified
                if let Some(since) = since_ms {
                    if let Some(ts) = entry.timestamp {
                        if ts < since {
                            continue;
                        }
                    }
                }

                entries.push(entry);
            }
            Err(e) => {
                log_parse_warning(&format!(
                    "history.jsonl line {}: parse error: {e}",
                    line_num + 1
                ));
            }
        }
    }

    Ok(entries)
}

/// Get all unique session IDs from history entries.
pub fn session_ids_from_history(entries: &[HistoryEntry]) -> Vec<String> {
    let mut ids: Vec<String> = entries
        .iter()
        .filter_map(|e| e.session_id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}
