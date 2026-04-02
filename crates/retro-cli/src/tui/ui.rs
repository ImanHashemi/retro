use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap},
    Frame,
};

use super::app::{App, DetailView, ScopeFilter, Tab, TypeFilter};
use retro_core::models::{KnowledgeNode, NodeType};

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Status bar
            Constraint::Length(2),  // Tab bar
            Constraint::Min(5),    // Main content
            Constraint::Length(1), // Help bar
        ])
        .split(frame.area());

    draw_status_bar(frame, app, chunks[0]);
    draw_tab_bar(frame, app, chunks[1]);
    draw_content(frame, app, chunks[2]);
    draw_help_bar(frame, app, chunks[3]);

    if let Some(ref detail) = app.detail_view {
        draw_detail_overlay(frame, detail);
    }
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let status_text = if app.runner_status.active { "Active" } else { "Stopped" };
    let status_color = if app.runner_status.active { Color::Green } else { Color::Red };

    let last_run = app.runner_status.last_run
        .map(|dt| {
            let diff = chrono::Utc::now() - dt;
            if diff.num_hours() > 0 { format!("{}h ago", diff.num_hours()) }
            else if diff.num_minutes() > 0 { format!("{}m ago", diff.num_minutes()) }
            else { "just now".to_string() }
        })
        .unwrap_or_else(|| "never".to_string());

    let message_text = app.message.as_ref()
        .filter(|(_, created)| created.elapsed().as_secs() < 3)
        .map(|(msg, _)| format!("  {msg}"))
        .unwrap_or_default();

    let line = Line::from(vec![
        Span::raw("  Status: "),
        Span::styled(status_text, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        Span::raw(" · Last run: "),
        Span::raw(last_run),
        Span::raw(format!(" · AI calls today: {}/{}", app.runner_status.ai_calls_today, app.runner_status.ai_calls_max)),
        Span::styled(message_text, Style::default().fg(Color::Yellow)),
    ]);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .title(" Retro Dashboard ")
        .title_style(Style::default().add_modifier(Modifier::BOLD));
    let paragraph = Paragraph::new(line).block(block);
    frame.render_widget(paragraph, area);
}

fn draw_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    let pending_count = app.pending_nodes.len();
    let knowledge_count = app.knowledge_nodes.len();
    let titles = vec![
        format!("Pending Review ({pending_count})"),
        format!("Knowledge ({knowledge_count})"),
    ];
    let selected = match app.active_tab { Tab::PendingReview => 0, Tab::Knowledge => 1 };
    let tabs = Tabs::new(titles)
        .select(selected)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .divider(" | ");
    frame.render_widget(tabs, area);
}

fn draw_content(frame: &mut Frame, app: &App, area: Rect) {
    match app.active_tab {
        Tab::PendingReview => draw_pending_list(frame, app, area),
        Tab::Knowledge => draw_knowledge_list(frame, app, area),
    }
}

fn draw_pending_list(frame: &mut Frame, app: &App, area: Rect) {
    let items = app.visible_items();
    if items.is_empty() {
        let msg = Paragraph::new("  No pending suggestions. All caught up!")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, area);
        return;
    }
    let list_items: Vec<ListItem> = items.iter().enumerate().map(|(i, node)| {
        let marker = if i == app.selected_index { "> " } else { "  " };
        let type_str = format_node_type(&node.node_type);
        let scope_str = format_scope(node);
        let scope_display = if scope_str.len() > 12 {
            format!("{}...", &scope_str[..9])
        } else {
            scope_str
        };
        let conf = format!("{:.2}", node.confidence);
        let fixed_cols = 2 + 14 + 14 + 6; // marker + [type] + scope + conf + spaces
        let content_width = (area.width as usize).saturating_sub(fixed_cols);
        let content = truncate(&node.content, content_width);
        ListItem::new(Line::from(vec![
            Span::raw(marker),
            Span::styled(format!("[{type_str}]"), Style::default().fg(type_color(&node.node_type))),
            Span::raw(" "),
            Span::styled(format!("{scope_display:<14}"), Style::default().fg(Color::DarkGray)),
            Span::raw(content),
            Span::raw("  "),
            Span::styled(conf, Style::default().fg(Color::Yellow)),
        ]))
    }).collect();
    let list = List::new(list_items);
    frame.render_widget(list, area);
}

fn draw_knowledge_list(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(3)])
        .split(area);

    // Filter bar
    let scope_text = match &app.scope_filter {
        ScopeFilter::All => "All".to_string(),
        ScopeFilter::Global => "Global".to_string(),
        ScopeFilter::Project(id) => id.clone(),
    };
    let type_text = match &app.type_filter {
        TypeFilter::All => "All", TypeFilter::Rules => "Rules", TypeFilter::Skills => "Skills",
        TypeFilter::Patterns => "Patterns", TypeFilter::Other => "Other",
    };
    let search_text = app.search_query.as_deref().unwrap_or("");
    let mut filter_spans = vec![
        Span::raw("  Scope: "),
        Span::styled(format!("[{scope_text}]"), Style::default().fg(Color::Cyan)),
        Span::raw("  Type: "),
        Span::styled(format!("[{type_text}]"), Style::default().fg(Color::Cyan)),
    ];
    if !search_text.is_empty() {
        filter_spans.push(Span::styled(format!("  /{search_text}"), Style::default().fg(Color::Yellow)));
    }
    frame.render_widget(Paragraph::new(Line::from(filter_spans)), chunks[0]);

    // List
    let items = app.visible_items();
    if items.is_empty() {
        let msg = Paragraph::new("  No matching knowledge nodes.")
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(msg, chunks[1]);
        return;
    }
    let list_items: Vec<ListItem> = items.iter().enumerate().map(|(i, node)| {
        let marker = if i == app.selected_index { "> " } else { "  " };
        let type_str = format_node_type(&node.node_type);
        let scope_str = format_scope(node);
        // Truncate scope to 12 chars max for clean columns
        let scope_display = if scope_str.len() > 12 {
            format!("{}...", &scope_str[..9])
        } else {
            scope_str
        };
        let conf = format!("{:.2}", node.confidence);
        let fixed_cols = 2 + 12 + 14 + 6; // marker + type + scope + conf + spaces
        let content_width = (chunks[1].width as usize).saturating_sub(fixed_cols);
        let content = truncate(&node.content, content_width);
        ListItem::new(Line::from(vec![
            Span::raw(marker),
            Span::styled(format!("{type_str:<12}"), Style::default().fg(type_color(&node.node_type))),
            Span::styled(format!("{scope_display:<14}"), Style::default().fg(Color::DarkGray)),
            Span::raw(content),
            Span::raw("  "),
            Span::styled(conf, Style::default().fg(Color::Yellow)),
        ]))
    }).collect();
    frame.render_widget(List::new(list_items), chunks[1]);
}

fn draw_help_bar(frame: &mut Frame, app: &App, area: Rect) {
    let help_text = match (&app.active_tab, &app.detail_view, app.search_mode) {
        (_, _, true) => "  Type to search · Esc: cancel · Enter: apply",
        (_, Some(_), _) => "  Enter/Esc: close detail",
        (Tab::PendingReview, None, _) => "  a: approve  d: dismiss  p: preview  Tab: switch  j/k: navigate  q: quit",
        (Tab::Knowledge, None, _) => "  Enter: detail  s: scope  t: type  /: search  Tab: switch  j/k: navigate  q: quit",
    };
    frame.render_widget(
        Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn draw_detail_overlay(frame: &mut Frame, detail: &DetailView) {
    let area = frame.area();
    let popup_width = (area.width as f32 * 0.7) as u16;
    let popup_height = (area.height as f32 * 0.6) as u16;
    let popup_area = Rect::new(
        (area.width - popup_width) / 2,
        (area.height - popup_height) / 2,
        popup_width,
        popup_height,
    );
    frame.render_widget(Clear, popup_area);
    let node = &detail.node;
    let type_str = format_node_type(&node.node_type);
    let scope_str = format_scope(node);
    let lines = vec![
        Line::from(vec![
            Span::raw("Type: "),
            Span::styled(type_str, Style::default().fg(type_color(&node.node_type))),
            Span::raw("  Scope: "), Span::raw(scope_str),
            Span::raw(format!("  Confidence: {:.2}", node.confidence)),
        ]),
        Line::from(""),
        Line::from(node.content.clone()),
        Line::from(""),
        Line::from(vec![
            Span::styled("ID: ", Style::default().fg(Color::DarkGray)),
            Span::styled(node.id.clone(), Style::default().fg(Color::DarkGray)),
        ]),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Detail ")
        .title_style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), popup_area);
}

fn format_node_type(nt: &NodeType) -> &'static str {
    match nt {
        NodeType::Rule => "rule", NodeType::Directive => "directive",
        NodeType::Pattern => "pattern", NodeType::Skill => "skill",
        NodeType::Memory => "memory", NodeType::Preference => "preference",
    }
}

fn type_color(nt: &NodeType) -> Color {
    match nt {
        NodeType::Rule | NodeType::Directive => Color::Blue,
        NodeType::Skill => Color::Green,
        NodeType::Pattern => Color::Yellow,
        NodeType::Memory | NodeType::Preference => Color::DarkGray,
    }
}

fn format_scope(node: &KnowledgeNode) -> String {
    match &node.project_id {
        Some(id) => id.clone(),
        None => "global".to_string(),
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len { return s.to_string(); }
    if max_len <= 3 { return ".".repeat(max_len); }
    let truncated = retro_core::util::truncate_str(s, max_len - 3);
    format!("{truncated}...")
}
