# Context Curator — Plan 1: Data Model & Config Foundation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the schema and configuration the Context Curator needs — `archived` / `archived_reason` on `Node` (emitted only when set), `is_active()` honouring them, a `[curator]` config section, a configurable `merge_similarity` threaded into `lint`, and a store-format marker — with no curator behaviour yet.

**Architecture:** Additive and defensive. Two optional frontmatter keys are written *only* on nodes that are actually archived, so a store synced to a machine running an older binary stays byte-compatible for every untouched node. `is_active()` gains one clause, and projection / reindex / lint inherit archived-exclusion for free because they already gate on it. Config grows a `[curator]` section whose only immediate consumer is `lint` (the hardcoded similarity literals move into config).

**Tech Stack:** Rust (sync, no async), `serde` + `toml` for config, `chrono::NaiveDate` for dates, inline `#[cfg(test)] mod tests` with `tempfile::TempDir`.

---

## Plan set

This plan is the first of four derived from `docs/superpowers/specs/2026-08-10-retro-v3-context-curator-design.md`. The spec's closing note asks whether the skill generator should be its own plan "with the static core shipping first"; it should, and the split is:

| Plan | Scope |
|---|---|
| **1 (this)** | Data model (`archived`, `archived_reason`, `is_active`), `[curator]` config, `merge_similarity` threaded into lint, `meta.toml` store-format marker |
| 2 | Static curator core: `store::curate`, shared detection, body-preserving merge sweep, archive-stale, per-run caps, first-pass gate, review-queue *storage*, `chore(curator)` commit, `retro curate [--dry-run] [--undo-last]`, runner stage + mutated-scope reprojection |
| 3 | Surfaces & analysis layer: `GET/POST /api/review`, archived bucket + column + `POST /api/node/unarchive`, doctor hygiene report, intake-filter prompt changes, conflict instrumentation |
| 4 | Skill extraction: `execute_agentic` on the `AnalysisBackend` trait, `skill_suggestion`, the generator (built, not reused), validation gate, store-held skills projected into `claude_dir/skills/`, absorb re-discovered nodes |

Plan 1 ships working, testable software on its own: the schema round-trips, the config parses, and `lint` becomes configurable. Nothing in it changes retro's runtime behaviour for an existing store.

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/retro-core/src/store/node.rs` | `Node` schema, strict frontmatter serialise/parse, `is_active()` | Modify — two fields, emit-only-when-set serialiser (now fallible), two parser arms, `is_active()` clause |
| `crates/retro-core/src/config.rs` | Typed config + defaults | Modify — `CuratorConfig`, `CuratorPolicy` enum, defaults |
| `crates/retro-core/src/lint.rs` | Free no-AI duplicate/stale report | Modify — read `merge_similarity` from config; derive the length pre-filter from it |
| `crates/retro-core/src/store/mod.rs` | Store layout, node read/write, `load_all` | Modify — `write_node` propagates the serialiser error; `ensure_layout` writes `meta.toml`; new `store_format` check |
| `crates/retro-core/src/store/meta.rs` | **New** — store-format marker read/write/compare | Create |
| `CLAUDE.md` | Checked-in project docs | Modify — Knowledge Store + Config notes |

`archived` deliberately does **not** get an index column in this plan. `index::build` writes `node.is_active() as i64` (`store/index.rs:112`), so archived nodes drop out of the index the moment `is_active()` changes — the surfaces that need to tell *archived* from *vetoed* are Plan 3.

---

## Task 1: `Node` gains `archived` + `archived_reason`, emitted only when set

**Files:**
- Modify: `crates/retro-core/src/store/node.rs:89-99` (struct), `:106-124` (`to_markdown`), `:128-226` (`from_markdown`)
- Test: `crates/retro-core/src/store/node.rs` (inline `mod tests`)

Why emit-only-when-set is a correctness requirement, not tidiness: `from_markdown` treats an unknown frontmatter key as a hard error (`node.rs:219-223`), `load_all` responds by skipping that node with a warning (`store/mod.rs:191-193`), and the 3.0.1 empty-wipe guard only fires at *zero* parsed nodes (`projection/local_md.rs:392-404`). Writing `archived: null` on every node would let an older binary sharing the git-synced store project a silently shrunken managed block — the 2026-07-23 data-loss class through a new door.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `crates/retro-core/src/store/node.rs`:

```rust
    #[test]
    fn archived_keys_are_absent_when_not_archived() {
        let md = sample_node().to_markdown().unwrap();
        assert!(
            !md.contains("archived"),
            "unarchived nodes must stay byte-compatible with older binaries: {md}"
        );
    }

    #[test]
    fn archived_keys_roundtrip_when_set() {
        let mut n = sample_node();
        n.archived = Some(NaiveDate::from_ymd_opt(2026, 9, 3).unwrap());
        n.archived_reason = Some("stale".to_string());
        let md = n.to_markdown().unwrap();
        assert!(md.contains("archived: 2026-09-03\n"));
        assert!(md.contains("archived_reason: stale\n"));
        let parsed = Node::from_markdown(&md).unwrap();
        assert_eq!(parsed, n);
    }

    #[test]
    fn archived_reason_keeps_colons_in_value() {
        // `extracted:skill:<name>` must survive the `split_once(':')` parser.
        let mut n = sample_node();
        n.archived = Some(NaiveDate::from_ymd_opt(2026, 9, 3).unwrap());
        n.archived_reason = Some("extracted:skill:release-ritual".to_string());
        let parsed = Node::from_markdown(&n.to_markdown().unwrap()).unwrap();
        assert_eq!(
            parsed.archived_reason.as_deref(),
            Some("extracted:skill:release-ritual")
        );
    }

    #[test]
    fn archived_reason_with_newline_is_rejected() {
        let mut n = sample_node();
        n.archived = Some(NaiveDate::from_ymd_opt(2026, 9, 3).unwrap());
        n.archived_reason = Some("stale\nid: injected".to_string());
        assert!(n.to_markdown().is_err());
        n.archived_reason = Some("stale\rid: injected".to_string());
        assert!(n.to_markdown().is_err());
    }

    #[test]
    fn archived_reason_without_archived_is_rejected() {
        // Would make an older binary hard-error on a node that is still
        // active — the exact cross-version hazard emit-only-when-set avoids.
        let mut n = sample_node();
        n.archived_reason = Some("stale".to_string());
        assert!(n.to_markdown().is_err());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p retro-core store::node 2>&1 | tail -20`
Expected: compile error — `Node` has no field `archived`; `to_markdown()` has no method `unwrap`.

- [ ] **Step 3: Add the two fields to the struct**

In `crates/retro-core/src/store/node.rs`, add to `pub struct Node` immediately after `invalidated_by`:

```rust
    pub invalidated_by: Option<String>,
    /// Set only when the curator archives this node. Emitted to frontmatter
    /// ONLY when `Some` — see `to_markdown` for why that is load-bearing.
    pub archived: Option<NaiveDate>,
    /// Why it was archived: `"stale"` or `"extracted:skill:<name>"`.
    /// Must be single-line and only present when `archived` is set.
    pub archived_reason: Option<String>,
    pub body: String,
```

- [ ] **Step 4: Make `to_markdown` fallible and emit conditionally**

Replace `to_markdown` (`node.rs:106-124`) with:

```rust
    /// Serialize to markdown.
    ///
    /// Fails if `archived_reason` is multi-line or is set without `archived`.
    /// The frontmatter serialiser is line-oriented and unescaped, so a stray
    /// `\n` in a model-derived reason would inject frontmatter keys and make
    /// the node unparseable — which `load_all` handles by silently skipping it.
    pub fn to_markdown(&self) -> Result<String, CoreError> {
        // NOTE: source IDs must not contain commas (comma-joined list format).
        let sources = self.sources.join(", ");
        let invalidated = self.invalidated_by.as_deref().unwrap_or("null");

        // `archived` / `archived_reason` are emitted ONLY when set. An older
        // binary reading the same git-synced store treats an unknown key as a
        // hard parse error and skips the node, so every untouched node must
        // stay byte-identical to the pre-curator format.
        let mut archived_keys = String::new();
        if let Some(date) = self.archived {
            archived_keys.push_str(&format!("archived: {date}\n"));
        }
        if let Some(reason) = &self.archived_reason {
            if self.archived.is_none() {
                return Err(CoreError::Parse(format!(
                    "archived_reason set without archived: {reason:?}"
                )));
            }
            if reason.contains('\n') || reason.contains('\r') {
                return Err(CoreError::Parse(format!(
                    "archived_reason must be single-line: {reason:?}"
                )));
            }
            archived_keys.push_str(&format!("archived_reason: {reason}\n"));
        }

        Ok(format!(
            "---\nid: {}\nscope: {}\ntype: {}\nconfidence: {:.2}\nsources: [{}]\ncreated: {}\nupdated: {}\ninvalidated_by: {}\n{}---\n{}\n",
            self.id,
            self.scope,
            self.node_type.as_str(),
            self.confidence,
            sources,
            self.created,
            self.updated,
            invalidated,
            archived_keys,
            self.body.trim_end_matches('\n'),
        ))
    }
```

- [ ] **Step 5: Register both keys in the strict parser**

In `from_markdown`, add the two locals beside the existing ones (after `let mut invalidated_by ...`, `node.rs:147`):

```rust
        let mut invalidated_by: Option<String> = None;
        let mut archived: Option<NaiveDate> = None;
        let mut archived_reason: Option<String> = None;
```

Add two match arms immediately after the `"invalidated_by"` arm (`node.rs:193-198`), before the catch-all `other =>`:

```rust
                "archived" => {
                    archived = match value {
                        "null" | "" => None,
                        other => Some(parse_date(other)?),
                    }
                }
                "archived_reason" => {
                    archived_reason = match value {
                        "null" | "" => None,
                        other => Some(other.to_string()),
                    }
                }
```

And add both to the returned `Node` literal (`node.rs:213-224`), after `invalidated_by`:

```rust
            invalidated_by,
            archived,
            archived_reason,
            body: body.trim_end_matches('\n').to_string(),
```

- [ ] **Step 6: Fix the two production `to_markdown` call sites**

`crates/retro-core/src/store/mod.rs:117`, inside `write_node`:

```rust
        std::fs::write(&path, node.to_markdown()?).map_err(io)?;
```

`crates/retro-core/src/store/mod.rs:385` — add `?` to the `n.to_markdown()` argument:

```rust
            n.to_markdown()?,
```

- [ ] **Step 7: Fix the existing `to_markdown` test call sites**

Twelve call sites in `node.rs`'s own `mod tests` (lines 296, 320, 329, 335, 344, 352, 365, 372, 395, 403, 411, 430) now need `.unwrap()`. The compiler lists each one.

Run: `cargo test -p retro-core 2>&1 | grep -E '^error' | head -20`
Fix each reported site by appending `.unwrap()` to the `to_markdown()` call, e.g. line 296:

```rust
        let md = sample_node().to_markdown().unwrap();
```

- [ ] **Step 8: Add the two fields to every remaining `Node` construction site**

Fifteen struct literals across seven files. The compiler enumerates them exactly:

Run: `cargo build 2>&1 | grep -E 'missing field|--> ' | head -40`

Sites: `analysis/v3.rs` (7: lines ~209, 432, 477, 524, 575, 622, 688), `migrate.rs` (2: ~169, 234), `store/index.rs` (2: ~267, 483), `store/mod.rs` (1: ~245), `projection/local_md.rs` (1: ~28), `lint.rs` (1: ~104), `retro-cli/src/ui/api.rs` (1: ~808).

At each, add the two fields after `invalidated_by`:

```rust
            invalidated_by: None,
            archived: None,
            archived_reason: None,
```

Note: `store/index.rs:36` and `:183` belong to `IndexedNode`, a different struct — leave both alone.

- [ ] **Step 9: Run the full suite**

Run: `cargo test 2>&1 | tail -20`
Expected: all tests pass, including the five new ones from Step 1.

- [ ] **Step 10: Commit**

```bash
git add crates/retro-core/src/store/node.rs crates/retro-core/src/store/mod.rs \
        crates/retro-core/src/analysis/v3.rs crates/retro-core/src/migrate.rs \
        crates/retro-core/src/store/index.rs crates/retro-core/src/projection/local_md.rs \
        crates/retro-core/src/lint.rs crates/retro-cli/src/ui/api.rs
git commit -m "feat(store): add archived/archived_reason to Node, emitted only when set

Two optional frontmatter keys written only on archived nodes, so every
untouched node stays byte-compatible with an older binary reading the
same git-synced store. to_markdown becomes fallible and rejects a
multi-line archived_reason (frontmatter injection) and a reason set
without a date."
```

---

## Task 2: `is_active()` honours `archived`

**Files:**
- Modify: `crates/retro-core/src/store/node.rs:102-104`
- Test: `crates/retro-core/src/store/node.rs` (inline `mod tests`)

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn is_active_reflects_archived() {
        let mut n = sample_node();
        assert!(n.is_active());
        n.archived = Some(NaiveDate::from_ymd_opt(2026, 9, 3).unwrap());
        n.archived_reason = Some("stale".to_string());
        assert!(!n.is_active());
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retro-core is_active_reflects_archived 2>&1 | tail -10`
Expected: FAIL — `assertion failed: !n.is_active()`.

- [ ] **Step 3: Add the clause**

```rust
    pub fn is_active(&self) -> bool {
        self.invalidated_by.is_none() && self.archived.is_none()
    }
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p retro-core is_active 2>&1 | tail -10`
Expected: PASS — both `is_active_reflects_invalidated_by` and `is_active_reflects_archived`.

- [ ] **Step 5: Confirm the inherited exclusions**

Projection (`projection/local_md.rs`), the index (`store/index.rs:112` writes `node.is_active() as i64`), and `lint` (`lint.rs:31`) all already filter on `is_active()`, so they exclude archived nodes with no further change. Verify nothing regressed:

Run: `cargo test 2>&1 | tail -20`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/retro-core/src/store/node.rs
git commit -m "feat(store): is_active() excludes archived nodes

Projection, reindex, and lint already gate on is_active(), so they
inherit archived-exclusion for free."
```

---

## Task 3: `[curator]` config section and `CuratorPolicy`

**Files:**
- Modify: `crates/retro-core/src/config.rs`
- Test: `crates/retro-core/src/config.rs` (inline `mod tests`)

- [ ] **Step 1: Write the failing tests**

Add a `mod tests` block at the end of `crates/retro-core/src/config.rs` (or extend the existing one if present):

```rust
#[cfg(test)]
mod curator_config_tests {
    use super::*;

    #[test]
    fn curator_defaults_match_the_spec() {
        let c = Config::default();
        assert_eq!(c.curator.merge_duplicates, CuratorPolicy::Auto);
        assert_eq!(c.curator.archive_stale, CuratorPolicy::Auto);
        assert_eq!(c.curator.skills, CuratorPolicy::Review);
        assert_eq!(c.curator.merge_similarity, 0.8);
        assert_eq!(c.curator.max_auto_merges_per_run, 3);
        assert_eq!(c.curator.max_auto_archives_per_run, 3);
    }

    #[test]
    fn curator_section_parses_lowercase_policies() {
        let toml_src = r#"
[curator]
merge_duplicates = "review"
archive_stale = "off"
skills = "auto"
merge_similarity = 0.75
max_auto_merges_per_run = 5
"#;
        let c: Config = toml::from_str(toml_src).unwrap();
        assert_eq!(c.curator.merge_duplicates, CuratorPolicy::Review);
        assert_eq!(c.curator.archive_stale, CuratorPolicy::Off);
        assert_eq!(c.curator.skills, CuratorPolicy::Auto);
        assert_eq!(c.curator.merge_similarity, 0.75);
        assert_eq!(c.curator.max_auto_merges_per_run, 5);
        // omitted key falls back to its default
        assert_eq!(c.curator.max_auto_archives_per_run, 3);
    }

    #[test]
    fn config_without_curator_section_still_loads() {
        let c: Config = toml::from_str("[runner]\nmax_ai_calls_per_day = 20\n").unwrap();
        assert_eq!(c.runner.max_ai_calls_per_day, 20);
        assert_eq!(c.curator.merge_duplicates, CuratorPolicy::Auto);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p retro-core curator_config 2>&1 | tail -15`
Expected: compile error — no `curator` field on `Config`, no `CuratorPolicy`.

- [ ] **Step 3: Add the policy enum and config struct**

Add to `crates/retro-core/src/config.rs`, after `UiConfig` (`config.rs:85-88`):

```rust
/// What the curator is allowed to do with one class of finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CuratorPolicy {
    /// Apply silently, under the per-run cap.
    Auto,
    /// Queue for the user to decide.
    Review,
    /// Skip detection entirely.
    Off,
}

/// Context-curator behaviour. Each op is independently settable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratorConfig {
    #[serde(default = "default_merge_duplicates")]
    pub merge_duplicates: CuratorPolicy,
    #[serde(default = "default_archive_stale")]
    pub archive_stale: CuratorPolicy,
    #[serde(default = "default_skills_policy")]
    pub skills: CuratorPolicy,
    /// Near-duplicate similarity threshold; also drives the length pre-filter.
    #[serde(default = "default_merge_similarity")]
    pub merge_similarity: f64,
    #[serde(default = "default_max_auto_per_run")]
    pub max_auto_merges_per_run: u32,
    #[serde(default = "default_max_auto_per_run")]
    pub max_auto_archives_per_run: u32,
}
```

Add the defaults beside the existing `default_*` functions:

```rust
fn default_merge_duplicates() -> CuratorPolicy {
    CuratorPolicy::Auto
}
fn default_archive_stale() -> CuratorPolicy {
    CuratorPolicy::Auto
}
fn default_skills_policy() -> CuratorPolicy {
    CuratorPolicy::Review
}
fn default_merge_similarity() -> f64 {
    0.8
}
fn default_max_auto_per_run() -> u32 {
    3
}

fn default_curator() -> CuratorConfig {
    CuratorConfig {
        merge_duplicates: default_merge_duplicates(),
        archive_stale: default_archive_stale(),
        skills: default_skills_policy(),
        merge_similarity: default_merge_similarity(),
        max_auto_merges_per_run: default_max_auto_per_run(),
        max_auto_archives_per_run: default_max_auto_per_run(),
    }
}
```

- [ ] **Step 4: Wire it into `Config`**

Add the field to `pub struct Config` (`config.rs:5-21`), after `ui`:

```rust
    #[serde(default = "default_ui")]
    pub ui: UiConfig,
    #[serde(default = "default_curator")]
    pub curator: CuratorConfig,
```

And to `impl Default for Config` (`config.rs:23-34`):

```rust
            ui: default_ui(),
            curator: default_curator(),
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p retro-core curator_config 2>&1 | tail -10`
Expected: PASS — all three tests.

- [ ] **Step 6: Confirm the CLI can reach the new types**

`crates/retro-core/src/lib.rs` declares every module `pub mod` with no `pub use` re-export list, so `retro-cli` already reaches the new types as `retro_core::config::{CuratorConfig, CuratorPolicy}`. No re-export is needed.

Verify:

Run: `grep -c 'pub use' crates/retro-core/src/lib.rs`
Expected: `0`. If that ever becomes non-zero, add `CuratorConfig` and `CuratorPolicy` to the re-export list.

- [ ] **Step 7: Commit**

```bash
git add crates/retro-core/src/config.rs
git commit -m "feat(config): add [curator] section with per-op policies

Each op is independently auto/review/off. Unknown TOML sections are
ignored, so adding the section is safe in both version directions."
```

---

## Task 4: Thread `merge_similarity` into `lint`

**Files:**
- Modify: `crates/retro-core/src/lint.rs:38-66`
- Test: `crates/retro-core/src/lint.rs` (inline `mod tests`)

`lint` currently hardcodes `0.2` for the length pre-filter and `0.8` for the similarity test (`lint.rs:47`, `:50`). The pre-filter must be *derived* from the threshold — Levenshtein distance is at least the length difference, so a length gap wider than `1.0 - merge_similarity` cannot reach the threshold. Hardcoding `0.2` would silently cap detection when the threshold is lowered.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/retro-core/src/lint.rs`:

```rust
    #[test]
    fn lowering_merge_similarity_widens_detection() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        // ~0.66 similar: same rule, materially different wording and length.
        store
            .write_node(&node("a", Scope::Global, 0.8, 1, "Always pin actions to SHAs"))
            .unwrap();
        store
            .write_node(&node(
                "b",
                Scope::Global,
                0.8,
                1,
                "Always pin GitHub actions to commit SHAs, never tags",
            ))
            .unwrap();

        let mut config = Config::default();
        // Default 0.8 is too strict for this pair — and the pre-filter derived
        // from it must also reject the pair on length alone.
        assert!(
            run_lint(&store, &config)
                .unwrap()
                .findings
                .iter()
                .all(|f| f.kind != "near-duplicate"),
            "0.8 should not pair these"
        );

        // Lowering the threshold must actually widen detection — which only
        // works if the length pre-filter is derived from it, not hardcoded.
        config.curator.merge_similarity = 0.5;
        let found = run_lint(&store, &config).unwrap();
        assert_eq!(
            found
                .findings
                .iter()
                .filter(|f| f.kind == "near-duplicate")
                .count(),
            1,
            "{:?}",
            found.findings
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p retro-core lowering_merge_similarity 2>&1 | tail -15`
Expected: FAIL — the second assertion finds 0 near-duplicates, because the hardcoded `0.2` pre-filter skips the pair on length before similarity is ever computed.

- [ ] **Step 3: Derive both values from config**

In `run_lint`, replace the pre-filter and comparison block (`lint.rs:44-51`) with:

```rust
            // Length-ratio pre-filter derived from the active threshold:
            // Levenshtein distance is at least the length difference, so a
            // wider gap than (1 - threshold) can never reach the threshold.
            // Deriving it (rather than hardcoding 0.2) keeps a lowered
            // merge_similarity from being silently capped here.
            let threshold = config.curator.merge_similarity;
            let (la, lb) = (a.body.chars().count(), b.body.chars().count());
            let max_len = la.max(lb);
            if max_len > 0 && (la.abs_diff(lb) as f64) / (max_len as f64) > 1.0 - threshold {
                continue;
            }
            if crate::util::normalized_similarity(&a.body, &b.body) > threshold {
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p retro-core lint 2>&1 | tail -15`
Expected: PASS — the new test plus the three existing lint tests (`near_duplicates_within_scope_are_found`, `stale_low_confidence_candidates_are_flagged`, `clean_store_yields_no_findings`) all still pass at the default 0.8.

- [ ] **Step 5: Commit**

```bash
git add crates/retro-core/src/lint.rs
git commit -m "refactor(lint): read merge_similarity from config

Removes the hardcoded 0.2/0.8 literals. The length pre-filter is now
derived as (1 - threshold) so lowering the threshold actually widens
detection instead of being capped by the pre-filter."
```

---

## Task 5: Store-format marker (`meta.toml`)

**Files:**
- Create: `crates/retro-core/src/store/meta.rs`
- Modify: `crates/retro-core/src/store/mod.rs:99-109` (`ensure_layout`), plus the `mod` declaration
- Test: `crates/retro-core/src/store/meta.rs` (inline `mod tests`)

Honest limitation to record in the code: this does nothing for already-released 3.1.x binaries, which cannot know to look for the file. For the 3.1.x → 3.2.0 window, Task 1's emit-only-when-set rule is the whole protection. The marker earns its place from 3.2.0 forward.

- [ ] **Step 1: Write the failing tests**

Create `crates/retro-core/src/store/meta.rs`:

```rust
//! Store-format marker (`~/.retro/meta.toml`).
//!
//! Lets a future binary refuse to operate on a store written by a newer one
//! instead of silently degrading. Note the limitation: released 3.1.x
//! binaries do not look for this file, so for the 3.1.x -> 3.2.0 window the
//! emit-only-when-set rule in `Node::to_markdown` is the real protection.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::errors::CoreError;

/// Store format this binary reads and writes.
pub const STORE_FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoreMeta {
    pub store_format: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn writes_then_reads_current_format() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path()).unwrap();
        assert_eq!(read(tmp.path()).unwrap().unwrap().store_format, STORE_FORMAT);
    }

    #[test]
    fn absent_marker_is_ok_and_none() {
        let tmp = TempDir::new().unwrap();
        assert!(read(tmp.path()).unwrap().is_none());
        // A pre-marker store must be usable, not an error.
        assert!(check_compatible(tmp.path()).is_ok());
    }

    #[test]
    fn newer_format_is_refused() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(
            tmp.path().join("meta.toml"),
            format!("store_format = {}\n", STORE_FORMAT + 1),
        )
        .unwrap();
        let err = check_compatible(tmp.path()).unwrap_err();
        assert!(
            err.to_string().contains("newer"),
            "error should name the cause: {err}"
        );
    }

    #[test]
    fn write_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path()).unwrap();
        let first = std::fs::read(tmp.path().join("meta.toml")).unwrap();
        write(tmp.path()).unwrap();
        let second = std::fs::read(tmp.path().join("meta.toml")).unwrap();
        assert_eq!(first, second, "rewriting the marker must not churn the file");
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p retro-core store::meta 2>&1 | tail -15`
Expected: compile error — `meta` module not declared; `read`, `write`, `check_compatible` not found.

- [ ] **Step 3: Implement the three functions**

Add above the `mod tests` block in `crates/retro-core/src/store/meta.rs`:

```rust
fn meta_path(root: &Path) -> std::path::PathBuf {
    root.join("meta.toml")
}

/// Read the marker. `Ok(None)` means a pre-marker store, which is valid.
pub fn read(root: &Path) -> Result<Option<StoreMeta>, CoreError> {
    let path = meta_path(root);
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path)
        .map_err(|e| CoreError::Io(format!("reading store meta: {e}")))?;
    let meta: StoreMeta =
        toml::from_str(&contents).map_err(|e| CoreError::Config(e.to_string()))?;
    Ok(Some(meta))
}

/// Write this binary's format marker. Idempotent.
pub fn write(root: &Path) -> Result<(), CoreError> {
    let meta = StoreMeta {
        store_format: STORE_FORMAT,
    };
    let contents = toml::to_string(&meta).map_err(|e| CoreError::Config(e.to_string()))?;
    std::fs::write(meta_path(root), contents)
        .map_err(|e| CoreError::Io(format!("writing store meta: {e}")))
}

/// Refuse a store written by a newer binary. A missing marker is fine.
pub fn check_compatible(root: &Path) -> Result<(), CoreError> {
    match read(root)? {
        Some(meta) if meta.store_format > STORE_FORMAT => Err(CoreError::Config(format!(
            "store format {} is newer than this binary supports ({STORE_FORMAT}) — upgrade retro",
            meta.store_format
        ))),
        _ => Ok(()),
    }
}
```

- [ ] **Step 4: Declare the module and write the marker in `ensure_layout`**

In `crates/retro-core/src/store/mod.rs`, add the module declaration beside the existing store submodules:

```rust
pub mod meta;
```

Then, at the end of `ensure_layout` (`store/mod.rs:99-109`), before `Ok(())`:

```rust
        meta::write(&self.root)?;
        Ok(())
```

`meta.toml` is committed with the store (it is *not* in `IGNORED_ENTRIES`) — the format of a synced store is shared state, not machine-local.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p retro-core store::meta 2>&1 | tail -10`
Expected: PASS — all four tests.

- [ ] **Step 6: Run the full suite**

Run: `cargo test 2>&1 | tail -20`
Expected: all tests pass. `ensure_layout` now creates an extra file; if a test asserts an exact directory listing of the store root, update it to expect `meta.toml`.

- [ ] **Step 7: Commit**

```bash
git add crates/retro-core/src/store/meta.rs crates/retro-core/src/store/mod.rs
git commit -m "feat(store): add meta.toml store-format marker

Lets a future binary refuse a store written by a newer one instead of
degrading silently. Does not help already-released 3.1.x binaries —
documented in the module."
```

---

## Task 6: Update the docs

**Files:**
- Modify: `CLAUDE.md`
- Modify: `README.md` (only if it documents the node frontmatter schema)

- [ ] **Step 1: Check whether the README describes the schema**

Run: `grep -n 'invalidated_by\|frontmatter\|confidence_threshold' README.md`
If nothing matches, the README needs no change in this plan (projection behaviour is unchanged so far) — note that and move on.

- [ ] **Step 2: Update the Knowledge Store section of `CLAUDE.md`**

In the **Knowledge Store** bullet list, extend the "Files as truth" bullet's frontmatter key list and add a new bullet after "Invalidation, not deletion":

```markdown
- **Archival, distinct from invalidation** — the curator sets `archived` (date) + `archived_reason` (`stale` | `extracted:skill:<name>`); `is_active()` is `invalidated_by.is_none() && archived.is_none()`. Both keys are emitted **only on nodes that are actually archived** — an older binary treats an unknown frontmatter key as a hard parse error and `load_all` skips that node, while the 3.0.1 empty-wipe guard only fires at *zero* nodes, so writing them unconditionally would let an older binary project a silently shrunken managed block. `to_markdown` is fallible: it rejects a multi-line `archived_reason` (frontmatter injection via a model-derived name) and a reason set without a date.
- **Store-format marker** — `~/.retro/meta.toml` (`store_format`, committed with the store) lets a future binary refuse a store written by a newer one. It does nothing for released 3.1.x binaries; for that window emit-only-when-set is the protection.
```

- [ ] **Step 3: Add the `[curator]` config note**

In the **Key Design Decisions** section, under the Knowledge Store subsection, add:

```markdown
- **Curator policy config** — `[curator]` in `config.toml`: `merge_duplicates` / `archive_stale` (default `auto`), `skills` (default `review`), each `auto` | `review` | `off` (`CuratorPolicy`); plus `merge_similarity` (0.8), `max_auto_merges_per_run` and `max_auto_archives_per_run` (3 each). `merge_similarity` also drives `lint`'s length pre-filter, derived as `1.0 - merge_similarity` — never hardcode it, or lowering the threshold is silently capped.
```

- [ ] **Step 4: Verify the build and full suite one more time**

Run: `cargo test && cargo run -- --help > /dev/null && echo BUILD_OK`
Expected: all tests pass, then `BUILD_OK`.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record archived schema, store-format marker, [curator] config"
```

---

## Task 7: Scenario check and company-name sweep

**Files:** none modified (verification only)

- [ ] **Step 1: Run the scenario suite**

Use the **run-scenarios** skill. Every scenario must run under the isolation preamble in `scenarios/README.md`: `RETRO_HOME` at a temp dir, config redirecting `[paths] claude_dir` to another temp dir, overridden `HOME`, a stubbed `launchctl` on `PATH`, and `./target/release/retro` — never a PATH binary.

**HARD RULE:** never run `retro init`, `retro migrate`, `retro uninstall`, `retro start`, or `retro stop` against the real environment during this work.

Expected: `v3-init-and-lifecycle`, `v3-pipeline-dry-run`, and `v3-migrate` all pass. `ensure_layout` now writes `meta.toml`, so a scenario asserting the store layout may need that file added to its expectations.

- [ ] **Step 2: Neutrality and local-path sweep before any push**

This is an open-source repo dogfooded on private repos, so public artifacts must not name an employer, organisation, or developer, and must not carry machine-specific paths. Set the pattern from your own context rather than hardcoding it here — this plan is itself a public artifact.

```bash
# Set to your employer/organisation and surname, pipe-separated, e.g. NEUTRALITY_PATTERN='acme|smith'
NEUTRALITY_PATTERN='<org>|<surname>'

git log origin/main..HEAD --format='%s%n%b' | grep -niE "$NEUTRALITY_PATTERN" || echo "commits clean"
git diff origin/main...HEAD | grep -niE "^\+.*($NEUTRALITY_PATTERN)" || echo "names clean"
git diff origin/main...HEAD | grep -nE '^\+.*(/Users/|/home/)' || echo "paths clean"
```

Expected: all three print their "clean" fallback. Any hit is a blocker — fix it in a follow-up commit before pushing.

- [ ] **Step 3: Push and open the PR**

```bash
git push -u origin <branch>
gh pr create --title "feat(v3): curator Plan 1 — data model & config foundation" --body "$(cat <<'BODY'
Plan 1 of 4 from the Context Curator spec (`docs/superpowers/specs/2026-08-10-retro-v3-context-curator-design.md`). Schema + config only — no curator behaviour yet, and no runtime change for an existing store.

- `Node.archived` / `archived_reason`, emitted **only when set** (an older binary reading the same synced store hard-errors on unknown keys and `load_all` skips the node, while the 3.0.1 empty-wipe guard only fires at zero nodes)
- `to_markdown` is now fallible: rejects a multi-line `archived_reason` and a reason set without a date
- `is_active()` excludes archived nodes; projection, reindex, and lint inherit the exclusion
- `[curator]` config section with per-op `auto` / `review` / `off` policies
- `merge_similarity` threaded into `lint`; the length pre-filter is derived as `1 - threshold` instead of hardcoded `0.2`
- `~/.retro/meta.toml` store-format marker

🤖 Generated with [Claude Code](https://claude.com/claude-code)
BODY
)"
```

- [ ] **Step 4: Monitor checks**

This repo has no PR CI (`publish.yml` is tag-triggered only), so there are no checks to wait on. Check for review/bugbot comments before merging:

Run: `gh pr view <number> --json reviews,comments`

---

## Self-Review

**1. Spec coverage (Plan 1 scope only).** Every "Data model" bullet maps to a task: the two fields and emit-only-when-set → Task 1; `is_active()` → Task 2; the rejected `invalidated_by` alternative needs no code; the store-format marker → Task 5. Every "Config" line → Task 3. The finding-9 pre-filter fix → Task 4. Docs → Task 6.

Deliberately deferred, with the plan that owns each: `archived` bucket / column / `POST /api/node/unarchive` and the doctor extensions → **Plan 3** (they are surfaces, and the index drops archived nodes via `is_active()` already); `store::curate`, detection sharing, merge, archive, caps, first-pass gate, review queue, `chore(curator)` commit, `retro curate`, mutated-scope reprojection → **Plan 2**; intake filter and conflict instrumentation → **Plan 3**; skills, `execute_agentic`, absorb-re-discovered → **Plan 4**.

**2. Placeholder scan.** No TBDs, no "add error handling", no "write tests for the above". Every code step carries the code; every run step carries the command and expected output. Two steps are deliberately conditional on a grep result (Task 3 Step 6's re-export, Task 6 Step 1's README check) — each states exactly what to do in both branches.

**3. Type consistency.** `CuratorPolicy { Auto, Review, Off }` and the field names `merge_duplicates`, `archive_stale`, `skills`, `merge_similarity`, `max_auto_merges_per_run`, `max_auto_archives_per_run` are spelled identically in the enum definition, `CuratorConfig`, `default_curator()`, and both config tests. `archived: Option<NaiveDate>` / `archived_reason: Option<String>` are consistent across the struct, `to_markdown`, `from_markdown`, and every test. `to_markdown` returns `Result<String, CoreError>` everywhere it appears. `meta::{read, write, check_compatible, StoreMeta, STORE_FORMAT}` match between implementation and tests.

**One note carried into Plan 2:** `check_compatible` is implemented and tested here but not yet *called* on any command path. Wiring it into the command entry points belongs with the curator stage in Plan 2, where there is a mutation worth refusing; calling it from `ensure_layout` would be circular, since that is what writes the marker.
