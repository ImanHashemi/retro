pub mod backend;
pub mod claude_cli;
pub mod merge;
pub mod prompts;

use crate::config::Config;
use crate::db;
use crate::errors::CoreError;
use crate::ingest::{context, session};
use crate::models::{AnalysisResponse, AnalyzeResult, PatternUpdate};
use crate::scrub;
use chrono::{Duration, Utc};
use rusqlite::Connection;
use std::path::Path;

use backend::AnalysisBackend;
use claude_cli::ClaudeCliBackend;

pub const BATCH_SIZE: usize = 20;

/// Run analysis: re-parse sessions, scrub, call AI, merge patterns, store results.
pub fn analyze(
    conn: &Connection,
    config: &Config,
    project: Option<&str>,
    window_days: u32,
) -> Result<AnalyzeResult, CoreError> {
    // Check claude CLI availability
    if !ClaudeCliBackend::is_available() {
        return Err(CoreError::Analysis(
            "claude CLI not found on PATH. Install Claude Code CLI to use analysis.".to_string(),
        ));
    }

    let since = Utc::now() - Duration::days(window_days as i64);

    // Get sessions to analyze (within window, not yet analyzed)
    let sessions_to_analyze = db::get_sessions_for_analysis(conn, project, &since)?;

    if sessions_to_analyze.is_empty() {
        return Ok(AnalyzeResult {
            sessions_analyzed: 0,
            new_patterns: 0,
            updated_patterns: 0,
            total_patterns: 0,
            input_tokens: 0,
            output_tokens: 0,
        });
    }

    // Re-parse session files from disk to get full content
    let mut parsed_sessions = Vec::new();
    for ingested in &sessions_to_analyze {
        let path = Path::new(&ingested.session_path);
        if !path.exists() {
            eprintln!(
                "warning: session file not found: {}",
                ingested.session_path
            );
            continue;
        }

        match session::parse_session_file(path, &ingested.session_id, &ingested.project) {
            Ok(mut s) => {
                // Apply secret scrubbing if enabled
                if config.privacy.scrub_secrets {
                    scrub::scrub_session(&mut s);
                }
                parsed_sessions.push(s);
            }
            Err(e) => {
                eprintln!(
                    "warning: failed to re-parse session {}: {e}",
                    ingested.session_id
                );
            }
        }
    }

    // Filter out low-signal sessions: single-message sessions are typically
    // programmatic `claude -p` calls (including retro's own analysis) or heavily
    // compacted sessions — not real multi-turn conversations with discoverable patterns.
    let before_filter = parsed_sessions.len();
    parsed_sessions.retain(|s| s.user_messages.len() >= 2);
    let filtered_out = before_filter - parsed_sessions.len();
    if filtered_out > 0 {
        eprintln!(
            "  Skipped {} single-message session{} (no pattern signal)",
            filtered_out,
            if filtered_out == 1 { "" } else { "s" }
        );
    }

    let analyzed_count = parsed_sessions.len();

    if parsed_sessions.is_empty() {
        // Still record all sessions as analyzed so we don't re-process low-signal ones
        for ingested in &sessions_to_analyze {
            db::record_analyzed_session(conn, &ingested.session_id, &ingested.project)?;
        }
        return Ok(AnalyzeResult {
            sessions_analyzed: 0,
            new_patterns: 0,
            updated_patterns: 0,
            total_patterns: 0,
            input_tokens: 0,
            output_tokens: 0,
        });
    }

    // Load context summary (best-effort — analysis proceeds without it)
    let context_summary = match project {
        Some(project_path) => context::snapshot_context(config, project_path)
            .ok()
            .map(|s| prompts::build_context_summary(&s))
            .filter(|s| !s.is_empty()),
        None => None,
    };

    // Create AI backend
    let backend = ClaudeCliBackend::new(&config.ai);

    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut new_count = 0;
    let mut update_count = 0;

    // Process in batches
    for batch in parsed_sessions.chunks(BATCH_SIZE) {
        // Reload existing patterns before each batch (picks up patterns from prior batches)
        let existing = db::get_patterns(conn, &["discovered", "active"], project)?;

        // Build prompt
        let prompt = prompts::build_analysis_prompt(batch, &existing, context_summary.as_deref());

        // Call AI backend
        let response = backend.execute(&prompt)?;
        total_input_tokens += response.input_tokens;
        total_output_tokens += response.output_tokens;

        // Parse AI response into PatternUpdate objects
        let updates = parse_analysis_response(&response.text)?;

        // Apply merge logic
        let (new_patterns, merge_updates) = merge::process_updates(updates, &existing, project);

        // Store new patterns
        for pattern in &new_patterns {
            db::insert_pattern(conn, pattern)?;
            new_count += 1;
        }

        // Apply merge updates
        for update in &merge_updates {
            db::update_pattern_merge(
                conn,
                &update.pattern_id,
                &update.new_sessions,
                update.new_confidence,
                Utc::now(),
                update.additional_times_seen,
            )?;
            update_count += 1;
        }
    }

    // Record all sessions as analyzed
    for ingested in &sessions_to_analyze {
        db::record_analyzed_session(conn, &ingested.session_id, &ingested.project)?;
    }

    // Get total pattern count
    let discovered = db::pattern_count_by_status(conn, "discovered")?;
    let active = db::pattern_count_by_status(conn, "active")?;

    Ok(AnalyzeResult {
        sessions_analyzed: analyzed_count,
        new_patterns: new_count,
        updated_patterns: update_count,
        total_patterns: (discovered + active) as usize,
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
    })
}

/// Parse the AI response text into pattern updates.
/// Handles multiple response formats:
/// 1. Pure JSON: `{"patterns": [...]}`
/// 2. Markdown-wrapped: ` ```json\n{...}\n``` `
/// 3. Prose with embedded JSON or code block (AI sometimes narrates before the JSON)
fn parse_analysis_response(text: &str) -> Result<Vec<PatternUpdate>, CoreError> {
    let trimmed = text.trim();

    // Strategy 1: Direct JSON parse
    if let Ok(response) = serde_json::from_str::<AnalysisResponse>(trimmed) {
        return Ok(response.patterns);
    }

    // Strategy 2: Strip leading code fences (```json ... ```)
    if trimmed.starts_with("```") {
        let stripped = crate::util::strip_code_fences(trimmed);
        if let Ok(response) = serde_json::from_str::<AnalysisResponse>(&stripped) {
            return Ok(response.patterns);
        }
    }

    // Strategy 3: Find a code-fenced JSON block embedded in prose
    if let Some(json) = extract_fenced_json(trimmed) {
        if let Ok(response) = serde_json::from_str::<AnalysisResponse>(&json) {
            return Ok(response.patterns);
        }
    }

    // Strategy 4: Find a bare JSON object containing "patterns" in the text
    if let Some(json) = extract_json_object(trimmed) {
        if let Ok(response) = serde_json::from_str::<AnalysisResponse>(&json) {
            return Ok(response.patterns);
        }
    }

    Err(CoreError::Analysis(format!(
        "failed to parse AI response as JSON (tried direct, code-fenced, and embedded extraction)\nresponse text: {}",
        truncate_for_error(text)
    )))
}

/// Extract JSON from a ```json ... ``` block embedded anywhere in the text.
fn extract_fenced_json(text: &str) -> Option<String> {
    // Find ```json or ``` followed by a JSON block
    let fence_start = text.find("```json").or_else(|| {
        // Look for ``` followed by { on the next line
        text.find("```\n{")
    })?;

    let content_start = text[fence_start..].find('\n')? + fence_start + 1;
    let remaining = &text[content_start..];
    let fence_end = remaining.find("\n```")?;
    let json = remaining[..fence_end].trim();

    if json.starts_with('{') {
        Some(json.to_string())
    } else {
        None
    }
}

/// Extract a JSON object containing "patterns" from prose text.
/// Finds the outermost `{...}` that contains `"patterns"`.
fn extract_json_object(text: &str) -> Option<String> {
    // Look for {"patterns" as a strong signal
    let start = text.find("{\"patterns\"")?;
    let rest = &text[start..];

    // Walk forward tracking brace depth to find the matching close
    let mut depth = 0;
    let mut end = 0;
    for (i, ch) in rest.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                    break;
                }
            }
            _ => {}
        }
    }

    if end > 0 {
        Some(rest[..end].to_string())
    } else {
        None
    }
}

fn truncate_for_error(s: &str) -> &str {
    if s.len() <= 500 {
        s
    } else {
        let mut i = 500;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
        &s[..i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_analysis_response_json() {
        let json = r#"{
            "patterns": [
                {
                    "action": "new",
                    "pattern_type": "repetitive_instruction",
                    "description": "User always asks to use uv",
                    "confidence": 0.85,
                    "source_sessions": ["sess-1"],
                    "related_files": [],
                    "suggested_content": "Always use uv",
                    "suggested_target": "claude_md"
                },
                {
                    "action": "update",
                    "existing_id": "pat-123",
                    "new_sessions": ["sess-2"],
                    "new_confidence": 0.92
                }
            ]
        }"#;

        let updates = parse_analysis_response(json).unwrap();
        assert_eq!(updates.len(), 2);
        assert!(matches!(&updates[0], PatternUpdate::New(_)));
        assert!(matches!(&updates[1], PatternUpdate::Update(_)));
    }

    #[test]
    fn test_parse_analysis_response_markdown_wrapped() {
        let text = r#"```json
{
    "patterns": [
        {
            "action": "new",
            "pattern_type": "recurring_mistake",
            "description": "Agent forgets to run tests",
            "confidence": 0.75,
            "source_sessions": [],
            "related_files": [],
            "suggested_content": "Run tests after changes",
            "suggested_target": "skill"
        }
    ]
}
```"#;

        let updates = parse_analysis_response(text).unwrap();
        assert_eq!(updates.len(), 1);
    }

    #[test]
    fn test_parse_analysis_response_null_fields() {
        let json = r#"{
            "patterns": [
                {
                    "action": "new",
                    "pattern_type": "repetitive_instruction",
                    "description": "Some pattern",
                    "confidence": 0.8,
                    "source_sessions": [],
                    "related_files": [],
                    "suggested_content": null,
                    "suggested_target": "claude_md"
                }
            ]
        }"#;
        let updates = parse_analysis_response(json).unwrap();
        assert_eq!(updates.len(), 1);
        if let PatternUpdate::New(ref p) = updates[0] {
            assert_eq!(p.suggested_content, "");
        } else {
            panic!("expected New pattern");
        }
    }

    #[test]
    fn test_parse_analysis_response_empty() {
        let json = r#"{"patterns": []}"#;
        let updates = parse_analysis_response(json).unwrap();
        assert!(updates.is_empty());
    }

    #[test]
    fn test_parse_analysis_response_prose_with_fenced_json() {
        let text = r#"I'll analyze these sessions to discover recurring patterns.

**Session Analysis:**

1. Session A - User asks to run tests
2. Session B - User asks to run tests again

Based on my analysis:

```json
{
    "patterns": [
        {
            "action": "new",
            "pattern_type": "repetitive_instruction",
            "description": "User always runs tests before committing",
            "confidence": 0.8,
            "source_sessions": ["sess-a", "sess-b"],
            "related_files": [],
            "suggested_content": "Always run tests before committing",
            "suggested_target": "claude_md"
        }
    ]
}
```"#;

        let updates = parse_analysis_response(text).unwrap();
        assert_eq!(updates.len(), 1);
        assert!(matches!(&updates[0], PatternUpdate::New(_)));
    }

    #[test]
    fn test_parse_analysis_response_prose_with_bare_json() {
        let text = r#"After analyzing the sessions, here are the patterns:

{"patterns": [{"action": "new", "pattern_type": "repetitive_instruction", "description": "Test pattern", "confidence": 0.7, "source_sessions": ["s1", "s2"], "related_files": [], "suggested_content": "Do the thing", "suggested_target": "claude_md"}]}"#;

        let updates = parse_analysis_response(text).unwrap();
        assert_eq!(updates.len(), 1);
    }

    #[test]
    fn test_parse_analysis_response_pure_prose_fails() {
        let text = "I analyzed the sessions but found no recurring patterns worth reporting.";
        let result = parse_analysis_response(text);
        assert!(result.is_err());
    }
}
