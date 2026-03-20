use crate::{ProjectedFile, Projector, WriteStrategy};
use retro_core::analysis::backend::AnalysisBackend;
use retro_core::errors::CoreError;
use retro_core::models::{KnowledgeNode, KnowledgeProject, NodeType};
use std::path::PathBuf;

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
        nodes: &[KnowledgeNode],
        project: &KnowledgeProject,
        _backend: &dyn AnalysisBackend,
    ) -> Result<Vec<ProjectedFile>, CoreError> {
        let mut files = Vec::new();
        files.extend(project_rules_to_claude_md(nodes, project));
        Ok(files)
    }
}

/// Generate CLAUDE.md managed section content from rule and directive nodes.
pub fn project_rules_to_claude_md(nodes: &[KnowledgeNode], project: &KnowledgeProject) -> Vec<ProjectedFile> {
    let rules: Vec<&KnowledgeNode> = nodes.iter()
        .filter(|n| matches!(n.node_type, NodeType::Rule | NodeType::Directive | NodeType::Preference | NodeType::Pattern))
        .collect();

    if rules.is_empty() {
        return Vec::new();
    }

    let mut content = String::new();
    content.push_str("<!-- retro:managed:start -->\n");
    for rule in &rules {
        content.push_str(&format!("- {}\n", rule.content));
    }
    content.push_str("<!-- retro:managed:end -->\n");

    vec![ProjectedFile {
        path: PathBuf::from(&project.path).join("CLAUDE.md"),
        content,
        strategy: WriteStrategy::ReplaceSection,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use retro_core::models::{NodeScope, NodeStatus};
    use chrono::Utc;

    fn make_node(id: &str, node_type: NodeType, scope: NodeScope, content: &str, confidence: f64) -> KnowledgeNode {
        let project_id = if scope == NodeScope::Project { Some("my-app".to_string()) } else { None };
        KnowledgeNode {
            id: id.to_string(),
            node_type,
            scope,
            project_id,
            content: content.to_string(),
            confidence,
            status: NodeStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn test_can_project_excludes_memory() {
        let projector = ClaudeCodeProjector;
        assert!(projector.can_project(&make_node("n1", NodeType::Rule, NodeScope::Global, "test", 0.8)));
        assert!(!projector.can_project(&make_node("n2", NodeType::Memory, NodeScope::Project, "ctx", 0.5)));
    }

    #[test]
    fn test_projector_name() {
        let projector = ClaudeCodeProjector;
        assert_eq!(projector.name(), "claude_code");
    }

    #[test]
    fn test_project_rules_to_claude_md() {
        let nodes = vec![
            make_node("r1", NodeType::Rule, NodeScope::Project, "Always run tests before committing", 0.85),
            make_node("r2", NodeType::Directive, NodeScope::Project, "Never use unwrap in production code", 0.9),
            make_node("m1", NodeType::Memory, NodeScope::Project, "Uses SQLite", 0.5),
        ];
        let project = KnowledgeProject {
            id: "my-app".to_string(),
            path: "/tmp/test-project".to_string(),
            remote_url: None,
            agent_type: "claude_code".to_string(),
            last_seen: Utc::now(),
        };
        let files = project_rules_to_claude_md(&nodes, &project);
        assert_eq!(files.len(), 1);
        assert!(files[0].path.to_str().unwrap().contains("CLAUDE.md"));
        assert!(files[0].content.contains("Always run tests"));
        assert!(files[0].content.contains("Never use unwrap"));
        assert!(!files[0].content.contains("Uses SQLite")); // Memory excluded
        assert_eq!(files[0].strategy, WriteStrategy::ReplaceSection);
    }

    #[test]
    fn test_project_no_rules_no_output() {
        let nodes = vec![
            make_node("m1", NodeType::Memory, NodeScope::Project, "Uses SQLite", 0.5),
        ];
        let project = KnowledgeProject {
            id: "my-app".to_string(),
            path: "/tmp/test".to_string(),
            remote_url: None,
            agent_type: "claude_code".to_string(),
            last_seen: Utc::now(),
        };
        let files = project_rules_to_claude_md(&nodes, &project);
        assert!(files.is_empty());
    }
}
