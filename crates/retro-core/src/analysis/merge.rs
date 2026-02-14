use crate::models::{Pattern, PatternStatus, PatternUpdate};
use chrono::Utc;
use uuid::Uuid;

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

/// Compute normalized Levenshtein similarity between two strings.
/// Returns a value in [0.0, 1.0] where 1.0 means identical.
pub fn normalized_similarity(a: &str, b: &str) -> f64 {
    let a_chars: Vec<char> = a.to_lowercase().chars().collect();
    let b_chars: Vec<char> = b.to_lowercase().chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    let max_len = std::cmp::max(a_len, b_len);
    if max_len == 0 {
        return 1.0;
    }

    let distance = levenshtein_distance(&a_chars, &b_chars);
    1.0 - (distance as f64 / max_len as f64)
}

fn levenshtein_distance(a: &[char], b: &[char]) -> usize {
    let a_len = a.len();
    let b_len = b.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    // Two-row optimization
    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];

    for (i, a_ch) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b.iter().enumerate() {
            let cost = if a_ch == b_ch { 0 } else { 1 };
            curr[j + 1] = std::cmp::min(
                std::cmp::min(prev[j + 1] + 1, curr[j] + 1),
                prev[j] + cost,
            );
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_strings() {
        assert!((normalized_similarity("hello", "hello") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_completely_different() {
        let sim = normalized_similarity("abc", "xyz");
        assert!(sim < 0.5);
    }

    #[test]
    fn test_similar_strings() {
        let sim = normalized_similarity(
            "Always use uv for Python packages",
            "Always use uv for Python package management",
        );
        assert!(sim > 0.7);
    }

    #[test]
    fn test_empty_strings() {
        assert!((normalized_similarity("", "") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_case_insensitive() {
        assert!((normalized_similarity("Hello World", "hello world") - 1.0).abs() < f64::EPSILON);
    }
}
