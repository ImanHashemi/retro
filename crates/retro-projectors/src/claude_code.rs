use crate::{ProjectedFile, Projector};
use retro_core::analysis::backend::AnalysisBackend;
use retro_core::errors::CoreError;
use retro_core::models::{KnowledgeNode, KnowledgeProject, NodeType};

/// Claude Code projector — generates CLAUDE.md rules and skills.
pub struct ClaudeCodeProjector;

impl Projector for ClaudeCodeProjector {
    fn name(&self) -> &str {
        "claude_code"
    }

    fn can_project(&self, node: &KnowledgeNode) -> bool {
        // Memory nodes are context-only, not projected
        !matches!(node.node_type, NodeType::Memory)
    }

    fn project(
        &self,
        _nodes: &[KnowledgeNode],
        _project: &KnowledgeProject,
        _backend: &dyn AnalysisBackend,
    ) -> Result<Vec<ProjectedFile>, CoreError> {
        // Shell implementation — full logic comes in Plan 2
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use retro_core::models::{NodeScope, NodeStatus};
    use chrono::Utc;

    #[test]
    fn test_can_project_excludes_memory() {
        let projector = ClaudeCodeProjector;
        let now = Utc::now();

        let rule = KnowledgeNode {
            id: "n1".to_string(), node_type: NodeType::Rule,
            scope: NodeScope::Global, project_id: None,
            content: "test".to_string(), confidence: 0.8,
            status: NodeStatus::Active, created_at: now, updated_at: now,
        };
        assert!(projector.can_project(&rule));

        let memory = KnowledgeNode {
            id: "n2".to_string(), node_type: NodeType::Memory,
            scope: NodeScope::Project, project_id: Some("app".to_string()),
            content: "context".to_string(), confidence: 0.5,
            status: NodeStatus::Active, created_at: now, updated_at: now,
        };
        assert!(!projector.can_project(&memory));
    }

    #[test]
    fn test_projector_name() {
        let projector = ClaudeCodeProjector;
        assert_eq!(projector.name(), "claude_code");
    }
}
