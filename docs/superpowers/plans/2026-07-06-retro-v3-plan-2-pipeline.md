# Retro v3 Plan 2: Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the v3 store into a working pipeline: hook-based capture (SessionEnd/SessionStart), automatic project registration with notify+exclude, AI analysis flowing into the markdown store, one-way projection to `~/.claude/CLAUDE.md` and per-project `CLAUDE.local.md`, all runnable via `retro run` behind a `[v3] enabled` config gate.

**Architecture:** New thin modules alongside v2 (state, queue, registry, health, analysis sink, local-md projection) that REUSE the v2 engine wholesale: `to_compact_session` + `build_graph_analysis_prompt` + `GRAPH_ANALYSIS_RESPONSE_SCHEMA` + `parse_graph_response` produce `GraphOperation`s exactly as today — only the sink changes (markdown store instead of SQLite). Two new hook-entry commands (`observe`, `brief`), a v3 path inside `retro run`, and `retro init --v3`. v2 behavior is unchanged unless `[v3] enabled = true`.

**Tech Stack:** Rust (edition 2024, sync only), existing deps only (serde_json for state/queue/hook-stdin, toml for project meta). No new crates.

**Spec:** `docs/superpowers/specs/2026-07-06-retro-v3-personal-redesign-design.md` (§5 Capture, §6 Analysis, §7 Projection, §9 Reliability partial — health.json writing; doctor/dashboard are Plan 3)

## Context for implementers (read first)

- **Conventions:** `CoreError` (thiserror) in retro-core; `anyhow::Result` + bare `?` in retro-cli. Shell-outs via `Command::new().args()`, never shell strings. Never run `cargo fmt` — only `rustfmt --edition 2024 <files you created/touched>`, and for SHARED files (main.rs, commands/mod.rs, lib.rs, existing modules) don't run rustfmt at all: match surrounding style manually.
- **The v3 gate:** every v3 behavior is behind `config.v3.enabled` (Task 2). When false (default), retro behaves exactly as v2 — the user's still-installed v2 launchd runner keeps working until `retro migrate` (Plan 3) flips them over.
- **Verified v2 API you will reuse** (do not modify these):
  - `analysis::prompts::to_compact_session(&Session) -> CompactSession` (prompts.rs:523)
  - `analysis::prompts::build_graph_analysis_prompt(&[CompactSession], &[KnowledgeNode], Option<&str>) -> String` (prompts.rs:458)
  - `analysis::GRAPH_ANALYSIS_RESPONSE_SCHEMA` (mod.rs:59) and `analysis::parse_graph_response(&str, Option<&str>) -> Result<Vec<GraphOperation>, CoreError>` (mod.rs:583)
  - `models::GraphOperation` (models.rs:924): `CreateNode{node_type, scope, project_id, content, confidence}`, `CreateEdge{source_id, target_id, edge_type}`, `UpdateNode{id, confidence, content}`, `MergeNodes{keep_id, remove_id}`
  - `models::Session` (models.rs:334): `session_id, project, session_path, user_messages: Vec<ParsedUserMessage{text, timestamp}>, assistant_messages, summaries, tools_used, errors, metadata: SessionMetadata{cwd, version, git_branch, model}`
  - `ingest::session::parse_session_file(&Path, session_id, project) -> Result<Session, CoreError>` (session.rs:8)
  - `scrub::scrub_session(&mut Session)` (scrub.rs:60); low-signal filter is `session.user_messages.len() >= 2`
  - `analysis::backend::{AnalysisBackend, BackendResponse{text, input_tokens, output_tokens}}` (backend.rs:4-19)
  - `observer::find_modified_sessions(&Path, Option<SystemTime>, &[String]) -> Vec<ModifiedSession{path, mtime}>` (observer.rs:24)
  - `projection::claude_md::{update_claude_md_content(existing, &[String]) -> String, read_managed_section(&str) -> Option<Vec<String>>}` (claude_md.rs:25,47); delimiters `<!-- retro:managed:start -->` / `<!-- retro:managed:end -->`
  - `util::backup_file(path: &str, backup_dir: &Path)` (util.rs:10)
  - `lock::LockFile::{acquire, try_acquire}` (lock.rs) — PID-based, auto-release on drop
  - v3 store (Plan 1): `store::{Store, Node, NodeType, Scope, slugify, LoadResult}`, `store::git::{ensure_repo, commit_all, push_best_effort, PushOutcome, has_changes}`, `store::index::build`
  - Config: `config::{Config, retro_dir()}`; `config.claude_dir()`; `config.privacy.exclude_projects: Vec<String>`; `config.knowledge.confidence_threshold: f64`
  - v2 hook-install precedent (JSON shape + idempotency pattern to imitate): `retro-cli/src/commands/init.rs:173-253`
- **v2 `KnowledgeNode` shim:** the reused prompt builder takes v2 `KnowledgeNode`s for existing-node context. Task 6 builds shims from v3 `store::Node`s — only `id`, `content`, `confidence`, `node_type`, `scope`, `project_id` matter to the prompt (verified: prompt truncates content to 200 chars, shows max 50 nodes).
- **Machine-local vs committed:** `state/` (runner state, path map) and `queue/` and `health.json` are gitignored (already in Plan 1's GITIGNORE_CONTENT). `knowledge/**` and `config.toml` are committed. Never put machine-local state in committed files.
- Baseline: 320 tests passing. Run `cargo test -p retro-core` frequently; full `cargo test` before each commit.

## File Structure

```
crates/retro-core/src/store/state.rs      # machine-local runner state + AI budget + notifications (state/state.json)
crates/retro-core/src/store/queue.rs      # pending-session queue (queue/<session_id>.json)
crates/retro-core/src/store/projects.rs   # v3 project registry: project.toml (committed) + path map (local)
crates/retro-core/src/store/git.rs        # MODIFY: #[must_use], apply_local_config extraction
crates/retro-core/src/store/mod.rs        # MODIFY: get() path context; pub mod state/queue/projects
crates/retro-core/src/health.rs           # health.json read/write (stage records)
crates/retro-core/src/analysis/v3.rs      # v3 sink: node shims, analyze_batch, apply ops to Store
crates/retro-core/src/analysis/backend.rs # MODIFY: add MockBackend (pub, used by tests)
crates/retro-core/src/projection/local_md.rs # one-way projection: global CLAUDE.md + CLAUDE.local.md + .git/info/exclude
crates/retro-core/src/runner_v3.rs        # the v3 pipeline: drain → analyze → project → commit → push → health
crates/retro-core/src/config.rs           # MODIFY: [v3] section
crates/retro-core/src/briefing.rs         # MODIFY: add build_v3_briefing()
crates/retro-core/src/lib.rs              # MODIFY: pub mod health; pub mod runner_v3;
crates/retro-cli/src/commands/observe.rs  # SessionEnd hook entry
crates/retro-cli/src/commands/brief.rs    # SessionStart hook entry (catch-up + briefing)
crates/retro-cli/src/commands/run.rs      # MODIFY: v3 dispatch at top
crates/retro-cli/src/commands/init.rs     # MODIFY: --v3 / --from paths + global hooks + backup remote
crates/retro-cli/src/commands/mod.rs      # MODIFY: new modules
crates/retro-cli/src/main.rs              # MODIFY: Observe/Brief variants + init flags
CLAUDE.md                                  # docs
```

---

### Task 1: Carry-over hardening from the Plan 1 final review

**Files:**
- Modify: `crates/retro-core/src/store/git.rs`
- Modify: `crates/retro-core/src/store/mod.rs`

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `crates/retro-core/src/store/git.rs`:

```rust
    #[test]
    fn apply_local_config_is_idempotent_and_standalone() {
        let tmp = TempDir::new().unwrap();
        // simulate the clone path: repo exists but local config was never applied
        assert!(ensure_repo(tmp.path()).unwrap());
        apply_local_config(tmp.path()).unwrap();
        apply_local_config(tmp.path()).unwrap(); // idempotent
        let out = std::process::Command::new("git")
            .args(["-C", tmp.path().to_str().unwrap(), "config", "--local", "commit.gpgsign"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "false");
        let out = std::process::Command::new("git")
            .args(["-C", tmp.path().to_str().unwrap(), "config", "--local", "core.hooksPath"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "/dev/null");
    }
```

Add to the `tests` module in `crates/retro-core/src/store/mod.rs`:

```rust
    #[test]
    fn get_parse_error_includes_file_path() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        std::fs::write(tmp.path().join("knowledge/global/bad.md"), "not a node").unwrap();
        let err = store.get(&Scope::Global, "bad").unwrap_err();
        assert!(err.to_string().contains("bad.md"), "got: {err}");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p retro-core store::`
Expected: compile error (`apply_local_config` not found); the `get` test would fail on assertion if it compiled.

- [ ] **Step 3: Implement**

In `crates/retro-core/src/store/git.rs`:

1. Add `#[must_use]` on the `PushOutcome` enum (above `#[derive(Debug)]`) so callers can't silently drop push failures.
2. Extract the local-config block from `ensure_repo` into a public function, and call it from `ensure_repo`:

```rust
/// Apply the store repo's local git config. Safe to call repeatedly.
/// Must also be applied on the clone path (`retro init --from`), which
/// bypasses `ensure_repo`'s create branch.
pub fn apply_local_config(root: &Path) -> Result<(), CoreError> {
    // Local identity fallback: only set if unset in any config scope.
    let email_set = git(root, &["config", "user.email"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !email_set {
        run_checked(root, &["config", "user.email", "retro@localhost"])?;
        run_checked(root, &["config", "user.name", "retro"])?;
    }
    run_checked(root, &["config", "commit.gpgsign", "false"])?;
    run_checked(root, &["config", "core.hooksPath", "/dev/null"])?;
    Ok(())
}
```

`ensure_repo` becomes: `if is_repo { return Ok(false) } → run_checked init → apply_local_config(root)? → add -A → initial commit → Ok(true)`.

In `crates/retro-core/src/store/mod.rs`, change `get()`'s parse call (replacing the TODO comment added in Plan 1):

```rust
        let node = Node::from_markdown(&content)
            .map_err(|e| CoreError::Parse(format!("{}: {}", path.display(), e)))?;
        Ok(Some(node))
```

Note: `CoreError::Parse` wrapping a `CoreError` display is fine here (message-in-message); `load_all`'s own warning path already includes the path and is unchanged.

- [ ] **Step 4: Run tests**

Run: `cargo test -p retro-core store::`
Expected: 44 PASS (42 + 2 new). If `#[must_use]` breaks any existing caller, fix that call site by binding the outcome (`let _outcome = ...` is NOT acceptable — handle or log it; in Plan 1 code there are no callers yet, so expect no breakage).

- [ ] **Step 5: Commit**

```bash
cargo test -p retro-core && git add crates/retro-core/src/store/git.rs crates/retro-core/src/store/mod.rs && git commit -m "fix(store): plan-1 review carry-overs — must_use push, config extraction, get() path context"
```

---

### Task 2: `[v3]` config section

**Files:**
- Modify: `crates/retro-core/src/config.rs`

- [ ] **Step 1: Write failing test**

Add to the tests module in `crates/retro-core/src/config.rs`:

```rust
    #[test]
    fn v3_section_defaults_off_and_roundtrips() {
        let config = Config::default();
        assert!(!config.v3.enabled);
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert!(!parsed.v3.enabled);
        // absent section parses as default (forward/backward compat)
        let parsed: Config = toml::from_str("").unwrap();
        assert!(!parsed.v3.enabled);
    }
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p retro-core config::`
Expected: compile error — no `v3` field.

- [ ] **Step 3: Implement**

In `crates/retro-core/src/config.rs`, following the exact pattern of the existing sections (e.g. `ClaudeMdConfig`): add to the `Config` struct a `#[serde(default)] pub v3: V3Config,` field, and define:

```rust
/// v3 "Personal" pipeline gate. When disabled (default), retro behaves as v2.
/// Enabled by `retro init --v3`; Plan 3's `retro migrate` will flip this for
/// existing users and remove the v2 paths.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct V3Config {
    #[serde(default)]
    pub enabled: bool,
}
```

(If `Config::default()` is hand-implemented rather than derived, add `v3: V3Config::default(),` there too — check the existing code and follow it.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p retro-core`
Expected: all pass (baseline + 1).

- [ ] **Step 5: Commit**

```bash
cargo test -p retro-core && git add crates/retro-core/src/config.rs && git commit -m "feat(config): [v3] section with enabled gate (default off)"
```

---

### Task 3: Machine-local runner state + health file

**Files:**
- Create: `crates/retro-core/src/store/state.rs`
- Create: `crates/retro-core/src/health.rs`
- Modify: `crates/retro-core/src/store/mod.rs` (add `pub mod state;`)
- Modify: `crates/retro-core/src/lib.rs` (add `pub mod health;` after `pub mod git;`)

Machine-local, gitignored, JSON. `state/state.json` carries the observe watermark, the AI-call budget counters (v3 equivalent of v2's DB metadata keys `ai_calls_date`/`ai_calls_today`), and pending briefing notifications. `health.json` carries per-stage status for the Plan 3 doctor/dashboard — but writing it starts NOW (spec §9: every stage records).

- [ ] **Step 1: Write failing tests**

Create `crates/retro-core/src/store/state.rs`:

```rust
//! Machine-local runner state: observe watermark, AI budget, notifications.
//! Lives at `<store>/state/state.json` — gitignored, disposable-ish (losing it
//! causes a catch-up rescan and budget reset, never data loss).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::CoreError;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn state_roundtrips_and_defaults() {
        let tmp = TempDir::new().unwrap();
        let s = RunnerState::load(tmp.path()).unwrap();
        assert_eq!(s.last_observed_unix, 0);
        assert_eq!(s.ai_calls_today, 0);
        assert!(s.notifications.is_empty());

        let mut s = s;
        s.last_observed_unix = 1234;
        s.notifications.push("retro is now watching my-proj".to_string());
        s.save(tmp.path()).unwrap();

        let loaded = RunnerState::load(tmp.path()).unwrap();
        assert_eq!(loaded.last_observed_unix, 1234);
        assert_eq!(loaded.notifications.len(), 1);
    }

    #[test]
    fn budget_resets_on_new_day_and_counts_within_day() {
        let tmp = TempDir::new().unwrap();
        let mut s = RunnerState::load(tmp.path()).unwrap();
        assert!(s.budget_remaining("2026-07-06", 3) == 3);
        s.record_ai_calls("2026-07-06", 2);
        assert_eq!(s.budget_remaining("2026-07-06", 3), 1);
        // new day resets
        assert_eq!(s.budget_remaining("2026-07-07", 3), 3);
        s.record_ai_calls("2026-07-07", 1);
        assert_eq!(s.ai_calls_today, 1);
        assert_eq!(s.ai_calls_date, "2026-07-07");
    }

    #[test]
    fn drain_notifications_empties_the_list() {
        let tmp = TempDir::new().unwrap();
        let mut s = RunnerState::load(tmp.path()).unwrap();
        s.notifications.push("a".to_string());
        s.notifications.push("b".to_string());
        let drained = s.drain_notifications();
        assert_eq!(drained, vec!["a".to_string(), "b".to_string()]);
        assert!(s.notifications.is_empty());
    }

    #[test]
    fn corrupt_state_file_resets_to_default() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("state")).unwrap();
        std::fs::write(tmp.path().join("state/state.json"), "{corrupt").unwrap();
        let s = RunnerState::load(tmp.path()).unwrap();
        assert_eq!(s.last_observed_unix, 0);
    }
}
```

Create `crates/retro-core/src/health.rs`:

```rust
//! Pipeline health: per-stage status records at `<store>/health.json`.
//! Written by every v3 stage; read by `retro doctor` and the dashboard (Plan 3),
//! and surfaced as warnings in the session briefing.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::errors::CoreError;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn record_and_warnings_roundtrip() {
        let tmp = TempDir::new().unwrap();
        record(tmp.path(), "observe", true, "enqueued 1 session").unwrap();
        record(tmp.path(), "analyze", false, "claude CLI exited 1").unwrap();

        let h = Health::load(tmp.path()).unwrap();
        assert_eq!(h.stages.len(), 2);
        assert!(h.stages["observe"].ok);
        assert!(!h.stages["analyze"].ok);

        let warnings = h.warnings();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("analyze"), "got: {warnings:?}");
        assert!(warnings[0].contains("claude CLI exited 1"));
    }

    #[test]
    fn missing_file_loads_empty() {
        let tmp = TempDir::new().unwrap();
        let h = Health::load(tmp.path()).unwrap();
        assert!(h.stages.is_empty());
        assert!(h.warnings().is_empty());
    }
}
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p retro-core state:: health::` (two invocations or one with no filter)
Expected: compile errors — types not found. (Wire `pub mod state;` into `crates/retro-core/src/store/mod.rs` and `pub mod health;` into `crates/retro-core/src/lib.rs` now.)

- [ ] **Step 3: Implement**

In `crates/retro-core/src/store/state.rs` above the tests:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunnerState {
    /// Unix seconds of the newest session mtime already enqueued (observe watermark).
    #[serde(default)]
    pub last_observed_unix: u64,
    /// Day the AI-call counter refers to (YYYY-MM-DD).
    #[serde(default)]
    pub ai_calls_date: String,
    #[serde(default)]
    pub ai_calls_today: u32,
    /// Messages for the next session briefing (new registrations, learned nodes).
    #[serde(default)]
    pub notifications: Vec<String>,
}

fn state_path(store_root: &Path) -> PathBuf {
    store_root.join("state").join("state.json")
}

impl RunnerState {
    /// Load state; a missing or corrupt file yields defaults (never an error —
    /// state is machine-local and safe to reset).
    pub fn load(store_root: &Path) -> Result<Self, CoreError> {
        let path = state_path(store_root);
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(serde_json::from_str(&content).unwrap_or_default()),
            Err(_) => Ok(RunnerState::default()),
        }
    }

    pub fn save(&self, store_root: &Path) -> Result<(), CoreError> {
        let io = |e: std::io::Error| CoreError::Io(e.to_string());
        let path = state_path(store_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| CoreError::Io(e.to_string()))?;
        std::fs::write(&path, json).map_err(io)
    }

    /// Remaining AI calls for `today` (YYYY-MM-DD) under `max_per_day`.
    /// A stored date != today means the counter is stale: full budget.
    pub fn budget_remaining(&self, today: &str, max_per_day: u32) -> u32 {
        if self.ai_calls_date == today {
            max_per_day.saturating_sub(self.ai_calls_today)
        } else {
            max_per_day
        }
    }

    /// Record `calls` AI calls made on `today`, resetting on day change.
    pub fn record_ai_calls(&mut self, today: &str, calls: u32) {
        if self.ai_calls_date != today {
            self.ai_calls_date = today.to_string();
            self.ai_calls_today = 0;
        }
        self.ai_calls_today += calls;
    }

    pub fn drain_notifications(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notifications)
    }
}
```

In `crates/retro-core/src/health.rs` above the tests:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageHealth {
    /// RFC3339 timestamp of the last run of this stage.
    pub at: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Health {
    #[serde(default)]
    pub stages: BTreeMap<String, StageHealth>,
}

impl Health {
    /// Missing or corrupt file loads empty (health is derived, never precious).
    pub fn load(store_root: &Path) -> Result<Self, CoreError> {
        let path = store_root.join("health.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(serde_json::from_str(&content).unwrap_or_default()),
            Err(_) => Ok(Health::default()),
        }
    }

    pub fn save(&self, store_root: &Path) -> Result<(), CoreError> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| CoreError::Io(e.to_string()))?;
        std::fs::write(store_root.join("health.json"), json)
            .map_err(|e| CoreError::Io(e.to_string()))
    }

    /// Human-readable warnings for every stage whose last run failed.
    pub fn warnings(&self) -> Vec<String> {
        self.stages
            .iter()
            .filter(|(_, s)| !s.ok)
            .map(|(name, s)| format!("retro {name} failed at {}: {}", s.at, s.detail))
            .collect()
    }
}

/// Record one stage result (load-modify-save; last writer wins, which is fine
/// for a single-user pipeline serialized by the run lockfile).
pub fn record(store_root: &Path, stage: &str, ok: bool, detail: &str) -> Result<(), CoreError> {
    let mut health = Health::load(store_root)?;
    health.stages.insert(
        stage.to_string(),
        StageHealth {
            at: chrono::Utc::now().to_rfc3339(),
            ok,
            detail: detail.to_string(),
        },
    );
    health.save(store_root)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p retro-core`
Expected: all pass (+6 new: 4 state, 2 health).

- [ ] **Step 5: Commit**

```bash
cargo test -p retro-core && git add crates/retro-core/src/store/state.rs crates/retro-core/src/health.rs crates/retro-core/src/store/mod.rs crates/retro-core/src/lib.rs && git commit -m "feat(v3): machine-local runner state and health records"
```

---

### Task 4: Session queue

**Files:**
- Create: `crates/retro-core/src/store/queue.rs`
- Modify: `crates/retro-core/src/store/mod.rs` (add `pub mod queue;`)

One JSON file per pending session at `<store>/queue/<session_id>.json`. Enqueue is idempotent by session id (re-observing an already-queued session refreshes the entry). Stale entries (transcript deleted) are pruned with a health note, never retried forever — this kills v2's "session file not found" warning loop.

- [ ] **Step 1: Write failing tests**

Create `crates/retro-core/src/store/queue.rs`:

```rust
//! Pending-session queue: `<store>/queue/<session_id>.json` (gitignored).
//! Populated by `retro observe` (SessionEnd hook) and the `retro brief`
//! catch-up scan; drained by the v3 pipeline in `runner_v3`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::CoreError;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(id: &str, transcript: &Path) -> QueueEntry {
        QueueEntry {
            session_id: id.to_string(),
            transcript_path: transcript.display().to_string(),
            cwd: Some("/tmp/some-project".to_string()),
            enqueued_at: "2026-07-06T10:00:00Z".to_string(),
        }
    }

    #[test]
    fn enqueue_list_remove_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let transcript = tmp.path().join("s1.jsonl");
        std::fs::write(&transcript, "{}").unwrap();

        enqueue(tmp.path(), &entry("s1", &transcript)).unwrap();
        enqueue(tmp.path(), &entry("s1", &transcript)).unwrap(); // idempotent
        let entries = list(tmp.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "s1");

        remove(tmp.path(), "s1").unwrap();
        assert!(list(tmp.path()).unwrap().is_empty());
    }

    #[test]
    fn list_is_sorted_by_enqueued_at() {
        let tmp = TempDir::new().unwrap();
        let t = tmp.path().join("t.jsonl");
        std::fs::write(&t, "{}").unwrap();
        let mut b = entry("b", &t);
        b.enqueued_at = "2026-07-06T09:00:00Z".to_string();
        let mut a = entry("a", &t);
        a.enqueued_at = "2026-07-06T11:00:00Z".to_string();
        enqueue(tmp.path(), &a).unwrap();
        enqueue(tmp.path(), &b).unwrap();
        let ids: Vec<String> = list(tmp.path()).unwrap().into_iter().map(|e| e.session_id).collect();
        assert_eq!(ids, vec!["b".to_string(), "a".to_string()]);
    }

    #[test]
    fn prune_stale_removes_entries_with_missing_transcripts() {
        let tmp = TempDir::new().unwrap();
        let alive = tmp.path().join("alive.jsonl");
        std::fs::write(&alive, "{}").unwrap();
        enqueue(tmp.path(), &entry("alive", &alive)).unwrap();
        enqueue(tmp.path(), &entry("gone", &tmp.path().join("gone.jsonl"))).unwrap();

        let pruned = prune_stale(tmp.path()).unwrap();
        assert_eq!(pruned, vec!["gone".to_string()]);
        let remaining = list(tmp.path()).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].session_id, "alive");
    }

    #[test]
    fn corrupt_queue_entry_is_pruned() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("queue")).unwrap();
        std::fs::write(tmp.path().join("queue/junk.json"), "{not json").unwrap();
        let pruned = prune_stale(tmp.path()).unwrap();
        assert_eq!(pruned, vec!["junk".to_string()]);
        assert!(list(tmp.path()).unwrap().is_empty());
    }
}
```

- [ ] **Step 2: Verify compile failure**

Run: `cargo test -p retro-core store::queue`
Expected: compile error. (Add `pub mod queue;` to `store/mod.rs` now.)

- [ ] **Step 3: Implement**

Add above the tests in `crates/retro-core/src/store/queue.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueEntry {
    pub session_id: String,
    /// Absolute path to the session JSONL transcript.
    pub transcript_path: String,
    /// Working directory of the session, if known at enqueue time
    /// (SessionEnd hook provides it; catch-up scan recovers it later from the transcript).
    #[serde(default)]
    pub cwd: Option<String>,
    /// RFC3339. Drain order.
    pub enqueued_at: String,
}

fn queue_dir(store_root: &Path) -> PathBuf {
    store_root.join("queue")
}

/// Session ids must be safe file names; anything else is rejected.
fn entry_path(store_root: &Path, session_id: &str) -> Result<PathBuf, CoreError> {
    let safe = session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if session_id.is_empty() || !safe {
        return Err(CoreError::Parse(format!("invalid session id: {session_id:?}")));
    }
    Ok(queue_dir(store_root).join(format!("{session_id}.json")))
}

/// Idempotent by session id: re-enqueueing overwrites the entry.
pub fn enqueue(store_root: &Path, entry: &QueueEntry) -> Result<(), CoreError> {
    let io = |e: std::io::Error| CoreError::Io(e.to_string());
    let path = entry_path(store_root, &entry.session_id)?;
    std::fs::create_dir_all(queue_dir(store_root)).map_err(io)?;
    let json = serde_json::to_string_pretty(entry).map_err(|e| CoreError::Io(e.to_string()))?;
    std::fs::write(&path, json).map_err(io)
}

/// All entries, oldest first. Unparseable files are skipped (prune_stale removes them).
pub fn list(store_root: &Path) -> Result<Vec<QueueEntry>, CoreError> {
    let dir = queue_dir(store_root);
    let mut entries = Vec::new();
    if !dir.is_dir() {
        return Ok(entries);
    }
    let read = std::fs::read_dir(&dir).map_err(|e| CoreError::Io(e.to_string()))?;
    for item in read.flatten() {
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(entry) = serde_json::from_str::<QueueEntry>(&content) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by(|a, b| a.enqueued_at.cmp(&b.enqueued_at));
    Ok(entries)
}

pub fn remove(store_root: &Path, session_id: &str) -> Result<(), CoreError> {
    let path = entry_path(store_root, session_id)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| CoreError::Io(e.to_string()))?;
    }
    Ok(())
}

/// Remove entries whose transcript no longer exists, plus unparseable entry
/// files. Returns the removed session ids (callers record them in health —
/// visible, never silently retried forever).
pub fn prune_stale(store_root: &Path) -> Result<Vec<String>, CoreError> {
    let dir = queue_dir(store_root);
    let mut pruned = Vec::new();
    if !dir.is_dir() {
        return Ok(pruned);
    }
    let read = std::fs::read_dir(&dir).map_err(|e| CoreError::Io(e.to_string()))?;
    for item in read.flatten() {
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let stale = match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<QueueEntry>(&content) {
                Ok(entry) => !Path::new(&entry.transcript_path).exists(),
                Err(_) => true, // corrupt entry
            },
            Err(_) => true,
        };
        if stale {
            std::fs::remove_file(&path).map_err(|e| CoreError::Io(e.to_string()))?;
            pruned.push(stem);
        }
    }
    pruned.sort();
    Ok(pruned)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p retro-core store::queue`
Expected: 4 PASS.

- [ ] **Step 5: Commit**

```bash
cargo test -p retro-core && git add crates/retro-core/src/store/queue.rs crates/retro-core/src/store/mod.rs && git commit -m "feat(v3): pending-session queue with stale pruning"
```

---

### Task 5: v3 project registry — auto-register, notify, exclude

**Files:**
- Create: `crates/retro-core/src/store/projects.rs`
- Modify: `crates/retro-core/src/store/mod.rs` (add `pub mod projects;`)

Identity per the spec: slug + remote URL are **committed** (`knowledge/projects/<slug>/project.toml` — a non-`.md` file, invisible to `load_all`); the slug→local-path map is **machine-local** (`state/projects.json`, rebuildable). Matching order: remote_url first (stable across machines/moves), then recorded path, else register new. Exclusion is by path prefix against `config.privacy.exclude_projects`; excluding a registered project deletes its knowledge subtree (recoverable via store git history) and its `CLAUDE.local.md` managed block.

- [ ] **Step 1: Write failing tests**

Create `crates/retro-core/src/store/projects.rs`:

```rust
//! v3 project registry. Committed identity (project.toml per project dir),
//! machine-local path map (state/projects.json), auto-registration from
//! session cwd, and exclusion with cleanup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Store, slugify};
use crate::errors::CoreError;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn git_project(dir: &Path, remote: Option<&str>) {
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .output()
                .unwrap()
        };
        run(&["init"]);
        if let Some(url) = remote {
            run(&["remote", "add", "origin", url]);
        }
    }

    #[test]
    fn register_new_project_creates_meta_and_notifies() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        let proj_tmp = TempDir::new().unwrap();
        let proj = proj_tmp.path().join("My API-Service");
        std::fs::create_dir_all(&proj).unwrap();
        git_project(&proj, Some("git@github.com:me/my-api.git"));

        let reg = register(&store, proj.to_str().unwrap()).unwrap();
        assert!(reg.newly_registered);
        assert_eq!(reg.slug, "my-api-service");
        // committed identity file exists
        let meta_path = store_tmp
            .path()
            .join("knowledge/projects/my-api-service/project.toml");
        let meta = std::fs::read_to_string(meta_path).unwrap();
        assert!(meta.contains("git@github.com:me/my-api.git"));
        // second registration is a no-op
        let again = register(&store, proj.to_str().unwrap()).unwrap();
        assert!(!again.newly_registered);
        assert_eq!(again.slug, "my-api-service");
    }

    #[test]
    fn register_matches_by_remote_url_when_path_moved() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        let a = TempDir::new().unwrap();
        git_project(a.path(), Some("git@github.com:me/stable.git"));
        let first = register(&store, a.path().to_str().unwrap()).unwrap();

        // same repo cloned elsewhere (different path, same remote)
        let b = TempDir::new().unwrap();
        git_project(b.path(), Some("git@github.com:me/stable.git"));
        let second = register(&store, b.path().to_str().unwrap()).unwrap();
        assert!(!second.newly_registered);
        assert_eq!(second.slug, first.slug);
        // path map updated to the new location
        let map = PathMap::load(store_tmp.path()).unwrap();
        assert_eq!(map.paths[&first.slug], b.path().display().to_string());
    }

    #[test]
    fn non_git_directory_registers_by_path() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        let proj = TempDir::new().unwrap();
        let reg = register(&store, proj.path().to_str().unwrap()).unwrap();
        assert!(reg.newly_registered);
        let again = register(&store, proj.path().to_str().unwrap()).unwrap();
        assert!(!again.newly_registered);
    }

    #[test]
    fn is_excluded_matches_path_prefixes() {
        let excludes = vec!["/Users/me/private".to_string()];
        assert!(is_excluded("/Users/me/private/notes", &excludes));
        assert!(is_excluded("/Users/me/private", &excludes));
        assert!(!is_excluded("/Users/me/privateer", &excludes));
        assert!(!is_excluded("/Users/me/work/app", &excludes));
    }

    #[test]
    fn cleanup_excluded_removes_knowledge_and_local_md_block() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        let proj = TempDir::new().unwrap();
        git_project(proj.path(), None);
        let reg = register(&store, proj.path().to_str().unwrap()).unwrap();

        // seed a node + a projected CLAUDE.local.md
        let dir = store_tmp
            .path()
            .join("knowledge/projects")
            .join(&reg.slug);
        assert!(dir.is_dir());
        std::fs::write(
            proj.path().join("CLAUDE.local.md"),
            "<!-- retro:managed:start -->\n- old rule\n<!-- retro:managed:end -->\n",
        )
        .unwrap();

        cleanup_excluded(&store, &reg.slug, Some(proj.path().to_str().unwrap())).unwrap();
        assert!(!dir.exists());
        assert!(!proj.path().join("CLAUDE.local.md").exists());
        let map = PathMap::load(store_tmp.path()).unwrap();
        assert!(!map.paths.contains_key(&reg.slug));
    }
}
```

- [ ] **Step 2: Verify compile failure**

Run: `cargo test -p retro-core store::projects`
Expected: compile error. (Add `pub mod projects;` to `store/mod.rs` now.)

- [ ] **Step 3: Implement**

Add above the tests in `crates/retro-core/src/store/projects.rs`:

```rust
/// Committed per-project identity (knowledge/projects/<slug>/project.toml).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub slug: String,
    #[serde(default)]
    pub remote_url: Option<String>,
    /// YYYY-MM-DD of first registration.
    pub registered: String,
}

/// Machine-local slug -> absolute path map (state/projects.json). Rebuildable:
/// re-derived from observed sessions, so losing it only delays resolution.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PathMap {
    #[serde(default)]
    pub paths: BTreeMap<String, String>,
}

impl PathMap {
    pub fn load(store_root: &Path) -> Result<Self, CoreError> {
        let path = store_root.join("state").join("projects.json");
        match std::fs::read_to_string(&path) {
            Ok(content) => Ok(serde_json::from_str(&content).unwrap_or_default()),
            Err(_) => Ok(PathMap::default()),
        }
    }

    pub fn save(&self, store_root: &Path) -> Result<(), CoreError> {
        let io = |e: std::io::Error| CoreError::Io(e.to_string());
        let dir = store_root.join("state");
        std::fs::create_dir_all(&dir).map_err(io)?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| CoreError::Io(e.to_string()))?;
        std::fs::write(dir.join("projects.json"), json).map_err(io)
    }
}

pub struct Registration {
    pub slug: String,
    pub newly_registered: bool,
}

fn git_in(dir: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn read_meta(store: &Store, slug: &str) -> Option<ProjectMeta> {
    let path = store
        .knowledge_dir()
        .join("projects")
        .join(slug)
        .join("project.toml");
    let content = std::fs::read_to_string(path).ok()?;
    toml::from_str(&content).ok()
}

fn write_meta(store: &Store, meta: &ProjectMeta) -> Result<(), CoreError> {
    let io = |e: std::io::Error| CoreError::Io(e.to_string());
    let dir = store.knowledge_dir().join("projects").join(&meta.slug);
    std::fs::create_dir_all(&dir).map_err(io)?;
    let content =
        toml::to_string_pretty(meta).map_err(|e| CoreError::Io(e.to_string()))?;
    std::fs::write(dir.join("project.toml"), content).map_err(io)
}

fn all_metas(store: &Store) -> Vec<ProjectMeta> {
    let mut metas = Vec::new();
    let projects_dir = store.knowledge_dir().join("projects");
    if let Ok(read) = std::fs::read_dir(&projects_dir) {
        for item in read.flatten() {
            if item.path().is_dir() {
                if let Some(slug) = item.file_name().to_str() {
                    if let Some(meta) = read_meta(store, slug) {
                        metas.push(meta);
                    }
                }
            }
        }
    }
    metas
}

/// Register (or recognize) the project containing `cwd`. Resolution:
/// git root of cwd (falls back to cwd for non-git dirs) -> match existing
/// registrations by remote_url, then by recorded path, else create new.
/// Never call this for excluded paths — check `is_excluded` first.
pub fn register(store: &Store, cwd: &str) -> Result<Registration, CoreError> {
    let root = git_in(cwd, &["rev-parse", "--show-toplevel"])
        .unwrap_or_else(|| cwd.to_string());
    let remote = git_in(&root, &["remote", "get-url", "origin"]);

    let mut map = PathMap::load(store.root())?;

    // 1. remote_url match (stable identity)
    if let Some(ref url) = remote {
        if let Some(meta) = all_metas(store)
            .into_iter()
            .find(|m| m.remote_url.as_deref() == Some(url.as_str()))
        {
            if map.paths.get(&meta.slug).map(String::as_str) != Some(root.as_str()) {
                map.paths.insert(meta.slug.clone(), root.clone());
                map.save(store.root())?;
            }
            return Ok(Registration { slug: meta.slug, newly_registered: false });
        }
    }
    // 2. recorded-path match (non-git dirs, or repos without remotes)
    if let Some((slug, _)) = map.paths.iter().find(|(_, p)| p.as_str() == root) {
        return Ok(Registration { slug: slug.clone(), newly_registered: false });
    }

    // 3. new registration
    let base = Path::new(&root)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");
    let mut slug = slugify(base);
    let mut i = 2;
    while read_meta(store, &slug).is_some() {
        slug = format!("{}-{}", slugify(base), i);
        i += 1;
    }
    write_meta(
        store,
        &ProjectMeta {
            slug: slug.clone(),
            remote_url: remote,
            registered: chrono::Utc::now().date_naive().to_string(),
        },
    )?;
    map.paths.insert(slug.clone(), root);
    map.save(store.root())?;
    Ok(Registration { slug, newly_registered: true })
}

/// Path-prefix exclusion against `config.privacy.exclude_projects`.
/// A prefix matches the directory itself or anything under it.
pub fn is_excluded(path: &str, exclude_projects: &[String]) -> bool {
    exclude_projects.iter().any(|prefix| {
        path == prefix.as_str() || path.starts_with(&format!("{}/", prefix.trim_end_matches('/')))
    })
}

/// Exclusion cleanup: delete the project's knowledge subtree (recoverable via
/// store git history), drop it from the path map, and remove its
/// CLAUDE.local.md (the whole file — it is retro-owned build output).
pub fn cleanup_excluded(
    store: &Store,
    slug: &str,
    project_path: Option<&str>,
) -> Result<(), CoreError> {
    let dir = store.knowledge_dir().join("projects").join(slug);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| CoreError::Io(e.to_string()))?;
    }
    let mut map = PathMap::load(store.root())?;
    if map.paths.remove(slug).is_some() {
        map.save(store.root())?;
    }
    if let Some(path) = project_path {
        let local_md = Path::new(path).join("CLAUDE.local.md");
        if local_md.exists() {
            std::fs::remove_file(&local_md).map_err(|e| CoreError::Io(e.to_string()))?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p retro-core store::projects`
Expected: 5 PASS.

- [ ] **Step 5: Commit**

```bash
cargo test -p retro-core && git add crates/retro-core/src/store/projects.rs crates/retro-core/src/store/mod.rs && git commit -m "feat(v3): project registry — auto-register, remote-url identity, exclusion cleanup"
```

---

### Task 6: Analysis sink — v2 engine, markdown store target (+ MockBackend)

**Files:**
- Create: `crates/retro-core/src/analysis/v3.rs`
- Modify: `crates/retro-core/src/analysis/mod.rs` (add `pub mod v3;` with the other module declarations at the top)
- Modify: `crates/retro-core/src/analysis/backend.rs` (add `MockBackend`)

The whole v2 engine is reused: compact sessions → `build_graph_analysis_prompt` (with v2 `KnowledgeNode` shims for existing-node context) → `GRAPH_ANALYSIS_RESPONSE_SCHEMA` → `parse_graph_response` → `Vec<GraphOperation>`. Only the APPLICATION differs: operations mutate the markdown store. Mapping (v3 has no edge table):
- `CreateNode` → `unique_slug(first 8 words of content)` + `write_node` (type mapping: `Skill`→`Pattern`, `Directive`→`Rule`; confidence clamped to [0,1]; `sources` = the batch's session ids)
- `UpdateNode{id}` → `get` + bump confidence/content + union batch session ids into sources + `updated=today` + `write_node`
- `MergeNodes{keep, remove}` → union `remove`'s sources into `keep`, then `invalidate(remove, by=keep)`
- `CreateEdge{Supersedes}` → `invalidate(target, by=source)`; all other edge types are counted and ignored (v3 stores no edges)
- Node ids in operations refer to ids shown in the prompt context — which for v3 are the store slugs (the shim uses the v3 slug as the v2 `id`).

- [ ] **Step 1: Add MockBackend with a failing smoke test**

In `crates/retro-core/src/analysis/backend.rs`, append:

```rust
/// Scripted backend for tests: returns canned responses in order, recording
/// prompts. Lives in production code (not cfg(test)) so retro-cli integration
/// tests and runner_v3 tests can use it too.
#[derive(Default)]
pub struct MockBackend {
    pub responses: std::sync::Mutex<Vec<String>>,
    pub prompts_seen: std::sync::Mutex<Vec<String>>,
}

impl MockBackend {
    pub fn with_responses(responses: Vec<String>) -> Self {
        MockBackend {
            responses: std::sync::Mutex::new(responses),
            prompts_seen: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl AnalysisBackend for MockBackend {
    fn execute(
        &self,
        prompt: &str,
        _json_schema: Option<&str>,
    ) -> Result<BackendResponse, CoreError> {
        self.prompts_seen.lock().unwrap().push(prompt.to_string());
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            return Err(CoreError::Analysis("MockBackend: no responses left".to_string()));
        }
        Ok(BackendResponse {
            text: responses.remove(0),
            input_tokens: 100,
            output_tokens: 50,
        })
    }
}
```

- [ ] **Step 2: Write failing tests for the sink**

Create `crates/retro-core/src/analysis/v3.rs`:

```rust
//! v3 analysis: reuse the v2 engine (compact sessions, graph prompt, schema,
//! response parsing) and apply the resulting GraphOperations to the markdown
//! store instead of SQLite.

use std::path::Path;

use chrono::Utc;

use crate::analysis::backend::AnalysisBackend;
use crate::analysis::{GRAPH_ANALYSIS_RESPONSE_SCHEMA, parse_graph_response, prompts};
use crate::errors::CoreError;
use crate::models::{
    EdgeType, GraphOperation, KnowledgeNode, NodeScope, NodeStatus, NodeType as V2NodeType,
    Session,
};
use crate::store::{Node, NodeType, Scope, Store};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::backend::MockBackend;
    use crate::models::{ParsedUserMessage, SessionMetadata};
    use tempfile::TempDir;

    fn session(id: &str, msgs: &[&str]) -> Session {
        Session {
            session_id: id.to_string(),
            project: "/tmp/proj".to_string(),
            session_path: format!("/tmp/{id}.jsonl"),
            user_messages: msgs
                .iter()
                .map(|m| ParsedUserMessage { text: m.to_string(), timestamp: None })
                .collect(),
            assistant_messages: vec![],
            summaries: vec![],
            tools_used: vec![],
            errors: vec![],
            metadata: SessionMetadata::default(),
        }
    }

    fn store() -> (TempDir, Store) {
        let tmp = TempDir::new().unwrap();
        let s = Store::open(tmp.path());
        s.ensure_layout().unwrap();
        (tmp, s)
    }

    #[test]
    fn create_node_writes_to_store_with_sources_and_clamped_confidence() {
        let (_tmp, store) = store();
        let response = r#"{"reasoning":"saw a rule","operations":[
            {"action":"create_node","node_type":"rule","scope":"project","content":"Always run smoke tests before full runs.","confidence":1.7}
        ]}"#;
        let backend = MockBackend::with_responses(vec![response.to_string()]);
        let result = analyze_sessions(
            &store,
            &backend,
            &[session("s1", &["please smoke test first", "ok run it"])],
            Some("my-proj"),
        )
        .unwrap();
        assert_eq!(result.nodes_created, 1);
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.nodes.len(), 1);
        let node = &loaded.nodes[0].1;
        assert_eq!(node.scope, Scope::Project("my-proj".to_string()));
        assert_eq!(node.node_type, NodeType::Rule);
        assert!((node.confidence - 1.0).abs() < f64::EPSILON, "clamped");
        assert_eq!(node.sources, vec!["session:s1".to_string()]);
        assert!(node.body.contains("smoke tests"));
    }

    #[test]
    fn update_and_merge_operations_mutate_existing_nodes() {
        let (_tmp, store) = store();
        let today = chrono::Utc::now().date_naive();
        let mk = |id: &str, conf: f64| Node {
            id: id.to_string(),
            scope: Scope::Global,
            node_type: NodeType::Rule,
            confidence: conf,
            sources: vec!["session:old".to_string()],
            created: today,
            updated: today,
            invalidated_by: None,
            body: format!("rule body {id}"),
        };
        store.write_node(&mk("keeper", 0.5)).unwrap();
        store.write_node(&mk("duplicate", 0.4)).unwrap();

        let response = r#"{"reasoning":"recurrence + dup","operations":[
            {"action":"update_node","node_id":"keeper","new_confidence":0.8},
            {"action":"merge_nodes","keep_id":"keeper","remove_id":"duplicate"}
        ]}"#;
        let backend = MockBackend::with_responses(vec![response.to_string()]);
        let result = analyze_sessions(
            &store,
            &backend,
            &[session("s2", &["msg one", "msg two"])],
            None,
        )
        .unwrap();
        assert_eq!(result.nodes_updated, 1);
        assert_eq!(result.nodes_merged, 1);

        let keeper = store.get(&Scope::Global, "keeper").unwrap().unwrap();
        assert!((keeper.confidence - 0.8).abs() < f64::EPSILON);
        assert!(keeper.sources.contains(&"session:s2".to_string()));
        assert!(keeper.sources.contains(&"session:old".to_string()), "merge unions sources");

        let dup = store.get(&Scope::Global, "duplicate").unwrap().unwrap();
        assert_eq!(dup.invalidated_by.as_deref(), Some("keeper"));
    }

    #[test]
    fn supersedes_edge_invalidates_target_other_edges_ignored() {
        let (_tmp, store) = store();
        let today = chrono::Utc::now().date_naive();
        let mk = |id: &str| Node {
            id: id.to_string(),
            scope: Scope::Global,
            node_type: NodeType::Rule,
            confidence: 0.7,
            sources: vec![],
            created: today,
            updated: today,
            invalidated_by: None,
            body: format!("body {id}"),
        };
        store.write_node(&mk("new-way")).unwrap();
        store.write_node(&mk("old-way")).unwrap();

        let response = r#"{"reasoning":"contradiction resolved","operations":[
            {"action":"create_edge","source_id":"new-way","target_id":"old-way","edge_type":"supersedes"},
            {"action":"create_edge","source_id":"new-way","target_id":"old-way","edge_type":"supports"}
        ]}"#;
        let backend = MockBackend::with_responses(vec![response.to_string()]);
        let result = analyze_sessions(
            &store,
            &backend,
            &[session("s3", &["a", "b"])],
            None,
        )
        .unwrap();
        assert_eq!(result.nodes_invalidated, 1);
        assert_eq!(result.edges_ignored, 1);
        let old = store.get(&Scope::Global, "old-way").unwrap().unwrap();
        assert_eq!(old.invalidated_by.as_deref(), Some("new-way"));
    }

    #[test]
    fn low_signal_sessions_are_filtered_before_any_ai_call() {
        let (_tmp, store) = store();
        let backend = MockBackend::with_responses(vec![]); // any AI call would error
        let result = analyze_sessions(
            &store,
            &backend,
            &[session("tiny", &["single message"])],
            None,
        )
        .unwrap();
        assert_eq!(result.sessions_analyzed, 0);
        assert_eq!(result.nodes_created, 0);
    }

    #[test]
    fn existing_nodes_appear_in_prompt_context() {
        let (_tmp, store) = store();
        let today = chrono::Utc::now().date_naive();
        store
            .write_node(&Node {
                id: "existing-rule".to_string(),
                scope: Scope::Global,
                node_type: NodeType::Rule,
                confidence: 0.9,
                sources: vec![],
                created: today,
                updated: today,
                invalidated_by: None,
                body: "a very distinctive existing rule body".to_string(),
            })
            .unwrap();
        let response = r#"{"reasoning":"nothing new","operations":[]}"#;
        let backend = MockBackend::with_responses(vec![response.to_string()]);
        analyze_sessions(&store, &backend, &[session("s4", &["a", "b"])], None).unwrap();
        let prompts = backend.prompts_seen.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("existing-rule"), "prompt shows store node ids");
    }
}
```

- [ ] **Step 3: Verify compile failure**

Run: `cargo test -p retro-core analysis::v3`
Expected: compile error — `analyze_sessions` not found. (Add `pub mod v3;` to `analysis/mod.rs` now. Also check the exact names of `EdgeType` variants and `SessionMetadata: Default` — if `SessionMetadata` doesn't derive `Default`, construct it with all-`None` fields in the test instead.)

- [ ] **Step 4: Implement the sink**

Add above the tests in `crates/retro-core/src/analysis/v3.rs`:

```rust
/// Result of one v3 analysis batch.
#[derive(Debug, Default)]
pub struct V3AnalyzeResult {
    pub sessions_analyzed: usize,
    pub nodes_created: usize,
    pub nodes_updated: usize,
    pub nodes_merged: usize,
    pub nodes_invalidated: usize,
    pub edges_ignored: usize,
    pub reasoning: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Bodies of nodes created/updated — for briefing notifications.
    pub learned: Vec<String>,
}

/// Shim: present a v3 store node to the v2 prompt builder. Only id, content,
/// confidence, type, and scope influence the prompt (content truncated to
/// 200 chars there); the rest are placeholders.
fn shim(node: &Node) -> KnowledgeNode {
    KnowledgeNode {
        id: node.id.clone(),
        node_type: match node.node_type {
            NodeType::Rule => V2NodeType::Rule,
            NodeType::Preference => V2NodeType::Preference,
            NodeType::Pattern => V2NodeType::Pattern,
            NodeType::Memory => V2NodeType::Memory,
        },
        scope: match &node.scope {
            Scope::Global => NodeScope::Global,
            Scope::Project(_) => NodeScope::Project,
        },
        project_id: match &node.scope {
            Scope::Global => None,
            Scope::Project(slug) => Some(slug.clone()),
        },
        content: node.body.clone(),
        confidence: node.confidence,
        status: NodeStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        projected_at: None,
        pr_url: None,
    }
}

fn v3_node_type(t: &V2NodeType) -> NodeType {
    match t {
        V2NodeType::Rule | V2NodeType::Directive => NodeType::Rule,
        V2NodeType::Preference => NodeType::Preference,
        V2NodeType::Pattern | V2NodeType::Skill => NodeType::Pattern,
        V2NodeType::Memory => NodeType::Memory,
    }
}

fn union_sources(existing: &mut Vec<String>, extra: &[String]) {
    for s in extra {
        if !existing.contains(s) {
            existing.push(s.clone());
        }
    }
}

/// Analyze one batch of parsed sessions against the store and apply the
/// resulting operations. `project_slug` scopes project-level operations.
/// Caller is responsible for: session filtering by project, scrubbing,
/// budget accounting (one backend call per invocation), and committing.
pub fn analyze_sessions(
    store: &Store,
    backend: &dyn AnalysisBackend,
    sessions: &[Session],
    project_slug: Option<&str>,
) -> Result<V3AnalyzeResult, CoreError> {
    let mut result = V3AnalyzeResult::default();

    // Low-signal filter (same rule as v2: < 2 user messages = skip).
    let signal: Vec<&Session> =
        sessions.iter().filter(|s| s.user_messages.len() >= 2).collect();
    if signal.is_empty() {
        return Ok(result);
    }
    result.sessions_analyzed = signal.len();
    let session_sources: Vec<String> =
        signal.iter().map(|s| format!("session:{}", s.session_id)).collect();

    // Existing-node context: active nodes for global + this project's scope.
    let loaded = store.load_all()?;
    let context: Vec<KnowledgeNode> = loaded
        .nodes
        .iter()
        .map(|(_, n)| n)
        .filter(|n| n.is_active())
        .filter(|n| match (&n.scope, project_slug) {
            (Scope::Global, _) => true,
            (Scope::Project(slug), Some(p)) => slug == p,
            (Scope::Project(_), None) => false,
        })
        .map(shim)
        .collect();

    let compact: Vec<_> = signal.iter().map(|s| prompts::to_compact_session(s)).collect();
    let prompt = prompts::build_graph_analysis_prompt(&compact, &context, project_slug);
    let response = backend.execute(&prompt, Some(GRAPH_ANALYSIS_RESPONSE_SCHEMA))?;
    result.input_tokens = response.input_tokens;
    result.output_tokens = response.output_tokens;

    let operations = parse_graph_response(&response.text, project_slug)?;
    let today = Utc::now().date_naive();

    for op in operations {
        match op {
            GraphOperation::CreateNode { node_type, scope, project_id, content, confidence } => {
                let v3_scope = match scope {
                    NodeScope::Global => Scope::Global,
                    NodeScope::Project => match project_id.or_else(|| project_slug.map(String::from)) {
                        Some(slug) => Scope::Project(slug),
                        None => Scope::Global,
                    },
                };
                let id = store.unique_slug(&content.split_whitespace().take(8).collect::<Vec<_>>().join(" "), &v3_scope);
                let node = Node {
                    id,
                    scope: v3_scope,
                    node_type: v3_node_type(&node_type),
                    confidence: confidence.clamp(0.0, 1.0),
                    sources: session_sources.clone(),
                    created: today,
                    updated: today,
                    invalidated_by: None,
                    body: content.trim().to_string(),
                };
                store.write_node(&node)?;
                result.learned.push(node.body.clone());
                result.nodes_created += 1;
            }
            GraphOperation::UpdateNode { id, confidence, content } => {
                let found = find_node(store, &id, project_slug)?;
                let Some((scope, mut node)) = found else { continue };
                if let Some(c) = confidence {
                    node.confidence = c.clamp(0.0, 1.0);
                }
                if let Some(c) = content {
                    node.body = c.trim().to_string();
                }
                union_sources(&mut node.sources, &session_sources);
                node.updated = today;
                node.scope = scope;
                store.write_node(&node)?;
                result.learned.push(node.body.clone());
                result.nodes_updated += 1;
            }
            GraphOperation::MergeNodes { keep_id, remove_id } => {
                let keep = find_node(store, &keep_id, project_slug)?;
                let removed = find_node(store, &remove_id, project_slug)?;
                let (Some((keep_scope, mut keep_node)), Some((remove_scope, remove_node))) =
                    (keep, removed)
                else {
                    continue;
                };
                union_sources(&mut keep_node.sources, &remove_node.sources);
                union_sources(&mut keep_node.sources, &session_sources);
                keep_node.updated = today;
                keep_node.scope = keep_scope;
                store.write_node(&keep_node)?;
                store.invalidate(&remove_scope, &remove_node.id, &keep_node.id)?;
                result.nodes_merged += 1;
            }
            GraphOperation::CreateEdge { source_id, target_id, edge_type } => {
                if edge_type == EdgeType::Supersedes {
                    if let Some((scope, node)) = find_node(store, &target_id, project_slug)? {
                        store.invalidate(&scope, &node.id, &source_id)?;
                        result.nodes_invalidated += 1;
                    }
                } else {
                    // v3 stores no edges; supports/contradicts/derived_from/applies_to
                    // are counted for the summary and dropped.
                    result.edges_ignored += 1;
                }
            }
        }
    }
    Ok(result)
}

/// Resolve an operation's node id: try the batch's project scope first, then global.
fn find_node(
    store: &Store,
    id: &str,
    project_slug: Option<&str>,
) -> Result<Option<(Scope, Node)>, CoreError> {
    if let Some(slug) = project_slug {
        let scope = Scope::Project(slug.to_string());
        if let Some(node) = store.get(&scope, id)? {
            return Ok(Some((scope, node)));
        }
    }
    let scope = Scope::Global;
    if let Some(node) = store.get(&scope, id)? {
        return Ok(Some((scope, node)));
    }
    Ok(None)
}
```

Adaptation note: check `EdgeType`'s actual variant names in models.rs and whether it derives `PartialEq` (add the derive if missing — that is an allowed one-line change). If `parse_graph_response` maps unknown ids differently than assumed, keep this sink's behavior (skip silently, counted implicitly by not incrementing) and note the deviation in your report.

- [ ] **Step 5: Run tests**

Run: `cargo test -p retro-core analysis::v3`
Expected: 5 PASS.

- [ ] **Step 6: Commit**

```bash
cargo test -p retro-core && git add crates/retro-core/src/analysis/ && git commit -m "feat(v3): analysis sink — v2 engine, markdown store target, MockBackend"
```

---

### Task 7: One-way projection — global CLAUDE.md + CLAUDE.local.md

**Files:**
- Create: `crates/retro-core/src/projection/local_md.rs`
- Modify: `crates/retro-core/src/projection/mod.rs` (add `pub mod local_md;`)

One-way, idempotent, full regeneration of managed blocks from the store. Reuses `claude_md::update_claude_md_content` (which replaces or appends the managed section and preserves everything outside it). Project scope goes to `CLAUDE.local.md` at the project root, kept out of the shared repo via `.git/info/exclude` (personal ignore — never touches the team's `.gitignore`).

- [ ] **Step 1: Write failing tests**

Create `crates/retro-core/src/projection/local_md.rs`:

```rust
//! v3 one-way projection: regenerate managed blocks from the store.
//! Global nodes -> ~/.claude/CLAUDE.md; project nodes -> <project>/CLAUDE.local.md.
//! Managed blocks are build output — edits belong in the store.

use std::path::Path;

use crate::config::Config;
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
        store.write_node(&node("high", Scope::Global, NodeType::Rule, 0.9, "high rule")).unwrap();
        store.write_node(&node("low", Scope::Global, NodeType::Rule, 0.5, "low rule")).unwrap();
        store.write_node(&node("mem", Scope::Global, NodeType::Memory, 0.9, "memory")).unwrap();
        let mut dead = node("dead", Scope::Global, NodeType::Rule, 0.9, "dead rule");
        dead.invalidated_by = Some("high".to_string());
        store.write_node(&dead).unwrap();
        store
            .write_node(&node("proj", Scope::Project("p".to_string()), NodeType::Rule, 0.9, "proj rule"))
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
            .write_node(&node("r", Scope::Project("p".to_string()), NodeType::Rule, 0.9, "the rule"))
            .unwrap();

        let proj = TempDir::new().unwrap();
        std::process::Command::new("git")
            .arg("-C").arg(proj.path()).arg("init")
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
        assert!(!content.contains("stale rule"), "managed block regenerated empty");
    }

    #[test]
    fn project_global_md_preserves_user_content() {
        let store_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        store.write_node(&node("g", Scope::Global, NodeType::Rule, 0.9, "global rule")).unwrap();

        let claude_tmp = TempDir::new().unwrap();
        let md = claude_tmp.path().join("CLAUDE.md");
        std::fs::write(&md, "# My instructions\n\nuser text\n").unwrap();

        project_global_md(&store, &md, 0.7, None).unwrap();
        let content = std::fs::read_to_string(&md).unwrap();
        assert!(content.contains("user text"));
        assert!(content.contains("- global rule"));
    }
}
```

- [ ] **Step 2: Verify compile failure**

Run: `cargo test -p retro-core projection::local_md`
Expected: compile error. (Add `pub mod local_md;` to `projection/mod.rs` now. Note: this task assumes `Config` is not needed by the functions below — remove the unused `use crate::config::Config;` import if the compiler flags it.)

- [ ] **Step 3: Implement**

Add above the tests in `crates/retro-core/src/projection/local_md.rs`:

```rust
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
    Ok(nodes.into_iter().map(|n| n.body).collect())
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

fn write_managed(path: &Path, rules: &[String], backup_dir: Option<&Path>) -> Result<(), CoreError> {
    let io = |e: std::io::Error| CoreError::Io(e.to_string());
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    if let Some(dir) = backup_dir {
        if path.exists() {
            crate::util::backup_file(&path.display().to_string(), dir)?;
        }
    }
    let updated = update_claude_md_content(&existing, rules);
    std::fs::write(path, updated).map_err(io)
}

/// Append CLAUDE.local.md to .git/info/exclude if the project is a git repo
/// and the line isn't already present. Non-git directories are a no-op.
fn ensure_git_exclude(project_root: &Path) -> Result<(), CoreError> {
    let io = |e: std::io::Error| CoreError::Io(e.to_string());
    let git_dir = project_root.join(".git");
    if !git_dir.is_dir() {
        return Ok(());
    }
    let info_dir = git_dir.join("info");
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
```

Adaptation note: verify `update_claude_md_content(existing, &[])` with an empty rules slice produces an empty managed section rather than removing it or panicking — read claude_md.rs:25-44. If it misbehaves on empty input, special-case: replace the managed block with an empty one using the same delimiter constants (import them or re-derive; they are private consts, so if needed add `pub(crate)` visibility to `MANAGED_START`/`MANAGED_END` in claude_md.rs and note it).

- [ ] **Step 4: Run tests**

Run: `cargo test -p retro-core projection::local_md`
Expected: 4 PASS.

- [ ] **Step 5: Commit**

```bash
cargo test -p retro-core && git add crates/retro-core/src/projection/ && git commit -m "feat(v3): one-way projection to CLAUDE.md and CLAUDE.local.md with git exclude"
```

---

### Task 8: Hook-event parsing + `retro observe` command

**Files:**
- Create: `crates/retro-core/src/hook_event.rs`
- Modify: `crates/retro-core/src/lib.rs` (add `pub mod hook_event;` after `pub mod health;`)
- Create: `crates/retro-cli/src/commands/observe.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs` (add `pub mod observe;`, alphabetical)
- Modify: `crates/retro-cli/src/main.rs` (variant + arm)

Claude Code hooks pipe a JSON event on stdin (`session_id`, `transcript_path`, `cwd`, plus fields we ignore). `retro observe` (SessionEnd): parse event → exclusion check → enqueue → register project (notify on new) → advance watermark → spawn detached `retro run --background` worker. It must NEVER fail the hook: any error path prints nothing to stdout, records health, exits 0.

- [ ] **Step 1: Write failing tests for event parsing**

Create `crates/retro-core/src/hook_event.rs`:

```rust
//! Claude Code hook stdin event (SessionEnd / SessionStart payload).

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct HookEvent {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub transcript_path: String,
    #[serde(default)]
    pub cwd: String,
}

impl HookEvent {
    /// Parse leniently: unknown fields ignored; empty/invalid input yields None
    /// (hook entries must never hard-fail on payload drift).
    pub fn parse(input: &str) -> Option<HookEvent> {
        let event: HookEvent = serde_json::from_str(input).ok()?;
        if event.session_id.is_empty() || event.transcript_path.is_empty() {
            return None;
        }
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_event_and_ignores_unknown_fields() {
        let json = r#"{"session_id":"abc-123","transcript_path":"/tmp/t.jsonl","cwd":"/tmp/proj","hook_event_name":"SessionEnd","extra":42}"#;
        let e = HookEvent::parse(json).unwrap();
        assert_eq!(e.session_id, "abc-123");
        assert_eq!(e.transcript_path, "/tmp/t.jsonl");
        assert_eq!(e.cwd, "/tmp/proj");
    }

    #[test]
    fn rejects_empty_or_invalid_input() {
        assert!(HookEvent::parse("").is_none());
        assert!(HookEvent::parse("not json").is_none());
        assert!(HookEvent::parse(r#"{"cwd":"/x"}"#).is_none()); // missing required fields
    }
}
```

- [ ] **Step 2: Verify failure, then wire `pub mod hook_event;` into lib.rs and run**

Run: `cargo test -p retro-core hook_event`
Expected: 2 PASS after wiring.

- [ ] **Step 3: Implement the observe command**

Create `crates/retro-cli/src/commands/observe.rs`:

```rust
use std::io::Read;

use anyhow::Result;
use retro_core::config::{Config, retro_dir};
use retro_core::hook_event::HookEvent;
use retro_core::store::{Store, projects, queue, state::RunnerState};
use retro_core::{git as _, health};

/// SessionEnd hook entry. Contract: NEVER fail the hook — errors are recorded
/// in health and swallowed; stdout stays clean; exit code is always 0.
pub fn run() -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml")).unwrap_or_default();
    if !config.v3.enabled {
        return Ok(());
    }
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Some(event) = HookEvent::parse(&input) else {
        let _ = health::record(&dir, "observe", false, "unparseable hook event");
        return Ok(());
    };
    if let Err(e) = observe_event(&dir, &config, &event) {
        let _ = health::record(&dir, "observe", false, &e.to_string());
        return Ok(());
    }
    let _ = health::record(&dir, "observe", true, &format!("enqueued {}", event.session_id));
    spawn_worker();
    Ok(())
}

fn observe_event(
    dir: &std::path::Path,
    config: &Config,
    event: &HookEvent,
) -> Result<(), retro_core::errors::CoreError> {
    if projects::is_excluded(&event.cwd, &config.privacy.exclude_projects) {
        return Ok(());
    }
    let store = Store::open(dir);
    store.ensure_layout()?;

    queue::enqueue(
        dir,
        &queue::QueueEntry {
            session_id: event.session_id.clone(),
            transcript_path: event.transcript_path.clone(),
            cwd: Some(event.cwd.clone()),
            enqueued_at: chrono::Utc::now().to_rfc3339(),
        },
    )?;

    let mut state = RunnerState::load(dir)?;
    if !event.cwd.is_empty() {
        let reg = projects::register(&store, &event.cwd)?;
        if reg.newly_registered {
            state.notifications.push(format!(
                "retro is now watching `{}` — exclude via privacy.exclude_projects in ~/.retro/config.toml",
                reg.slug
            ));
        }
    }
    // Advance the observe watermark to the transcript's mtime so the
    // SessionStart catch-up scan doesn't re-enqueue this session.
    if let Ok(meta) = std::fs::metadata(&event.transcript_path) {
        if let Ok(mtime) = meta.modified() {
            if let Ok(secs) = mtime.duration_since(std::time::UNIX_EPOCH) {
                state.last_observed_unix = state.last_observed_unix.max(secs.as_secs());
            }
        }
    }
    state.save(dir)
}

/// Detached background worker; inherits this process's environment (auth works
/// here — the whole point of hook-time capture). Errors ignored: the next
/// observe/brief will spawn again.
fn spawn_worker() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .args(["run", "--background"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}
```

(Remove the `use retro_core::{git as _, health};` oddity if the compiler objects — import `retro_core::health` plainly.)

- [ ] **Step 4: Wire into CLI**

`crates/retro-cli/src/commands/mod.rs`: add `pub mod observe;` (alphabetical, after `pub mod log;` / before `pub mod patterns;`).

`crates/retro-cli/src/main.rs`: add after the `Reindex` variant:

```rust
    /// (v3 hook entry) Enqueue a finished session for analysis — called by the SessionEnd hook
    Observe,
```

and the match arm next to `Commands::Reindex`:

```rust
        Commands::Observe => commands::observe::run(),
```

- [ ] **Step 5: Build + behavior check**

```bash
cargo build
echo '{"session_id":"smoke-1","transcript_path":"/tmp/nope.jsonl","cwd":"/tmp"}' | RETRO_HOME=$(mktemp -d) ./target/debug/retro observe; echo "exit=$?"
```

Expected: no output, `exit=0` (v3 disabled by default → early exit). Then with v3 enabled:

```bash
export RH=$(mktemp -d) && printf '[v3]\nenabled = true\n' > "$RH/config.toml" && touch /tmp/smoke-transcript.jsonl && echo '{"session_id":"smoke-1","transcript_path":"/tmp/smoke-transcript.jsonl","cwd":"/tmp"}' | RETRO_HOME="$RH" ./target/debug/retro observe && ls "$RH/queue/" && cat "$RH/health.json" | head -8
```

Expected: `smoke-1.json` in queue, health has an `observe` stage with `"ok": true`. (A background `retro run --background` was spawned; it exits quickly.)

- [ ] **Step 6: Run full tests and commit**

```bash
cargo test && git add crates/retro-core/src/hook_event.rs crates/retro-core/src/lib.rs crates/retro-cli/src/commands/observe.rs crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs && git commit -m "feat(v3): observe command — SessionEnd hook entry with never-fail contract"
```

---

### Task 9: `retro brief` command + v3 briefing builder

**Files:**
- Modify: `crates/retro-core/src/briefing.rs` (add `build_v3_briefing`)
- Create: `crates/retro-cli/src/commands/brief.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs` + `crates/retro-cli/src/main.rs` (wiring)

SessionStart hook entry: catch-up scan (sessions modified since the watermark — covers crashed sessions and other machines), enqueue them, spawn the worker, and print the briefing (drained notifications + health warnings). Stdout of a SessionStart hook is added to the session context — that's the delivery mechanism.

- [ ] **Step 1: Write failing test for the briefing builder**

Add to the tests module in `crates/retro-core/src/briefing.rs`:

```rust
    #[test]
    fn v3_briefing_formats_sections_and_empties_to_empty() {
        assert_eq!(build_v3_briefing(&[], &[]), "");
        let out = build_v3_briefing(
            &["retro is now watching `my-proj`".to_string(), "Learned: always smoke test".to_string()],
            &["retro analyze failed at 2026-07-06T10:00:00Z: exit 1".to_string()],
        );
        assert!(out.starts_with("Retro update"));
        assert!(out.contains("- retro is now watching `my-proj`"));
        assert!(out.contains("- Learned: always smoke test"));
        assert!(out.contains("⚠ retro analyze failed"));
    }
```

- [ ] **Step 2: Verify failure, implement the builder**

Add to `crates/retro-core/src/briefing.rs`:

```rust
/// v3 session briefing: notifications (new registrations, learned rules) plus
/// health warnings. Empty inputs produce an empty string (hook prints nothing).
pub fn build_v3_briefing(notifications: &[String], health_warnings: &[String]) -> String {
    if notifications.is_empty() && health_warnings.is_empty() {
        return String::new();
    }
    let mut out = String::from("Retro update — mention briefly to the user at conversation start.\n");
    for n in notifications {
        out.push_str(&format!("- {n}\n"));
    }
    for w in health_warnings {
        out.push_str(&format!("⚠ {w}\n"));
    }
    out
}
```

Run: `cargo test -p retro-core briefing` — expected PASS.

- [ ] **Step 3: Implement the brief command**

Create `crates/retro-cli/src/commands/brief.rs`:

```rust
use anyhow::Result;
use retro_core::config::{Config, retro_dir};
use retro_core::store::{queue, state::RunnerState};
use retro_core::{briefing, health, observer};

/// SessionStart hook entry: catch-up scan + briefing to stdout.
/// Same never-fail contract as observe.
pub fn run() -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml")).unwrap_or_default();
    if !config.v3.enabled {
        return Ok(());
    }
    let mut state = RunnerState::load(&dir).unwrap_or_default();

    // Catch-up: enqueue sessions modified since the watermark (crashed
    // sessions, other machines, missed hooks). cwd is unknown here; the
    // pipeline recovers it from the transcript's metadata.
    let since = if state.last_observed_unix > 0 {
        Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(state.last_observed_unix))
    } else {
        None
    };
    let modified = observer::find_modified_sessions(&config.claude_dir().join("projects"), since, &[]);
    let mut enqueued = 0usize;
    let mut max_seen = state.last_observed_unix;
    for m in &modified {
        let Some(stem) = m.path.file_stem().and_then(|s| s.to_str()) else { continue };
        let entry = queue::QueueEntry {
            session_id: stem.to_string(),
            transcript_path: m.path.display().to_string(),
            cwd: None,
            enqueued_at: chrono::Utc::now().to_rfc3339(),
        };
        if queue::enqueue(&dir, &entry).is_ok() {
            enqueued += 1;
        }
        if let Ok(secs) = m.mtime.duration_since(std::time::UNIX_EPOCH) {
            max_seen = max_seen.max(secs.as_secs());
        }
    }
    state.last_observed_unix = max_seen;

    // Briefing: drained notifications + current health warnings.
    let notifications = state.drain_notifications();
    let warnings = health::Health::load(&dir).map(|h| h.warnings()).unwrap_or_default();
    let text = briefing::build_v3_briefing(&notifications, &warnings);
    if !text.is_empty() {
        print!("{text}");
    }
    let _ = state.save(&dir);
    let _ = health::record(&dir, "brief", true, &format!("caught up {enqueued} session(s)"));

    if enqueued > 0 {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe)
                .args(["run", "--background"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
    Ok(())
}
```

Adaptation note: check `observer::find_modified_sessions`'s first parameter — verify whether it expects `~/.claude` or `~/.claude/projects` (read observer.rs:24-46) and pass accordingly.

- [ ] **Step 4: Wire into CLI** (same pattern as Task 8)

main.rs variant after `Observe`:

```rust
    /// (v3 hook entry) Catch-up scan + session briefing — called by the SessionStart hook
    Brief,
```

arm: `Commands::Brief => commands::brief::run(),` — and `pub mod brief;` in commands/mod.rs (alphabetical: after `pub mod apply;`, before `pub mod audit;` — check actual order).

- [ ] **Step 5: Extend the terminal nudge with v3 health (spec §5 opportunistic sweep)**

In `crates/retro-cli/src/commands/mod.rs`, find `check_and_display_nudge()` (mod.rs:271). At its top, add a v3 block (before the v2 logic, which stays untouched):

```rust
    // v3: surface pipeline health warnings on any interactive command.
    let dir = retro_core::config::retro_dir();
    if let Ok(config) = retro_core::config::Config::load(&dir.join("config.toml")) {
        if config.v3.enabled {
            if let Ok(health) = retro_core::health::Health::load(&dir) {
                for w in health.warnings() {
                    eprintln!("{} {}", "retro:".yellow(), w);
                }
            }
        }
    }
```

(Match the function's existing imports/colored usage; if it prints via `println!` styled blocks, follow that style instead of `eprintln!` — read the function first.)

- [ ] **Step 6: Behavior check + commit**

```bash
cargo build
RETRO_HOME=$(mktemp -d) ./target/debug/retro brief; echo "exit=$?"   # v3 off: silent, exit 0
cargo test && git add crates/retro-core/src/briefing.rs crates/retro-cli/src/commands/brief.rs crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs && git commit -m "feat(v3): brief command, session briefing, health nudge"
```

---

### Task 10: The v3 pipeline (`runner_v3`) + `retro run` dispatch

**Files:**
- Create: `crates/retro-core/src/runner_v3.rs`
- Modify: `crates/retro-core/src/lib.rs` (add `pub mod runner_v3;`)
- Modify: `crates/retro-cli/src/commands/run.rs` (v3 dispatch at the top of `run()`)
- Modify: `crates/retro-cli/src/main.rs` (add `--background` flag to `Run`)

Pipeline (all stages record health): lock → commit manual edits → prune stale queue → drain (parse, filter, scrub, resolve/register project, group by project) → budget-gated analysis per group (one AI call each; groups beyond budget stay queued = visible backpressure) → projection (global + touched projects) → notifications → index rebuild → store commit → best-effort push.

- [ ] **Step 1: Write failing tests**

Create `crates/retro-core/src/runner_v3.rs` with tests first:

```rust
//! The v3 pipeline: drain the session queue into the knowledge store.
//! No daemon — invoked by hooks (`retro run --background`) or manually.

use std::path::{Path, PathBuf};

use crate::analysis::backend::AnalysisBackend;
use crate::analysis::v3 as analysis_v3;
use crate::config::Config;
use crate::errors::CoreError;
use crate::health;
use crate::ingest::session::parse_session_file;
use crate::lock::LockFile;
use crate::models::Session;
use crate::projection::local_md;
use crate::scrub;
use crate::store::state::RunnerState;
use crate::store::{Store, git as store_git, index, projects, queue};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::backend::MockBackend;
    use tempfile::TempDir;

    /// Minimal session JSONL the v2 parser accepts: two user entries with cwd.
    /// ADAPTATION: copy the exact entry shape from the existing fixtures used by
    /// ingest tests (see crates/retro-core/src/ingest/session.rs tests and
    /// tests/fixtures/) — the shape below must be adjusted to match what
    /// parse_session_file actually parses. Verify the parsed session has
    /// user_messages.len() >= 2 and metadata.cwd == the project path.
    fn write_fixture_session(dir: &Path, id: &str, cwd: &str) -> PathBuf {
        let path = dir.join(format!("{id}.jsonl"));
        let line = |text: &str| {
            format!(
                r#"{{"type":"user","cwd":"{cwd}","message":{{"role":"user","content":"{text}"}}}}"#
            )
        };
        std::fs::write(&path, format!("{}\n{}\n", line("first message"), line("second message"))).unwrap();
        path
    }

    /// CRITICAL: config.paths.claude_dir MUST point at a tempdir — with the
    /// default (~/.claude) the projection stage would write the developer's
    /// real global CLAUDE.md during tests. The returned TempDir keeps it alive.
    fn setup() -> (TempDir, TempDir, Config) {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store_git::ensure_repo(tmp.path()).unwrap();
        let mut config = Config::default();
        config.v3.enabled = true;
        config.paths.claude_dir = claude.path().display().to_string();
        (tmp, claude, config)
    }

    #[test]
    fn empty_queue_is_a_quiet_noop() {
        let (tmp, _claude, config) = setup();
        let backend = MockBackend::with_responses(vec![]);
        let summary = run_v3(tmp.path(), &config, &backend, false).unwrap();
        let summary = summary.expect("lock acquired");
        assert_eq!(summary.sessions_processed, 0);
        assert_eq!(summary.ai_calls, 0);
    }

    #[test]
    fn drains_queue_analyzes_and_projects() {
        let (tmp, _claude, config) = setup();
        let proj = TempDir::new().unwrap();
        std::process::Command::new("git").arg("-C").arg(proj.path()).arg("init").output().unwrap();
        let transcript = write_fixture_session(tmp.path(), "sess-1", proj.path().to_str().unwrap());
        queue::enqueue(
            tmp.path(),
            &queue::QueueEntry {
                session_id: "sess-1".to_string(),
                transcript_path: transcript.display().to_string(),
                cwd: Some(proj.path().display().to_string()),
                enqueued_at: "2026-07-06T10:00:00Z".to_string(),
            },
        )
        .unwrap();

        let response = r#"{"reasoning":"found one","operations":[
            {"action":"create_node","node_type":"rule","scope":"project","content":"Project rule from analysis.","confidence":0.9}
        ]}"#;
        let backend = MockBackend::with_responses(vec![response.to_string()]);
        let summary = run_v3(tmp.path(), &config, &backend, false).unwrap().unwrap();

        assert_eq!(summary.sessions_processed, 1);
        assert_eq!(summary.ai_calls, 1);
        assert_eq!(summary.nodes_created, 1);
        // queue drained
        assert!(queue::list(tmp.path()).unwrap().is_empty());
        // node landed in the project scope
        let store = Store::open(tmp.path());
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.nodes.len(), 1);
        // CLAUDE.local.md projected into the project
        let local = std::fs::read_to_string(proj.path().join("CLAUDE.local.md")).unwrap();
        assert!(local.contains("Project rule from analysis."));
        // store committed (clean tree)
        assert!(!store_git::has_changes(tmp.path()).unwrap());
        // health recorded
        let h = health::Health::load(tmp.path()).unwrap();
        assert!(h.stages.contains_key("analyze"));
    }

    #[test]
    fn budget_exhaustion_leaves_sessions_queued_with_health_warning() {
        let (tmp, _claude, mut config) = setup();
        config.runner.max_ai_calls_per_day = 0; // no budget at all
        let proj = TempDir::new().unwrap();
        let transcript = write_fixture_session(tmp.path(), "sess-2", proj.path().to_str().unwrap());
        queue::enqueue(
            tmp.path(),
            &queue::QueueEntry {
                session_id: "sess-2".to_string(),
                transcript_path: transcript.display().to_string(),
                cwd: Some(proj.path().display().to_string()),
                enqueued_at: "2026-07-06T10:00:00Z".to_string(),
            },
        )
        .unwrap();
        let backend = MockBackend::with_responses(vec![]); // any call would error
        let summary = run_v3(tmp.path(), &config, &backend, false).unwrap().unwrap();
        assert_eq!(summary.ai_calls, 0);
        assert_eq!(queue::list(tmp.path()).unwrap().len(), 1, "stays queued");
        let h = health::Health::load(tmp.path()).unwrap();
        let w = h.warnings();
        assert!(w.iter().any(|x| x.contains("budget")), "got: {w:?}");
    }

    #[test]
    fn dry_run_makes_no_ai_calls_and_no_writes() {
        let (tmp, _claude, config) = setup();
        let proj = TempDir::new().unwrap();
        let transcript = write_fixture_session(tmp.path(), "sess-3", proj.path().to_str().unwrap());
        queue::enqueue(
            tmp.path(),
            &queue::QueueEntry {
                session_id: "sess-3".to_string(),
                transcript_path: transcript.display().to_string(),
                cwd: Some(proj.path().display().to_string()),
                enqueued_at: "2026-07-06T10:00:00Z".to_string(),
            },
        )
        .unwrap();
        let backend = MockBackend::with_responses(vec![]);
        let summary = run_v3(tmp.path(), &config, &backend, true).unwrap().unwrap();
        assert_eq!(summary.ai_calls, 0);
        assert_eq!(summary.sessions_pending, 1);
        assert_eq!(queue::list(tmp.path()).unwrap().len(), 1, "queue untouched");
        assert!(Store::open(tmp.path()).load_all().unwrap().nodes.is_empty());
    }
}
```

- [ ] **Step 2: Verify compile failure** (wire `pub mod runner_v3;` into lib.rs)

Run: `cargo test -p retro-core runner_v3`
Expected: compile error — `run_v3`/`RunV3Summary` missing.

- [ ] **Step 3: Implement the pipeline**

Add above the tests in `crates/retro-core/src/runner_v3.rs`:

```rust
#[derive(Debug, Default)]
pub struct RunV3Summary {
    pub sessions_processed: usize,
    pub sessions_pending: usize,
    pub sessions_skipped: usize,
    pub ai_calls: u32,
    pub nodes_created: usize,
    pub nodes_updated: usize,
    pub nodes_merged: usize,
    pub nodes_invalidated: usize,
    pub rules_projected_global: usize,
    pub pushed: bool,
}

/// Run the v3 pipeline once. Returns Ok(None) if another run holds the lock
/// (normal when hooks race — not an error). `dry_run` reports what WOULD
/// happen: no AI calls, no writes, no commits.
pub fn run_v3(
    store_root: &Path,
    config: &Config,
    backend: &dyn AnalysisBackend,
    dry_run: bool,
) -> Result<Option<RunV3Summary>, CoreError> {
    let Some(_lock) = LockFile::try_acquire(&store_root.join("run.lock")) else {
        return Ok(None);
    };
    let mut summary = RunV3Summary::default();
    let store = Store::open(store_root);
    store.ensure_layout()?;

    // Stage: commit manual edits (files-as-truth: user edits become history).
    if !dry_run {
        store_git::ensure_repo(store_root)?;
        if store_git::commit_all(store_root, "user: edit knowledge")? {
            health::record(store_root, "manual-edits", true, "committed user edits")?;
        }
    }

    // Stage: exclusion sweep — a project excluded AFTER registration gets its
    // knowledge deleted (recoverable via store git history) and its
    // CLAUDE.local.md removed. Spec §5: exclusion = removal.
    if !dry_run {
        let map = projects::PathMap::load(store_root)?;
        for (slug, path) in map.paths.clone() {
            if projects::is_excluded(&path, &config.privacy.exclude_projects) {
                projects::cleanup_excluded(&store, &slug, Some(&path))?;
                let mut st = RunnerState::load(store_root)?;
                st.notifications.push(format!(
                    "retro stopped watching `{slug}` (excluded) and removed its knowledge"
                ));
                st.save(store_root)?;
                health::record(store_root, "exclude", true, &format!("cleaned up {slug}"))?;
            }
        }
    }

    // Stage: prune stale queue entries (deleted transcripts) — visible, not silent.
    let pruned = queue::prune_stale(store_root)?;
    if !pruned.is_empty() {
        health::record(
            store_root,
            "queue",
            true,
            &format!("pruned {} stale entr(ies): {}", pruned.len(), pruned.join(", ")),
        )?;
    }

    // Stage: load + parse queue into per-project groups.
    let entries = queue::list(store_root)?;
    let mut groups: Vec<(String, String, Vec<(String, Session)>)> = Vec::new(); // (slug, project_path, [(session_id, session)])
    for entry in &entries {
        let path = PathBuf::from(&entry.transcript_path);
        let cwd_hint = entry.cwd.clone().unwrap_or_default();
        let mut session = match parse_session_file(&path, &entry.session_id, &cwd_hint) {
            Ok(s) => s,
            Err(_) => {
                // unparseable transcript: drop from queue, note in health
                queue::remove(store_root, &entry.session_id)?;
                summary.sessions_skipped += 1;
                continue;
            }
        };
        let cwd = session
            .metadata
            .cwd
            .clone()
            .filter(|c| !c.is_empty())
            .unwrap_or(cwd_hint);
        if cwd.is_empty() {
            queue::remove(store_root, &entry.session_id)?;
            summary.sessions_skipped += 1;
            continue;
        }
        if projects::is_excluded(&cwd, &config.privacy.exclude_projects) {
            queue::remove(store_root, &entry.session_id)?;
            summary.sessions_skipped += 1;
            continue;
        }
        if session.user_messages.len() < 2 {
            // low signal: processed (removed), never analyzed
            if !dry_run {
                queue::remove(store_root, &entry.session_id)?;
            }
            summary.sessions_skipped += 1;
            continue;
        }
        if config.privacy.scrub_secrets {
            scrub::scrub_session(&mut session);
        }
        let slug = if dry_run {
            // dry-run must not write project.toml; use a path-derived label
            crate::store::slugify(
                Path::new(&cwd).file_name().and_then(|n| n.to_str()).unwrap_or("project"),
            )
        } else {
            let reg = projects::register(&store, &cwd)?;
            if reg.newly_registered {
                let mut state = RunnerState::load(store_root)?;
                state.notifications.push(format!(
                    "retro is now watching `{}` — exclude via privacy.exclude_projects in ~/.retro/config.toml",
                    reg.slug
                ));
                state.save(store_root)?;
            }
            reg.slug
        };
        match groups.iter_mut().find(|(s, _, _)| s == &slug) {
            Some((_, _, sessions)) => sessions.push((entry.session_id.clone(), session)),
            None => groups.push((slug, cwd, vec![(entry.session_id.clone(), session)])),
        }
    }

    if dry_run {
        summary.sessions_pending = groups.iter().map(|(_, _, s)| s.len()).sum();
        return Ok(Some(summary));
    }

    // Stage: budget-gated analysis, one AI call per project group.
    let today = chrono::Utc::now().date_naive().to_string();
    let mut state = RunnerState::load(store_root)?;
    let mut touched: Vec<(String, String)> = Vec::new(); // (slug, path) that got/changed nodes
    let mut learned: Vec<String> = Vec::new();
    for (slug, project_path, group) in &groups {
        if state.budget_remaining(&today, config.runner.max_ai_calls_per_day) == 0 {
            let waiting: usize = groups.iter().map(|(_, _, s)| s.len()).sum::<usize>()
                - summary.sessions_processed;
            health::record(
                store_root,
                "analyze",
                false,
                &format!("daily AI budget exhausted; {waiting} session(s) remain queued"),
            )?;
            summary.sessions_pending = waiting;
            break;
        }
        let sessions: Vec<Session> = group.iter().map(|(_, s)| s.clone()).collect();
        let result = match analysis_v3::analyze_sessions(&store, backend, &sessions, Some(slug)) {
            Ok(r) => r,
            Err(e) => {
                health::record(store_root, "analyze", false, &e.to_string())?;
                // leave this group queued for a future run; keep going with others
                continue;
            }
        };
        state.record_ai_calls(&today, 1);
        state.save(store_root)?;
        summary.ai_calls += 1;
        summary.sessions_processed += result.sessions_analyzed;
        summary.nodes_created += result.nodes_created;
        summary.nodes_updated += result.nodes_updated;
        summary.nodes_merged += result.nodes_merged;
        summary.nodes_invalidated += result.nodes_invalidated;
        learned.extend(result.learned.iter().map(|b| {
            let first_line = b.lines().next().unwrap_or(b);
            format!("Learned: {}", crate::util::truncate_str(first_line, 100))
        }));
        for (session_id, _) in group {
            queue::remove(store_root, session_id)?;
        }
        touched.push((slug.clone(), project_path.clone()));
        health::record(
            store_root,
            "analyze",
            true,
            &format!("{}: +{} nodes", slug, result.nodes_created),
        )?;
    }

    // Stage: projection (global always — cheap and idempotent; locals for touched projects).
    let threshold = config.knowledge.confidence_threshold;
    let global_md = config.claude_dir().join("CLAUDE.md");
    let backups = store_root.join("backups");
    match local_md::project_global_md(&store, &global_md, threshold, Some(&backups)) {
        Ok(n) => {
            summary.rules_projected_global = n;
            health::record(store_root, "project", true, &format!("global: {n} rule(s)"))?;
        }
        Err(e) => health::record(store_root, "project", false, &e.to_string())?,
    }
    for (slug, project_path) in &touched {
        if let Err(e) = local_md::project_local_md(&store, slug, Path::new(project_path), threshold) {
            health::record(store_root, "project", false, &format!("{slug}: {e}"))?;
        }
    }

    // Stage: notifications for the next briefing.
    if !learned.is_empty() {
        let mut st = RunnerState::load(store_root)?;
        st.notifications.extend(learned);
        st.save(store_root)?;
    }

    // Stage: index, commit, push.
    index::build(&store)?;
    let committed = store_git::commit_all(
        store_root,
        &format!(
            "retro: learn {} node(s), update {}",
            summary.nodes_created,
            summary.nodes_updated + summary.nodes_merged
        ),
    )?;
    if committed {
        match store_git::push_best_effort(store_root) {
            store_git::PushOutcome::Pushed => {
                summary.pushed = true;
                health::record(store_root, "push", true, "pushed")?;
            }
            store_git::PushOutcome::NoRemote => {
                health::record(store_root, "push", true, "no remote configured")?;
            }
            store_git::PushOutcome::Failed(err) => {
                health::record(store_root, "push", false, &err)?;
            }
        }
    }
    health::record(store_root, "run", true, &format!("{} session(s)", summary.sessions_processed))?;
    Ok(Some(summary))
}
```

- [ ] **Step 4: Adapt the JSONL fixture until tests pass**

Run: `cargo test -p retro-core runner_v3`
The fixture in `write_fixture_session` MUST be adapted to the real parser: read `crates/retro-core/src/ingest/session.rs` (its tests and/or `tests/fixtures/`) and produce two parseable user messages with a `cwd`. Iterate until the 4 tests pass. If `parse_session_file` requires fields the simple format lacks (e.g. wrapped `message.content` arrays), copy the exact shape from the existing fixtures. Report what shape was needed.

- [ ] **Step 5: CLI dispatch + --background flag**

In `crates/retro-cli/src/main.rs`, add to the `Run` variant's fields (it already has `verbose` and `dry_run` — check exact names at main.rs around the Run variant):

```rust
        /// (v3) Quiet background mode: exit silently if another run holds the lock
        #[arg(long)]
        background: bool,
```

and thread it through the match arm to `commands::run::run(...)` following the existing arm's style.

In `crates/retro-cli/src/commands/run.rs`, change `pub fn run(verbose: bool, dry_run: bool)` to accept `background: bool`, and add AT THE TOP:

```rust
    let dir = retro_core::config::retro_dir();
    let config = retro_core::config::Config::load(&dir.join("config.toml"))?;
    if config.v3.enabled {
        let backend = retro_core::analysis::claude_cli::ClaudeCliBackend::new(config.ai.model.clone());
        let summary = retro_core::runner_v3::run_v3(&dir, &config, &backend, dry_run)?;
        match summary {
            None => {
                if !background {
                    println!("Another retro run is in progress — skipped.");
                }
            }
            Some(s) => {
                if !background {
                    println!(
                        "v3 run: {} session(s) analyzed ({} AI call(s)) — +{} nodes, {} updated, {} merged, {} invalidated; {} global rule(s) projected{}",
                        s.sessions_processed, s.ai_calls, s.nodes_created, s.nodes_updated,
                        s.nodes_merged, s.nodes_invalidated, s.rules_projected_global,
                        if s.sessions_pending > 0 { format!("; {} pending (budget)", s.sessions_pending) } else { String::new() }
                    );
                }
            }
        }
        return Ok(());
    }
```

Adaptation notes: (a) check `ClaudeCliBackend`'s actual constructor (claude_cli.rs) — it may be `ClaudeCliBackend::new()` with the model from config passed differently; read the v2 call site in run.rs/analysis and copy it. (b) `--dry-run` in v3 mode prints the pending summary — add a `println!` for the dry-run fields (sessions_pending / sessions_skipped). (c) The legacy v2 body below the inserted block stays byte-identical.

- [ ] **Step 6: Full test run + behavior check + commit**

```bash
cargo test
cargo build && RETRO_HOME=$(mktemp -d) ./target/debug/retro run --dry-run   # v3 off → legacy v2 path output
git add crates/retro-core/src/runner_v3.rs crates/retro-core/src/lib.rs crates/retro-cli/src/commands/run.rs crates/retro-cli/src/main.rs && git commit -m "feat(v3): pipeline — drain, budget-gated analysis, projection, commit, push"
```

---

### Task 11: `retro init --v3` — store init, global hooks, backup remote

**Files:**
- Create: `crates/retro-core/src/claude_settings.rs`
- Modify: `crates/retro-core/src/lib.rs` (add `pub mod claude_settings;`)
- Modify: `crates/retro-cli/src/commands/init.rs`
- Modify: `crates/retro-cli/src/main.rs` (Init gains `--v3` and `--from <remote>`)

- [ ] **Step 1: Write failing tests for the settings merge**

Create `crates/retro-core/src/claude_settings.rs`:

```rust
//! Non-destructive editing of Claude Code settings JSON (global
//! ~/.claude/settings.json): ensure retro's hook entries exist without
//! touching anything else in the file.

use serde_json::{Value, json};

use crate::errors::CoreError;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_hook_to_empty_settings() {
        let out = ensure_hook(json!({}), "SessionEnd", "/usr/local/bin/retro observe").unwrap();
        let arr = out["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["hooks"][0]["command"], "/usr/local/bin/retro observe");
    }

    #[test]
    fn preserves_existing_unrelated_hooks_and_settings() {
        let existing = json!({
            "model": "opus",
            "hooks": {
                "SessionEnd": [
                    {"matcher": "", "hooks": [{"type": "command", "command": "other-tool cleanup"}]}
                ],
                "PostToolUse": [
                    {"matcher": "Bash", "hooks": [{"type": "command", "command": "audit.sh"}]}
                ]
            }
        });
        let out = ensure_hook(existing, "SessionEnd", "/bin/retro observe").unwrap();
        assert_eq!(out["model"], "opus");
        let se = out["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(se.len(), 2, "appended, not replaced");
        assert!(out["hooks"]["PostToolUse"].is_array());
    }

    #[test]
    fn is_idempotent_by_command_substring() {
        let once = ensure_hook(json!({}), "SessionStart", "/bin/retro brief").unwrap();
        let twice = ensure_hook(once.clone(), "SessionStart", "/bin/retro brief").unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn recognizes_retro_hook_even_if_binary_path_changed() {
        let old = ensure_hook(json!({}), "SessionEnd", "/old/path/retro observe").unwrap();
        let new = ensure_hook(old, "SessionEnd", "/new/path/retro observe").unwrap();
        let arr = new["hooks"]["SessionEnd"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "updated in place, not duplicated");
        assert_eq!(arr[0]["hooks"][0]["command"], "/new/path/retro observe");
    }
}
```

- [ ] **Step 2: Verify failure (wire lib.rs), implement**

```rust
/// Ensure a `{event}` hook running `command` exists. Identity: an existing
/// entry whose command ends with the same `retro <subcommand>` suffix is
/// retro's and gets updated in place (binary paths change across installs);
/// anything else is preserved untouched.
pub fn ensure_hook(mut settings: Value, event: &str, command: &str) -> Result<Value, CoreError> {
    let suffix = command
        .rsplit_once('/')
        .map(|(_, tail)| format!("/{tail}"))
        .unwrap_or_else(|| command.to_string());
    let retro_marker = suffix
        .rsplit_once('/')
        .map(|(_, t)| t.to_string())
        .unwrap_or(suffix.clone()); // e.g. "retro observe"

    if !settings.is_object() {
        return Err(CoreError::Parse("settings.json is not a JSON object".to_string()));
    }
    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        return Err(CoreError::Parse("settings.json 'hooks' is not an object".to_string()));
    }
    let event_arr = hooks
        .as_object_mut()
        .unwrap()
        .entry(event)
        .or_insert_with(|| json!([]));
    let Some(arr) = event_arr.as_array_mut() else {
        return Err(CoreError::Parse(format!("hooks.{event} is not an array")));
    };

    // Update an existing retro entry in place.
    for group in arr.iter_mut() {
        if let Some(inner) = group.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            for hook in inner.iter_mut() {
                let is_retro = hook
                    .get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| c.ends_with(&retro_marker))
                    .unwrap_or(false);
                if is_retro {
                    hook["command"] = json!(command);
                    return Ok(settings);
                }
            }
        }
    }
    arr.push(json!({
        "matcher": "",
        "hooks": [{"type": "command", "command": command}]
    }));
    Ok(settings)
}
```

Run: `cargo test -p retro-core claude_settings` — expected 4 PASS.

- [ ] **Step 3: Extend `retro init`**

In `crates/retro-cli/src/main.rs`, the `Init` variant gains:

```rust
        /// Initialize the v3 personal store (git-backed ~/.retro, global hooks)
        #[arg(long)]
        v3: bool,
        /// Clone an existing v3 knowledge repo instead of starting fresh (implies --v3)
        #[arg(long, value_name = "REMOTE")]
        from: Option<String>,
```

Thread both to `commands::init::run(...)` following the existing arm style.

In `crates/retro-cli/src/commands/init.rs`, extend the signature and add at the TOP of `run` (before any v2 logic):

```rust
    if v3 || from.is_some() {
        return init_v3(from.as_deref());
    }
```

and implement (same file):

```rust
/// v3 initialization. Ordering matters: layout (writes .gitignore) MUST come
/// before repo init (whose first commit stages everything) — otherwise
/// derived files get permanently committed into the knowledge repo.
fn init_v3(from: Option<&str>) -> Result<()> {
    use retro_core::store::{Store, git as store_git, index};

    let dir = retro_core::config::retro_dir();

    if let Some(remote) = from {
        // Clone path: target must not already be a store.
        if dir.join(".git").exists() || dir.join("knowledge").exists() {
            anyhow::bail!(
                "{} already contains a store — remove it or run plain `retro init --v3`",
                dir.display()
            );
        }
        std::fs::create_dir_all(dir.parent().unwrap_or(&dir))?;
        let status = std::process::Command::new("git")
            .args(["clone", remote, &dir.display().to_string()])
            .status()?;
        anyhow::ensure!(status.success(), "git clone failed");
        // Clone bypasses ensure_repo's create branch: apply local config explicitly.
        store_git::apply_local_config(&dir)?;
        println!("Cloned knowledge store from {remote}");
    }

    let store = Store::open(&dir);
    store.ensure_layout()?; // BEFORE ensure_repo — see doc comment
    let created = store_git::ensure_repo(&dir)?;
    if created {
        println!("Initialized knowledge store repo at {}", dir.display());
    }
    let stats = index::build(&store)?;
    println!("Indexed {} node(s)", stats.nodes);

    // Global hooks in ~/.claude/settings.json (absolute binary path — hooks
    // run outside any shell profile, PATH is not guaranteed).
    let config_path = dir.join("config.toml");
    let mut config = retro_core::config::Config::load(&config_path)?;
    let exe = std::env::current_exe()?.display().to_string();
    let settings_path = config.claude_dir().join("settings.json");
    let existing: serde_json::Value = match std::fs::read_to_string(&settings_path) {
        Ok(content) => serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("cannot parse {}: {e}", settings_path.display()))?,
        Err(_) => serde_json::json!({}),
    };
    let with_end =
        retro_core::claude_settings::ensure_hook(existing, "SessionEnd", &format!("{exe} observe"))?;
    let with_both =
        retro_core::claude_settings::ensure_hook(with_end, "SessionStart", &format!("{exe} brief"))?;
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&settings_path, serde_json::to_string_pretty(&with_both)?)?;
    println!("Installed v3 hooks in {}", settings_path.display());

    // Enable the gate.
    config.v3.enabled = true;
    config.save(&config_path)?;
    println!("v3 pipeline enabled");

    // Backup remote (skip when cloning — a remote already exists).
    if from.is_none() && !store_git::has_remote(&dir) {
        print!("Back up your knowledge to a private GitHub repo? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim().eq_ignore_ascii_case("y") {
            let status = std::process::Command::new("gh")
                .args(["repo", "create", "retro-knowledge", "--private"])
                .status()?;
            if status.success() {
                let user_out = std::process::Command::new("gh")
                    .args(["api", "user", "-q", ".login"])
                    .output()?;
                let user = String::from_utf8_lossy(&user_out.stdout).trim().to_string();
                anyhow::ensure!(!user.is_empty(), "could not determine GitHub username");
                let url = format!("git@github.com:{user}/retro-knowledge.git");
                let st = std::process::Command::new("git")
                    .args(["-C", &dir.display().to_string(), "remote", "add", "origin", &url])
                    .status()?;
                anyhow::ensure!(st.success(), "git remote add failed");
                match store_git::push_best_effort(&dir) {
                    retro_core::store::git::PushOutcome::Pushed => {
                        println!("Backed up to {url}")
                    }
                    outcome => println!("Remote added ({url}); first push pending: {outcome:?}"),
                }
            } else {
                println!("gh repo create failed — you can add a remote later with git -C {} remote add origin <url>", dir.display());
            }
        }
    }
    println!("\nv3 ready. Sessions are captured automatically from now on.");
    Ok(())
}
```

This needs one small addition to `crates/retro-core/src/store/git.rs` (allowed):

```rust
pub fn has_remote(root: &Path) -> bool {
    git(root, &["remote"])
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}
```

with a one-line test in its tests module:

```rust
    #[test]
    fn has_remote_false_on_fresh_repo() {
        let tmp = TempDir::new().unwrap();
        ensure_repo(tmp.path()).unwrap();
        assert!(!has_remote(tmp.path()));
    }
```

- [ ] **Step 4: Behavior check (isolated), full tests, commit**

```bash
cargo build
export RH=$(mktemp -d) && export CD=$(mktemp -d)
printf '[paths]\nclaude_dir = "%s"\n' "$CD" > "$RH/config.toml"
printf 'n\n' | RETRO_HOME="$RH" ./target/debug/retro init --v3
cat "$CD/settings.json" && cat "$RH/config.toml" | grep -A1 "\[v3\]" && ls "$RH/.git" >/dev/null && echo "repo ok"
```

Expected: settings.json contains SessionEnd/SessionStart entries pointing at the debug binary with `observe`/`brief`; config has `[v3] enabled = true`; `.git` exists; declining the backup prompt is honored. Then:

```bash
cargo test && git add crates/retro-core/src/claude_settings.rs crates/retro-core/src/lib.rs crates/retro-core/src/store/git.rs crates/retro-cli/src/commands/init.rs crates/retro-cli/src/main.rs && git commit -m "feat(v3): retro init --v3/--from — store init, global hooks, backup remote"
```

---

### Task 12: Documentation and plan completion

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update CLAUDE.md**

In the `### v3 "Personal" (in progress)` subsection, append after the Plan 1 bullet:

```markdown
- **Plan 2: DONE** — Pipeline. Hook-based capture (`retro observe` on SessionEnd,
  `retro brief` on SessionStart with catch-up scan), automatic project registration
  (remote-url identity, notify-on-register, exclusion with cleanup), analysis via the
  v2 engine with a markdown-store sink (`analysis/v3.rs`), one-way projection to
  `~/.claude/CLAUDE.md` + per-project `CLAUDE.local.md` (ignored via `.git/info/exclude`),
  budget-gated `runner_v3` pipeline behind `[v3] enabled`, `retro init --v3 [--from <remote>]`
  with global hooks and optional private backup remote. All v3 state: `queue/`, `state/`,
  `health.json` (machine-local, gitignored).
```

And add the new commands to the Core Commands table:

```markdown
| `retro observe` | (v3) SessionEnd hook entry: enqueue session, spawn worker |
| `retro brief` | (v3) SessionStart hook entry: catch-up scan + session briefing |
| `retro init --v3 [--from <remote>]` | (v3) Initialize personal store, install global hooks |
```

- [ ] **Step 2: Final verification**

```bash
cargo test 2>&1 | grep -E "^test result" && cargo run -- --help | grep -E "observe|brief"
```

Expected: all green; both commands listed.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md && git commit -m "docs: v3 plan 2 pipeline status"
```

---

## Rollout (manual, after merge — per the user's smoke-test-first rule)

1. `retro init --v3` on the maintainer's machine (accept the backup-remote prompt).
2. Work one real Claude Code session; end it; verify: queue drained, node files under `~/.retro/knowledge/`, `retro: learn` commit in `git -C ~/.retro log`, `CLAUDE.local.md` in the project, briefing on next session start.
3. Only then let the catch-up scan enqueue the historical backlog, and drain it over days under the daily AI budget (do NOT raise the budget for a one-shot drain).

## Out of scope for Plan 2 (Plan 3)

- `retro ui` dashboard, `retro doctor`, `retro status` v3 view — health.json is being written now; surfaces come next
- `retro lint` (store-wide dedup/contradiction/staleness pass)
- `retro migrate` (v2 SQLite → v3 store, launchd removal, `[v3]` default flip), `retro uninstall` v3, deletion of v1/v2 code paths
- Scenario tests for v3 flows (rewritten wholesale in Plan 3)
