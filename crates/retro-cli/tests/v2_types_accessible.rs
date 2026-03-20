use retro_core::models::{
    KnowledgeNode, KnowledgeEdge, KnowledgeProject,
    NodeType, NodeScope, NodeStatus, EdgeType, GraphOperation,
};
use retro_core::config::{RunnerConfig, TrustConfig, KnowledgeConfig};

#[test]
fn test_v2_types_are_exported() {
    // This test just verifies the types compile and are accessible
    let _ = NodeType::Rule;
    let _ = NodeScope::Global;
    let _ = NodeStatus::Active;
    let _ = EdgeType::Supports;
}
