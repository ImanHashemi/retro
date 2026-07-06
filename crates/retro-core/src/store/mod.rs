//! Retro v3 file-based knowledge store.
//!
//! Markdown files under `<root>/knowledge/` are the source of truth.
//! SQLite (`index.db`) is a disposable, rebuildable index — files always win.

pub mod git;
mod node;
mod slug;

pub use node::{Node, NodeType, Scope};
pub use slug::slugify;

use std::path::{Path, PathBuf};

use crate::errors::CoreError;

/// Contents of the store's .gitignore: everything derived or machine-local.
pub const GITIGNORE_CONTENT: &str = "\
index.db
index.db-wal
index.db-shm
health.json
queue/
state/
";

/// Result of loading the store from disk: parsed nodes with their paths,
/// plus warnings for files that were skipped (unparseable).
pub struct LoadResult {
    pub nodes: Vec<(PathBuf, Node)>,
    pub warnings: Vec<String>,
}

/// Handle to a store rooted at a directory (usually `retro_dir()`).
/// All operations are file operations; git integration lives in `store::git`.
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn knowledge_dir(&self) -> PathBuf {
        self.root.join("knowledge")
    }

    fn scope_dir(&self, scope: &Scope) -> PathBuf {
        match scope {
            Scope::Global => self.knowledge_dir().join("global"),
            Scope::Project(slug) => self.knowledge_dir().join("projects").join(slug),
        }
    }

    pub fn node_path(&self, scope: &Scope, id: &str) -> PathBuf {
        self.scope_dir(scope).join(format!("{id}.md"))
    }

    /// Create the directory layout and .gitignore. Idempotent;
    /// never overwrites an existing .gitignore (user may have edited it).
    pub fn ensure_layout(&self) -> Result<(), CoreError> {
        let io = |e: std::io::Error| CoreError::Io(e.to_string());
        std::fs::create_dir_all(self.knowledge_dir().join("global")).map_err(io)?;
        std::fs::create_dir_all(self.knowledge_dir().join("projects")).map_err(io)?;
        let gitignore = self.root.join(".gitignore");
        if !gitignore.exists() {
            std::fs::write(&gitignore, GITIGNORE_CONTENT).map_err(io)?;
        }
        Ok(())
    }

    /// Write a node to its canonical path (creates the project dir if needed).
    pub fn write_node(&self, node: &Node) -> Result<PathBuf, CoreError> {
        let io = |e: std::io::Error| CoreError::Io(e.to_string());
        let path = self.node_path(&node.scope, &node.id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(&path, node.to_markdown()).map_err(io)?;
        Ok(path)
    }

    pub fn get(&self, scope: &Scope, id: &str) -> Result<Option<Node>, CoreError> {
        let path = self.node_path(scope, id);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path).map_err(|e| CoreError::Io(e.to_string()))?;
        // TODO: include the file path in parse errors (load_all adds it; get() callers currently don't).
        Ok(Some(Node::from_markdown(&content)?))
    }

    /// Load every node in the store. Unparseable .md files are skipped
    /// with a warning (matches the JSONL-skipping convention elsewhere).
    /// Layout is fixed depth: knowledge/global/*.md and knowledge/projects/*/*.md.
    pub fn load_all(&self) -> Result<LoadResult, CoreError> {
        let mut result = LoadResult {
            nodes: Vec::new(),
            warnings: Vec::new(),
        };
        let mut dirs = vec![self.knowledge_dir().join("global")];
        let projects_dir = self.knowledge_dir().join("projects");
        if projects_dir.is_dir() {
            let entries =
                std::fs::read_dir(&projects_dir).map_err(|e| CoreError::Io(e.to_string()))?;
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    dirs.push(entry.path());
                }
            }
        }
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            let entries = std::fs::read_dir(&dir).map_err(|e| CoreError::Io(e.to_string()))?;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        result.warnings.push(format!("{}: {}", path.display(), e));
                        continue;
                    }
                };
                match Node::from_markdown(&content) {
                    Ok(node) => result.nodes.push((path, node)),
                    Err(e) => {
                        result.warnings.push(format!("{}: {}", path.display(), e));
                    }
                }
            }
        }
        Ok(result)
    }

    /// Slug for a new node: slugified base, `-2`/`-3`... appended on collision.
    pub fn unique_slug(&self, base: &str, scope: &Scope) -> String {
        let slug = slugify(base);
        if !self.node_path(scope, &slug).exists() {
            return slug;
        }
        let mut i = 2;
        loop {
            let candidate = format!("{slug}-{i}");
            if !self.node_path(scope, &candidate).exists() {
                return candidate;
            }
            i += 1;
        }
    }

    /// Invalidate a node (set `invalidated_by`, touch `updated`).
    /// Returns false if the node does not exist. Never deletes.
    /// NOTE: rewriting normalizes formatting (CRLF → LF, confidence to two decimals).
    pub fn invalidate(&self, scope: &Scope, id: &str, by: &str) -> Result<bool, CoreError> {
        let Some(mut node) = self.get(scope, id)? else {
            return Ok(false);
        };
        node.invalidated_by = Some(by.to_string());
        node.updated = chrono::Utc::now().date_naive();
        self.write_node(&node)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use tempfile::TempDir;

    fn node(id: &str, scope: Scope) -> Node {
        Node {
            id: id.to_string(),
            scope,
            node_type: NodeType::Rule,
            confidence: 0.8,
            sources: vec!["session:abc".to_string()],
            created: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            updated: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            invalidated_by: None,
            body: "Test rule body.".to_string(),
        }
    }

    #[test]
    fn ensure_layout_creates_dirs_and_gitignore() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        assert!(tmp.path().join("knowledge/global").is_dir());
        assert!(tmp.path().join("knowledge/projects").is_dir());
        let gi = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(gi.contains("index.db"));
        assert!(gi.contains("queue/"));
        // idempotent + does not clobber user edits
        std::fs::write(tmp.path().join(".gitignore"), "custom\n").unwrap();
        store.ensure_layout().unwrap();
        let gi = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert_eq!(gi, "custom\n");
    }

    #[test]
    fn node_paths_by_scope() {
        let store = Store::open("/tmp/x");
        assert_eq!(
            store.node_path(&Scope::Global, "my-rule"),
            std::path::Path::new("/tmp/x/knowledge/global/my-rule.md")
        );
        assert_eq!(
            store.node_path(&Scope::Project("proj".to_string()), "my-rule"),
            std::path::Path::new("/tmp/x/knowledge/projects/proj/my-rule.md")
        );
    }

    #[test]
    fn write_then_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        let g = node("global-rule", Scope::Global);
        let p = node("proj-rule", Scope::Project("my-proj".to_string()));
        store.write_node(&g).unwrap();
        store.write_node(&p).unwrap();

        let result = store.load_all().unwrap();
        assert_eq!(result.warnings.len(), 0);
        assert_eq!(result.nodes.len(), 2);
        let ids: Vec<&str> = result.nodes.iter().map(|(_, n)| n.id.as_str()).collect();
        assert!(ids.contains(&"global-rule"));
        assert!(ids.contains(&"proj-rule"));
    }

    #[test]
    fn load_all_skips_unparseable_with_warning() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store.write_node(&node("good", Scope::Global)).unwrap();
        std::fs::write(
            tmp.path().join("knowledge/global/broken.md"),
            "not a node at all",
        )
        .unwrap();

        let result = store.load_all().unwrap();
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("broken.md"));
    }

    #[test]
    fn load_all_ignores_non_md_files() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        std::fs::write(tmp.path().join("knowledge/global/.DS_Store"), "junk").unwrap();
        let result = store.load_all().unwrap();
        assert!(result.nodes.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn get_reads_single_node() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store.write_node(&node("findable", Scope::Global)).unwrap();
        assert!(store.get(&Scope::Global, "findable").unwrap().is_some());
        assert!(store.get(&Scope::Global, "missing").unwrap().is_none());
    }

    #[test]
    fn unique_slug_appends_counter_on_collision() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        assert_eq!(store.unique_slug("My Rule", &Scope::Global), "my-rule");
        store.write_node(&node("my-rule", Scope::Global)).unwrap();
        assert_eq!(store.unique_slug("My Rule", &Scope::Global), "my-rule-2");
        store.write_node(&node("my-rule-2", Scope::Global)).unwrap();
        assert_eq!(store.unique_slug("My Rule", &Scope::Global), "my-rule-3");
        // project scope resolves collisions independently
        assert_eq!(
            store.unique_slug("My Rule", &Scope::Project("p".to_string())),
            "my-rule"
        );
    }

    #[test]
    fn invalidate_sets_field_and_touches_updated() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store.write_node(&node("old-rule", Scope::Global)).unwrap();

        let found = store
            .invalidate(&Scope::Global, "old-rule", "new-rule")
            .unwrap();
        assert!(found);
        let n = store.get(&Scope::Global, "old-rule").unwrap().unwrap();
        assert_eq!(n.invalidated_by.as_deref(), Some("new-rule"));
        assert_eq!(n.updated, chrono::Utc::now().date_naive());

        assert!(!store.invalidate(&Scope::Global, "missing", "x").unwrap());
    }
}
