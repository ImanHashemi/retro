use chrono::{DateTime, Utc};
use retro_core::db::Connection;
use retro_core::models::{KnowledgeNode, KnowledgeProject, NodeScope, NodeStatus, NodeType};

/// The active tab in the TUI.
#[derive(Debug, Clone, PartialEq)]
pub enum Tab {
    PendingReview,
    Knowledge,
}

/// Filter by scope (global vs project-scoped).
#[derive(Debug, Clone, PartialEq)]
pub enum ScopeFilter {
    All,
    Global,
    Project(String),
}

/// Filter by node type category.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeFilter {
    All,
    /// Rule + Directive
    Rules,
    Skills,
    Patterns,
    /// Memory + Preference
    Other,
}

/// Status of the scheduled runner.
#[derive(Debug, Clone)]
pub struct RunnerStatus {
    /// Whether the launchd/systemd runner is currently loaded/active.
    pub active: bool,
    /// Timestamp of the last pipeline run.
    pub last_run: Option<DateTime<Utc>>,
    /// AI calls used today.
    pub ai_calls_today: u32,
    /// Configured maximum AI calls per day.
    pub ai_calls_max: u32,
    /// Whether the superpowers plugin is installed (needed for skill generation).
    pub superpowers_installed: bool,
}

/// Detail view state: the node being viewed and its scroll offset.
#[derive(Debug, Clone)]
pub struct DetailView {
    pub node: KnowledgeNode,
    pub scroll_offset: u16,
}

/// Top-level TUI application state.
pub struct App {
    pub active_tab: Tab,
    pub pending_nodes: Vec<KnowledgeNode>,
    pub knowledge_nodes: Vec<KnowledgeNode>,
    pub projects: Vec<KnowledgeProject>,
    pub selected_index: usize,
    pub detail_view: Option<DetailView>,
    pub search_query: Option<String>,
    pub search_mode: bool,
    pub scope_filter: ScopeFilter,
    pub type_filter: TypeFilter,
    pub runner_status: RunnerStatus,
    pub should_quit: bool,
    /// Transient status message with the timestamp it was set.
    pub message: Option<(String, std::time::Instant)>,
}

impl App {
    /// Load app state from the database and config.
    pub fn load(conn: &Connection, config: &retro_core::config::Config) -> Self {
        let pending_nodes = retro_core::db::get_nodes_by_status(conn, &NodeStatus::PendingReview)
            .unwrap_or_default();
        let knowledge_nodes = retro_core::db::get_nodes_by_status(conn, &NodeStatus::Active)
            .unwrap_or_default();
        let projects = retro_core::db::get_all_projects(conn).unwrap_or_default();
        let (ai_used, ai_max) = retro_core::runner::ai_calls_today(conn, config);
        let last_run = retro_core::runner::last_run_time(conn);

        #[cfg(target_os = "macos")]
        let active = crate::launchd::is_loaded();
        #[cfg(not(target_os = "macos"))]
        let active = false;

        App {
            active_tab: if !pending_nodes.is_empty() {
                Tab::PendingReview
            } else {
                Tab::Knowledge
            },
            pending_nodes,
            knowledge_nodes,
            projects,
            selected_index: 0,
            detail_view: None,
            search_query: None,
            search_mode: false,
            scope_filter: ScopeFilter::All,
            type_filter: TypeFilter::All,
            runner_status: RunnerStatus {
                active,
                last_run,
                ai_calls_today: ai_used,
                ai_calls_max: ai_max,
                superpowers_installed: retro_core::projection::skill::is_superpowers_installed(),
            },
            should_quit: false,
            message: None,
        }
    }

    /// Returns a filtered view of the current tab's items.
    pub fn visible_items(&self) -> Vec<&KnowledgeNode> {
        let items = match self.active_tab {
            Tab::PendingReview => &self.pending_nodes,
            Tab::Knowledge => &self.knowledge_nodes,
        };
        items
            .iter()
            .filter(|node| {
                let scope_match = match &self.scope_filter {
                    ScopeFilter::All => true,
                    ScopeFilter::Global => node.scope == NodeScope::Global,
                    ScopeFilter::Project(id) => {
                        node.project_id.as_deref() == Some(id.as_str())
                    }
                };
                let type_match = match &self.type_filter {
                    TypeFilter::All => true,
                    TypeFilter::Rules => {
                        matches!(node.node_type, NodeType::Rule | NodeType::Directive)
                    }
                    TypeFilter::Skills => matches!(node.node_type, NodeType::Skill),
                    TypeFilter::Patterns => matches!(node.node_type, NodeType::Pattern),
                    TypeFilter::Other => {
                        matches!(node.node_type, NodeType::Memory | NodeType::Preference)
                    }
                };
                let search_match = match &self.search_query {
                    Some(q) if !q.is_empty() => {
                        node.content.to_lowercase().contains(&q.to_lowercase())
                    }
                    _ => true,
                };
                scope_match && type_match && search_match
            })
            .collect()
    }

    /// Cycle to the next tab.
    pub fn next_tab(&mut self) {
        self.active_tab = match self.active_tab {
            Tab::PendingReview => Tab::Knowledge,
            Tab::Knowledge => Tab::PendingReview,
        };
        // Reset selection when switching tabs.
        self.selected_index = 0;
        self.detail_view = None;
    }

    /// Cycle to the next scope filter.
    pub fn next_scope_filter(&mut self) {
        self.scope_filter = match &self.scope_filter {
            ScopeFilter::All => ScopeFilter::Global,
            ScopeFilter::Global => {
                // Cycle through projects if any, then back to All
                if let Some(first_project) = self.projects.first() {
                    ScopeFilter::Project(first_project.id.clone())
                } else {
                    ScopeFilter::All
                }
            }
            ScopeFilter::Project(current_id) => {
                // Find the next project after the current one
                let pos = self
                    .projects
                    .iter()
                    .position(|p| &p.id == current_id);
                match pos {
                    Some(i) if i + 1 < self.projects.len() => {
                        ScopeFilter::Project(self.projects[i + 1].id.clone())
                    }
                    _ => ScopeFilter::All,
                }
            }
        };
        // Reset selection after filter change.
        self.selected_index = 0;
    }

    /// Cycle to the next type filter.
    pub fn next_type_filter(&mut self) {
        self.type_filter = match self.type_filter {
            TypeFilter::All => TypeFilter::Rules,
            TypeFilter::Rules => TypeFilter::Skills,
            TypeFilter::Skills => TypeFilter::Patterns,
            TypeFilter::Patterns => TypeFilter::Other,
            TypeFilter::Other => TypeFilter::All,
        };
        // Reset selection after filter change.
        self.selected_index = 0;
    }

    /// Move selection up by one.
    pub fn move_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// Move selection down by one.
    pub fn move_down(&mut self) {
        let count = self.visible_items().len();
        if count > 0 && self.selected_index + 1 < count {
            self.selected_index += 1;
        }
    }

    /// Jump to the first item.
    pub fn jump_top(&mut self) {
        self.selected_index = 0;
    }

    /// Jump to the last item.
    pub fn jump_bottom(&mut self) {
        let count = self.visible_items().len();
        if count > 0 {
            self.selected_index = count - 1;
        }
    }

    /// Set a transient status message.
    pub fn set_message(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), std::time::Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use retro_core::models::*;

    fn make_node(
        id: &str,
        node_type: NodeType,
        scope: NodeScope,
        content: &str,
        conf: f64,
    ) -> KnowledgeNode {
        KnowledgeNode {
            id: id.to_string(),
            node_type,
            scope: scope.clone(),
            project_id: if scope == NodeScope::Project {
                Some("my-app".to_string())
            } else {
                None
            },
            content: content.to_string(),
            confidence: conf,
            status: NodeStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            projected_at: None,
            pr_url: None,
        }
    }

    fn test_app() -> App {
        App {
            active_tab: Tab::Knowledge,
            pending_nodes: vec![],
            knowledge_nodes: vec![
                make_node("n1", NodeType::Rule, NodeScope::Project, "Always run tests", 0.85),
                make_node("n2", NodeType::Directive, NodeScope::Global, "Use snake_case", 0.9),
                make_node("n3", NodeType::Pattern, NodeScope::Project, "Forgets docs", 0.6),
            ],
            projects: vec![KnowledgeProject {
                id: "my-app".to_string(),
                path: "/tmp/test".to_string(),
                remote_url: None,
                agent_type: "claude_code".to_string(),
                last_seen: Utc::now(),
            }],
            selected_index: 0,
            detail_view: None,
            search_query: None,
            search_mode: false,
            scope_filter: ScopeFilter::All,
            type_filter: TypeFilter::All,
            runner_status: RunnerStatus {
                active: false,
                last_run: None,
                ai_calls_today: 0,
                ai_calls_max: 10,
                superpowers_installed: true,
            },
            should_quit: false,
            message: None,
        }
    }

    #[test]
    fn test_visible_items_no_filters() {
        let app = test_app();
        let items = app.visible_items();
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn test_visible_items_scope_filter() {
        let mut app = test_app();
        app.scope_filter = ScopeFilter::Global;
        let items = app.visible_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "n2");
    }

    #[test]
    fn test_visible_items_type_filter() {
        let mut app = test_app();
        // Rules filter matches Rule and Directive; n1 is Rule (project), n2 is Directive (global)
        // Set scope to Project so only n1 (Rule, project) is visible
        app.scope_filter = ScopeFilter::Project("my-app".to_string());
        app.type_filter = TypeFilter::Rules;
        let items = app.visible_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "n1");
    }

    #[test]
    fn test_visible_items_search() {
        let mut app = test_app();
        app.search_query = Some("snake".to_string());
        let items = app.visible_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "n2");
    }

    #[test]
    fn test_tab_switching() {
        let mut app = test_app();
        assert_eq!(app.active_tab, Tab::Knowledge);
        app.next_tab();
        assert_eq!(app.active_tab, Tab::PendingReview);
        app.next_tab();
        assert_eq!(app.active_tab, Tab::Knowledge);
    }

    #[test]
    fn test_navigation() {
        let mut app = test_app();
        // 3 items visible (n1, n2, n3)
        assert_eq!(app.selected_index, 0);

        app.move_down();
        assert_eq!(app.selected_index, 1);

        app.move_down();
        assert_eq!(app.selected_index, 2);

        // At bottom — should not go past end
        app.move_down();
        assert_eq!(app.selected_index, 2);

        app.move_up();
        assert_eq!(app.selected_index, 1);

        app.jump_top();
        assert_eq!(app.selected_index, 0);

        app.jump_bottom();
        assert_eq!(app.selected_index, 2);
    }

    #[test]
    fn test_type_filter_cycling() {
        let mut app = test_app();
        assert_eq!(app.type_filter, TypeFilter::All);

        app.next_type_filter();
        assert_eq!(app.type_filter, TypeFilter::Rules);

        app.next_type_filter();
        assert_eq!(app.type_filter, TypeFilter::Skills);

        app.next_type_filter();
        assert_eq!(app.type_filter, TypeFilter::Patterns);

        app.next_type_filter();
        assert_eq!(app.type_filter, TypeFilter::Other);

        app.next_type_filter();
        assert_eq!(app.type_filter, TypeFilter::All);
    }
}
