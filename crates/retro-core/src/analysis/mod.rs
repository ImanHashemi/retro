pub mod backend;
pub mod claude_cli;
pub mod prompts;
pub mod v3;

use crate::errors::CoreError;
use crate::models::{EdgeType, GraphAnalysisResponse, GraphOperation, NodeScope, NodeType};

pub const BATCH_SIZE: usize = 20;

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

#[cfg(test)]
mod tests {
    use super::*;

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
