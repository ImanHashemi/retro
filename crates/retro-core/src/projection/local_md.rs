//! v3 one-way projection: regenerate managed blocks from the store.
//! Global nodes -> ~/.claude/CLAUDE.md; project nodes -> <project>/CLAUDE.local.md.
//! Managed blocks are build output — edits belong in the store.

use std::path::Path;

use crate::errors::CoreError;
use crate::projection::claude_md::update_claude_md_content;
use crate::store::{NodeType, Scope, Store};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Node;
    use chrono::Utc;
    use tempfile::TempDir;

    fn node(id: &str, scope: Scope, t: NodeType, conf: f64, body: &str) -> Node {
        let today = Utc::now().date_naive();
        Node {
            id: id.to_string(),
            scope,
            node_type: t,
            confidence: conf,
            sources: vec![],
            created: today,
            updated: today,
            invalidated_by: None,
            body: body.to_string(),
        }
    }

    #[test]
    fn projectable_rules_filters_confidence_memory_and_invalidated() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store
            .write_node(&node(
                "high",
                Scope::Global,
                NodeType::Rule,
                0.9,
                "high rule",
            ))
            .unwrap();
        store
            .write_node(&node("low", Scope::Global, NodeType::Rule, 0.5, "low rule"))
            .unwrap();
        store
            .write_node(&node("mem", Scope::Global, NodeType::Memory, 0.9, "memory"))
            .unwrap();
        let mut dead = node("dead", Scope::Global, NodeType::Rule, 0.9, "dead rule");
        dead.invalidated_by = Some("high".to_string());
        store.write_node(&dead).unwrap();
        store
            .write_node(&node(
                "proj",
                Scope::Project("p".to_string()),
                NodeType::Rule,
                0.9,
                "proj rule",
            ))
            .unwrap();

        let rules = projectable_rules(&store, &Scope::Global, 0.7).unwrap();
        assert_eq!(rules, vec!["high rule".to_string()]);
        let rules = projectable_rules(&store, &Scope::Project("p".to_string()), 0.7).unwrap();
        assert_eq!(rules, vec!["proj rule".to_string()]);
    }

    #[test]
    fn project_local_md_writes_managed_block_and_git_exclude() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        store
            .write_node(&node(
                "r",
                Scope::Project("p".to_string()),
                NodeType::Rule,
                0.9,
                "the rule",
            ))
            .unwrap();

        let proj = TempDir::new().unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(proj.path())
            .arg("init")
            .output()
            .unwrap();

        project_local_md(&store, "p", proj.path(), 0.7).unwrap();

        let content = std::fs::read_to_string(proj.path().join("CLAUDE.local.md")).unwrap();
        assert!(content.contains("retro:managed:start"));
        assert!(content.contains("- the rule"));
        let exclude = std::fs::read_to_string(proj.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("CLAUDE.local.md"));

        // idempotent: run again, no duplicate exclude line, block regenerated
        project_local_md(&store, "p", proj.path(), 0.7).unwrap();
        let exclude = std::fs::read_to_string(proj.path().join(".git/info/exclude")).unwrap();
        assert_eq!(exclude.matches("CLAUDE.local.md").count(), 1);
    }

    #[test]
    fn project_local_md_with_no_rules_removes_managed_content() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        let proj = TempDir::new().unwrap();
        // pre-existing file with a stale managed block and user content
        std::fs::write(
            proj.path().join("CLAUDE.local.md"),
            "my own notes\n\n<!-- retro:managed:start -->\n- stale rule\n<!-- retro:managed:end -->\n",
        )
        .unwrap();
        project_local_md(&store, "p", proj.path(), 0.7).unwrap();
        let content = std::fs::read_to_string(proj.path().join("CLAUDE.local.md")).unwrap();
        assert!(content.contains("my own notes"), "user content preserved");
        assert!(
            !content.contains("stale rule"),
            "managed block regenerated empty"
        );
    }

    #[test]
    fn project_global_md_preserves_user_content() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        store
            .write_node(&node(
                "g",
                Scope::Global,
                NodeType::Rule,
                0.9,
                "global rule",
            ))
            .unwrap();

        let claude_tmp = TempDir::new().unwrap();
        let md = claude_tmp.path().join("CLAUDE.md");
        std::fs::write(&md, "# My instructions\n\nuser text\n").unwrap();

        project_global_md(&store, &md, 0.7, None).unwrap();
        let content = std::fs::read_to_string(&md).unwrap();
        assert!(content.contains("user text"));
        assert!(content.contains("- global rule"));
    }

    #[test]
    fn unchanged_projection_writes_nothing_and_makes_no_backup() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        store
            .write_node(&node(
                "g",
                Scope::Global,
                NodeType::Rule,
                0.9,
                "stable rule",
            ))
            .unwrap();

        let claude_tmp = TempDir::new().unwrap();
        let md = claude_tmp.path().join("CLAUDE.md");
        std::fs::write(&md, "# Mine\n").unwrap();
        let backups = store_tmp.path().join("backups");

        project_global_md(&store, &md, 0.7, Some(&backups)).unwrap();
        let first_backup_count = std::fs::read_dir(&backups).unwrap().count();
        let mtime_after_first = std::fs::metadata(&md).unwrap().modified().unwrap();

        // second run: identical content -> no new backup, no rewrite
        project_global_md(&store, &md, 0.7, Some(&backups)).unwrap();
        assert_eq!(
            std::fs::read_dir(&backups).unwrap().count(),
            first_backup_count
        );
        assert_eq!(
            std::fs::metadata(&md).unwrap().modified().unwrap(),
            mtime_after_first
        );
    }

    #[test]
    fn multiline_body_flattens_to_single_line_bullet() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store
            .write_node(&node(
                "multi",
                Scope::Global,
                NodeType::Rule,
                0.9,
                "Always do X.\n\n**Why:** because Y.\n**How to apply:** do Z.",
            ))
            .unwrap();
        let rules = projectable_rules(&store, &Scope::Global, 0.7).unwrap();
        assert_eq!(
            rules,
            vec!["Always do X. **Why:** because Y. **How to apply:** do Z.".to_string()]
        );
        // and the projected file has it as ONE bullet line
        let claude_tmp = TempDir::new().unwrap();
        let md = claude_tmp.path().join("CLAUDE.md");
        project_global_md(&store, &md, 0.7, None).unwrap();
        let content = std::fs::read_to_string(&md).unwrap();
        assert!(content.contains("- Always do X. **Why:** because Y. **How to apply:** do Z."));
    }

    #[test]
    fn git_exclude_works_in_worktrees() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        store
            .write_node(&node(
                "r",
                Scope::Project("p".to_string()),
                NodeType::Rule,
                0.9,
                "rule",
            ))
            .unwrap();

        // real repo with a commit, then a linked worktree
        let main = TempDir::new().unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(main.path(), &["init"]);
        run(main.path(), &["config", "user.email", "t@t"]);
        run(main.path(), &["config", "user.name", "t"]);
        run(main.path(), &["config", "commit.gpgsign", "false"]);
        run(main.path(), &["commit", "--allow-empty", "-m", "init"]);
        let wt = main.path().join("wt");
        run(main.path(), &["worktree", "add", wt.to_str().unwrap()]);

        project_local_md(&store, "p", &wt, 0.7).unwrap();
        // exclude lands in the COMMON dir's info/exclude
        let exclude = std::fs::read_to_string(main.path().join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("CLAUDE.local.md"), "got: {exclude}");
    }
}

/// Bodies of projectable nodes for a scope: active, non-memory, confidence >= threshold.
/// Ordered by node id for stable output (idempotent regeneration).
pub fn projectable_rules(
    store: &Store,
    scope: &Scope,
    threshold: f64,
) -> Result<Vec<String>, CoreError> {
    let loaded = store.load_all()?;
    let mut nodes: Vec<_> = loaded
        .nodes
        .into_iter()
        .map(|(_, n)| n)
        .filter(|n| n.is_active())
        .filter(|n| n.node_type != NodeType::Memory)
        .filter(|n| n.confidence >= threshold)
        .filter(|n| &n.scope == scope)
        .collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(nodes.into_iter().map(|n| flatten_body(&n.body)).collect())
}

/// Managed-block bullets are single-line (the v2-compatible, renderer-safe
/// format). Multi-line store bodies are flattened: blank lines dropped,
/// newlines become single spaces. The store file keeps the readable layout.
fn flatten_body(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Regenerate the managed block in an arbitrary CLAUDE.md-style file.
/// `backup_dir`: when Some, the existing file is backed up first.
pub fn project_global_md(
    store: &Store,
    claude_md_path: &Path,
    threshold: f64,
    backup_dir: Option<&Path>,
) -> Result<usize, CoreError> {
    let rules = projectable_rules(store, &Scope::Global, threshold)?;
    // Parity with project_local_md: never create an empty shell on a machine
    // that has no CLAUDE.md and no rules yet.
    if rules.is_empty() && !claude_md_path.exists() {
        return Ok(0);
    }
    write_managed(claude_md_path, &rules, backup_dir)?;
    Ok(rules.len())
}

/// Regenerate <project>/CLAUDE.local.md and ensure it is ignored via
/// .git/info/exclude (personal ignore file — the team's .gitignore is never touched).
pub fn project_local_md(
    store: &Store,
    slug: &str,
    project_root: &Path,
    threshold: f64,
) -> Result<usize, CoreError> {
    let rules = projectable_rules(store, &Scope::Project(slug.to_string()), threshold)?;
    let path = project_root.join("CLAUDE.local.md");
    // No rules and no existing file: don't create an empty shell.
    if rules.is_empty() && !path.exists() {
        return Ok(0);
    }
    write_managed(&path, &rules, None)?;
    ensure_git_exclude(project_root)?;
    Ok(rules.len())
}

fn write_managed(
    path: &Path,
    rules: &[String],
    backup_dir: Option<&Path>,
) -> Result<(), CoreError> {
    let io = |e: std::io::Error| CoreError::Io(e.to_string());
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let updated = update_claude_md_content(&existing, rules);
    // Idempotent regeneration: unchanged content means no write, no backup —
    // hook-triggered runs must not churn the user's files.
    if updated == existing {
        return Ok(());
    }
    if let Some(dir) = backup_dir {
        if path.exists() {
            crate::util::backup_file(&path.display().to_string(), dir)?;
        }
    }
    // Atomic swap: Claude Code may read this file mid-run.
    let tmp = path.with_extension("md.retro-tmp");
    std::fs::write(&tmp, updated).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

/// Append CLAUDE.local.md to the repo's personal ignore file
/// (<common-git-dir>/info/exclude). Handles regular repos AND worktrees
/// (where .git is a file); git reads info/exclude from the COMMON dir.
/// Non-git directories are a no-op.
/// Uninstall counterpart of `ensure_git_exclude`: drop the CLAUDE.local.md
/// line retro added to the repo's info/exclude. Missing repo/file tolerated.
pub fn remove_git_exclude(project_root: &Path) -> Result<(), CoreError> {
    let io = |e: std::io::Error| CoreError::Io(e.to_string());
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .map_err(io)?;
    if !out.status.success() {
        return Ok(()); // not a git repo
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if common.is_empty() {
        return Ok(());
    }
    let exclude = std::path::Path::new(&common).join("info").join("exclude");
    let Ok(existing) = std::fs::read_to_string(&exclude) else {
        return Ok(());
    };
    if !existing.lines().any(|l| l.trim() == "CLAUDE.local.md") {
        return Ok(());
    }
    let updated: String = existing
        .lines()
        .filter(|l| l.trim() != "CLAUDE.local.md")
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&exclude, updated).map_err(io)
}

fn ensure_git_exclude(project_root: &Path) -> Result<(), CoreError> {
    let io = |e: std::io::Error| CoreError::Io(e.to_string());
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .map_err(io)?;
    if !out.status.success() {
        return Ok(()); // not a git repo
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if common.is_empty() {
        return Ok(());
    }
    let info_dir = std::path::Path::new(&common).join("info");
    std::fs::create_dir_all(&info_dir).map_err(io)?;
    let exclude = info_dir.join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == "CLAUDE.local.md") {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str("CLAUDE.local.md\n");
    std::fs::write(&exclude, updated).map_err(io)
}
