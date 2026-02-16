pub mod backend;
pub mod claude_cli;
pub mod merge;
pub mod prompts;

use crate::config::Config;
use crate::db;
use crate::errors::CoreError;
use crate::ingest::session;
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

    if parsed_sessions.is_empty() {
        return Ok(AnalyzeResult {
            sessions_analyzed: 0,
            new_patterns: 0,
            updated_patterns: 0,
            total_patterns: 0,
            input_tokens: 0,
            output_tokens: 0,
        });
    }

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
        let prompt = prompts::build_analysis_prompt(batch, &existing);

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
        sessions_analyzed: sessions_to_analyze.len(),
        new_patterns: new_count,
        updated_patterns: update_count,
        total_patterns: (discovered + active) as usize,
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
    })
}

/// Parse the AI response text into pattern updates.
fn parse_analysis_response(text: &str) -> Result<Vec<PatternUpdate>, CoreError> {
    // Try to parse as AnalysisResponse first
    let trimmed = text.trim();

    // Handle case where AI wraps response in markdown code blocks
    let json_str = if trimmed.starts_with("```") {
        crate::util::strip_code_fences(trimmed)
    } else {
        trimmed.to_string()
    };

    let response: AnalysisResponse = serde_json::from_str(&json_str).map_err(|e| {
        CoreError::Analysis(format!(
            "failed to parse AI response as JSON: {e}\nresponse text: {}",
            truncate_for_error(text)
        ))
    })?;

    Ok(response.patterns)
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
}
