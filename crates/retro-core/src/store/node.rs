//! Knowledge node: the atomic unit of the v3 store.
//! One node == one markdown file with strict frontmatter.

use chrono::NaiveDate;

use crate::errors::CoreError;

/// Node type. v3 collapses v2's six types to four
/// (`directive` → `rule`, `skill` → `pattern`, handled at migration).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    Rule,
    Preference,
    Pattern,
    /// Context-only: stored and browsable, never projected.
    Memory,
}

impl NodeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeType::Rule => "rule",
            NodeType::Preference => "preference",
            NodeType::Pattern => "pattern",
            NodeType::Memory => "memory",
        }
    }

    pub fn parse(s: &str) -> Result<Self, CoreError> {
        match s {
            "rule" => Ok(NodeType::Rule),
            "preference" => Ok(NodeType::Preference),
            "pattern" => Ok(NodeType::Pattern),
            "memory" => Ok(NodeType::Memory),
            other => Err(CoreError::Parse(format!("unknown node type: {other:?}"))),
        }
    }
}

/// Where a node applies: everywhere, or one project (by slug).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project(String),
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Scope::Global => write!(f, "global"),
            Scope::Project(slug) => write!(f, "project/{slug}"),
        }
    }
}

impl Scope {
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        if s == "global" {
            return Ok(Scope::Global);
        }
        if let Some(slug) = s.strip_prefix("project/") {
            if !slug.is_empty() {
                return Ok(Scope::Project(slug.to_string()));
            }
        }
        Err(CoreError::Parse(format!("invalid scope: {s:?}")))
    }
}

/// One knowledge node. Serialized as one markdown file:
/// strict frontmatter between `---` delimiters, then the body.
/// The body is stored WITHOUT a trailing newline; `to_markdown`
/// appends exactly one (normalization keeps round-trips stable).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub scope: Scope,
    pub node_type: NodeType,
    pub confidence: f64,
    pub sources: Vec<String>,
    pub created: NaiveDate,
    pub updated: NaiveDate,
    pub invalidated_by: Option<String>,
    pub body: String,
}

impl Node {
    pub fn is_active(&self) -> bool {
        self.invalidated_by.is_none()
    }

    pub fn to_markdown(&self) -> String {
        // NOTE: source IDs must not contain commas (comma-joined list format).
        let sources = self.sources.join(", ");
        let invalidated = self.invalidated_by.as_deref().unwrap_or("null");
        format!(
            "---\nid: {}\nscope: {}\ntype: {}\nconfidence: {:.2}\nsources: [{}]\ncreated: {}\nupdated: {}\ninvalidated_by: {}\n---\n{}\n",
            self.id,
            self.scope,
            self.node_type.as_str(),
            self.confidence,
            sources,
            self.created,
            self.updated,
            invalidated,
            self.body.trim_end_matches('\n'),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_node() -> Node {
        Node {
            id: "ab-paired-observations".to_string(),
            scope: Scope::Project("my-api-service".to_string()),
            node_type: NodeType::Rule,
            confidence: 0.9,
            sources: vec![
                "session:1a2b3c4d".to_string(),
                "session:5e6f7a8b".to_string(),
            ],
            created: NaiveDate::from_ymd_opt(2026, 5, 19).unwrap(),
            updated: NaiveDate::from_ymd_opt(2026, 6, 2).unwrap(),
            invalidated_by: None,
            body: "A/B comparisons must always use paired observations.\n\n**Why:** Unpaired comparisons mix traffic distributions.".to_string(),
        }
    }

    #[test]
    fn scope_roundtrip() {
        assert_eq!(Scope::Global.to_string(), "global");
        assert_eq!(
            Scope::Project("my-api".to_string()).to_string(),
            "project/my-api"
        );
        assert_eq!(Scope::parse("global").unwrap(), Scope::Global);
        assert_eq!(
            Scope::parse("project/my-api").unwrap(),
            Scope::Project("my-api".to_string())
        );
        assert!(Scope::parse("team/my-api").is_err());
        assert!(Scope::parse("project/").is_err());
        assert!(Scope::parse("").is_err());
    }

    #[test]
    fn node_type_roundtrip() {
        for (t, s) in [
            (NodeType::Rule, "rule"),
            (NodeType::Preference, "preference"),
            (NodeType::Pattern, "pattern"),
            (NodeType::Memory, "memory"),
        ] {
            assert_eq!(t.as_str(), s);
            assert_eq!(NodeType::parse(s).unwrap(), t);
        }
        assert!(NodeType::parse("skill").is_err());
        assert!(NodeType::parse("").is_err());
    }

    #[test]
    fn is_active_reflects_invalidated_by() {
        let mut n = sample_node();
        assert!(n.is_active());
        n.invalidated_by = Some("newer-rule".to_string());
        assert!(!n.is_active());
    }

    #[test]
    fn to_markdown_emits_fixed_frontmatter_order() {
        let md = sample_node().to_markdown();
        let expected = "\
---
id: ab-paired-observations
scope: project/my-api-service
type: rule
confidence: 0.90
sources: [session:1a2b3c4d, session:5e6f7a8b]
created: 2026-05-19
updated: 2026-06-02
invalidated_by: null
---
A/B comparisons must always use paired observations.

**Why:** Unpaired comparisons mix traffic distributions.
";
        assert_eq!(md, expected);
    }

    #[test]
    fn to_markdown_empty_sources_and_invalidated() {
        let mut n = sample_node();
        n.sources = vec![];
        n.invalidated_by = Some("other-node".to_string());
        let md = n.to_markdown();
        assert!(md.contains("sources: []\n"));
        assert!(md.contains("invalidated_by: other-node\n"));
    }

    #[test]
    fn to_markdown_empty_body_single_trailing_newline() {
        let mut n = sample_node();
        n.body = String::new();
        assert!(n.to_markdown().ends_with("---\n\n"));
    }
}
