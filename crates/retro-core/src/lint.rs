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
    for (i, a) in active.iter().enumerate() {
        for b in active.iter().skip(i + 1) {
            if a.scope != b.scope {
                continue;
            }
            if crate::analysis::merge::normalized_similarity(&a.body, &b.body) > 0.8 {
                report.findings.push(LintFinding {
                    kind: "near-duplicate".to_string(),
                    node_ids: vec![a.id.clone(), b.id.clone()],
                    detail: format!(
                        "`{}` and `{}` look like the same rule — consider merging (invalidate one)",
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
