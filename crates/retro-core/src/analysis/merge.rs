use crate::models::{Pattern, PatternStatus, PatternUpdate};
use chrono::Utc;
use uuid::Uuid;

// Similarity helpers moved to `util.rs` (v3 migrate.rs needs them without
// depending on the v2-only `analysis` module). Re-exported here so existing
// v2 callers keep compiling unchanged.
pub use crate::util::{levenshtein, normalized_similarity};

/// Threshold for Levenshtein similarity — above this, merge instead of creating new.
const SIMILARITY_THRESHOLD: f64 = 0.8;

/// Process AI-returned pattern updates against existing patterns.
/// Returns (new patterns to insert, updates to apply to existing patterns).
pub fn process_updates(
    updates: Vec<PatternUpdate>,
    existing: &[Pattern],
    project: Option<&str>,
) -> (Vec<Pattern>, Vec<MergeUpdate>) {
    let mut new_patterns = Vec::new();
    let mut merge_updates = Vec::new();
    let now = Utc::now();

    for update in updates {
        match update {
            PatternUpdate::New(new) => {
                // Safety net: check if this is a near-duplicate of an existing pattern
                if let Some(match_id) = find_similar_pattern(&new.description, existing) {
                    // Merge into existing instead of creating new
                    merge_updates.push(MergeUpdate {
                        pattern_id: match_id,
                        new_sessions: new.source_sessions,
                        new_confidence: new.confidence,
                        additional_times_seen: 1,
                    });
                } else {
                    // Genuinely new pattern
                    let pattern = Pattern {
                        id: Uuid::new_v4().to_string(),
                        pattern_type: new.pattern_type,
                        description: new.description,
                        confidence: new.confidence,
                        times_seen: 1,
                        first_seen: now,
                        last_seen: now,
                        last_projected: None,
                        status: PatternStatus::Discovered,
                        source_sessions: new.source_sessions,
                        related_files: new.related_files,
                        suggested_content: new.suggested_content,
                        suggested_target: new.suggested_target,
                        project: project.map(String::from),
                        generation_failed: false,
                    };
                    new_patterns.push(pattern);
                }
            }
            PatternUpdate::Update(upd) => {
                // Verify the referenced pattern exists
                if existing.iter().any(|p| p.id == upd.existing_id) {
                    merge_updates.push(MergeUpdate {
                        pattern_id: upd.existing_id,
                        new_sessions: upd.new_sessions,
                        new_confidence: upd.new_confidence,
                        additional_times_seen: 1,
                    });
                } else {
                    eprintln!(
                        "warning: AI referenced non-existent pattern ID: {}",
                        upd.existing_id
                    );
                }
            }
        }
    }

    (new_patterns, merge_updates)
}

/// A merge update to apply to an existing pattern in the DB.
pub struct MergeUpdate {
    pub pattern_id: String,
    pub new_sessions: Vec<String>,
    pub new_confidence: f64,
    pub additional_times_seen: i64,
}

/// Find an existing pattern with description similarity > threshold.
/// Returns the ID of the best match, if any.
fn find_similar_pattern(description: &str, existing: &[Pattern]) -> Option<String> {
    let mut best_match: Option<(String, f64)> = None;

    for pattern in existing {
        let similarity = normalized_similarity(description, &pattern.description);
        if similarity > SIMILARITY_THRESHOLD {
            match &best_match {
                Some((_, best_sim)) if similarity > *best_sim => {
                    best_match = Some((pattern.id.clone(), similarity));
                }
                None => {
                    best_match = Some((pattern.id.clone(), similarity));
                }
                _ => {}
            }
        }
    }

    best_match.map(|(id, _)| id)
}
