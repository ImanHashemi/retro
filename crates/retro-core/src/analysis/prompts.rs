use crate::models::{CompactSession, CompactUserMessage, KnowledgeNode, Session};

const MAX_USER_MSG_LEN: usize = 500;
const MAX_USER_MSGS_PER_SESSION: usize = 300;

/// Build the v2 analysis prompt with graph context and scope classification instructions.
pub fn build_graph_analysis_prompt(
    sessions: &[CompactSession],
    existing_nodes: &[KnowledgeNode],
    project: Option<&str>,
) -> String {
    let mut prompt = String::new();

    prompt.push_str("You are analyzing coding session transcripts to discover patterns, rules, preferences, and skills.\n\n");

    prompt.push_str("## Scope Classification\n\n");
    prompt.push_str("For each piece of knowledge, classify its scope:\n");
    prompt.push_str("- **global**: Personal style, communication preferences, general coding habits (e.g., 'always use snake_case', 'prefer concise responses')\n");
    prompt.push_str("- **project**: Code-specific conventions, architecture decisions, project tooling (e.g., 'this project uses SQLite WAL mode', 'run cargo test before committing')\n");
    prompt.push_str("- When ambiguous, default to **project**\n\n");

    prompt.push_str("## Node Types\n\n");
    prompt.push_str("- **preference**: How the user likes things done\n");
    prompt.push_str("- **pattern**: Observed recurring behavior\n");
    prompt.push_str("- **rule**: An explicit directive from the user\n");
    prompt.push_str("- **skill**: A reusable capability or workflow\n");
    prompt.push_str("- **memory**: Factual context about the project or user\n");
    prompt.push_str("- **directive**: Strong instruction ('always'/'never'/'must')\n\n");

    // Include existing knowledge for dedup and relationship detection
    if !existing_nodes.is_empty() {
        prompt.push_str("## Existing Knowledge\n\n");
        for node in existing_nodes.iter().take(50) {
            prompt.push_str(&format!(
                "- [{}] {} ({}) conf={:.2}: {}\n",
                node.id,
                node.node_type,
                node.scope,
                node.confidence,
                crate::util::truncate_str(&node.content, 200),
            ));
        }
        prompt.push_str("\n");
        prompt.push_str("If a session reinforces existing knowledge, emit an update_node with higher confidence.\n");
        prompt.push_str("If new knowledge contradicts existing, note it but still create the new node.\n");
        prompt.push_str("If new knowledge is semantically identical to existing, emit merge_nodes.\n\n");
    }

    // Include sessions
    prompt.push_str("## Sessions to Analyze\n\n");
    let sessions_json = serde_json::to_string_pretty(&sessions).unwrap_or_default();
    prompt.push_str(&sessions_json);
    prompt.push_str("\n\n");

    if let Some(proj) = project {
        prompt.push_str(&format!("Current project: {proj}\n\n"));
    }

    prompt.push_str("## Instructions\n\n");
    prompt.push_str("Analyze these sessions and emit graph operations:\n");
    prompt.push_str("- create_node: New knowledge discovered\n");
    prompt.push_str("- update_node: Existing knowledge reinforced (bump confidence)\n");
    prompt.push_str("- create_edge: Relationship between nodes (supports, derived_from)\n");
    prompt.push_str("- merge_nodes: Duplicate knowledge detected\n\n");
    prompt.push_str("Be selective. Only emit operations for clear, actionable knowledge. Prefer fewer high-quality nodes over many weak ones.\n");
    prompt.push_str("Explicit user directives ('always', 'never', 'must') get confidence 0.7-0.85.\n");
    prompt.push_str("Single-session observations get confidence 0.4-0.5.\n");

    prompt
}

pub fn to_compact_session(session: &Session) -> CompactSession {
    let user_messages: Vec<CompactUserMessage> = session
        .user_messages
        .iter()
        .take(MAX_USER_MSGS_PER_SESSION)
        .map(|m| CompactUserMessage {
            text: truncate_str(&m.text, MAX_USER_MSG_LEN),
            timestamp: m.timestamp.clone(),
        })
        .collect();

    let thinking_highlights: Vec<String> = session
        .assistant_messages
        .iter()
        .filter_map(|m| m.thinking_summary.clone())
        .collect();

    CompactSession {
        session_id: session.session_id.clone(),
        project: session.project.clone(),
        user_messages,
        tools_used: session.tools_used.clone(),
        errors: session.errors.clone(),
        thinking_highlights,
        summaries: session.summaries.clone(),
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    // Find valid UTF-8 boundary
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    format!("{}...", &s[..i])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, texts: &[&str]) -> Session {
        Session {
            session_id: id.to_string(),
            project: "/test".to_string(),
            session_path: format!("/test/{id}.jsonl"),
            user_messages: texts
                .iter()
                .map(|t| crate::models::ParsedUserMessage {
                    text: t.to_string(),
                    timestamp: None,
                })
                .collect(),
            assistant_messages: vec![],
            summaries: vec![],
            tools_used: vec![],
            errors: vec![],
            metadata: crate::models::SessionMetadata {
                cwd: None,
                version: None,
                git_branch: None,
                model: None,
            },
        }
    }

    #[test]
    fn test_to_compact_session_truncates_and_caps() {
        let long_text = "x".repeat(MAX_USER_MSG_LEN + 50);
        let s = session("sess-1", &[&long_text]);
        let compact = to_compact_session(&s);
        assert_eq!(compact.session_id, "sess-1");
        assert_eq!(compact.user_messages.len(), 1);
        assert!(compact.user_messages[0].text.ends_with("..."));
        assert!(compact.user_messages[0].text.len() <= MAX_USER_MSG_LEN + 3);
    }

    #[test]
    fn test_build_graph_analysis_prompt_includes_sessions_and_context() {
        let compact = vec![to_compact_session(&session("sess-1", &["please add tests"]))];
        let nodes = vec![KnowledgeNode {
            id: "existing-rule".to_string(),
            node_type: crate::models::NodeType::Rule,
            scope: crate::models::NodeScope::Global,
            content: "Always run tests".to_string(),
            confidence: 0.8,
        }];
        let prompt = build_graph_analysis_prompt(&compact, &nodes, Some("my-app"));
        assert!(prompt.contains("existing-rule"));
        assert!(prompt.contains("please add tests"));
        assert!(prompt.contains("Current project: my-app"));
    }

    #[test]
    fn test_build_graph_analysis_prompt_no_existing_nodes() {
        let compact = vec![to_compact_session(&session("sess-1", &["hello"]))];
        let prompt = build_graph_analysis_prompt(&compact, &[], None);
        assert!(!prompt.contains("## Existing Knowledge"));
    }
}
