pub mod claude_md;
pub mod global_agent;
pub mod skill;

use crate::analysis::backend::AnalysisBackend;
use crate::config::Config;
use crate::db;
use crate::errors::CoreError;
use crate::models::{
    ApplyAction, ApplyPlan, ApplyTrack, ClaudeMdEdit, ClaudeMdEditType, Pattern, PatternStatus,
    Projection, ProjectionStatus, SuggestedTarget,
};
use crate::util::backup_file;
use chrono::Utc;
use rusqlite::Connection;
use std::path::Path;

/// Returns true if the content string is a JSON edit action (starts with `{` and contains `"edit_type"`).
pub fn is_edit_action(content: &str) -> bool {
    let trimmed = content.trim();
    trimmed.starts_with('{') && trimmed.contains("\"edit_type\"")
}

/// Parse a JSON edit from an action's content field.
///
/// The JSON format is:
/// ```json
/// {"edit_type":"reword","original":"old text","replacement":"new text","target_section":null,"reasoning":"why"}
/// ```
///
/// Maps fields: `original` → `original_text`, `replacement` → `suggested_content`.
pub fn parse_edit(content: &str) -> Option<ClaudeMdEdit> {
    let trimmed = content.trim();
    let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let obj = v.as_object()?;

    let edit_type_str = obj.get("edit_type")?.as_str()?;
    let edit_type = match edit_type_str {
        "add" => ClaudeMdEditType::Add,
        "remove" => ClaudeMdEditType::Remove,
        "reword" => ClaudeMdEditType::Reword,
        "move" => ClaudeMdEditType::Move,
        _ => return None,
    };

    let original_text = obj
        .get("original")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let suggested_content = obj
        .get("replacement")
        .and_then(|v| v.as_str())
        .map(String::from);

    let target_section = obj
        .get("target_section")
        .and_then(|v| v.as_str())
        .map(String::from);

    let reasoning = obj
        .get("reasoning")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(ClaudeMdEdit {
        edit_type,
        original_text,
        suggested_content,
        target_section,
        reasoning,
    })
}

/// Build an apply plan: select qualifying patterns and generate projected content.
/// For skills and global agents, this calls the AI backend.
/// For CLAUDE.md rules, no AI is needed (uses suggested_content directly).
pub fn build_apply_plan(
    conn: &Connection,
    config: &Config,
    backend: &dyn AnalysisBackend,
    project: Option<&str>,
) -> Result<ApplyPlan, CoreError> {
    let patterns = get_qualifying_patterns(conn, config, project)?;

    if patterns.is_empty() {
        return Ok(ApplyPlan {
            actions: Vec::new(),
        });
    }

    let mut actions = Vec::new();

    // Group patterns by target type
    let claude_md_patterns: Vec<&Pattern> = patterns
        .iter()
        .filter(|p| p.suggested_target == SuggestedTarget::ClaudeMd)
        .collect();
    let skill_patterns: Vec<&Pattern> = patterns
        .iter()
        .filter(|p| p.suggested_target == SuggestedTarget::Skill)
        .collect();
    let agent_patterns: Vec<&Pattern> = patterns
        .iter()
        .filter(|p| p.suggested_target == SuggestedTarget::GlobalAgent)
        .collect();

    // CLAUDE.md rules — no AI needed, use suggested_content directly
    if !claude_md_patterns.is_empty() {
        let claude_md_path = match project {
            Some(proj) => format!("{proj}/CLAUDE.md"),
            None => "CLAUDE.md".to_string(),
        };

        for p in &claude_md_patterns {
            actions.push(ApplyAction {
                pattern_id: p.id.clone(),
                pattern_description: p.description.clone(),
                target_type: SuggestedTarget::ClaudeMd,
                target_path: claude_md_path.clone(),
                content: p.suggested_content.clone(),
                track: ApplyTrack::Shared,
            });
        }
    }

    // Skills — AI generation with two-phase pipeline
    for pattern in &skill_patterns {
        let project_root = project.unwrap_or(".");
        match skill::generate_with_retry(backend, pattern, 2) {
            Ok(draft) => {
                let path = skill::skill_path(project_root, &draft.name);
                actions.push(ApplyAction {
                    pattern_id: pattern.id.clone(),
                    pattern_description: pattern.description.clone(),
                    target_type: SuggestedTarget::Skill,
                    target_path: path,
                    content: draft.content,
                    track: ApplyTrack::Shared,
                });
            }
            Err(e) => {
                eprintln!(
                    "warning: skill generation failed for pattern {}: {e}",
                    pattern.id
                );
                let _ = db::set_generation_failed(conn, &pattern.id, true);
            }
        }
    }

    // Global agents — AI generation
    let claude_dir = config.claude_dir().to_string_lossy().to_string();
    for pattern in &agent_patterns {
        match global_agent::generate_agent(backend, pattern) {
            Ok(draft) => {
                let path = global_agent::agent_path(&claude_dir, &draft.name);
                actions.push(ApplyAction {
                    pattern_id: pattern.id.clone(),
                    pattern_description: pattern.description.clone(),
                    target_type: SuggestedTarget::GlobalAgent,
                    target_path: path,
                    content: draft.content,
                    track: ApplyTrack::Personal,
                });
            }
            Err(e) => {
                eprintln!(
                    "warning: agent generation failed for pattern {}: {e}",
                    pattern.id
                );
                let _ = db::set_generation_failed(conn, &pattern.id, true);
            }
        }
    }

    Ok(ApplyPlan { actions })
}

/// Execute actions from an apply plan, optionally filtered by track.
/// When `track_filter` is Some, only actions matching that track are executed.
/// When None, all actions are executed.
pub fn execute_plan(
    conn: &Connection,
    _config: &Config,
    plan: &ApplyPlan,
    _project: Option<&str>,
    track_filter: Option<&ApplyTrack>,
) -> Result<ExecuteResult, CoreError> {
    let mut files_written = 0;
    let mut patterns_activated = 0;

    let backup_dir = crate::config::retro_dir().join("backups");
    std::fs::create_dir_all(&backup_dir)
        .map_err(|e| CoreError::Io(format!("creating backup dir: {e}")))?;

    let actions: Vec<&ApplyAction> = plan
        .actions
        .iter()
        .filter(|a| match track_filter {
            Some(track) => a.track == *track,
            None => true,
        })
        .collect();

    // Collect CLAUDE.md actions and separate edits from plain rules
    let claude_md_actions: Vec<&&ApplyAction> = actions
        .iter()
        .filter(|a| a.target_type == SuggestedTarget::ClaudeMd)
        .collect();

    if !claude_md_actions.is_empty() {
        let target_path = &claude_md_actions[0].target_path;

        // Separate JSON edits from plain rule additions
        let mut edits: Vec<ClaudeMdEdit> = Vec::new();
        let mut plain_rules: Vec<String> = Vec::new();

        for action in &claude_md_actions {
            if is_edit_action(&action.content) {
                if let Some(edit) = parse_edit(&action.content) {
                    edits.push(edit);
                } else {
                    // Fallback: treat unparseable JSON edits as plain rules
                    plain_rules.push(action.content.clone());
                }
            } else {
                plain_rules.push(action.content.clone());
            }
        }

        write_claude_md_with_edits(target_path, &edits, &plain_rules, &backup_dir)?;
        files_written += 1;

        // Record projections and update status for each pattern
        for action in &claude_md_actions {
            record_projection(conn, action, target_path)?;
            db::update_pattern_status(conn, &action.pattern_id, &PatternStatus::Active)?;
            db::update_pattern_last_projected(conn, &action.pattern_id)?;
            patterns_activated += 1;
        }
    }

    // Write skills and global agents individually
    for action in &actions {
        if action.target_type == SuggestedTarget::ClaudeMd {
            continue; // Already handled above
        }

        write_file_with_backup(&action.target_path, &action.content, &backup_dir)?;
        files_written += 1;

        record_projection(conn, action, &action.target_path)?;
        db::update_pattern_status(conn, &action.pattern_id, &PatternStatus::Active)?;
        db::update_pattern_last_projected(conn, &action.pattern_id)?;
        patterns_activated += 1;
    }

    Ok(ExecuteResult {
        files_written,
        patterns_activated,
    })
}

/// Save an apply plan's actions as pending_review projections in the database.
/// Does NOT write files or create PRs — just records the generated content for later review.
pub fn save_plan_for_review(
    conn: &Connection,
    plan: &ApplyPlan,
    project: Option<&str>,
) -> Result<usize, CoreError> {
    let mut saved = 0;

    for action in &plan.actions {
        let target_path = if action.target_type == SuggestedTarget::ClaudeMd {
            match project {
                Some(proj) => format!("{proj}/CLAUDE.md"),
                None => "CLAUDE.md".to_string(),
            }
        } else {
            action.target_path.clone()
        };

        let proj = Projection {
            id: uuid::Uuid::new_v4().to_string(),
            pattern_id: action.pattern_id.clone(),
            target_type: action.target_type.to_string(),
            target_path,
            content: action.content.clone(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::PendingReview,
        };
        db::insert_projection(conn, &proj)?;
        saved += 1;
    }

    Ok(saved)
}

/// Result of executing an apply plan.
pub struct ExecuteResult {
    pub files_written: usize,
    pub patterns_activated: usize,
}

/// Get patterns qualifying for projection.
fn get_qualifying_patterns(
    conn: &Connection,
    config: &Config,
    project: Option<&str>,
) -> Result<Vec<Pattern>, CoreError> {
    let patterns = db::get_patterns(conn, &["discovered", "active"], project)?;
    let projected_ids = db::get_projected_pattern_ids_by_status(
        conn,
        &[ProjectionStatus::Applied, ProjectionStatus::PendingReview],
    )?;
    Ok(patterns
        .into_iter()
        .filter(|p| p.confidence >= config.analysis.confidence_threshold)
        // times_seen filter removed: the confidence threshold (default 0.7)
        // is the primary gate. The AI assigns low confidence (0.4-0.5) to weak
        // single-session observations and high confidence (0.6-0.75) to explicit
        // directives ("always"/"never"), so the threshold naturally filters.
        .filter(|p| p.suggested_target != SuggestedTarget::DbOnly)
        .filter(|p| !p.generation_failed)
        .filter(|p| !projected_ids.contains(&p.id))
        .collect())
}

/// Write CLAUDE.md: apply edits first, then add plain rules to managed section.
fn write_claude_md_with_edits(
    target_path: &str,
    edits: &[ClaudeMdEdit],
    rules: &[String],
    backup_dir: &Path,
) -> Result<(), CoreError> {
    let existing = if Path::new(target_path).exists() {
        backup_file(target_path, backup_dir)?;
        std::fs::read_to_string(target_path)
            .map_err(|e| CoreError::Io(format!("reading {target_path}: {e}")))?
    } else {
        String::new()
    };

    // Phase 1: apply edits to full file content
    let after_edits = if edits.is_empty() {
        existing
    } else {
        claude_md::apply_edits(&existing, edits)
    };

    // Phase 2: add plain rules to managed section
    let updated = if rules.is_empty() {
        after_edits
    } else {
        claude_md::update_claude_md_content(&after_edits, rules)
    };

    if let Some(parent) = Path::new(target_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Io(format!("creating dir for {target_path}: {e}")))?;
    }

    std::fs::write(target_path, &updated)
        .map_err(|e| CoreError::Io(format!("writing {target_path}: {e}")))?;

    Ok(())
}

/// Write a file, backing up the original if it exists.
fn write_file_with_backup(
    target_path: &str,
    content: &str,
    backup_dir: &Path,
) -> Result<(), CoreError> {
    if Path::new(target_path).exists() {
        backup_file(target_path, backup_dir)?;
    }

    if let Some(parent) = Path::new(target_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Io(format!("creating dir for {target_path}: {e}")))?;
    }

    std::fs::write(target_path, content)
        .map_err(|e| CoreError::Io(format!("writing {target_path}: {e}")))?;

    Ok(())
}

/// Record a projection in the database.
fn record_projection(
    conn: &Connection,
    action: &ApplyAction,
    target_path: &str,
) -> Result<(), CoreError> {
    let proj = Projection {
        id: uuid::Uuid::new_v4().to_string(),
        pattern_id: action.pattern_id.clone(),
        target_type: action.target_type.to_string(),
        target_path: target_path.to_string(),
        content: action.content.clone(),
        applied_at: Utc::now(),
        pr_url: None,
        status: crate::models::ProjectionStatus::Applied,
    };
    db::insert_projection(conn, &proj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_edit_action_reword() {
        let content = r#"{"edit_type":"reword","original":"old text","replacement":"new text","reasoning":"clarity"}"#;
        assert!(is_edit_action(content));
    }

    #[test]
    fn test_is_edit_action_remove() {
        let content = r#"{"edit_type":"remove","original":"stale rule","reasoning":"no longer relevant"}"#;
        assert!(is_edit_action(content));
    }

    #[test]
    fn test_is_edit_action_plain_rule() {
        let content = "Always use uv for Python packages";
        assert!(!is_edit_action(content));
    }

    #[test]
    fn test_is_edit_action_json_without_edit_type() {
        let content = r#"{"name":"something","value":42}"#;
        assert!(!is_edit_action(content));
    }

    #[test]
    fn test_is_edit_action_with_whitespace() {
        let content = r#"  {"edit_type":"add","replacement":"new rule","reasoning":"new pattern"}  "#;
        assert!(is_edit_action(content));
    }

    #[test]
    fn test_is_edit_action_empty() {
        assert!(!is_edit_action(""));
    }

    #[test]
    fn test_parse_edit_reword() {
        let content = r#"{"edit_type":"reword","original":"No async","replacement":"Sync only — no tokio, no async","target_section":null,"reasoning":"too terse"}"#;
        let edit = parse_edit(content).unwrap();
        assert_eq!(edit.edit_type, ClaudeMdEditType::Reword);
        assert_eq!(edit.original_text, "No async");
        assert_eq!(edit.suggested_content.unwrap(), "Sync only — no tokio, no async");
        assert!(edit.target_section.is_none());
        assert_eq!(edit.reasoning, "too terse");
    }

    #[test]
    fn test_parse_edit_remove() {
        let content = r#"{"edit_type":"remove","original":"stale rule","reasoning":"no longer relevant"}"#;
        let edit = parse_edit(content).unwrap();
        assert_eq!(edit.edit_type, ClaudeMdEditType::Remove);
        assert_eq!(edit.original_text, "stale rule");
        assert!(edit.suggested_content.is_none());
        assert_eq!(edit.reasoning, "no longer relevant");
    }

    #[test]
    fn test_parse_edit_add() {
        let content = r#"{"edit_type":"add","original":"","replacement":"- New rule","reasoning":"new pattern"}"#;
        let edit = parse_edit(content).unwrap();
        assert_eq!(edit.edit_type, ClaudeMdEditType::Add);
        assert_eq!(edit.original_text, "");
        assert_eq!(edit.suggested_content.unwrap(), "- New rule");
    }

    #[test]
    fn test_parse_edit_move() {
        let content = r#"{"edit_type":"move","original":"misplaced rule","replacement":"misplaced rule","target_section":"Build","reasoning":"wrong section"}"#;
        let edit = parse_edit(content).unwrap();
        assert_eq!(edit.edit_type, ClaudeMdEditType::Move);
        assert_eq!(edit.original_text, "misplaced rule");
        assert_eq!(edit.target_section.unwrap(), "Build");
    }

    #[test]
    fn test_parse_edit_plain_text_returns_none() {
        let content = "Always use uv for Python packages";
        assert!(parse_edit(content).is_none());
    }

    #[test]
    fn test_parse_edit_invalid_edit_type_returns_none() {
        let content = r#"{"edit_type":"unknown","original":"text","reasoning":"why"}"#;
        assert!(parse_edit(content).is_none());
    }

    #[test]
    fn test_parse_edit_missing_edit_type_returns_none() {
        let content = r#"{"original":"text","reasoning":"why"}"#;
        assert!(parse_edit(content).is_none());
    }
}
