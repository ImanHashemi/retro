pub mod backend;
pub mod claude_cli;
pub mod prompts;
pub mod v3;

use crate::errors::CoreError;
use crate::models::{
    AnalysisResponse, EdgeType, GraphAnalysisResponse, GraphOperation, NodeScope, NodeType,
};

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

/// JSON schema for v2 graph-based analysis responses.
pub const GRAPH_ANALYSIS_RESPONSE_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "reasoning": { "type": "string", "description": "1-2 sentence summary of what you observed" },
        "operations": {
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create_node", "update_node", "create_edge", "merge_nodes"] },
                    "node_type": { "type": "string", "enum": ["preference", "pattern", "rule", "skill", "memory", "directive"] },
                    "scope": { "type": "string", "enum": ["global", "project"] },
                    "project_id": { "type": "string" },
                    "content": { "type": "string" },
                    "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "node_id": { "type": "string" },
                    "new_confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "new_content": { "type": "string" },
                    "source_id": { "type": "string" },
                    "target_id": { "type": "string" },
                    "edge_type": { "type": "string", "enum": ["supports", "contradicts", "supersedes", "derived_from", "applies_to"] },
                    "keep_id": { "type": "string" },
                    "remove_id": { "type": "string" }
                },
                "required": ["action"],
                "additionalProperties": false
            }
        }
    },
    "required": ["reasoning", "operations"],
    "additionalProperties": false
}"#;

/// Extended JSON schema that includes `claude_md_edits` for full_management mode.
/// Built programmatically from `ANALYSIS_RESPONSE_SCHEMA` to avoid duplication.
pub fn full_management_analysis_schema() -> String {
    let mut schema: serde_json::Value = serde_json::from_str(ANALYSIS_RESPONSE_SCHEMA)
        .expect("ANALYSIS_RESPONSE_SCHEMA must be valid JSON");

    let edits_schema: serde_json::Value = serde_json::json!({
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
    });

    schema["properties"]["claude_md_edits"] = edits_schema;

    serde_json::to_string_pretty(&schema).expect("schema serialization cannot fail")
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

/// Parse an AI response into a GraphOperation batch.
pub fn parse_graph_response(json: &str, default_project: Option<&str>) -> Result<Vec<GraphOperation>, CoreError> {
    parse_graph_response_full(json, default_project).map(|(_, ops)| ops)
}

/// Like `parse_graph_response`, but also returns the model's reasoning summary.
pub fn parse_graph_response_full(
    json: &str,
    default_project: Option<&str>,
) -> Result<(String, Vec<GraphOperation>), CoreError> {
    let response: GraphAnalysisResponse = serde_json::from_str(json)
        .map_err(|e| CoreError::Parse(format!("failed to parse graph analysis response: {e}")))?;

    let mut ops = Vec::new();
    for op_resp in &response.operations {
        match op_resp.action.as_str() {
            "create_node" => {
                let node_type = op_resp.node_type.as_deref()
                    .map(NodeType::from_str)
                    .unwrap_or(NodeType::Pattern);
                let scope = op_resp.scope.as_deref()
                    .map(NodeScope::from_str)
                    .unwrap_or(NodeScope::Project);
                let project_id = match scope {
                    NodeScope::Global => None,
                    NodeScope::Project => op_resp.project_id.clone()
                        .or_else(|| default_project.map(String::from)),
                };
                ops.push(GraphOperation::CreateNode {
                    node_type,
                    scope,
                    project_id,
                    content: op_resp.content.clone().unwrap_or_default(),
                    confidence: op_resp.confidence.unwrap_or(0.5),
                });
            }
            "update_node" => {
                if let Some(id) = &op_resp.node_id {
                    ops.push(GraphOperation::UpdateNode {
                        id: id.clone(),
                        confidence: op_resp.new_confidence,
                        content: op_resp.new_content.clone(),
                    });
                }
            }
            "create_edge" => {
                if let (Some(source), Some(target)) = (&op_resp.source_id, &op_resp.target_id) {
                    let edge_type = op_resp.edge_type.as_deref()
                        .and_then(EdgeType::from_str)
                        .unwrap_or(EdgeType::Supports);
                    ops.push(GraphOperation::CreateEdge {
                        source_id: source.clone(),
                        target_id: target.clone(),
                        edge_type,
                    });
                }
            }
            "merge_nodes" => {
                if let (Some(keep), Some(remove)) = (&op_resp.keep_id, &op_resp.remove_id) {
                    ops.push(GraphOperation::MergeNodes {
                        keep_id: keep.clone(),
                        remove_id: remove.clone(),
                    });
                }
            }
            _ => {} // Skip unknown actions
        }
    }
    Ok((response.reasoning, ops))
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

    #[test]
    fn test_graph_analysis_schema_is_valid_json() {
        let _: serde_json::Value = serde_json::from_str(GRAPH_ANALYSIS_RESPONSE_SCHEMA)
            .expect("schema must be valid JSON");
    }

    #[test]
    fn test_parse_graph_response() {
        let json = r#"{
            "reasoning": "Found testing pattern",
            "operations": [
                {
                    "action": "create_node",
                    "node_type": "rule",
                    "scope": "project",
                    "content": "Always run tests",
                    "confidence": 0.85
                },
                {
                    "action": "update_node",
                    "node_id": "existing-1",
                    "new_confidence": 0.9
                }
            ]
        }"#;
        let ops = parse_graph_response(json, Some("my-app")).unwrap();
        assert_eq!(ops.len(), 2);
        match &ops[0] {
            GraphOperation::CreateNode { content, scope, .. } => {
                assert_eq!(content, "Always run tests");
                assert_eq!(*scope, NodeScope::Project);
            }
            _ => panic!("Expected CreateNode"),
        }
        match &ops[1] {
            GraphOperation::UpdateNode { id, confidence, .. } => {
                assert_eq!(id, "existing-1");
                assert_eq!(*confidence, Some(0.9));
            }
            _ => panic!("Expected UpdateNode"),
        }
    }
}
