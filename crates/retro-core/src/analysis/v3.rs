//! v3 analysis: reuse the v2 engine (compact sessions, graph prompt, schema,
//! response parsing) and apply the resulting GraphOperations to the markdown
//! store instead of SQLite.

use chrono::Utc;

use crate::analysis::backend::AnalysisBackend;
use crate::analysis::{GRAPH_ANALYSIS_RESPONSE_SCHEMA, parse_graph_response, prompts};
use crate::errors::CoreError;
use crate::models::{
    EdgeType, GraphOperation, KnowledgeNode, NodeScope, NodeStatus, NodeType as V2NodeType, Session,
};
use crate::store::{Node, NodeType, Scope, Store};

/// Result of one v3 analysis batch.
#[derive(Debug, Default)]
pub struct V3AnalyzeResult {
    pub sessions_analyzed: usize,
    pub nodes_created: usize,
    pub nodes_updated: usize,
    pub nodes_merged: usize,
    pub nodes_invalidated: usize,
    pub edges_ignored: usize,
    pub reasoning: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Bodies of nodes created/updated — for briefing notifications.
    pub learned: Vec<String>,
}

/// Shim: present a v3 store node to the v2 prompt builder. Only id, content,
/// confidence, type, and scope influence the prompt (content truncated to
/// 200 chars there); the rest are placeholders.
fn shim(node: &Node) -> KnowledgeNode {
    KnowledgeNode {
        id: node.id.clone(),
        node_type: match node.node_type {
            NodeType::Rule => V2NodeType::Rule,
            NodeType::Preference => V2NodeType::Preference,
            NodeType::Pattern => V2NodeType::Pattern,
            NodeType::Memory => V2NodeType::Memory,
        },
        scope: match &node.scope {
            Scope::Global => NodeScope::Global,
            Scope::Project(_) => NodeScope::Project,
        },
        project_id: match &node.scope {
            Scope::Global => None,
            Scope::Project(slug) => Some(slug.clone()),
        },
        content: node.body.clone(),
        confidence: node.confidence,
        status: NodeStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        projected_at: None,
        pr_url: None,
    }
}

fn v3_node_type(t: &V2NodeType) -> NodeType {
    match t {
        V2NodeType::Rule | V2NodeType::Directive => NodeType::Rule,
        V2NodeType::Preference => NodeType::Preference,
        V2NodeType::Pattern | V2NodeType::Skill => NodeType::Pattern,
        V2NodeType::Memory => NodeType::Memory,
    }
}

fn union_sources(existing: &mut Vec<String>, extra: &[String]) {
    for s in extra {
        if !existing.contains(s) {
            existing.push(s.clone());
        }
    }
}

/// Analyze one batch of parsed sessions against the store and apply the
/// resulting operations. `project_slug` scopes project-level operations.
/// Caller is responsible for: session filtering by project, scrubbing,
/// budget accounting (one backend call per invocation), and committing.
pub fn analyze_sessions(
    store: &Store,
    backend: &dyn AnalysisBackend,
    sessions: &[Session],
    project_slug: Option<&str>,
) -> Result<V3AnalyzeResult, CoreError> {
    let mut result = V3AnalyzeResult::default();

    // Low-signal filter (same rule as v2: < 2 user messages = skip).
    let signal: Vec<&Session> = sessions
        .iter()
        .filter(|s| s.user_messages.len() >= 2)
        .collect();
    if signal.is_empty() {
        return Ok(result);
    }
    result.sessions_analyzed = signal.len();
    let session_sources: Vec<String> = signal
        .iter()
        .map(|s| format!("session:{}", s.session_id))
        .collect();

    // Existing-node context: active nodes for global + this project's scope.
    let loaded = store.load_all()?;
    let context: Vec<KnowledgeNode> = loaded
        .nodes
        .iter()
        .map(|(_, n)| n)
        .filter(|n| n.is_active())
        .filter(|n| match (&n.scope, project_slug) {
            (Scope::Global, _) => true,
            (Scope::Project(slug), Some(p)) => slug == p,
            (Scope::Project(_), None) => false,
        })
        .map(shim)
        .collect();

    let compact: Vec<_> = signal
        .iter()
        .map(|s| prompts::to_compact_session(s))
        .collect();
    let prompt = prompts::build_graph_analysis_prompt(&compact, &context, project_slug);
    let response = backend.execute(&prompt, Some(GRAPH_ANALYSIS_RESPONSE_SCHEMA))?;
    result.input_tokens = response.input_tokens;
    result.output_tokens = response.output_tokens;

    let operations = parse_graph_response(&response.text, project_slug)?;
    let today = Utc::now().date_naive();

    for op in operations {
        match op {
            GraphOperation::CreateNode {
                node_type,
                scope,
                project_id,
                content,
                confidence,
            } => {
                let v3_scope = match scope {
                    NodeScope::Global => Scope::Global,
                    NodeScope::Project => {
                        match project_id.or_else(|| project_slug.map(String::from)) {
                            Some(slug) => Scope::Project(slug),
                            None => Scope::Global,
                        }
                    }
                };
                let id = store.unique_slug(
                    &content
                        .split_whitespace()
                        .take(8)
                        .collect::<Vec<_>>()
                        .join(" "),
                    &v3_scope,
                );
                let node = Node {
                    id,
                    scope: v3_scope,
                    node_type: v3_node_type(&node_type),
                    confidence: confidence.clamp(0.0, 1.0),
                    sources: session_sources.clone(),
                    created: today,
                    updated: today,
                    invalidated_by: None,
                    body: content.trim().to_string(),
                };
                store.write_node(&node)?;
                result.learned.push(node.body.clone());
                result.nodes_created += 1;
            }
            GraphOperation::UpdateNode {
                id,
                confidence,
                content,
            } => {
                let found = find_node(store, &id, project_slug)?;
                let Some((scope, mut node)) = found else {
                    continue;
                };
                if let Some(c) = confidence {
                    node.confidence = c.clamp(0.0, 1.0);
                }
                if let Some(c) = content {
                    node.body = c.trim().to_string();
                }
                union_sources(&mut node.sources, &session_sources);
                node.updated = today;
                node.scope = scope;
                store.write_node(&node)?;
                result.learned.push(node.body.clone());
                result.nodes_updated += 1;
            }
            GraphOperation::MergeNodes { keep_id, remove_id } => {
                let keep = find_node(store, &keep_id, project_slug)?;
                let removed = find_node(store, &remove_id, project_slug)?;
                let (Some((keep_scope, mut keep_node)), Some((remove_scope, remove_node))) =
                    (keep, removed)
                else {
                    continue;
                };
                union_sources(&mut keep_node.sources, &remove_node.sources);
                union_sources(&mut keep_node.sources, &session_sources);
                keep_node.updated = today;
                keep_node.scope = keep_scope;
                store.write_node(&keep_node)?;
                store.invalidate(&remove_scope, &remove_node.id, &keep_node.id)?;
                result.nodes_merged += 1;
            }
            GraphOperation::CreateEdge {
                source_id,
                target_id,
                edge_type,
            } => {
                if edge_type == EdgeType::Supersedes {
                    if let Some((scope, node)) = find_node(store, &target_id, project_slug)? {
                        store.invalidate(&scope, &node.id, &source_id)?;
                        result.nodes_invalidated += 1;
                    }
                } else {
                    // v3 stores no edges; supports/contradicts/derived_from/applies_to
                    // are counted for the summary and dropped.
                    result.edges_ignored += 1;
                }
            }
        }
    }
    Ok(result)
}

/// Resolve an operation's node id: try the batch's project scope first, then global.
fn find_node(
    store: &Store,
    id: &str,
    project_slug: Option<&str>,
) -> Result<Option<(Scope, Node)>, CoreError> {
    if let Some(slug) = project_slug {
        let scope = Scope::Project(slug.to_string());
        if let Some(node) = store.get(&scope, id)? {
            return Ok(Some((scope, node)));
        }
    }
    let scope = Scope::Global;
    if let Some(node) = store.get(&scope, id)? {
        return Ok(Some((scope, node)));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::backend::MockBackend;
    use crate::models::{ParsedUserMessage, SessionMetadata};
    use tempfile::TempDir;

    fn session(id: &str, msgs: &[&str]) -> Session {
        Session {
            session_id: id.to_string(),
            project: "/tmp/proj".to_string(),
            session_path: format!("/tmp/{id}.jsonl"),
            user_messages: msgs
                .iter()
                .map(|m| ParsedUserMessage {
                    text: m.to_string(),
                    timestamp: None,
                })
                .collect(),
            assistant_messages: vec![],
            summaries: vec![],
            tools_used: vec![],
            errors: vec![],
            metadata: SessionMetadata {
                cwd: None,
                version: None,
                git_branch: None,
                model: None,
            },
        }
    }

    fn store() -> (TempDir, Store) {
        let tmp = TempDir::new().unwrap();
        let s = Store::open(tmp.path());
        s.ensure_layout().unwrap();
        (tmp, s)
    }

    #[test]
    fn create_node_writes_to_store_with_sources_and_clamped_confidence() {
        let (_tmp, store) = store();
        let response = r#"{"reasoning":"saw a rule","operations":[
            {"action":"create_node","node_type":"rule","scope":"project","content":"Always run smoke tests before full runs.","confidence":1.7}
        ]}"#;
        let backend = MockBackend::with_responses(vec![response.to_string()]);
        let result = analyze_sessions(
            &store,
            &backend,
            &[session("s1", &["please smoke test first", "ok run it"])],
            Some("my-proj"),
        )
        .unwrap();
        assert_eq!(result.nodes_created, 1);
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.nodes.len(), 1);
        let node = &loaded.nodes[0].1;
        assert_eq!(node.scope, Scope::Project("my-proj".to_string()));
        assert_eq!(node.node_type, NodeType::Rule);
        assert!((node.confidence - 1.0).abs() < f64::EPSILON, "clamped");
        assert_eq!(node.sources, vec!["session:s1".to_string()]);
        assert!(node.body.contains("smoke tests"));
    }

    #[test]
    fn update_and_merge_operations_mutate_existing_nodes() {
        let (_tmp, store) = store();
        let today = chrono::Utc::now().date_naive();
        let mk = |id: &str, conf: f64| Node {
            id: id.to_string(),
            scope: Scope::Global,
            node_type: NodeType::Rule,
            confidence: conf,
            sources: vec!["session:old".to_string()],
            created: today,
            updated: today,
            invalidated_by: None,
            body: format!("rule body {id}"),
        };
        store.write_node(&mk("keeper", 0.5)).unwrap();
        store.write_node(&mk("duplicate", 0.4)).unwrap();

        let response = r#"{"reasoning":"recurrence + dup","operations":[
            {"action":"update_node","node_id":"keeper","new_confidence":0.8},
            {"action":"merge_nodes","keep_id":"keeper","remove_id":"duplicate"}
        ]}"#;
        let backend = MockBackend::with_responses(vec![response.to_string()]);
        let result = analyze_sessions(
            &store,
            &backend,
            &[session("s2", &["msg one", "msg two"])],
            None,
        )
        .unwrap();
        assert_eq!(result.nodes_updated, 1);
        assert_eq!(result.nodes_merged, 1);

        let keeper = store.get(&Scope::Global, "keeper").unwrap().unwrap();
        assert!((keeper.confidence - 0.8).abs() < f64::EPSILON);
        assert!(keeper.sources.contains(&"session:s2".to_string()));
        assert!(
            keeper.sources.contains(&"session:old".to_string()),
            "merge unions sources"
        );

        let dup = store.get(&Scope::Global, "duplicate").unwrap().unwrap();
        assert_eq!(dup.invalidated_by.as_deref(), Some("keeper"));
    }

    #[test]
    fn supersedes_edge_invalidates_target_other_edges_ignored() {
        let (_tmp, store) = store();
        let today = chrono::Utc::now().date_naive();
        let mk = |id: &str| Node {
            id: id.to_string(),
            scope: Scope::Global,
            node_type: NodeType::Rule,
            confidence: 0.7,
            sources: vec![],
            created: today,
            updated: today,
            invalidated_by: None,
            body: format!("body {id}"),
        };
        store.write_node(&mk("new-way")).unwrap();
        store.write_node(&mk("old-way")).unwrap();

        let response = r#"{"reasoning":"contradiction resolved","operations":[
            {"action":"create_edge","source_id":"new-way","target_id":"old-way","edge_type":"supersedes"},
            {"action":"create_edge","source_id":"new-way","target_id":"old-way","edge_type":"supports"}
        ]}"#;
        let backend = MockBackend::with_responses(vec![response.to_string()]);
        let result =
            analyze_sessions(&store, &backend, &[session("s3", &["a", "b"])], None).unwrap();
        assert_eq!(result.nodes_invalidated, 1);
        assert_eq!(result.edges_ignored, 1);
        let old = store.get(&Scope::Global, "old-way").unwrap().unwrap();
        assert_eq!(old.invalidated_by.as_deref(), Some("new-way"));
    }

    #[test]
    fn low_signal_sessions_are_filtered_before_any_ai_call() {
        let (_tmp, store) = store();
        let backend = MockBackend::with_responses(vec![]); // any AI call would error
        let result = analyze_sessions(
            &store,
            &backend,
            &[session("tiny", &["single message"])],
            None,
        )
        .unwrap();
        assert_eq!(result.sessions_analyzed, 0);
        assert_eq!(result.nodes_created, 0);
    }

    #[test]
    fn existing_nodes_appear_in_prompt_context() {
        let (_tmp, store) = store();
        let today = chrono::Utc::now().date_naive();
        store
            .write_node(&Node {
                id: "existing-rule".to_string(),
                scope: Scope::Global,
                node_type: NodeType::Rule,
                confidence: 0.9,
                sources: vec![],
                created: today,
                updated: today,
                invalidated_by: None,
                body: "a very distinctive existing rule body".to_string(),
            })
            .unwrap();
        let response = r#"{"reasoning":"nothing new","operations":[]}"#;
        let backend = MockBackend::with_responses(vec![response.to_string()]);
        analyze_sessions(&store, &backend, &[session("s4", &["a", "b"])], None).unwrap();
        let prompts = backend.prompts_seen.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(
            prompts[0].contains("existing-rule"),
            "prompt shows store node ids"
        );
    }
}
