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

    /// Parse a node from markdown with strict frontmatter.
    /// Unknown keys are an error (catches human typos); the fixed
    /// schema is owned by this binary — migration controls format changes.
    /// Parsing normalizes on rewrite: CRLF becomes LF, confidence is written back with two decimals.
    pub fn from_markdown(content: &str) -> Result<Node, CoreError> {
        let content = content.replace("\r\n", "\n");
        let rest = content
            .strip_prefix("---\n")
            .ok_or_else(|| CoreError::Parse("missing frontmatter open delimiter".to_string()))?;
        let (front, body) = rest
            .split_once("\n---\n")
            .ok_or_else(|| CoreError::Parse("missing frontmatter close delimiter".to_string()))?;

        let mut id: Option<String> = None;
        let mut scope: Option<Scope> = None;
        let mut node_type: Option<NodeType> = None;
        let mut confidence: Option<f64> = None;
        let mut sources: Vec<String> = Vec::new();
        let mut created: Option<NaiveDate> = None;
        let mut updated: Option<NaiveDate> = None;
        let mut invalidated_by: Option<String> = None;
        let mut seen_keys: Vec<String> = Vec::new();

        for line in front.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let (key, value) = line
                .split_once(':')
                .ok_or_else(|| CoreError::Parse(format!("invalid frontmatter line: {line:?}")))?;
            let key = key.trim();
            let value = value.trim();
            if seen_keys.iter().any(|k| k == key) {
                return Err(CoreError::Parse(format!(
                    "duplicate frontmatter key: {key:?}"
                )));
            }
            seen_keys.push(key.to_string());
            match key {
                "id" => id = Some(value.to_string()),
                "scope" => scope = Some(Scope::parse(value)?),
                "type" => node_type = Some(NodeType::parse(value)?),
                "confidence" => {
                    let c: f64 = value
                        .parse()
                        .map_err(|_| CoreError::Parse(format!("invalid confidence: {value:?}")))?;
                    if !c.is_finite() || !(0.0..=1.0).contains(&c) {
                        return Err(CoreError::Parse(format!(
                            "confidence out of range [0.0, 1.0]: {value:?}"
                        )));
                    }
                    confidence = Some(c);
                }
                "sources" => {
                    let inner = value
                        .strip_prefix('[')
                        .and_then(|v| v.strip_suffix(']'))
                        .ok_or_else(|| {
                            CoreError::Parse(format!("invalid sources list: {value:?}"))
                        })?;
                    sources = inner
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect();
                }
                "created" => created = Some(parse_date(value)?),
                "updated" => updated = Some(parse_date(value)?),
                "invalidated_by" => {
                    invalidated_by = match value {
                        "null" | "" => None,
                        other => Some(other.to_string()),
                    }
                }
                other => {
                    return Err(CoreError::Parse(format!(
                        "unknown frontmatter key: {other:?}"
                    )));
                }
            }
        }

        let missing = |k: &str| CoreError::Parse(format!("missing frontmatter key: {k}"));
        Ok(Node {
            id: id.ok_or_else(|| missing("id"))?,
            scope: scope.ok_or_else(|| missing("scope"))?,
            node_type: node_type.ok_or_else(|| missing("type"))?,
            confidence: confidence.ok_or_else(|| missing("confidence"))?,
            sources,
            created: created.ok_or_else(|| missing("created"))?,
            updated: updated.ok_or_else(|| missing("updated"))?,
            invalidated_by,
            body: body.trim_end_matches('\n').to_string(),
        })
    }
}

fn parse_date(s: &str) -> Result<NaiveDate, CoreError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| CoreError::Parse(format!("invalid date: {s:?}")))
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

    #[test]
    fn from_markdown_roundtrip() {
        let n = sample_node();
        let parsed = Node::from_markdown(&n.to_markdown()).unwrap();
        assert_eq!(parsed, n);
    }

    #[test]
    fn from_markdown_roundtrip_with_invalidated_and_empty_sources() {
        let mut n = sample_node();
        n.sources = vec![];
        n.invalidated_by = Some("other".to_string());
        let parsed = Node::from_markdown(&n.to_markdown()).unwrap();
        assert_eq!(parsed, n);
    }

    #[test]
    fn from_markdown_body_may_contain_dashes() {
        let mut n = sample_node();
        n.body = "line one\n---\nline after a dash rule".to_string();
        let parsed = Node::from_markdown(&n.to_markdown()).unwrap();
        assert_eq!(parsed.body, n.body);
    }

    #[test]
    fn from_markdown_missing_required_key_errors() {
        let md = "---\nid: x\nscope: global\ntype: rule\n---\nbody\n";
        let err = Node::from_markdown(md).unwrap_err();
        assert!(err.to_string().contains("confidence"), "got: {err}");
    }

    #[test]
    fn from_markdown_unknown_key_errors() {
        let md = sample_node().to_markdown().replace("updated:", "updatedd:");
        let err = Node::from_markdown(&md).unwrap_err();
        assert!(err.to_string().contains("updatedd"), "got: {err}");
    }

    #[test]
    fn from_markdown_bad_values_error() {
        let base = sample_node().to_markdown();
        for (needle, replacement) in [
            ("confidence: 0.90", "confidence: high"),
            ("created: 2026-05-19", "created: yesterday"),
            ("type: rule", "type: law"),
            ("scope: project/my-api-service", "scope: team/x"),
        ] {
            let md = base.replace(needle, replacement);
            assert!(
                Node::from_markdown(&md).is_err(),
                "should fail: {replacement}"
            );
        }
    }

    #[test]
    fn from_markdown_requires_frontmatter_delimiters() {
        assert!(Node::from_markdown("no frontmatter here").is_err());
        assert!(Node::from_markdown("---\nid: x\nno closing delimiter").is_err());
    }

    #[test]
    fn from_markdown_accepts_crlf_line_endings() {
        let crlf = sample_node().to_markdown().replace('\n', "\r\n");
        let parsed = Node::from_markdown(&crlf).unwrap();
        assert_eq!(parsed.id, "ab-paired-observations");
    }

    #[test]
    fn from_markdown_duplicate_key_errors() {
        let md = sample_node()
            .to_markdown()
            .replace("type: rule\n", "type: rule\ntype: pattern\n");
        let err = Node::from_markdown(&md).unwrap_err();
        assert!(err.to_string().contains("duplicate"), "got: {err}");
    }

    #[test]
    fn from_markdown_confidence_out_of_range_errors() {
        for bad in ["1.5", "-0.1", "NaN", "inf"] {
            let md = sample_node()
                .to_markdown()
                .replace("confidence: 0.90", &format!("confidence: {bad}"));
            assert!(Node::from_markdown(&md).is_err(), "should fail: {bad}");
        }
    }
}
