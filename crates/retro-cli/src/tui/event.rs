use crossterm::event::{KeyCode, KeyEvent};
use retro_core::db;
use retro_core::models::NodeStatus;

use super::app::{App, DetailView, Tab};

pub fn handle_key(app: &mut App, key: KeyEvent, conn: &retro_core::db::Connection) -> bool {
    if app.search_mode {
        return handle_search_key(app, key);
    }
    if app.detail_view.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                app.detail_view = None;
                return true;
            }
            _ => return false,
        }
    }
    match key.code {
        KeyCode::Char('q') => {
            app.should_quit = true;
            return true;
        }
        KeyCode::Tab => {
            app.next_tab();
            return true;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.move_down();
            return true;
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.move_up();
            return true;
        }
        KeyCode::Char('g') => {
            app.jump_top();
            return true;
        }
        KeyCode::Char('G') => {
            app.jump_bottom();
            return true;
        }
        KeyCode::Char('/') => {
            app.search_mode = true;
            app.search_query = Some(String::new());
            return true;
        }
        _ => {}
    }
    match app.active_tab {
        Tab::PendingReview => handle_pending_key(app, key, conn),
        Tab::Knowledge => handle_knowledge_key(app, key),
    }
}

fn handle_pending_key(app: &mut App, key: KeyEvent, conn: &retro_core::db::Connection) -> bool {
    match key.code {
        KeyCode::Char('a') => {
            let (selected_id, node_scope, node_type) = {
                let items = app.visible_items();
                match items.get(app.selected_index) {
                    Some(node) => (node.id.clone(), node.scope.clone(), node.node_type.clone()),
                    None => return false,
                }
            };

            if db::update_node_status(conn, &selected_id, &NodeStatus::Active).is_err() {
                return true;
            }

            // Immediate projection for global rules/directives/preferences
            let is_global = node_scope == retro_core::models::NodeScope::Global;
            let is_rule_type = matches!(
                node_type,
                retro_core::models::NodeType::Rule
                    | retro_core::models::NodeType::Directive
                    | retro_core::models::NodeType::Preference
            );

            if is_global && is_rule_type {
                if let Ok(Some(node)) = db::get_node(conn, &selected_id) {
                    let home =
                        std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                    let claude_md_path = std::path::PathBuf::from(home)
                        .join(".claude")
                        .join("CLAUDE.md");
                    if retro_core::projection::claude_md::project_rule_to_claude_md(
                        &claude_md_path,
                        &node.content,
                    )
                    .is_ok()
                    {
                        let _ = db::mark_node_projected(conn, &selected_id);
                        app.set_message("Applied to ~/.claude/CLAUDE.md".to_string());
                    } else {
                        app.set_message(
                            "Approved (write failed — will retry on next run)".to_string(),
                        );
                    }
                }
            } else if is_global {
                app.set_message(
                    "Approved — skill will be generated on next run".to_string(),
                );
            } else {
                app.set_message("Approved — will be projected on next run".to_string());
            }

            // Move from pending to knowledge
            app.pending_nodes.retain(|n| n.id != selected_id);
            if let Ok(Some(node)) = db::get_node(conn, &selected_id) {
                app.knowledge_nodes.push(node);
                app.knowledge_nodes.sort_by(|a, b| {
                    b.confidence
                        .partial_cmp(&a.confidence)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            if app.selected_index > 0 && app.selected_index >= app.pending_nodes.len() {
                app.selected_index = app.pending_nodes.len().saturating_sub(1);
            }
            true
        }
        KeyCode::Char('d') => {
            let selected_id = {
                let items = app.visible_items();
                match items.get(app.selected_index) {
                    Some(node) => node.id.clone(),
                    None => return false,
                }
            };
            if let Ok(()) = db::update_node_status(conn, &selected_id, &NodeStatus::Dismissed) {
                app.pending_nodes.retain(|n| n.id != selected_id);
                if app.selected_index > 0 && app.selected_index >= app.pending_nodes.len() {
                    app.selected_index = app.pending_nodes.len().saturating_sub(1);
                }
                app.set_message("Dismissed.".to_string());
            }
            true
        }
        KeyCode::Char('p') | KeyCode::Enter => {
            let node = {
                let items = app.visible_items();
                items.get(app.selected_index).map(|n| (*n).clone())
            };
            if let Some(node) = node {
                app.detail_view = Some(DetailView {
                    node,
                    scroll_offset: 0,
                });
            }
            true
        }
        _ => false,
    }
}

fn handle_knowledge_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('s') => {
            app.next_scope_filter();
            true
        }
        KeyCode::Char('t') => {
            app.next_type_filter();
            true
        }
        KeyCode::Enter => {
            let node = {
                let items = app.visible_items();
                items.get(app.selected_index).map(|n| (*n).clone())
            };
            if let Some(node) = node {
                app.detail_view = Some(DetailView {
                    node,
                    scroll_offset: 0,
                });
            }
            true
        }
        _ => false,
    }
}

fn handle_search_key(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.search_mode = false;
            app.search_query = None;
            app.selected_index = 0;
            true
        }
        KeyCode::Enter => {
            app.search_mode = false;
            app.selected_index = 0;
            true
        }
        KeyCode::Backspace => {
            if let Some(ref mut q) = app.search_query {
                q.pop();
            }
            app.selected_index = 0;
            true
        }
        KeyCode::Char(c) => {
            if let Some(ref mut q) = app.search_query {
                q.push(c);
            }
            app.selected_index = 0;
            true
        }
        _ => false,
    }
}
