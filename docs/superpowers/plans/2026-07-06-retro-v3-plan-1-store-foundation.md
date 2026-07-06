# Retro v3 Plan 1: Store Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build retro v3's file-based knowledge store: markdown nodes as source of truth in a git-backed `~/.retro`, with SQLite as a disposable, rebuildable index, exposed via a new `retro reindex` command.

**Architecture:** New `store` module in `retro-core` (nodes as markdown files with strict frontmatter, git layer for auto-commit, FTS5-backed index built from files), plus one new CLI command. Built **alongside** existing v2 code — nothing existing is modified except `lib.rs` (one line), `main.rs`/`commands/mod.rs` (wiring), and CLAUDE.md (docs). v2 keeps working throughout.

**Tech Stack:** Rust (edition 2024, sync only — no tokio), rusqlite (bundled, FTS5 available), chrono, thiserror (`CoreError`) in core / anyhow in CLI, tempfile for tests, shell-out to `git` via `std::process::Command`.

**Spec:** `docs/superpowers/specs/2026-07-06-retro-v3-personal-redesign-design.md` (§4 Storage, §10 CLI `retro reindex`)

## Context for implementers (read first)

- **Conventions in this codebase:** errors in retro-core are `CoreError` (thiserror, `crates/retro-core/src/errors.rs`) — map io errors with `.map_err(|e| CoreError::Io(e.to_string()))`. CLI commands return `anyhow::Result<()>` and use `?` directly on `CoreError`. Git shell-outs use `Command::new("git").args([...])` — never shell strings. String truncation must use existing `truncate_str()` (`crates/retro-core/src/util.rs`) if ever needed — never byte-slice.
- **The store root** is `retro_dir()` from `crates/retro-core/src/config.rs:358` — respects `RETRO_HOME` env var (test isolation). Store code takes an explicit root path; only the CLI resolves `retro_dir()`.
- **Frontmatter is a strict, fixed schema** parsed by hand (no YAML crate — avoids an unmaintained-dep supply-chain flag and keeps parsing exact). Unknown keys are a hard parse error (catches human typos); unparseable files are skipped with a warning at load time (matches the codebase's JSONL-skipping convention), never a crash.
- **The index is disposable by contract:** `build()` deletes and recreates `index.db`. No state may live only in the index. Files always win.
- Run `cargo test -p retro-core` frequently; full `cargo test` before each commit. Current baseline: 278 tests passing (three workspace crates: retro-core, retro-cli, retro-projectors).

## File Structure

```
crates/retro-core/src/store/
├── mod.rs        # Store struct: layout, paths, CRUD, unique_slug, invalidate
├── node.rs       # Node, NodeType, Scope + markdown (de)serialization
├── slug.rs       # slugify()
├── git.rs        # store-repo git ops: ensure_repo, commit_all, push_best_effort
└── index.rs      # SQLite index: build from files, query, freshness fingerprint
crates/retro-core/src/lib.rs          # + pub mod store;
crates/retro-cli/src/commands/reindex.rs   # retro reindex
crates/retro-cli/src/commands/mod.rs       # + pub mod reindex;
crates/retro-cli/src/main.rs               # + Reindex command variant + match arm
CLAUDE.md                                  # docs: v3 store + reindex
```

---

### Task 1: Node types and markdown serialization

**Files:**
- Create: `crates/retro-core/src/store/mod.rs` (module shell)
- Create: `crates/retro-core/src/store/node.rs`
- Modify: `crates/retro-core/src/lib.rs` (add `pub mod store;` after `pub mod scrub;`)

- [ ] **Step 1: Create the module shell and wire it into lib.rs**

`crates/retro-core/src/store/mod.rs`:

```rust
//! Retro v3 file-based knowledge store.
//!
//! Markdown files under `<root>/knowledge/` are the source of truth.
//! SQLite (`index.db`) is a disposable, rebuildable index — files always win.

mod node;

pub use node::{Node, NodeType, Scope};
```

In `crates/retro-core/src/lib.rs`, after the line `pub mod scrub;` add:

```rust
pub mod store;
```

- [ ] **Step 2: Write failing tests for types + serialization**

Create `crates/retro-core/src/store/node.rs` with the test module only (implementation comes in Step 4):

```rust
//! Knowledge node: the atomic unit of the v3 store.
//! One node == one markdown file with strict frontmatter.

use chrono::NaiveDate;

use crate::errors::CoreError;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_node() -> Node {
        Node {
            id: "ab-paired-observations".to_string(),
            scope: Scope::Project("my-api-service".to_string()),
            node_type: NodeType::Rule,
            confidence: 0.9,
            sources: vec![
                "session:1a2b3c4d".to_string(),
                "session:5e6f7a8b".to_string(),
            ],
            created: NaiveDate::from_ymd_opt(2026, 5, 19).unwrap(),
            updated: NaiveDate::from_ymd_opt(2026, 6, 2).unwrap(),
            invalidated_by: None,
            body: "A/B comparisons must always use paired observations.\n\n**Why:** Unpaired comparisons mix traffic distributions.".to_string(),
        }
    }

    #[test]
    fn scope_roundtrip() {
        assert_eq!(Scope::Global.as_str(), "global");
        assert_eq!(
            Scope::Project("my-api".to_string()).as_str(),
            "project/my-api"
        );
        assert_eq!(Scope::parse("global").unwrap(), Scope::Global);
        assert_eq!(
            Scope::parse("project/my-api").unwrap(),
            Scope::Project("my-api".to_string())
        );
        assert!(Scope::parse("team/my-api").is_err());
        assert!(Scope::parse("project/").is_err());
        assert!(Scope::parse("").is_err());
    }

    #[test]
    fn node_type_roundtrip() {
        for (t, s) in [
            (NodeType::Rule, "rule"),
            (NodeType::Preference, "preference"),
            (NodeType::Pattern, "pattern"),
            (NodeType::Memory, "memory"),
        ] {
            assert_eq!(t.as_str(), s);
            assert_eq!(NodeType::parse(s).unwrap(), t);
        }
        assert!(NodeType::parse("skill").is_err());
        assert!(NodeType::parse("").is_err());
    }

    #[test]
    fn is_active_reflects_invalidated_by() {
        let mut n = sample_node();
        assert!(n.is_active());
        n.invalidated_by = Some("newer-rule".to_string());
        assert!(!n.is_active());
    }

    #[test]
    fn to_markdown_emits_fixed_frontmatter_order() {
        let md = sample_node().to_markdown();
        let expected = "\
---
id: ab-paired-observations
scope: project/my-api-service
type: rule
confidence: 0.90
sources: [session:1a2b3c4d, session:5e6f7a8b]
created: 2026-05-19
updated: 2026-06-02
invalidated_by: null
---
A/B comparisons must always use paired observations.

**Why:** Unpaired comparisons mix traffic distributions.
";
        assert_eq!(md, expected);
    }

    #[test]
    fn to_markdown_empty_sources_and_invalidated() {
        let mut n = sample_node();
        n.sources = vec![];
        n.invalidated_by = Some("other-node".to_string());
        let md = n.to_markdown();
        assert!(md.contains("sources: []\n"));
        assert!(md.contains("invalidated_by: other-node\n"));
    }
}
```

- [ ] **Step 3: Run tests to verify they fail to compile (types missing)**

Run: `cargo test -p retro-core store::`
Expected: compile error — `Node`, `NodeType`, `Scope` not found.

- [ ] **Step 4: Implement the types and `to_markdown`**

Add above the test module in `crates/retro-core/src/store/node.rs`:

```rust
/// Node type. v3 collapses v2's six types to four
/// (`directive` → `rule`, `skill` → `pattern`, handled at migration).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    Rule,
    Preference,
    Pattern,
    /// Context-only: stored and browsable, never projected.
    Memory,
}

impl NodeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            NodeType::Rule => "rule",
            NodeType::Preference => "preference",
            NodeType::Pattern => "pattern",
            NodeType::Memory => "memory",
        }
    }

    pub fn parse(s: &str) -> Result<Self, CoreError> {
        match s {
            "rule" => Ok(NodeType::Rule),
            "preference" => Ok(NodeType::Preference),
            "pattern" => Ok(NodeType::Pattern),
            "memory" => Ok(NodeType::Memory),
            other => Err(CoreError::Parse(format!("unknown node type: {other:?}"))),
        }
    }
}

/// Where a node applies: everywhere, or one project (by slug).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project(String),
}

impl Scope {
    pub fn as_str(&self) -> String {
        match self {
            Scope::Global => "global".to_string(),
            Scope::Project(slug) => format!("project/{slug}"),
        }
    }

    pub fn parse(s: &str) -> Result<Self, CoreError> {
        if s == "global" {
            return Ok(Scope::Global);
        }
        if let Some(slug) = s.strip_prefix("project/") {
            if !slug.is_empty() {
                return Ok(Scope::Project(slug.to_string()));
            }
        }
        Err(CoreError::Parse(format!("invalid scope: {s:?}")))
    }
}

/// One knowledge node. Serialized as one markdown file:
/// strict frontmatter between `---` delimiters, then the body.
/// The body is stored WITHOUT a trailing newline; `to_markdown`
/// appends exactly one (normalization keeps round-trips stable).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: String,
    pub scope: Scope,
    pub node_type: NodeType,
    pub confidence: f64,
    pub sources: Vec<String>,
    pub created: NaiveDate,
    pub updated: NaiveDate,
    pub invalidated_by: Option<String>,
    pub body: String,
}

impl Node {
    pub fn is_active(&self) -> bool {
        self.invalidated_by.is_none()
    }

    pub fn to_markdown(&self) -> String {
        let sources = self.sources.join(", ");
        let invalidated = self
            .invalidated_by
            .as_deref()
            .unwrap_or("null");
        format!(
            "---\nid: {}\nscope: {}\ntype: {}\nconfidence: {}\nsources: [{}]\ncreated: {}\nupdated: {}\ninvalidated_by: {}\n---\n{}\n",
            self.id,
            self.scope.as_str(),
            self.node_type.as_str(),
            self.confidence,
            sources,
            self.created,
            self.updated,
            invalidated,
            self.body.trim_end_matches('\n'),
        )
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p retro-core store::`
Expected: 5 tests PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo test -p retro-core && git add crates/retro-core/src/store/ crates/retro-core/src/lib.rs && git commit -m "feat(store): v3 node types and markdown serialization"
```

---

### Task 2: Frontmatter parsing (`Node::from_markdown`)

**Files:**
- Modify: `crates/retro-core/src/store/node.rs`

- [ ] **Step 1: Write failing tests**

Add inside the `tests` module in `crates/retro-core/src/store/node.rs`:

```rust
    #[test]
    fn from_markdown_roundtrip() {
        let n = sample_node();
        let parsed = Node::from_markdown(&n.to_markdown()).unwrap();
        assert_eq!(parsed, n);
    }

    #[test]
    fn from_markdown_roundtrip_with_invalidated_and_empty_sources() {
        let mut n = sample_node();
        n.sources = vec![];
        n.invalidated_by = Some("other".to_string());
        let parsed = Node::from_markdown(&n.to_markdown()).unwrap();
        assert_eq!(parsed, n);
    }

    #[test]
    fn from_markdown_body_may_contain_dashes() {
        let mut n = sample_node();
        n.body = "line one\n---\nline after a dash rule".to_string();
        let parsed = Node::from_markdown(&n.to_markdown()).unwrap();
        assert_eq!(parsed.body, n.body);
    }

    #[test]
    fn from_markdown_missing_required_key_errors() {
        let md = "---\nid: x\nscope: global\ntype: rule\n---\nbody\n";
        let err = Node::from_markdown(md).unwrap_err();
        assert!(err.to_string().contains("confidence"), "got: {err}");
    }

    #[test]
    fn from_markdown_unknown_key_errors() {
        let md = sample_node().to_markdown().replace("updated:", "updatedd:");
        let err = Node::from_markdown(&md).unwrap_err();
        assert!(err.to_string().contains("updatedd"), "got: {err}");
    }

    #[test]
    fn from_markdown_bad_values_error() {
        let base = sample_node().to_markdown();
        for (needle, replacement) in [
            ("confidence: 0.9", "confidence: high"),
            ("created: 2026-05-19", "created: yesterday"),
            ("type: rule", "type: law"),
            ("scope: project/my-api-service", "scope: team/x"),
        ] {
            let md = base.replace(needle, replacement);
            assert!(Node::from_markdown(&md).is_err(), "should fail: {replacement}");
        }
    }

    #[test]
    fn from_markdown_requires_frontmatter_delimiters() {
        assert!(Node::from_markdown("no frontmatter here").is_err());
        assert!(Node::from_markdown("---\nid: x\nno closing delimiter").is_err());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p retro-core store::`
Expected: compile error — `from_markdown` not found.

- [ ] **Step 3: Implement `from_markdown`**

Add to `impl Node` in `crates/retro-core/src/store/node.rs`:

```rust
    /// Parse a node from markdown with strict frontmatter.
    /// Unknown keys are an error (catches human typos); the fixed
    /// schema is owned by this binary — migration controls format changes.
    pub fn from_markdown(content: &str) -> Result<Node, CoreError> {
        let rest = content
            .strip_prefix("---\n")
            .ok_or_else(|| CoreError::Parse("missing frontmatter open delimiter".to_string()))?;
        let (front, body) = rest
            .split_once("\n---\n")
            .ok_or_else(|| CoreError::Parse("missing frontmatter close delimiter".to_string()))?;

        let mut id: Option<String> = None;
        let mut scope: Option<Scope> = None;
        let mut node_type: Option<NodeType> = None;
        let mut confidence: Option<f64> = None;
        let mut sources: Vec<String> = Vec::new();
        let mut created: Option<NaiveDate> = None;
        let mut updated: Option<NaiveDate> = None;
        let mut invalidated_by: Option<String> = None;

        for line in front.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let (key, value) = line.split_once(':').ok_or_else(|| {
                CoreError::Parse(format!("invalid frontmatter line: {line:?}"))
            })?;
            let key = key.trim();
            let value = value.trim();
            match key {
                "id" => id = Some(value.to_string()),
                "scope" => scope = Some(Scope::parse(value)?),
                "type" => node_type = Some(NodeType::parse(value)?),
                "confidence" => {
                    confidence = Some(value.parse::<f64>().map_err(|_| {
                        CoreError::Parse(format!("invalid confidence: {value:?}"))
                    })?)
                }
                "sources" => {
                    let inner = value
                        .strip_prefix('[')
                        .and_then(|v| v.strip_suffix(']'))
                        .ok_or_else(|| {
                            CoreError::Parse(format!("invalid sources list: {value:?}"))
                        })?;
                    sources = inner
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect();
                }
                "created" => created = Some(parse_date(value)?),
                "updated" => updated = Some(parse_date(value)?),
                "invalidated_by" => {
                    invalidated_by = match value {
                        "null" | "" => None,
                        other => Some(other.to_string()),
                    }
                }
                other => {
                    return Err(CoreError::Parse(format!(
                        "unknown frontmatter key: {other:?}"
                    )));
                }
            }
        }

        let missing = |k: &str| CoreError::Parse(format!("missing frontmatter key: {k}"));
        Ok(Node {
            id: id.ok_or_else(|| missing("id"))?,
            scope: scope.ok_or_else(|| missing("scope"))?,
            node_type: node_type.ok_or_else(|| missing("type"))?,
            confidence: confidence.ok_or_else(|| missing("confidence"))?,
            sources,
            created: created.ok_or_else(|| missing("created"))?,
            updated: updated.ok_or_else(|| missing("updated"))?,
            invalidated_by,
            body: body.trim_end_matches('\n').to_string(),
        })
    }
```

And add the free function below the `impl` block:

```rust
fn parse_date(s: &str) -> Result<NaiveDate, CoreError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|_| CoreError::Parse(format!("invalid date: {s:?}")))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p retro-core store::`
Expected: 12 tests PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo test -p retro-core && git add crates/retro-core/src/store/node.rs && git commit -m "feat(store): strict frontmatter parsing with roundtrip guarantee"
```

---

### Task 3: Slug generation

**Files:**
- Create: `crates/retro-core/src/store/slug.rs`
- Modify: `crates/retro-core/src/store/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `crates/retro-core/src/store/slug.rs`:

```rust
//! Kebab-case slug generation for node ids and project directory names.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("AB Paired Observations"), "ab-paired-observations");
        assert_eq!(slugify("use_pytest fixtures!"), "use-pytest-fixtures");
        assert_eq!(slugify("already-kebab-case"), "already-kebab-case");
    }

    #[test]
    fn slugify_collapses_and_trims_dashes() {
        assert_eq!(slugify("--weird   input--"), "weird-input");
        assert_eq!(slugify("a///b"), "a-b");
    }

    #[test]
    fn slugify_drops_non_ascii() {
        assert_eq!(slugify("café rules ☕"), "caf-rules");
    }

    #[test]
    fn slugify_caps_length_at_60() {
        let long = "x".repeat(100);
        assert_eq!(slugify(&long).len(), 60);
    }

    #[test]
    fn slugify_empty_and_symbol_only_fall_back() {
        assert_eq!(slugify(""), "node");
        assert_eq!(slugify("!!!"), "node");
    }
}
```

In `crates/retro-core/src/store/mod.rs`, change the module list to:

```rust
mod node;
mod slug;

pub use node::{Node, NodeType, Scope};
pub use slug::slugify;
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test -p retro-core store::slug`
Expected: compile error — `slugify` not found.

- [ ] **Step 3: Implement `slugify`**

Add above the test module in `crates/retro-core/src/store/slug.rs`:

```rust
/// Lowercase ASCII-alphanumeric kebab-case, dashes collapsed, max 60 chars.
/// Falls back to "node" for inputs with no usable characters.
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = true; // suppress leading dash
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 60 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "node".to_string()
    } else {
        out
    }
}
```

Note: length check uses `out.len()` on an ASCII-only string, so byte length == char length; the final trim can only shorten. (General truncation elsewhere must use `truncate_str()`; here the string is guaranteed ASCII.)

- [ ] **Step 4: Run tests — the length test expects exactly 60**

Run: `cargo test -p retro-core store::slug`
Expected: PASS (input of 100 `x`s: loop breaks at 60, no dashes to trim).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo test -p retro-core && git add crates/retro-core/src/store/ && git commit -m "feat(store): slugify helper"
```

---

### Task 4: Store layout and CRUD

**Files:**
- Modify: `crates/retro-core/src/store/mod.rs`

- [ ] **Step 1: Write failing tests**

Add at the bottom of `crates/retro-core/src/store/mod.rs`:

```rust
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
    }

    #[test]
    fn invalidate_sets_field_and_touches_updated() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store.write_node(&node("old-rule", Scope::Global)).unwrap();

        let found = store.invalidate(&Scope::Global, "old-rule", "new-rule").unwrap();
        assert!(found);
        let n = store.get(&Scope::Global, "old-rule").unwrap().unwrap();
        assert_eq!(n.invalidated_by.as_deref(), Some("new-rule"));
        assert_eq!(n.updated, chrono::Utc::now().date_naive());

        assert!(!store.invalidate(&Scope::Global, "missing", "x").unwrap());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test -p retro-core store::`
Expected: compile error — `Store` not found.

- [ ] **Step 3: Implement `Store`**

Add to `crates/retro-core/src/store/mod.rs` (between the `pub use` lines and the test module):

```rust
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
        let content = std::fs::read_to_string(&path)
            .map_err(|e| CoreError::Io(e.to_string()))?;
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
            let entries = std::fs::read_dir(&projects_dir)
                .map_err(|e| CoreError::Io(e.to_string()))?;
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
            let entries =
                std::fs::read_dir(&dir).map_err(|e| CoreError::Io(e.to_string()))?;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        result
                            .warnings
                            .push(format!("{}: {}", path.display(), e));
                        continue;
                    }
                };
                match Node::from_markdown(&content) {
                    Ok(node) => result.nodes.push((path, node)),
                    Err(e) => {
                        result
                            .warnings
                            .push(format!("{}: {}", path.display(), e));
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
    pub fn invalidate(
        &self,
        scope: &Scope,
        id: &str,
        by: &str,
    ) -> Result<bool, CoreError> {
        let Some(mut node) = self.get(scope, id)? else {
            return Ok(false);
        };
        node.invalidated_by = Some(by.to_string());
        node.updated = chrono::Utc::now().date_naive();
        self.write_node(&node)?;
        Ok(true)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p retro-core store::`
Expected: 25 tests PASS (12 node + 5 slug + 8 store).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo test -p retro-core && git add crates/retro-core/src/store/mod.rs && git commit -m "feat(store): store layout, CRUD, unique slugs, invalidation"
```

---

### Task 5: Store git layer

**Files:**
- Create: `crates/retro-core/src/store/git.rs`
- Modify: `crates/retro-core/src/store/mod.rs` (add `pub mod git;`)

Note: the existing `crates/retro-core/src/git.rs` operates on the *current working directory* (v2 project repos). The store git layer is separate on purpose: every command targets the store root explicitly via `git -C <root>`, and it needs none of the v2 branch/PR machinery.

- [ ] **Step 1: Write failing tests**

Create `crates/retro-core/src/store/git.rs`:

```rust
//! Git operations for the store repository (`~/.retro`).
//! All commands run against an explicit root via `git -C <root>`.
//! Commits are local-first; pushing is strictly best-effort.

use std::path::Path;
use std::process::Command;

use crate::errors::CoreError;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn ensure_repo_initializes_once() {
        let tmp = TempDir::new().unwrap();
        assert!(!is_repo(tmp.path()));
        let created = ensure_repo(tmp.path()).unwrap();
        assert!(created);
        assert!(is_repo(tmp.path()));
        // idempotent
        let created_again = ensure_repo(tmp.path()).unwrap();
        assert!(!created_again);
        // has an initial commit (HEAD resolves)
        assert!(head_exists(tmp.path()));
    }

    #[test]
    fn commit_all_commits_changes_and_reports_clean() {
        let tmp = TempDir::new().unwrap();
        ensure_repo(tmp.path()).unwrap();
        // clean repo → no commit
        assert!(!commit_all(tmp.path(), "retro: noop").unwrap());
        // new file → commit
        std::fs::write(tmp.path().join("note.md"), "hello").unwrap();
        assert!(has_changes(tmp.path()).unwrap());
        assert!(commit_all(tmp.path(), "retro: learn note").unwrap());
        assert!(!has_changes(tmp.path()).unwrap());
        // modification → commit
        std::fs::write(tmp.path().join("note.md"), "hello again").unwrap();
        assert!(commit_all(tmp.path(), "user: edit note").unwrap());
    }

    #[test]
    fn push_without_remote_reports_no_remote() {
        let tmp = TempDir::new().unwrap();
        ensure_repo(tmp.path()).unwrap();
        assert!(matches!(push_best_effort(tmp.path()), PushOutcome::NoRemote));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test -p retro-core store::git`
Expected: compile error — functions not found. (Also add `pub mod git;` to `crates/retro-core/src/store/mod.rs` under `mod node;` now, or this file isn't compiled at all.)

- [ ] **Step 3: Implement the git layer**

Add above the test module in `crates/retro-core/src/store/git.rs`:

```rust
/// Outcome of a best-effort push. Failures are data, not errors —
/// callers record them in health, they never abort a pipeline.
#[derive(Debug)]
pub enum PushOutcome {
    Pushed,
    NoRemote,
    Failed(String),
}

fn git(root: &Path, args: &[&str]) -> Result<std::process::Output, CoreError> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| CoreError::Io(format!("failed to run git: {e}")))
}

pub fn is_repo(root: &Path) -> bool {
    root.join(".git").exists()
}

pub fn head_exists(root: &Path) -> bool {
    git(root, &["rev-parse", "--verify", "HEAD"])
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Initialize the store repo if needed. Returns true if newly created.
/// Sets a local identity fallback and disables gpg signing locally so
/// automated commits never depend on the user's global git setup.
pub fn ensure_repo(root: &Path) -> Result<bool, CoreError> {
    if is_repo(root) {
        return Ok(false);
    }
    run_checked(root, &["init"])?;
    // Local identity fallback: only set if unset globally.
    let email_set = git(root, &["config", "user.email"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !email_set {
        run_checked(root, &["config", "user.email", "retro@localhost"])?;
        run_checked(root, &["config", "user.name", "retro"])?;
    }
    run_checked(root, &["config", "commit.gpgsign", "false"])?;
    run_checked(root, &["add", "-A"])?;
    run_checked(
        root,
        &["commit", "--allow-empty", "-m", "retro: initialize store"],
    )?;
    Ok(true)
}

pub fn has_changes(root: &Path) -> Result<bool, CoreError> {
    let out = git(root, &["status", "--porcelain"])?;
    if !out.status.success() {
        return Err(CoreError::Io(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(!out.stdout.is_empty())
}

/// Stage everything and commit. Returns true if a commit was made,
/// false if the tree was clean. Never errors on "nothing to commit".
pub fn commit_all(root: &Path, message: &str) -> Result<bool, CoreError> {
    if !has_changes(root)? {
        return Ok(false);
    }
    run_checked(root, &["add", "-A"])?;
    run_checked(root, &["commit", "-m", message])?;
    Ok(true)
}

/// Push to origin if a remote exists. Never fails the caller.
pub fn push_best_effort(root: &Path) -> PushOutcome {
    let remotes = match git(root, &["remote"]) {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => return PushOutcome::Failed(String::from_utf8_lossy(&o.stderr).to_string()),
        Err(e) => return PushOutcome::Failed(e.to_string()),
    };
    if remotes.is_empty() {
        return PushOutcome::NoRemote;
    }
    match git(root, &["push", "origin", "HEAD"]) {
        Ok(o) if o.status.success() => PushOutcome::Pushed,
        Ok(o) => PushOutcome::Failed(String::from_utf8_lossy(&o.stderr).to_string()),
        Err(e) => PushOutcome::Failed(e.to_string()),
    }
}

fn run_checked(root: &Path, args: &[&str]) -> Result<(), CoreError> {
    let out = git(root, args)?;
    if !out.status.success() {
        return Err(CoreError::Io(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p retro-core store::git`
Expected: 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo test -p retro-core && git add crates/retro-core/src/store/ && git commit -m "feat(store): git layer — init, auto-commit, best-effort push"
```

---

### Task 6: SQLite index — build, query, freshness

**Files:**
- Create: `crates/retro-core/src/store/index.rs`
- Modify: `crates/retro-core/src/store/mod.rs` (add `pub mod index;`)

The index is **disposable by contract**: `build()` deletes `index.db` and recreates it from the files. FTS5 is available (bundled SQLite compiles with `SQLITE_ENABLE_FTS5`). The freshness fingerprint is a sorted `path|mtime|len` listing stored at build time; `is_fresh` recomputes and compares (used by `retro doctor` in Plan 3).

- [ ] **Step 1: Write failing tests**

Create `crates/retro-core/src/store/index.rs`:

```rust
//! Disposable SQLite index over the file store.
//! `build()` fully rebuilds `index.db` from the markdown files.
//! No state lives here that is not derivable from the files.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::Store;
use crate::errors::CoreError;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Node, NodeType, Scope};
    use chrono::NaiveDate;
    use tempfile::TempDir;

    fn seeded_store() -> (TempDir, Store) {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        let mk = |id: &str, scope: Scope, t: NodeType, inv: Option<&str>, body: &str| Node {
            id: id.to_string(),
            scope,
            node_type: t,
            confidence: 0.8,
            sources: vec![format!("session:src-{id}")],
            created: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            updated: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            invalidated_by: inv.map(String::from),
            body: body.to_string(),
        };
        store
            .write_node(&mk("g-rule", Scope::Global, NodeType::Rule, None, "always run smoke tests first"))
            .unwrap();
        store
            .write_node(&mk(
                "p-pattern",
                Scope::Project("my-proj".to_string()),
                NodeType::Pattern,
                None,
                "paired observations for experiments",
            ))
            .unwrap();
        store
            .write_node(&mk("dead-rule", Scope::Global, NodeType::Rule, Some("g-rule"), "obsolete advice"))
            .unwrap();
        (tmp, store)
    }

    #[test]
    fn build_indexes_all_nodes_and_is_rebuildable() {
        let (_tmp, store) = seeded_store();
        let stats = build(&store).unwrap();
        assert_eq!(stats.nodes, 3);
        assert!(stats.warnings.is_empty());
        // rebuild from scratch works (delete + recreate)
        let stats = build(&store).unwrap();
        assert_eq!(stats.nodes, 3);
    }

    #[test]
    fn query_filters_by_scope_type_and_active() {
        let (_tmp, store) = seeded_store();
        build(&store).unwrap();
        let conn = open(store.root()).unwrap();

        let all = query(&conn, &NodeFilter::default()).unwrap();
        assert_eq!(all.len(), 3);

        let active_only = query(
            &conn,
            &NodeFilter { active_only: true, ..Default::default() },
        )
        .unwrap();
        assert_eq!(active_only.len(), 2);

        let global = query(
            &conn,
            &NodeFilter { scope: Some("global".to_string()), ..Default::default() },
        )
        .unwrap();
        assert_eq!(global.len(), 2);

        let patterns = query(
            &conn,
            &NodeFilter { node_type: Some("pattern".to_string()), ..Default::default() },
        )
        .unwrap();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].id, "p-pattern");
        assert_eq!(patterns[0].sources, vec!["session:src-p-pattern".to_string()]);
    }

    #[test]
    fn query_full_text_search() {
        let (_tmp, store) = seeded_store();
        build(&store).unwrap();
        let conn = open(store.root()).unwrap();
        let hits = query(
            &conn,
            &NodeFilter { text: Some("paired observations".to_string()), ..Default::default() },
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "p-pattern");
        // hostile input must not cause an FTS syntax error
        let hits = query(
            &conn,
            &NodeFilter { text: Some("\"unbalanced -NOT (".to_string()), ..Default::default() },
        )
        .unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn freshness_detects_file_changes() {
        let (_tmp, store) = seeded_store();
        build(&store).unwrap();
        let conn = open(store.root()).unwrap();
        assert!(is_fresh(&store, &conn).unwrap());
        // adding a node makes the index stale
        store
            .write_node(&Node {
                id: "new-rule".to_string(),
                scope: Scope::Global,
                node_type: NodeType::Rule,
                confidence: 0.7,
                sources: vec![],
                created: NaiveDate::from_ymd_opt(2026, 7, 2).unwrap(),
                updated: NaiveDate::from_ymd_opt(2026, 7, 2).unwrap(),
                invalidated_by: None,
                body: "fresh".to_string(),
            })
            .unwrap();
        assert!(!is_fresh(&store, &conn).unwrap());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail to compile**

Run: `cargo test -p retro-core store::index`
Expected: compile error — `build`, `open`, `query`, `NodeFilter`, `is_fresh` not found. (Add `pub mod index;` to `crates/retro-core/src/store/mod.rs` now.)

- [ ] **Step 3: Implement the index**

Add above the test module in `crates/retro-core/src/store/index.rs`:

```rust
/// Result of an index build.
pub struct IndexStats {
    pub nodes: usize,
    pub warnings: Vec<String>,
}

/// Query filter; all fields are AND-combined. Default = everything.
#[derive(Default)]
pub struct NodeFilter {
    pub scope: Option<String>,
    pub node_type: Option<String>,
    pub active_only: bool,
    pub text: Option<String>,
}

/// One row from the index (denormalized for surfaces).
pub struct NodeRow {
    pub id: String,
    pub scope: String,
    pub node_type: String,
    pub confidence: f64,
    pub active: bool,
    pub created: String,
    pub updated: String,
    pub invalidated_by: Option<String>,
    pub body: String,
    pub path: String,
    pub sources: Vec<String>,
}

pub fn index_path(store_root: &Path) -> PathBuf {
    store_root.join("index.db")
}

pub fn open(store_root: &Path) -> Result<Connection, CoreError> {
    Ok(Connection::open(index_path(store_root))?)
}

/// Full rebuild: delete index.db, recreate schema, insert every node.
pub fn build(store: &Store) -> Result<IndexStats, CoreError> {
    let db = index_path(store.root());
    for suffix in ["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{}", db.display(), suffix));
        if p.exists() {
            std::fs::remove_file(&p).map_err(|e| CoreError::Io(e.to_string()))?;
        }
    }
    let conn = Connection::open(&db)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE nodes (
             id TEXT NOT NULL,
             scope TEXT NOT NULL,
             type TEXT NOT NULL,
             confidence REAL NOT NULL,
             active INTEGER NOT NULL,
             created TEXT NOT NULL,
             updated TEXT NOT NULL,
             invalidated_by TEXT,
             body TEXT NOT NULL,
             path TEXT NOT NULL,
             PRIMARY KEY (scope, id)
         );
         CREATE TABLE node_sources (
             scope TEXT NOT NULL,
             node_id TEXT NOT NULL,
             source TEXT NOT NULL
         );
         CREATE VIRTUAL TABLE nodes_fts USING fts5(id, scope, body);
         PRAGMA user_version = 1;",
    )?;

    let loaded = store.load_all()?;
    for (path, node) in &loaded.nodes {
        let scope = node.scope.to_string();
        conn.execute(
            "INSERT INTO nodes (id, scope, type, confidence, active, created, updated, invalidated_by, body, path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                node.id,
                scope,
                node.node_type.as_str(),
                node.confidence,
                node.is_active() as i64,
                node.created.to_string(),
                node.updated.to_string(),
                node.invalidated_by,
                node.body,
                path.display().to_string(),
            ],
        )?;
        for source in &node.sources {
            conn.execute(
                "INSERT INTO node_sources (scope, node_id, source) VALUES (?1, ?2, ?3)",
                rusqlite::params![scope, node.id, source],
            )?;
        }
        conn.execute(
            "INSERT INTO nodes_fts (id, scope, body) VALUES (?1, ?2, ?3)",
            rusqlite::params![node.id, scope, node.body],
        )?;
    }
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('fingerprint', ?1)",
        rusqlite::params![fingerprint(store)?],
    )?;
    Ok(IndexStats {
        nodes: loaded.nodes.len(),
        warnings: loaded.warnings,
    })
}

pub fn query(conn: &Connection, filter: &NodeFilter) -> Result<Vec<NodeRow>, CoreError> {
    let mut sql = String::from(
        "SELECT id, scope, type, confidence, active, created, updated, invalidated_by, body, path
         FROM nodes WHERE 1=1",
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(scope) = &filter.scope {
        sql.push_str(" AND scope = ?");
        params.push(Box::new(scope.clone()));
    }
    if let Some(t) = &filter.node_type {
        sql.push_str(" AND type = ?");
        params.push(Box::new(t.clone()));
    }
    if filter.active_only {
        sql.push_str(" AND active = 1");
    }
    if let Some(text) = &filter.text {
        sql.push_str(
            " AND (scope || '/' || id) IN (SELECT scope || '/' || id FROM nodes_fts WHERE nodes_fts MATCH ?)",
        );
        params.push(Box::new(fts_escape(text)));
    }
    sql.push_str(" ORDER BY scope, id");

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|p| p.as_ref()).collect();
    let mut rows_iter = stmt.query(param_refs.as_slice())?;
    let mut rows = Vec::new();
    while let Some(row) = rows_iter.next()? {
        rows.push(NodeRow {
            id: row.get(0)?,
            scope: row.get(1)?,
            node_type: row.get(2)?,
            confidence: row.get(3)?,
            active: row.get::<_, i64>(4)? != 0,
            created: row.get(5)?,
            updated: row.get(6)?,
            invalidated_by: row.get(7)?,
            body: row.get(8)?,
            path: row.get(9)?,
            sources: Vec::new(),
        });
    }
    for r in &mut rows {
        let mut stmt = conn.prepare(
            "SELECT source FROM node_sources WHERE scope = ?1 AND node_id = ?2 ORDER BY source",
        )?;
        let sources = stmt
            .query_map(rusqlite::params![r.scope, r.id], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        r.sources = sources;
    }
    Ok(rows)
}

/// Wrap each whitespace-separated term in double quotes so hostile
/// input can't hit FTS5 query-syntax errors. Quotes inside terms are stripped.
fn fts_escape(input: &str) -> String {
    input
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Cheap store fingerprint: sorted `path|mtime|len` lines.
/// String comparison — no hashing dependency needed.
pub fn fingerprint(store: &Store) -> Result<String, CoreError> {
    let loaded = store.load_all()?;
    let mut lines: Vec<String> = Vec::with_capacity(loaded.nodes.len());
    for (path, _) in &loaded.nodes {
        let meta = std::fs::metadata(path).map_err(|e| CoreError::Io(e.to_string()))?;
        let mtime = meta
            .modified()
            .map_err(|e| CoreError::Io(e.to_string()))?
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        lines.push(format!("{}|{}|{}", path.display(), mtime, meta.len()));
    }
    lines.sort();
    Ok(lines.join("\n"))
}

/// True if the index was built from the store's current file state.
pub fn is_fresh(store: &Store, conn: &Connection) -> Result<bool, CoreError> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'fingerprint'",
            [],
            |row| row.get(0),
        )
        .ok();
    match stored {
        Some(fp) => Ok(fp == fingerprint(store)?),
        None => Ok(false),
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p retro-core store::index`
Expected: 4 tests PASS. If `nodes_fts` creation fails with "no such module: fts5", stop and report — do not work around silently (bundled SQLite is expected to include FTS5; a failure here means a dependency change that needs human review).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo test -p retro-core && git add crates/retro-core/src/store/ && git commit -m "feat(store): disposable SQLite index with FTS5 search and freshness check"
```

---

### Task 7: `retro reindex` CLI command

**Files:**
- Create: `crates/retro-cli/src/commands/reindex.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs` (add `pub mod reindex;` to the module list, alphabetical: between `pub mod patterns;` and `pub mod review;`)
- Modify: `crates/retro-cli/src/main.rs` (new command variant + match arm)

- [ ] **Step 1: Implement the command**

Create `crates/retro-cli/src/commands/reindex.rs`:

```rust
use anyhow::Result;
use colored::Colorize;
use retro_core::config::retro_dir;
use retro_core::store::{Store, index};

/// Rebuild the v3 store index (`index.db`) from the markdown files.
/// The index is disposable — this is always safe to run.
pub fn run() -> Result<()> {
    let store = Store::open(retro_dir());
    store.ensure_layout()?;
    let stats = index::build(&store)?;
    for warning in &stats.warnings {
        eprintln!("{} {}", "warning:".yellow(), warning);
    }
    println!(
        "Indexed {} node(s) from {}",
        stats.nodes,
        store.root().join("knowledge").display()
    );
    Ok(())
}
```

- [ ] **Step 2: Wire into the CLI**

In `crates/retro-cli/src/commands/mod.rs`, add to the module list:

```rust
pub mod reindex;
```

In `crates/retro-cli/src/main.rs`, add a variant to the `Commands` enum (place it after the `Status` variant, around line 93):

```rust
    /// Rebuild the v3 store index from knowledge files (safe anytime)
    Reindex,
```

And in the `match` dispatching commands in `main()`, add the arm (mirror the style of the `Commands::Status` arm found there):

```rust
        Commands::Reindex => commands::reindex::run(),
```

Note: the match arms may or may not end with `?`/`,` uniformly — copy the exact style of the surrounding arms.

- [ ] **Step 3: Verify build and behavior manually**

```bash
cargo build
RETRO_HOME=$(mktemp -d) ./target/debug/retro reindex
```

Expected output: `Indexed 0 node(s) from /tmp/<tmpdir>/knowledge` and exit code 0. Then verify it indexes a real node:

```bash
export RETRO_TEST_HOME=$(mktemp -d)
mkdir -p "$RETRO_TEST_HOME/knowledge/global"
printf -- '---\nid: test-rule\nscope: global\ntype: rule\nconfidence: 0.8\nsources: []\ncreated: 2026-07-06\nupdated: 2026-07-06\ninvalidated_by: null\n---\nTest body.\n' > "$RETRO_TEST_HOME/knowledge/global/test-rule.md"
RETRO_HOME="$RETRO_TEST_HOME" ./target/debug/retro reindex
```

Expected: `Indexed 1 node(s) ...`, and `$RETRO_TEST_HOME/index.db` exists.

- [ ] **Step 4: Run the full test suite**

Run: `cargo test`
Expected: all tests pass (baseline 278 + ~32 new).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && git add crates/retro-cli/src/ && git commit -m "feat(cli): retro reindex command"
```

---

### Task 8: Documentation and plan completion

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Document the v3 store in CLAUDE.md**

In `CLAUDE.md`, under `## Implementation Status`, add a new subsection after the `### v2 "The Watcher" (retro 2.0)` block:

```markdown
### v3 "Personal" (in progress)

Spec: `docs/superpowers/specs/2026-07-06-retro-v3-personal-redesign-design.md`.

- **Plan 1: DONE** — Store foundation. File-based knowledge store (`retro-core/src/store/`):
  markdown nodes with strict frontmatter as source of truth under `~/.retro/knowledge/`,
  git-backed mutations (`store::git`), disposable SQLite index with FTS5 (`store::index`),
  `retro reindex` command. v2 continues to work unchanged alongside.
```

And in the `## Commands Overview` v2 table area, add a note row to the Core Commands table:

```markdown
| `retro reindex` | (v3) Rebuild the store index from knowledge files |
```

- [ ] **Step 2: Final verification**

```bash
cargo test && cargo run -- --help | grep -i reindex
```

Expected: all tests pass; `reindex` appears in help output.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md && git commit -m "docs: v3 plan 1 store foundation status"
```

---

## Out of scope for Plan 1 (comes in Plans 2–3)

- Hooks, observe/queue, `retro brief`, registration — Plan 2
- Analysis sink → store, reconciliation, projection (CLAUDE.md / CLAUDE.local.md) — Plan 2
- Backup remote setup in `retro init`, auto-push wiring — Plan 2
- Dashboard, health.json, doctor, lint, migrate, uninstall, v1/v2 deletion, scenario tests — Plan 3
