//! Store-wide lint: free (no-AI) checks for near-duplicate active nodes and
//! stale low-confidence candidates. Findings are data; `retro lint` renders
//! them and (non-dry-run) records them as briefing notifications.

use serde::Serialize;

use crate::config::Config;
use crate::errors::CoreError;
use crate::store::Store;

#[derive(Debug, Clone, Serialize)]
pub struct LintFinding {
    pub kind: String, // "near-duplicate" | "stale-candidate"
    pub node_ids: Vec<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct LintReport {
    pub findings: Vec<LintFinding>,
    pub nodes_scanned: usize,
}

/// Free lint pass: no AI calls, no writes. Compares ACTIVE nodes only.
pub fn run_lint(store: &Store, config: &Config) -> Result<LintReport, CoreError> {
    let loaded = store.load_all()?;
    let active: Vec<_> = loaded
        .nodes
        .iter()
        .map(|(_, n)| n)
        .filter(|n| n.is_active())
        .collect();
    let mut report = LintReport {
        nodes_scanned: active.len(),
        ..Default::default()
    };

    // Near-duplicates: pairwise within the same scope (store scale is small).
    let threshold = config.curator.merge_similarity;
    for (i, a) in active.iter().enumerate() {
        for b in active.iter().skip(i + 1) {
            if a.scope != b.scope {
                continue;
            }
            // Length-ratio pre-filter derived from the active threshold:
            // Levenshtein distance is at least the length difference, so a
            // wider gap than (1 - threshold) can never reach the threshold.
            // Deriving it (rather than hardcoding 0.2) keeps a lowered
            // merge_similarity from being silently capped here.
            let (la, lb) = (a.body.chars().count(), b.body.chars().count());
            let max_len = la.max(lb);
            if max_len > 0 && (la.abs_diff(lb) as f64) / (max_len as f64) > 1.0 - threshold {
                continue;
            }
            if crate::util::normalized_similarity(&a.body, &b.body) > threshold {
                let cross_type = if a.node_type == b.node_type {
                    "consider merging (invalidate one)"
                } else {
                    "similar content across node types — review whether both are needed"
                };
                report.findings.push(LintFinding {
                    kind: "near-duplicate".to_string(),
                    node_ids: vec![a.id.clone(), b.id.clone()],
                    detail: format!(
                        "`{}` and `{}` look like the same rule — {cross_type}",
                        a.id, b.id
                    ),
                });
            }
        }
    }

    // Stale candidates: sub-threshold confidence that never matured.
    let staleness = chrono::Duration::days(config.analysis.staleness_days as i64);
    let cutoff = chrono::Utc::now().date_naive() - staleness;
    for n in &active {
        if n.confidence < config.knowledge.confidence_threshold && n.updated < cutoff {
            report.findings.push(LintFinding {
                kind: "stale-candidate".to_string(),
                node_ids: vec![n.id.clone()],
                detail: format!(
                    "`{}` has sat below the projection threshold ({:.2} < {:.2}) since {} — dead weight?",
                    n.id, n.confidence, config.knowledge.confidence_threshold, n.updated
                ),
            });
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Node, NodeType, Scope};
    use chrono::Utc;
    use tempfile::TempDir;

    fn node(id: &str, scope: Scope, conf: f64, days_old: i64, body: &str) -> Node {
        let date = Utc::now().date_naive() - chrono::Duration::days(days_old);
        Node {
            id: id.to_string(),
            scope,
            node_type: NodeType::Rule,
            confidence: conf,
            sources: vec![],
            created: date,
            updated: date,
            invalidated_by: None,
            archived: None,
            archived_reason: None,
            body: body.to_string(),
        }
    }

    #[test]
    fn near_duplicates_within_scope_are_found() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store
            .write_node(&node(
                "a",
                Scope::Global,
                0.8,
                1,
                "Always run the smoke tests before full runs",
            ))
            .unwrap();
        store
            .write_node(&node(
                "b",
                Scope::Global,
                0.8,
                1,
                "Always run the smoke tests before full runs!",
            ))
            .unwrap();
        store
            .write_node(&node(
                "c",
                Scope::Global,
                0.8,
                1,
                "Use uv for python environments",
            ))
            .unwrap();
        // same body in a DIFFERENT scope must not pair with global ones
        store
            .write_node(&node(
                "a2",
                Scope::Project("p".to_string()),
                0.8,
                1,
                "Always run the smoke tests before full runs",
            ))
            .unwrap();

        let report = run_lint(&store, &Config::default()).unwrap();
        let dups: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "near-duplicate")
            .collect();
        assert_eq!(dups.len(), 1, "{:?}", report.findings);
        assert!(dups[0].node_ids.contains(&"a".to_string()));
        assert!(dups[0].node_ids.contains(&"b".to_string()));
    }

    #[test]
    fn stale_low_confidence_candidates_are_flagged() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        // default staleness_days is 28, confidence_threshold 0.7
        store
            .write_node(&node(
                "old-weak",
                Scope::Global,
                0.5,
                60,
                "some tentative pattern",
            ))
            .unwrap();
        store
            .write_node(&node(
                "old-strong",
                Scope::Global,
                0.9,
                60,
                "an established rule",
            ))
            .unwrap();
        store
            .write_node(&node(
                "new-weak",
                Scope::Global,
                0.5,
                2,
                "a fresh observation",
            ))
            .unwrap();

        let report = run_lint(&store, &Config::default()).unwrap();
        let stale: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "stale-candidate")
            .collect();
        assert_eq!(stale.len(), 1, "{:?}", report.findings);
        assert_eq!(stale[0].node_ids, vec!["old-weak".to_string()]);
    }

    #[test]
    fn archived_nodes_are_excluded_from_lint() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        // Both nodes have the same stale/sub-threshold shape as `old-weak` in
        // `stale_low_confidence_candidates_are_flagged` (it WOULD be flagged
        // if not archived). Keeping a non-archived control node alongside the
        // archived one proves selective exclusion within this test itself —
        // nodes_scanned == 1 (not 0) rules out "the store just wasn't read".
        let mut archived = node("old-weak", Scope::Global, 0.5, 60, "some tentative pattern");
        archived.archived = Some(Utc::now().date_naive());
        archived.archived_reason = Some("stale".to_string());
        store.write_node(&archived).unwrap();
        let control = node(
            "still-weak",
            Scope::Global,
            0.5,
            60,
            "another tentative pattern",
        );
        store.write_node(&control).unwrap();

        let report = run_lint(&store, &Config::default()).unwrap();
        assert_eq!(
            report.nodes_scanned, 1,
            "only the non-archived control node is scanned"
        );
        let stale: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "stale-candidate")
            .collect();
        assert_eq!(stale.len(), 1, "{:?}", report.findings);
        assert_eq!(stale[0].node_ids, vec!["still-weak".to_string()]);
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.node_ids.contains(&"old-weak".to_string())),
            "archived node must not be reported: {:?}",
            report.findings
        );
    }

    #[test]
    fn lowering_merge_similarity_widens_detection() {
        // Fixture (measured via crate::util::normalized_similarity, see
        // scratch measurement in task history): length-gap ratio = 0.2951,
        // normalized_similarity = 0.7049. The gap ratio exceeds the old
        // hardcoded 0.2 pre-filter, so at a lowered threshold this pair
        // proves derivation: the old hardcoded pre-filter would still skip
        // it on length alone, even though the similarity comfortably clears
        // a lowered threshold.
        let a = "Run the smoke test before the full backfill";
        let b = "Always run the smoke test first before the full backfill runs";

        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store.write_node(&node("a", Scope::Global, 0.8, 1, a)).unwrap();
        store.write_node(&node("b", Scope::Global, 0.8, 1, b)).unwrap();

        // At the default threshold (0.8), 0.7049 similarity doesn't clear
        // it — no near-duplicate finding.
        let mut config = Config::default();
        assert_eq!(config.curator.merge_similarity, 0.8);
        let report = run_lint(&store, &config).unwrap();
        assert!(
            !report.findings.iter().any(|f| f.kind == "near-duplicate"),
            "should not be flagged at the default threshold: {:?}",
            report.findings
        );

        // Lowering merge_similarity to 0.5 must widen detection to catch
        // this pair — the derived pre-filter (1 - 0.5 = 0.5) admits a 0.2951
        // length-gap ratio, and 0.7049 similarity clears 0.5.
        config.curator.merge_similarity = 0.5;
        let report = run_lint(&store, &config).unwrap();
        let dups: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "near-duplicate")
            .collect();
        assert_eq!(
            dups.len(),
            1,
            "lowered threshold should detect the pair: {:?}",
            report.findings
        );
        assert!(dups[0].node_ids.contains(&"a".to_string()));
        assert!(dups[0].node_ids.contains(&"b".to_string()));
    }

    #[test]
    fn clean_store_yields_no_findings() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store
            .write_node(&node("only", Scope::Global, 0.9, 1, "unique healthy rule"))
            .unwrap();
        let report = run_lint(&store, &Config::default()).unwrap();
        assert!(report.findings.is_empty());
        assert_eq!(report.nodes_scanned, 1);
    }
}
