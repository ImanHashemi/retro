use crate::db;
use crate::errors::CoreError;
use crate::models::{KnowledgeNode, NodeScope, NodeStatus, NodeType};
use crate::projection::claude_md::read_managed_section;
use chrono::Utc;
use rusqlite::Connection;
use std::collections::HashSet;

#[derive(Debug, Default)]
pub struct ReconcileResult {
    pub imported: usize,
    pub archived: usize,
}

/// Given rules from a CLAUDE.md file and the DB state for a scope, compute imports and archives.
///
/// - Import: rules present in the file but not in the DB → insert as new Active Rule nodes.
/// - Archive: DB nodes whose content is no longer in the file → mark as Archived.
pub fn reconcile_for_scope(
    conn: &Connection,
    scope: &NodeScope,
    project_id: Option<&str>,
    file_rules: &[String],
) -> Result<ReconcileResult, CoreError> {
    let db_nodes = db::get_projected_nodes_for_scope(conn, scope, project_id)?;

    let db_contents: HashSet<&str> = db_nodes.iter().map(|n| n.content.as_str()).collect();
    let file_set: HashSet<&str> = file_rules.iter().map(|r| r.as_str()).collect();

    let mut result = ReconcileResult::default();

    // Import: rules in file not in DB
    for rule in file_rules {
        if !db_contents.contains(rule.as_str()) {
            let node = KnowledgeNode {
                id: uuid::Uuid::new_v4().to_string(),
                node_type: NodeType::Rule,
                scope: scope.clone(),
                project_id: project_id.map(|s| s.to_string()),
                content: rule.clone(),
                confidence: 0.8,
                status: NodeStatus::Active,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                projected_at: Some(Utc::now().to_rfc3339()),
                pr_url: None,
            };
            db::insert_node(conn, &node)?;
            result.imported += 1;
        }
    }

    // Archive: DB nodes whose content is not in the file
    for node in &db_nodes {
        if !file_set.contains(node.content.as_str()) {
            db::update_node_status(conn, &node.id, &NodeStatus::Archived)?;
            result.archived += 1;
        }
    }

    Ok(result)
}

/// Read rules from a CLAUDE.md file's managed section.
/// Returns empty vec if file doesn't exist or has no managed section.
pub fn read_rules_from_file(path: &str) -> Vec<String> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    read_managed_section(&content).unwrap_or_default()
}

/// Reconcile a single CLAUDE.md file with the DB for a given scope.
/// Reads the managed section from the file and calls `reconcile_for_scope()`.
/// Returns default (0/0) if the file doesn't exist or has no managed section.
pub fn reconcile_claude_md(
    conn: &Connection,
    claude_md_path: &str,
    scope: &NodeScope,
    project_id: Option<&str>,
) -> Result<ReconcileResult, CoreError> {
    let file_rules = read_rules_from_file(claude_md_path);
    reconcile_for_scope(conn, scope, project_id, &file_rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::insert_node;
    use crate::models::{KnowledgeNode, NodeScope, NodeStatus, NodeType};
    use chrono::Utc;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn).unwrap();
        conn
    }

    fn make_projected_node(id: &str, content: &str, scope: NodeScope, project_id: Option<&str>) -> KnowledgeNode {
        KnowledgeNode {
            id: id.to_string(),
            node_type: NodeType::Rule,
            scope,
            project_id: project_id.map(|s| s.to_string()),
            content: content.to_string(),
            confidence: 0.8,
            status: NodeStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: Some(Utc::now().to_rfc3339()),
            pr_url: None,
        }
    }

    #[test]
    fn test_reconcile_imports_missing_rules() {
        let conn = test_db();
        let file_rules = vec![
            "Always use descriptive variable names".to_string(),
            "Never skip tests".to_string(),
        ];

        let result = reconcile_for_scope(&conn, &NodeScope::Global, None, &file_rules).unwrap();

        assert_eq!(result.imported, 2);
        assert_eq!(result.archived, 0);

        // Verify nodes were created with correct fields
        let nodes = db::get_projected_nodes_for_scope(&conn, &NodeScope::Global, None).unwrap();
        assert_eq!(nodes.len(), 2);
        for node in &nodes {
            assert_eq!(node.node_type, NodeType::Rule);
            assert!((node.confidence - 0.8).abs() < f64::EPSILON);
            assert_eq!(node.status, NodeStatus::Active);
            assert!(node.projected_at.is_some());
        }
    }

    #[test]
    fn test_reconcile_archives_removed_rules() {
        let conn = test_db();

        // Pre-insert a projected node
        let node = make_projected_node("node-1", "Obsolete rule", NodeScope::Global, None);
        insert_node(&conn, &node).unwrap();

        // File has no rules
        let result = reconcile_for_scope(&conn, &NodeScope::Global, None, &[]).unwrap();

        assert_eq!(result.imported, 0);
        assert_eq!(result.archived, 1);

        // Verify the node is now archived
        let archived_nodes = db::get_nodes_by_status(&conn, &NodeStatus::Archived).unwrap();
        assert_eq!(archived_nodes.len(), 1);
        assert_eq!(archived_nodes[0].id, "node-1");
    }

    #[test]
    fn test_reconcile_bidirectional() {
        let conn = test_db();

        // DB has projected nodes B and C
        let node_b = make_projected_node("node-b", "Rule B", NodeScope::Global, None);
        let node_c = make_projected_node("node-c", "Rule C", NodeScope::Global, None);
        insert_node(&conn, &node_b).unwrap();
        insert_node(&conn, &node_c).unwrap();

        // File has rules A and B (C is removed, A is new)
        let file_rules = vec!["Rule A".to_string(), "Rule B".to_string()];

        let result = reconcile_for_scope(&conn, &NodeScope::Global, None, &file_rules).unwrap();

        assert_eq!(result.imported, 1); // A imported
        assert_eq!(result.archived, 1); // C archived

        // B should still be Active
        let active_nodes = db::get_projected_nodes_for_scope(&conn, &NodeScope::Global, None).unwrap();
        assert_eq!(active_nodes.len(), 2); // B + newly imported A
        let contents: Vec<&str> = active_nodes.iter().map(|n| n.content.as_str()).collect();
        assert!(contents.contains(&"Rule B"));
        assert!(contents.contains(&"Rule A"));
    }

    #[test]
    fn test_reconcile_no_duplicates() {
        let conn = test_db();

        // DB already has the rule
        let node = make_projected_node("node-1", "Existing rule", NodeScope::Global, None);
        insert_node(&conn, &node).unwrap();

        // File has the same rule
        let file_rules = vec!["Existing rule".to_string()];

        let result = reconcile_for_scope(&conn, &NodeScope::Global, None, &file_rules).unwrap();

        assert_eq!(result.imported, 0);
        assert_eq!(result.archived, 0);

        // Only the original node should exist
        let nodes = db::get_projected_nodes_for_scope(&conn, &NodeScope::Global, None).unwrap();
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn test_reconcile_project_scope() {
        let conn = test_db();

        let file_rules = vec!["Project-specific rule".to_string()];

        let result =
            reconcile_for_scope(&conn, &NodeScope::Project, Some("my-project"), &file_rules)
                .unwrap();

        assert_eq!(result.imported, 1);

        // Verify the node has correct scope and project_id
        let nodes =
            db::get_projected_nodes_for_scope(&conn, &NodeScope::Project, Some("my-project"))
                .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].scope, NodeScope::Project);
        assert_eq!(nodes[0].project_id, Some("my-project".to_string()));
    }

    #[test]
    fn test_reconcile_claude_md_reads_file() {
        use std::io::Write;
        use tempfile::TempDir;

        let conn = test_db();
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(
            b"# Project\n\n<!-- retro:managed:start -->\n## Retro-Discovered Patterns\n\n- Rule from file\n<!-- retro:managed:end -->\n",
        )
        .unwrap();
        drop(f);

        let path_str = path.to_str().unwrap();
        let result = reconcile_claude_md(
            &conn,
            path_str,
            &NodeScope::Project,
            Some("test-project"),
        )
        .unwrap();

        assert_eq!(result.imported, 1);
        assert_eq!(result.archived, 0);

        let nodes =
            db::get_projected_nodes_for_scope(&conn, &NodeScope::Project, Some("test-project"))
                .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].content, "Rule from file");
    }

    #[test]
    fn test_reconcile_claude_md_missing_file() {
        let conn = test_db();

        let result = reconcile_claude_md(
            &conn,
            "/nonexistent/CLAUDE.md",
            &NodeScope::Project,
            Some("test-project"),
        )
        .unwrap();

        assert_eq!(result.imported, 0);
        assert_eq!(result.archived, 0);
    }

    #[test]
    fn test_reconcile_claude_md_no_managed_section() {
        use std::io::Write;
        use tempfile::TempDir;

        let conn = test_db();
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"# Project\n\nNo managed section here.\n").unwrap();
        drop(f);

        let path_str = path.to_str().unwrap();
        let result = reconcile_claude_md(
            &conn,
            path_str,
            &NodeScope::Project,
            Some("test-project"),
        )
        .unwrap();

        assert_eq!(result.imported, 0);
        assert_eq!(result.archived, 0);
    }
}
