pub mod claude_code;

use retro_core::analysis::backend::AnalysisBackend;
use retro_core::errors::CoreError;
use retro_core::models::{KnowledgeNode, KnowledgeProject};
use std::path::PathBuf;

/// How a projected file should be written.
#[derive(Debug, Clone, PartialEq)]
pub enum WriteStrategy {
    /// Append content to existing file.
    Append,
    /// Replace content within managed delimiters.
    ReplaceSection,
    /// Overwrite the entire file.
    FullRewrite,
}

/// A file to be written by a projector.
#[derive(Debug, Clone)]
pub struct ProjectedFile {
    pub path: PathBuf,
    pub content: String,
    pub strategy: WriteStrategy,
}

/// Agent-agnostic trait for translating knowledge into agent-specific files.
pub trait Projector {
    /// Unique name for this projector (e.g., "claude_code").
    fn name(&self) -> &str;

    /// Whether this projector can handle a given node type.
    fn can_project(&self, node: &KnowledgeNode) -> bool;

    /// Generate output files from a set of knowledge nodes.
    fn project(
        &self,
        nodes: &[KnowledgeNode],
        project: &KnowledgeProject,
        backend: &dyn AnalysisBackend,
    ) -> Result<Vec<ProjectedFile>, CoreError>;
}
