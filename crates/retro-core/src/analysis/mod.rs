pub mod backend;
pub mod claude_cli;
pub mod merge;
pub mod prompts;

use crate::config::Config;
use crate::db;
use crate::errors::CoreError;
use crate::ingest::{context, session};
use crate::models::{AnalysisResponse, AnalyzeResult, BatchDetail};
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
    "reasoning": {"type": "string"},
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
  "required": ["reasoning", "patterns"],
  "additionalProperties": false
}"#;

/// Extended JSON schema that includes `claude_md_edits` for full_management mode.
/// Returns a `String` because it's dynamically constructed (unlike the const base schema).
pub fn full_management_analysis_schema() -> String {
    r#"{
  "type": "object",
  "properties": {
    "reasoning": {"type": "string"},
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
    },
    "claude_md_edits": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "edit_type": {"type": "string", "enum": ["add", "remove", "reword", "move"]},
          "original_text": {"type": "string"},
          "suggested_content": {"type": "string"},
          "target_section": {"type": "string"},
          "reasoning": {"type": "string"}
        },
        "required": ["edit_type", "reasoning"],
        "additionalProperties": false
      }
    }
  },
  "required": ["reasoning", "patterns"],
  "additionalProperties": false
}"#.to_string()
}

/// Run analysis: re-parse sessions, scrub, call AI, merge patterns, store results.
///
/// `on_batch_start` is called before each AI call with (batch_index, total_batches, session_count, prompt_chars).
pub fn analyze<F>(
    conn: &Connection,
    config: &Config,
    project: Option<&str>,
    window_days: u32,
    on_batch_start: F,
) -> Result<AnalyzeResult, CoreError>
where
    F: Fn(usize, usize, usize, usize),
{
    // Check claude CLI availability and auth
    if !ClaudeCliBackend::is_available() {
        return Err(CoreError::Analysis(
            "claude CLI not found on PATH. Install Claude Code CLI to use analysis.".to_string(),
        ));
    }
    // Pre-flight auth check: a minimal prompt without --json-schema returns immediately
    // on auth failure. With --json-schema, auth errors cause an infinite StructuredOutput
    // retry loop in the CLI (it keeps injecting "You MUST call StructuredOutput" but the
    // auth error response is always plain text, never a tool call).
    ClaudeCliBackend::check_auth()?;

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
            batch_details: Vec::new(),
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
            batch_details: Vec::new(),
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
    let mut batch_details: Vec<BatchDetail> = Vec::new();

    // Process in batches
    let total_batches = (parsed_sessions.len() + BATCH_SIZE - 1) / BATCH_SIZE;

    for (batch_idx, batch) in parsed_sessions.chunks(BATCH_SIZE).enumerate() {
        // Reload existing patterns before each batch (picks up patterns from prior batches)
        let existing = db::get_patterns(conn, &["discovered", "active"], project)?;

        // Build prompt
        let prompt = prompts::build_analysis_prompt(batch, &existing, context_summary.as_deref(), false);
        let prompt_chars = prompt.len();

        on_batch_start(batch_idx, total_batches, batch.len(), prompt_chars);

        // Call AI backend
        let response = backend.execute(&prompt, Some(ANALYSIS_RESPONSE_SCHEMA))?;
        total_input_tokens += response.input_tokens;
        total_output_tokens += response.output_tokens;

        // Parse AI response into AnalysisResponse (reasoning + pattern updates)
        let analysis_resp = parse_analysis_response(&response.text).map_err(|e| {
            CoreError::Analysis(format!(
                "{e}\n(prompt_chars={}, output_tokens={}, result_chars={})",
                prompt_chars,
                response.output_tokens,
                response.text.len()
            ))
        })?;

        let reasoning = analysis_resp.reasoning;

        // Apply merge logic
        let (new_patterns, merge_updates) =
            merge::process_updates(analysis_resp.patterns, &existing, project);

        let batch_new = new_patterns.len();
        let batch_updated = merge_updates.len();

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

        // Collect per-batch diagnostics
        let preview = truncate_for_error(&response.text, 500).to_string();
        batch_details.push(BatchDetail {
            batch_index: batch_idx,
            session_count: batch.len(),
            session_ids: batch.iter().map(|s| s.session_id.clone()).collect(),
            prompt_chars,
            input_tokens: response.input_tokens,
            output_tokens: response.output_tokens,
            new_patterns: batch_new,
            updated_patterns: batch_updated,
            reasoning,
            ai_response_preview: preview,
        });
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
        batch_details,
    })
}

/// Parse the AI response text into an AnalysisResponse (reasoning + pattern updates).
/// With `--json-schema` constrained decoding, the response is guaranteed valid JSON.
fn parse_analysis_response(text: &str) -> Result<AnalysisResponse, CoreError> {
    let trimmed = text.trim();
    let response: AnalysisResponse = serde_json::from_str(trimmed).map_err(|e| {
        CoreError::Analysis(format!(
            "failed to parse AI response as JSON: {e}\nresponse text: {}",
            truncate_for_error(text, 1500)
        ))
    })?;
    Ok(response)
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
    use crate::models::PatternUpdate;

    #[test]
    fn test_parse_analysis_response_json() {
        let json = r#"{
            "reasoning": "Found recurring instruction across sessions.",
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

        let resp = parse_analysis_response(json).unwrap();
        assert_eq!(resp.reasoning, "Found recurring instruction across sessions.");
        assert_eq!(resp.patterns.len(), 2);
        assert!(matches!(&resp.patterns[0], PatternUpdate::New(_)));
        assert!(matches!(&resp.patterns[1], PatternUpdate::Update(_)));
    }

    #[test]
    fn test_parse_analysis_response_null_fields() {
        let json = r#"{
            "reasoning": "Observed a single pattern.",
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
        let resp = parse_analysis_response(json).unwrap();
        assert_eq!(resp.patterns.len(), 1);
        if let PatternUpdate::New(ref p) = resp.patterns[0] {
            assert_eq!(p.suggested_content, "");
        } else {
            panic!("expected New pattern");
        }
    }

    #[test]
    fn test_parse_analysis_response_empty() {
        let json = r#"{"reasoning": "No recurring patterns found.", "patterns": []}"#;
        let resp = parse_analysis_response(json).unwrap();
        assert_eq!(resp.reasoning, "No recurring patterns found.");
        assert!(resp.patterns.is_empty());
    }

    #[test]
    fn test_parse_analysis_response_missing_reasoning_defaults_empty() {
        let json = r#"{"patterns": []}"#;
        let resp = parse_analysis_response(json).unwrap();
        assert_eq!(resp.reasoning, "");
        assert!(resp.patterns.is_empty());
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

    #[test]
    fn test_full_management_analysis_schema_is_valid_json() {
        let schema_str = full_management_analysis_schema();
        let value: serde_json::Value =
            serde_json::from_str(&schema_str).expect("full_management schema must be valid JSON");
        assert_eq!(value["type"], "object");
        assert!(value["properties"]["patterns"].is_object());
    }

    #[test]
    fn test_full_management_analysis_schema_contains_claude_md_edits() {
        let schema_str = full_management_analysis_schema();
        let value: serde_json::Value = serde_json::from_str(&schema_str).unwrap();

        // claude_md_edits should be in properties
        let edits = &value["properties"]["claude_md_edits"];
        assert!(edits.is_object(), "claude_md_edits should be in properties");
        assert_eq!(edits["type"], "array");

        // Items should have edit_type, reasoning as required
        let items = &edits["items"];
        assert_eq!(items["type"], "object");
        let required: Vec<String> = items["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(required.contains(&"edit_type".to_string()));
        assert!(required.contains(&"reasoning".to_string()));

        // edit_type should have the right enum values
        let edit_type_enum = items["properties"]["edit_type"]["enum"]
            .as_array()
            .unwrap();
        let enum_values: Vec<&str> = edit_type_enum.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(enum_values.contains(&"add"));
        assert!(enum_values.contains(&"remove"));
        assert!(enum_values.contains(&"reword"));
        assert!(enum_values.contains(&"move"));

        // additionalProperties should be false on items
        assert_eq!(items["additionalProperties"], false);
    }

    #[test]
    fn test_full_management_schema_claude_md_edits_not_required() {
        let schema_str = full_management_analysis_schema();
        let value: serde_json::Value = serde_json::from_str(&schema_str).unwrap();

        // claude_md_edits should NOT be in the top-level required array
        let required: Vec<String> = value["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            !required.contains(&"claude_md_edits".to_string()),
            "claude_md_edits should NOT be in top-level required"
        );
        // But reasoning and patterns should still be required
        assert!(required.contains(&"reasoning".to_string()));
        assert!(required.contains(&"patterns".to_string()));
    }

    #[test]
    fn test_full_management_schema_preserves_base_patterns() {
        // The full_management schema should have the same patterns structure as the base schema
        let base: serde_json::Value = serde_json::from_str(ANALYSIS_RESPONSE_SCHEMA).unwrap();
        let full: serde_json::Value =
            serde_json::from_str(&full_management_analysis_schema()).unwrap();

        assert_eq!(
            base["properties"]["patterns"],
            full["properties"]["patterns"],
            "patterns schema should be identical between base and full_management"
        );
        assert_eq!(
            base["properties"]["reasoning"],
            full["properties"]["reasoning"],
            "reasoning schema should be identical"
        );
    }
}
