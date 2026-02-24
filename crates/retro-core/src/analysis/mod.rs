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

/// JSON schema for constrained decoding of analysis responses.
/// Flat schema — serde's `#[serde(tag = "action")]` handles variant discrimination.
/// All fields optional except `action`; `additionalProperties: false` required by structured output.
pub const ANALYSIS_RESPONSE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "patterns": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "action": {"type": "string", "enum": ["new", "update"]},
          "pattern_type": {"type": "string", "enum": ["repetitive_instruction", "recurring_mistake", "workflow_pattern", "stale_context", "redundant_context"]},
          "description": {"type": "string"},
          "confidence": {"type": "number"},
          "source_sessions": {"type": "array", "items": {"type": "string"}},
          "related_files": {"type": "array", "items": {"type": "string"}},
          "suggested_content": {"type": "string"},
          "suggested_target": {"type": "string", "enum": ["skill", "claude_md", "global_agent", "db_only"]},
          "existing_id": {"type": "string"},
          "new_sessions": {"type": "array", "items": {"type": "string"}},
          "new_confidence": {"type": "number"}
        },
        "required": ["action"],
        "additionalProperties": false
      }
    }
  },
  "required": ["patterns"],
  "additionalProperties": false
}"#;

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

    // Get sessions to analyze — rolling_window=true re-analyzes all sessions in window,
    // false only picks up sessions not yet analyzed.
    let rolling = config.analysis.rolling_window;
    let sessions_to_analyze = db::get_sessions_for_analysis(conn, project, &since, rolling)?;

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
        let response = backend.execute(&prompt, Some(ANALYSIS_RESPONSE_SCHEMA))?;
        total_input_tokens += response.input_tokens;
        total_output_tokens += response.output_tokens;

        // Parse AI response into PatternUpdate objects
        let updates = parse_analysis_response(&response.text).map_err(|e| {
            CoreError::Analysis(format!(
                "{e}\n(prompt_chars={}, output_tokens={}, result_chars={})",
                prompt.len(),
                response.output_tokens,
                response.text.len()
            ))
        })?;

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
/// With `--json-schema` constrained decoding, the response is guaranteed valid JSON.
fn parse_analysis_response(text: &str) -> Result<Vec<PatternUpdate>, CoreError> {
    let trimmed = text.trim();
    let response: AnalysisResponse = serde_json::from_str(trimmed).map_err(|e| {
        CoreError::Analysis(format!(
            "failed to parse AI response as JSON: {e}\nresponse text: {}",
            truncate_for_error(text, 1500)
        ))
    })?;
    Ok(response.patterns)
}

fn truncate_for_error(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let mut i = max;
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
    fn test_parse_analysis_response_pure_prose_fails() {
        let text = "I analyzed the sessions but found no recurring patterns worth reporting.";
        let result = parse_analysis_response(text);
        assert!(result.is_err());
    }

    #[test]
    fn test_analysis_response_schema_is_valid_json() {
        let value: serde_json::Value = serde_json::from_str(ANALYSIS_RESPONSE_SCHEMA)
            .expect("ANALYSIS_RESPONSE_SCHEMA must be valid JSON");
        assert_eq!(value["type"], "object");
        assert!(value["properties"]["patterns"].is_object());
    }
}
