use crate::errors::CoreError;
use crate::models::*;
use crate::util::log_parse_warning;
use std::io::BufRead;
use std::path::Path;

/// Parse a main session JSONL file into a structured Session.
pub fn parse_session_file(
    path: &Path,
    session_id: &str,
    project: &str,
) -> Result<Session, CoreError> {
    let entries = parse_jsonl_entries(path)?;
    build_session(entries, session_id, project, path)
}

/// Parse all subagent JSONL files in a directory.
pub fn parse_subagent_dir(
    dir: &Path,
    parent_session_id: &str,
    project: &str,
) -> Result<Vec<Session>, CoreError> {
    let pattern = dir.join("agent-*.jsonl");
    let pattern_str = pattern.to_string_lossy();

    let mut sessions = Vec::new();

    let paths: Vec<_> = glob::glob(&pattern_str)
        .map_err(|e| CoreError::Parse(format!("glob error: {e}")))?
        .filter_map(|r| r.ok())
        .collect();

    for path in paths {
        let agent_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let sub_id = format!("{parent_session_id}/{agent_id}");

        match parse_session_file(&path, &sub_id, project) {
            Ok(session) => sessions.push(session),
            Err(e) => {
                log_parse_warning(&format!(
                    "subagent {}: parse error: {e}",
                    path.display()
                ));
            }
        }
    }

    Ok(sessions)
}

/// Entry types we care about parsing fully.
const KNOWN_TYPES: &[&str] = &["user", "assistant", "summary"];
/// Entry types we intentionally skip.
const SKIP_TYPES: &[&str] = &["file-history-snapshot", "progress"];

/// Parse JSONL entries from a file, skipping unparseable lines.
fn parse_jsonl_entries(path: &Path) -> Result<Vec<SessionEntry>, CoreError> {
    let file =
        std::fs::File::open(path).map_err(|e| CoreError::Io(format!("opening {}: {e}", path.display())))?;
    let reader = std::io::BufReader::new(file);

    let mut entries = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                log_parse_warning(&format!(
                    "{}: line {}: read error: {e}",
                    path.display(),
                    line_num + 1
                ));
                continue;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Pre-parse the type field to distinguish unknown types from parse errors
        let entry_type = serde_json::from_str::<serde_json::Value>(trimmed)
            .ok()
            .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from));

        match &entry_type {
            Some(t) if SKIP_TYPES.contains(&t.as_str()) => continue,
            Some(t) if !KNOWN_TYPES.contains(&t.as_str()) => continue, // unknown future type
            _ => {}
        }

        match serde_json::from_str::<SessionEntry>(trimmed) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                // Log parse errors for known types — these indicate real problems
                if let Some(t) = &entry_type {
                    log_parse_warning(&format!(
                        "{}: line {} (type={}): parse error: {e}",
                        path.display(),
                        line_num + 1,
                        t
                    ));
                }
            }
        }
    }

    Ok(entries)
}

/// Build a Session from parsed entries.
fn build_session(
    entries: Vec<SessionEntry>,
    session_id: &str,
    project: &str,
    path: &Path,
) -> Result<Session, CoreError> {
    let mut user_messages = Vec::new();
    let mut assistant_messages = Vec::new();
    let mut summaries = Vec::new();
    let mut tools_used = Vec::new();
    let mut errors = Vec::new();
    let mut metadata = SessionMetadata {
        cwd: None,
        version: None,
        git_branch: None,
        model: None,
    };

    for entry in entries {
        match entry {
            SessionEntry::User(user) => {
                // Extract metadata from first user entry
                if metadata.cwd.is_none() {
                    metadata.cwd = user.cwd.clone();
                    metadata.version = user.version.clone();
                    metadata.git_branch = user.git_branch.clone();
                }

                // Only include actual user prompts, not tool results
                if !user.message.content.is_tool_result() {
                    let text = user.message.content.as_text();
                    if !text.is_empty() {
                        user_messages.push(ParsedUserMessage {
                            text,
                            timestamp: user.timestamp.clone(),
                        });
                    }
                }
            }
            SessionEntry::Assistant(assistant) => {
                let mut text_parts = Vec::new();
                let mut thinking_summary = None;
                let mut msg_tools = Vec::new();

                for block in &assistant.message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            text_parts.push(text.clone());
                        }
                        ContentBlock::Thinking { thinking, .. } => {
                            thinking_summary = Some(summarize_thinking(thinking));
                        }
                        ContentBlock::ToolUse { name, .. } => {
                            msg_tools.push(name.clone());
                            if !tools_used.contains(name) {
                                tools_used.push(name.clone());
                            }
                        }
                        ContentBlock::ToolResult { content, .. } => {
                            // Check for error content
                            if let Some(c) = content {
                                let text = c.as_text();
                                let lower = text.to_lowercase();
                                if lower.contains("error")
                                    || lower.contains("failed")
                                    || lower.contains("not found")
                                {
                                    errors.push(truncate(&text, 200));
                                }
                            }
                        }
                        ContentBlock::Unknown => {}
                    }
                }

                if metadata.model.is_none() {
                    metadata.model = assistant.message.model.clone();
                }

                let text = text_parts.join("\n");
                if !text.is_empty() || !msg_tools.is_empty() || thinking_summary.is_some() {
                    assistant_messages.push(ParsedAssistantMessage {
                        text,
                        thinking_summary,
                        tools: msg_tools,
                        timestamp: assistant.timestamp.clone(),
                    });
                }
            }
            SessionEntry::Summary(summary) => {
                if let Some(s) = &summary.summary {
                    summaries.push(s.clone());
                }
                // Also check message field for summary content
                if let Some(msg) = &summary.message {
                    if let Some(content) = msg.get("content") {
                        if let Some(text) = content.as_str() {
                            summaries.push(text.to_string());
                        }
                    }
                }
            }
            SessionEntry::FileHistorySnapshot(_) | SessionEntry::Progress(_) => {
                // Skip — not useful for pattern analysis
            }
        }
    }

    Ok(Session {
        session_id: session_id.to_string(),
        project: project.to_string(),
        session_path: path.to_string_lossy().to_string(),
        user_messages,
        assistant_messages,
        summaries,
        tools_used,
        errors,
        metadata,
    })
}

/// Summarize a thinking block: first 500 chars + keyword-extracted segments.
/// Thinking blocks can be 32K+ tokens, so we must bound the output.
fn summarize_thinking(thinking: &str) -> String {
    let keywords = ["error", "mistake", "wrong", "failed", "retry", "fix", "bug", "issue"];
    let mut parts = Vec::new();

    // First 500 chars
    let prefix = truncate(thinking, 500);
    parts.push(prefix);

    // Find keyword-containing sentences beyond the first 500 chars
    let split_point = char_boundary(thinking, 500);
    if split_point < thinking.len() {
        let rest = &thinking[split_point..];
        for sentence in rest.split('.') {
            let lower = sentence.to_lowercase();
            if keywords.iter().any(|kw| lower.contains(kw)) {
                let trimmed = sentence.trim();
                if !trimmed.is_empty() {
                    parts.push(truncate(trimmed, 200));
                }
            }
        }
    }

    // Cap total output
    let result = parts.join(" ... ");
    truncate(&result, 2000)
}

/// Find the largest byte index <= `max` that is a valid UTF-8 char boundary.
fn char_boundary(s: &str, max: usize) -> usize {
    if max >= s.len() {
        return s.len();
    }
    // Walk backwards from max to find a char boundary
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let boundary = char_boundary(s, max);
        format!("{}...", &s[..boundary])
    }
}
