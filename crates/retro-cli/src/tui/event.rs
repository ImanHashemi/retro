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
    let selected_id = {
        let items = app.visible_items();
        if items.is_empty() {
            return false;
        }
        match items.get(app.selected_index) {
            Some(node) => node.id.clone(),
            None => return false,
        }
    };
    match key.code {
        KeyCode::Char('a') => {
            if let Ok(()) = db::update_node_status(conn, &selected_id, &NodeStatus::Active) {
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
                app.set_message("Approved!".to_string());
            }
            true
        }
        KeyCode::Char('d') => {
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
