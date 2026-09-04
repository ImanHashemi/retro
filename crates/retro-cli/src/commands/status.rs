use anyhow::Result;
use colored::Colorize;
use retro_core::config::{retro_dir, Config};

pub fn run() -> Result<()> {
    let dir = retro_dir();
    if !dir.join("knowledge").exists() {
        anyhow::bail!("retro is not initialized — run `retro init`");
    }
    let config_path = dir.join("config.toml");
    let config = Config::load(&config_path)?;

    print_v3_status(&dir, &config)
}

/// Counts backing the `nodes:` status line. `archived` and `invalidated`
/// are computed independently — a node with both fields set (not possible
/// yet, but not forbidden by the schema) would count in both, which is
/// more honest than the old `len() - active` subtraction.
struct NodeCounts {
    active: usize,
    global: usize,
    invalidated: usize,
    archived: usize,
}

fn node_counts(loaded: &retro_core::store::LoadResult) -> NodeCounts {
    let active = loaded.nodes.iter().filter(|(_, n)| n.is_active()).count();
    let global = loaded
        .nodes
        .iter()
        .filter(|(_, n)| n.is_active() && n.scope == retro_core::store::Scope::Global)
        .count();
    let invalidated = loaded
        .nodes
        .iter()
        .filter(|(_, n)| n.invalidated_by.is_some())
        .count();
    let archived = loaded
        .nodes
        .iter()
        .filter(|(_, n)| n.archived.is_some())
        .count();
    NodeCounts {
        active,
        global,
        invalidated,
        archived,
    }
}

/// Renders the `nodes:` line. The archived clause is appended only when
/// non-zero, so the line stays byte-identical to the pre-archival format
/// for every store that has nothing archived yet (a scenario test asserts
/// on this line).
fn format_nodes_line(c: &NodeCounts) -> String {
    let project = c.active - c.global;
    let mut line = format!(
        "  nodes:   {} active ({} global, {project} project), {} invalidated",
        c.active, c.global, c.invalidated
    );
    if c.archived > 0 {
        line.push_str(&format!(", {} archived", c.archived));
    }
    line
}

fn print_v3_status(dir: &std::path::Path, config: &Config) -> Result<()> {
    use retro_core::store::{queue, state::RunnerState, Store};

    let store = Store::open(dir);
    let loaded = store.load_all()?;
    let counts = node_counts(&loaded);
    let queue_len = queue::list(dir).map(|q| q.len()).unwrap_or(0);
    let state = RunnerState::load(dir)?;
    let today = chrono::Utc::now().date_naive().to_string();
    let budget_left = state.budget_remaining(&today, config.runner.max_ai_calls_per_day);

    println!("{}", "v3 knowledge store".bold());
    println!("{}", format_nodes_line(&counts));
    println!("  queue:   {queue_len} pending session(s)");
    println!(
        "  budget:  {budget_left}/{} AI call(s) left today",
        config.runner.max_ai_calls_per_day
    );
    if let Ok(health) = retro_core::health::Health::load(dir) {
        let warnings = health.warnings();
        if warnings.is_empty() {
            println!("  health:  {}", "ok".green());
        } else {
            for w in warnings {
                println!("  health:  {} {}", "⚠".yellow(), w);
            }
        }
    }
    println!("  hint:    retro ui — dashboard; retro doctor — full checks");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use retro_core::store::{Node, NodeType, Scope, Store};
    use tempfile::TempDir;

    fn node(id: &str, scope: Scope) -> Node {
        Node {
            id: id.to_string(),
            scope,
            node_type: NodeType::Rule,
            confidence: 0.8,
            sources: vec![],
            created: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            updated: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            invalidated_by: None,
            archived: None,
            archived_reason: None,
            body: "a rule".to_string(),
        }
    }

    #[test]
    fn archived_node_counts_as_archived_not_invalidated() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();

        store.write_node(&node("active-one", Scope::Global)).unwrap();

        let mut invalidated = node("invalidated-one", Scope::Global);
        invalidated.invalidated_by = Some("active-one".to_string());
        store.write_node(&invalidated).unwrap();

        let mut archived = node("archived-one", Scope::Global);
        archived.archived = Some(NaiveDate::from_ymd_opt(2026, 9, 3).unwrap());
        archived.archived_reason = Some("stale".to_string());
        store.write_node(&archived).unwrap();

        let loaded = store.load_all().unwrap();
        let counts = node_counts(&loaded);

        assert_eq!(counts.active, 1, "only active-one is active");
        assert_eq!(counts.global, 1);
        assert_eq!(counts.invalidated, 1, "the invalidated node, not the archived one");
        assert_eq!(counts.archived, 1, "the archived node, not the invalidated one");
    }

    #[test]
    fn nodes_line_omits_archived_clause_when_zero() {
        // Byte-identical to the pre-archival format for every existing
        // store, since nothing archives yet — a scenario test asserts on
        // this exact line.
        let counts = NodeCounts {
            active: 254,
            global: 188,
            invalidated: 4,
            archived: 0,
        };
        assert_eq!(
            format_nodes_line(&counts),
            "  nodes:   254 active (188 global, 66 project), 4 invalidated"
        );
    }

    #[test]
    fn nodes_line_appends_archived_clause_when_nonzero() {
        let counts = NodeCounts {
            active: 254,
            global: 188,
            invalidated: 4,
            archived: 2,
        };
        assert_eq!(
            format_nodes_line(&counts),
            "  nodes:   254 active (188 global, 66 project), 4 invalidated, 2 archived"
        );
    }
}
