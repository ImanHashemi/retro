pub mod claude_md;
pub mod global_agent;
pub mod skill;

use crate::analysis::backend::AnalysisBackend;
use crate::config::Config;
use crate::db;
use crate::errors::CoreError;
use crate::models::{
    ApplyAction, ApplyPlan, ApplyTrack, Pattern, PatternStatus, Projection, ProjectionStatus, SuggestedTarget,
};
use crate::util::backup_file;
use chrono::Utc;
use rusqlite::Connection;
use std::path::Path;

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

    // Collect CLAUDE.md rules and write as a batch
    let claude_md_actions: Vec<&&ApplyAction> = actions
        .iter()
        .filter(|a| a.target_type == SuggestedTarget::ClaudeMd)
        .collect();

    if !claude_md_actions.is_empty() {
        let target_path = &claude_md_actions[0].target_path;
        let rules: Vec<String> = claude_md_actions.iter().map(|a| a.content.clone()).collect();

        write_claude_md(target_path, &rules, &backup_dir)?;
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
        .filter(|p| p.suggested_target != SuggestedTarget::DbOnly)
        .filter(|p| !p.generation_failed)
        .filter(|p| !projected_ids.contains(&p.id))
        .collect())
}

/// Write CLAUDE.md with managed section.
fn write_claude_md(
    target_path: &str,
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

    let updated = claude_md::update_claude_md_content(&existing, rules);

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
