# Retro v3 Plan 3: Surfaces Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give retro v3 its visibility and control surfaces: the `retro ui` local web dashboard (context X-ray, knowledge browser, health, history), `retro doctor`, a v3-aware `retro status`, and `retro lint` — plus the dogfood fixes and review carry-overs from Plan 2.

**Architecture:** A sync `tiny_http` server (localhost-only, single embedded HTML file, JSON API) reading the store/index/health/state modules that already exist; write actions go through the store (file edit → commit → reindex → reproject). Doctor/status/lint are thin CLI commands over the same modules. Everything stays behind `[v3] enabled` except `retro ui`/`doctor`, which print a pointer to `retro init --v3` when disabled.

**Tech Stack:** Rust (edition 2024, sync only — no tokio). ONE new dependency: `tiny_http = "0.12"` (retro-cli only; small, long-established, no transitive async). Everything else exists.

**Spec:** `docs/superpowers/specs/2026-07-06-retro-v3-personal-redesign-design.md` (§8 Visibility, §9 Reliability, §6 lint; §5 queue-age nudge gap from the Plan 2 final review)

## Context for implementers (read first)

- **Conventions:** `CoreError` in retro-core, `anyhow` + bare `?` in retro-cli. NEVER `cargo fmt` — per-file `rustfmt --edition 2024` on NEW files only; manual style in shared files. Mandatory preflight every dispatch: `cd <repo>/.claude/worktrees/v3-surfaces && git branch --show-current` must print `v3-surfaces`.
- **SAFETY (absolute):** never run `retro init` (any form), `retro start`, or `retro stop` — they write launchd plists/hooks OUTSIDE the RETRO_HOME sandbox. Live checks only via observe/brief/run/reindex/status/doctor/ui with isolated `RETRO_HOME` + `[paths] claude_dir` override. NEVER run `retro run` with a NON-EMPTY queue against a config whose backend could reach the real `claude` CLI (it would spend real tokens) — use empty queues or `--dry-run` for CLI checks; MockBackend in tests.
- **Existing API you build on (all verified in the merged code):**
  - `store::{Store, Node, NodeType, Scope}`; `Store::{open, ensure_layout, load_all, get, write_node, invalidate, unique_slug}`; `store.load_all() -> LoadResult{nodes: Vec<(PathBuf, Node)>, warnings}`
  - `store::index::{build, open, query, NodeFilter{scope, node_type, active_only, text}, NodeRow, is_fresh, index_path}` — `open()` errors `NotInitialized` if never built
  - `store::git::{ensure_repo, apply_local_config, commit_all, push_best_effort, PushOutcome, has_remote, has_changes}`
  - `store::queue::{list, QueueEntry}`; `store::state::RunnerState{last_observed_unix, ai_calls_date, ai_calls_today, notifications, processed, budget_remaining, record_ai_calls}`; `store::projects::{PathMap, ProjectMeta, is_excluded, cleanup_excluded}`
  - `health::{Health, StageHealth, record}`; `briefing::build_v3_briefing`
  - `analysis::v3::{analyze_sessions, V3AnalyzeResult}`; `analysis::backend::{AnalysisBackend, MockBackend, BackendResponse}`; `analysis::claude_cli::ClaudeCliBackend::new(&config.ai)`
  - `projection::local_md::{projectable_rules, project_global_md, project_local_md}`
  - `analysis::merge` has the v2 Levenshtein helpers (similarity threshold 0.8) — reuse the algorithm (check its exact private/pub surface before coding; lift the normalized-Levenshtein fn into a shared location if it's private, WITHOUT changing v2 behavior)
  - CLI: `check_and_display_nudge()` in `commands/mod.rs` (v3 block at ~line 277); hook-entry nudge exemption via `is_hook_entry` in main.rs; `status.rs` bails early when `retro.db` missing (v3 block must come BEFORE that bail)
- **Dogfood findings already verified:** the drain loop's low-signal check ALREADY precedes registration (runner_v3.rs:201 vs :221) — do NOT "fix" that. The real fixes are Task 1's.
- Baseline: 383 tests passing workspace-wide (350 retro-core). Run `cargo test` before each commit.

## File Structure

```
crates/retro-core/src/runner_v3.rs         # MODIFY: T1 fixes (self-exclusion, commit order, index-continue, dry-run store-warnings gate)
crates/retro-cli/src/commands/brief.rs     # MODIFY: skip /subagents/ transcripts in catch-up
crates/retro-core/src/store/projects.rs    # MODIFY: is_excluded gains built-in store-dir guard (via new helper)
crates/retro-core/src/doctor.rs            # NEW: end-to-end checks, consumed by CLI + dashboard
crates/retro-core/src/lint.rs              # NEW: dedup/contradiction/staleness pass (Levenshtein net + optional AI)
crates/retro-cli/src/commands/doctor.rs    # NEW: retro doctor
crates/retro-cli/src/commands/lint.rs      # NEW: retro lint [--dry-run]
crates/retro-cli/src/commands/status.rs    # MODIFY: v3 summary block before the v2 bail
crates/retro-cli/src/commands/mod.rs       # MODIFY: queue-age in nudge; new pub mods
crates/retro-cli/src/ui/mod.rs             # NEW: tiny_http server + routes
crates/retro-cli/src/ui/api.rs             # NEW: JSON handlers (xray, nodes, node detail, actions, health, history, doctor)
crates/retro-cli/src/ui/assets/index.html  # NEW: single-file frontend (embedded via include_str!)
crates/retro-cli/src/commands/ui.rs        # NEW: retro ui command
crates/retro-cli/src/main.rs               # MODIFY: Doctor/Lint/Ui variants + arms
crates/retro-cli/Cargo.toml                # MODIFY: + tiny_http = "0.12"
crates/retro-core/src/config.rs            # MODIFY: [ui] port (default 7777)
CLAUDE.md                                  # docs
```

---

### Task 1: Dogfood fixes + Plan 2 carry-overs in the pipeline

**Files:**
- Modify: `crates/retro-core/src/store/projects.rs`
- Modify: `crates/retro-cli/src/commands/brief.rs`
- Modify: `crates/retro-core/src/runner_v3.rs`

Four fixes, each TDD where testable:

- [ ] **Step 1: Built-in store self-exclusion.** Retro must never watch its own store (dogfood: a historical session run in `~/.retro` registered it as project `retro-2`). In `projects.rs`, add:

```rust
/// True when `path` is the store itself (or inside it) — retro never watches
/// its own home, independent of user config (self-observation guard).
pub fn is_store_dir(store_root: &Path, path: &str) -> bool {
    let canon = |p: &Path| -> String {
        std::fs::canonicalize(p)
            .ok()
            .and_then(|c| c.to_str().map(str::to_string))
            .unwrap_or_else(|| p.display().to_string())
    };
    let root = canon(store_root);
    let candidate = canon(Path::new(path));
    candidate == root || candidate.starts_with(&format!("{}/", root.trim_end_matches('/')))
}
```

Call sites (both): `runner_v3.rs` drain loop — extend the existing exclusion arm to `if projects::is_excluded(&cwd, &config.privacy.exclude_projects) || projects::is_store_dir(store_root, &cwd)`; and `observe.rs`? — NO: observe receives store_root implicitly via retro_dir(); check the current observe code — its exclusion check is in `observe_event` with access to `dir`; add the same guard there (`crates/retro-cli/src/commands/observe.rs` — this ADDS a file to this task's list).

Test (projects.rs):
```rust
    #[test]
    fn store_dir_and_children_are_self_excluded() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let root_str = root.to_str().unwrap();
        assert!(is_store_dir(root, root_str));
        let child = root.join("knowledge");
        std::fs::create_dir_all(&child).unwrap();
        assert!(is_store_dir(root, child.to_str().unwrap()));
        let other = TempDir::new().unwrap();
        assert!(!is_store_dir(root, other.path().to_str().unwrap()));
    }
```

- [ ] **Step 2: Skip subagent transcripts in the catch-up scan.** In `brief.rs`, inside the loop over `modified`, before deriving the stem:

```rust
        // Subagent transcripts (<session>/subagents/agent-*.jsonl) are parts
        // of their parent session, not sessions — never enqueue them.
        if m.path.components().any(|c| c.as_os_str() == "subagents") {
            continue;
        }
```

(No unit test — brief.rs has no test harness; the runner-level behavior is covered by the low-signal filter and this is a scan-level optimization. Note it in the commit message.)

- [ ] **Step 3: Carry-over Minor-7 (commit after analysis; index build continues on failure).** In `runner_v3.rs`: move the knowledge commit (`retro: learn N node(s), update M`) to immediately AFTER the analysis loop (before projection); change `index::build(&store)?` to record-and-continue:

```rust
    if let Err(e) = index::build(&store) {
        health::record(store_root, "index", false, &e.to_string())?;
    }
```

The end-of-pipeline `commit_all` stays as the straggler sweep with message `"retro: maintenance"` when analysis counts are zero (check the current message logic and adapt: if `summary.nodes_created + summary.nodes_updated + summary.nodes_merged == 0`, use "retro: maintenance"). Keep `committed_any` OR-ing intact.

Test: extend `drains_queue_analyzes_and_projects` — after the run, assert `git -C <store> log` contains a commit whose subject starts with `retro: learn` (it already implicitly does; make it explicit with an assertion on `git log --format=%s`).

- [ ] **Step 4: Dry-run gating of the store-warnings health write.** The store-warnings stage currently writes health.json even in dry-run. Gate the `health::record` call (NOT the load_all itself — dry-run may still print warnings count later if desired, but must not write): wrap in `if !dry_run { ... }`.

Test: extend `dry_run_makes_no_ai_calls_and_no_writes` — seed a corrupt knowledge file (`std::fs::write(tmp.path().join("knowledge/global/bad.md"), "junk")`) before the dry run and assert `!tmp.path().join("health.json").exists()` after.

- [ ] **Step 5: Run and commit**

```bash
cargo test -p retro-core && cargo test && git add crates/retro-core/src/store/projects.rs crates/retro-core/src/runner_v3.rs crates/retro-cli/src/commands/brief.rs crates/retro-cli/src/commands/observe.rs && git commit -m "fix(v3): dogfood round — store self-exclusion, subagent skip, commit order, dry-run purity"
```

Expected: 350 retro-core + 1 new = 351; workspace 384.

---

### Task 2: `[ui]` config section + queue-age in the nudge

**Files:**
- Modify: `crates/retro-core/src/config.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs`

- [ ] **Step 1: Config.** Following the exact existing section pattern (`default_v3` precedent): add

```rust
/// v3 dashboard server settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default = "default_ui_port")]
    pub port: u16,
}

fn default_ui_port() -> u16 {
    7777
}
```

with `#[serde(default = "default_ui")] pub ui: UiConfig` on Config, `default_ui()` returning `UiConfig { port: default_ui_port() }`, and the field in the manual `Default` impl. Test:

```rust
    #[test]
    fn ui_section_defaults_and_roundtrips() {
        let config = Config::default();
        assert_eq!(config.ui.port, 7777);
        let parsed: Config = toml::from_str("").unwrap();
        assert_eq!(parsed.ui.port, 7777);
        let parsed: Config = toml::from_str("[ui]\nport = 9000\n").unwrap();
        assert_eq!(parsed.ui.port, 9000);
    }
```

- [ ] **Step 2: Queue-age nudge (spec §5 gap).** In `check_and_display_nudge()`'s v3 block (commands/mod.rs ~277), after the health warnings loop, add a queue staleness check:

```rust
            if let Ok(entries) = retro_core::store::queue::list(&dir) {
                if !entries.is_empty() {
                    // enqueued_at is RFC3339; oldest entry first (list is sorted)
                    let oldest = &entries[0].enqueued_at;
                    let stale = chrono::DateTime::parse_from_rfc3339(oldest)
                        .map(|t| {
                            chrono::Utc::now().signed_duration_since(t) > chrono::Duration::hours(24)
                        })
                        .unwrap_or(false);
                    if stale {
                        eprintln!(
                            "{} {} session(s) queued for over a day — run `retro run` or `retro doctor`",
                            "retro:".yellow(),
                            entries.len()
                        );
                    }
                }
            }
```

(Match the block's existing eprintln/colored style — read it first. chrono is a retro-cli dep — verify; if not, do the parse in retro-core behind a helper `queue::oldest_age_hours(&Path) -> Option<i64>` instead and keep the CLI side dep-free. Choose whichever compiles without adding dependencies, and report which.)

- [ ] **Step 3: Run and commit**

```bash
cargo test && git add crates/retro-core/src/config.rs crates/retro-cli/src/commands/mod.rs && git commit -m "feat(v3): [ui] config; queue-age nudge"
```

Expected workspace: 385.

---

### Task 3: `doctor` core module

**Files:**
- Create: `crates/retro-core/src/doctor.rs`
- Modify: `crates/retro-core/src/lib.rs` (add `pub mod doctor;` after `pub mod db;` — alphabetical)

A pure data producer consumed by both the CLI command (Task 4) and the dashboard API (Task 8). NO writes, NO AI calls; the `claude` CLI probe is `claude --version` (subprocess, no tokens).

- [ ] **Step 1: Write failing tests.** Create `crates/retro-core/src/doctor.rs`:

```rust
//! End-to-end health verification for the v3 pipeline. Read-only:
//! every check inspects state; none mutate. Consumed by `retro doctor`
//! and the dashboard.

use std::path::Path;

use serde::Serialize;

use crate::config::Config;
use crate::errors::CoreError;

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn all_ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Store, git as store_git, index};
    use tempfile::TempDir;

    fn config_for(claude_dir: &Path) -> Config {
        let mut config = Config::default();
        config.v3.enabled = true;
        config.paths.claude_dir = claude_dir.display().to_string();
        config
    }

    #[test]
    fn healthy_store_passes_structural_checks() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store_git::ensure_repo(tmp.path()).unwrap();
        index::build(&store).unwrap();
        // hooks present in settings.json
        std::fs::write(
            claude.path().join("settings.json"),
            r#"{"hooks":{"SessionEnd":[{"matcher":"","hooks":[{"type":"command","command":"/bin/retro observe"}]}],"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"/bin/retro brief"}]}]}}"#,
        )
        .unwrap();

        let report = run_checks_for_tests(tmp.path(), &config_for(claude.path()));
        let by_name = |n: &str| report.checks.iter().find(|c| c.name == n).unwrap();
        assert!(by_name("v3-enabled").ok);
        assert!(by_name("store-repo").ok);
        assert!(by_name("index").ok);
        assert!(by_name("hooks").ok);
        assert!(by_name("queue").ok);
    }

    #[test]
    fn unhealthy_conditions_are_reported() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        // no repo, no index, no hooks, stale index after node write
        let report = run_checks_for_tests(tmp.path(), &config_for(claude.path()));
        let by_name = |n: &str| report.checks.iter().find(|c| c.name == n).unwrap();
        assert!(!by_name("store-repo").ok);
        assert!(!by_name("index").ok);
        assert!(!by_name("hooks").ok);
        assert!(!report.all_ok());
    }

    #[test]
    fn disabled_v3_short_circuits() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let mut config = config_for(claude.path());
        config.v3.enabled = false;
        let report = run_checks_for_tests(tmp.path(), &config);
        assert_eq!(report.checks.len(), 1);
        assert!(!report.checks[0].ok);
        assert!(report.checks[0].detail.contains("init --v3"));
    }
}
```

- [ ] **Step 2: Verify compile failure** (wire `pub mod doctor;` in lib.rs), then implement:

```rust
/// Run all checks. `probe_claude` additionally spawns `claude --version`
/// (subprocess, no tokens) — optional because it's slow and env-dependent.
/// `probe_env` additionally checks machine-level state (the v2 launchd plist
/// under $HOME) — off in tests, on in the CLI.
pub fn run_checks(store_root: &Path, config: &Config, probe_claude: bool) -> DoctorReport {
    run_checks_inner(store_root, config, probe_claude, true)
}

pub fn run_checks_for_tests(store_root: &Path, config: &Config) -> DoctorReport {
    run_checks_inner(store_root, config, false, false)
}

fn run_checks_inner(store_root: &Path, config: &Config, probe_claude: bool, probe_env: bool) -> DoctorReport {
    let mut checks = Vec::new();

    if !config.v3.enabled {
        checks.push(Check {
            name: "v3-enabled".to_string(),
            ok: false,
            detail: "v3 is disabled — run `retro init --v3`".to_string(),
        });
        return DoctorReport { checks };
    }
    checks.push(Check {
        name: "v3-enabled".to_string(),
        ok: true,
        detail: "enabled".to_string(),
    });

    // Store repo
    let repo_ok = crate::store::git::is_repo(store_root);
    checks.push(Check {
        name: "store-repo".to_string(),
        ok: repo_ok,
        detail: if repo_ok {
            format!("git repo at {}", store_root.display())
        } else {
            "store is not a git repo — run `retro init --v3`".to_string()
        },
    });

    // Index built + fresh
    let store = crate::store::Store::open(store_root);
    let index_check = match crate::store::index::open(store_root) {
        Ok(conn) => match crate::store::index::is_fresh(&store, &conn) {
            Ok(true) => (true, "built and fresh".to_string()),
            Ok(false) => (false, "stale — run `retro reindex`".to_string()),
            Err(e) => (false, format!("freshness check failed: {e}")),
        },
        Err(e) => (false, format!("{e} — run `retro reindex`")),
    };
    checks.push(Check {
        name: "index".to_string(),
        ok: index_check.0,
        detail: index_check.1,
    });

    // Hooks installed (global settings.json contains retro observe + brief)
    let settings_path = config.claude_dir().join("settings.json");
    let hooks_ok = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .map(|v| {
            let has = |event: &str, sub: &str| {
                v["hooks"][event]
                    .as_array()
                    .map(|groups| {
                        groups.iter().any(|g| {
                            g["hooks"].as_array().is_some_and(|hs| {
                                hs.iter().any(|h| {
                                    h["command"]
                                        .as_str()
                                        .is_some_and(|c| c.contains(&format!("retro {sub}")))
                                })
                            })
                        })
                    })
                    .unwrap_or(false)
            };
            has("SessionEnd", "observe") && has("SessionStart", "brief")
        })
        .unwrap_or(false);
    checks.push(Check {
        name: "hooks".to_string(),
        ok: hooks_ok,
        detail: if hooks_ok {
            "SessionEnd + SessionStart installed".to_string()
        } else {
            format!("missing in {} — run `retro init --v3`", settings_path.display())
        },
    });

    // Queue age
    let queue_check = match crate::store::queue::list(store_root) {
        Ok(entries) if entries.is_empty() => (true, "empty".to_string()),
        Ok(entries) => {
            let oldest_stale = chrono::DateTime::parse_from_rfc3339(&entries[0].enqueued_at)
                .map(|t| chrono::Utc::now().signed_duration_since(t) > chrono::Duration::hours(24))
                .unwrap_or(false);
            if oldest_stale {
                (false, format!("{} entr(ies), oldest > 24h — pipeline not draining?", entries.len()))
            } else {
                (true, format!("{} entr(ies), draining", entries.len()))
            }
        }
        Err(e) => (false, format!("unreadable: {e}")),
    };
    checks.push(Check {
        name: "queue".to_string(),
        ok: queue_check.0,
        detail: queue_check.1,
    });

    // Recent stage failures (from health.json)
    if let Ok(health) = crate::health::Health::load(store_root) {
        let warnings = health.warnings();
        checks.push(Check {
            name: "stages".to_string(),
            ok: warnings.is_empty(),
            detail: if warnings.is_empty() {
                "all stages healthy".to_string()
            } else {
                warnings.join("; ")
            },
        });
    }

    // Projections current (spec §9): the global managed block must match a
    // fresh regeneration from the store (cheap string comparison, no writes).
    let projection_check = (|| -> Result<(bool, String), CoreError> {
        let rules = crate::projection::local_md::projectable_rules(
            &store,
            &crate::store::Scope::Global,
            config.knowledge.confidence_threshold,
        )?;
        let path = config.claude_dir().join("CLAUDE.md");
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        // Reuse the writer's own idempotence contract: regenerating over the
        // current content must be a no-op when projections are current.
        let regenerated =
            crate::projection::claude_md::update_claude_md_content(&existing, &rules);
        if regenerated == existing {
            Ok((true, format!("{} global rule(s) projected", rules.len())))
        } else {
            Ok((false, "global CLAUDE.md out of date — run `retro run`".to_string()))
        }
    })();
    match projection_check {
        Ok((ok, detail)) => checks.push(Check { name: "projection".to_string(), ok, detail }),
        Err(e) => checks.push(Check {
            name: "projection".to_string(),
            ok: false,
            detail: e.to_string(),
        }),
    }

    // v2 runner coexistence (Plan 2 final-review carry-over): both pipelines
    // being live doubles AI spend and double-writes the global managed block.
    let v2_plist = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/LaunchAgents/com.retro.runner.plist");
    if probe_env && v2_plist.exists() {
        checks.push(Check {
            name: "v2-runner".to_string(),
            ok: false,
            detail: "v2 launchd runner still installed — run `retro stop` (v3 replaces it; `retro migrate` in Plan 4 removes it)".to_string(),
        });
    }

    // Backup remote (informational: ok either way, detail differs)
    let has_remote = crate::store::git::has_remote(store_root);
    checks.push(Check {
        name: "backup".to_string(),
        ok: true,
        detail: if has_remote {
            "remote configured".to_string()
        } else {
            "no backup remote (optional) — rerun `retro init --v3` to set one up".to_string()
        },
    });

    // claude CLI probe (optional)
    if probe_claude {
        let probe = std::process::Command::new("claude").arg("--version").output();
        let (ok, detail) = match probe {
            Ok(out) if out.status.success() => {
                (true, String::from_utf8_lossy(&out.stdout).trim().to_string())
            }
            Ok(out) => (false, format!("claude --version exited {}", out.status)),
            Err(e) => (false, format!("claude CLI not runnable: {e}")),
        };
        checks.push(Check {
            name: "claude-cli".to_string(),
            ok,
            detail,
        });
    }

    DoctorReport { checks }
}
```

(Add `use serde::Serialize;` — chrono/serde_json are already retro-core deps. Adaptation: `projection::claude_md::update_claude_md_content` — check its visibility; if private to the projection module, make it `pub(crate)` (visibility-only change, note it). The healthy-store test seeds no CLAUDE.md and no rules → projection check passes trivially (empty == empty after the no-file guard: with zero rules and no existing file, `update_claude_md_content("", &[])` produces a managed block — if that makes the healthy test fail, mirror project_global_md's empty-guard: skip the check with ok=true "nothing to project" when rules are empty AND the file is absent).)

- [ ] **Step 3: Run tests, format, commit**

```bash
cargo test -p retro-core doctor && rustfmt --edition 2024 crates/retro-core/src/doctor.rs && cargo test && git add crates/retro-core/src/doctor.rs crates/retro-core/src/lib.rs && git commit -m "feat(v3): doctor checks module"
```

Expected: +3 tests (workspace 388).

---

### Task 4: `retro doctor` + v3 `retro status` CLI

**Files:**
- Create: `crates/retro-cli/src/commands/doctor.rs`
- Modify: `crates/retro-cli/src/commands/status.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs` (`pub mod doctor;` alphabetical)
- Modify: `crates/retro-cli/src/main.rs` (Doctor variant + arm; place after `Dash`)

- [ ] **Step 1: doctor command.** Create `crates/retro-cli/src/commands/doctor.rs`:

```rust
use anyhow::Result;
use colored::Colorize;
use retro_core::config::{Config, retro_dir};
use retro_core::doctor;

/// End-to-end v3 health verification. Read-only; the claude CLI probe is
/// a --version subprocess (no tokens).
pub fn run() -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml"))?;
    let report = doctor::run_checks(&dir, &config, true);
    for check in &report.checks {
        let mark = if check.ok { "✓".green() } else { "✗".red() };
        println!("  {} {:<12} {}", mark, check.name, check.detail);
    }
    if report.all_ok() {
        println!("\n{}", "All checks passed.".green());
        Ok(())
    } else {
        println!("\n{}", "Some checks failed — see above.".yellow());
        std::process::exit(1);
    }
}
```

- [ ] **Step 2: v3 status block.** In `status.rs`, insert BEFORE the `if !db_path.exists()` bail:

```rust
    let config = Config::load(&config_path)?;
    if config.v3.enabled {
        print_v3_status(&dir, &config)?;
        if !db_path.exists() {
            return Ok(()); // v3-only install: no v2 section to show
        }
        println!();
    }
```

(then let the existing v2 code continue — it re-loads config; leave that as-is to keep the v2 body untouched, or reuse the binding if trivially compatible — implementer judgment, minimal diff wins). Add at the bottom of the file:

```rust
fn print_v3_status(dir: &std::path::Path, config: &Config) -> Result<()> {
    use retro_core::store::{Store, queue, state::RunnerState};

    let store = Store::open(dir);
    let loaded = store.load_all()?;
    let active = loaded.nodes.iter().filter(|(_, n)| n.is_active()).count();
    let invalidated = loaded.nodes.len() - active;
    let global = loaded
        .nodes
        .iter()
        .filter(|(_, n)| n.is_active() && n.scope == retro_core::store::Scope::Global)
        .count();
    let queue_len = queue::list(dir).map(|q| q.len()).unwrap_or(0);
    let state = RunnerState::load(dir)?;
    let today = chrono::Utc::now().date_naive().to_string();
    let budget_left = state.budget_remaining(&today, config.runner.max_ai_calls_per_day);

    println!("{}", "v3 knowledge store".bold());
    println!("  nodes:   {active} active ({global} global, {} project), {invalidated} invalidated", active - global);
    println!("  queue:   {queue_len} pending session(s)");
    println!("  budget:  {budget_left}/{} AI call(s) left today", config.runner.max_ai_calls_per_day);
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
```

(Verify the exact existing imports/config-load flow in status.rs first; adapt minimally. `chrono` availability in retro-cli was resolved in Task 2 — reuse that outcome.)

- [ ] **Step 3: Wiring** — `pub mod doctor;` in commands/mod.rs (alphabetical after dash); main.rs variant:

```rust
    /// (v3) End-to-end health verification (read-only)
    Doctor,
```

with arm `Commands::Doctor => commands::doctor::run(),`.

- [ ] **Step 4: Behavior checks (paste outputs)**

```bash
cargo build
export RH=$(mktemp -d) && export CD=$(mktemp -d)
printf '[v3]\nenabled = true\n[paths]\nclaude_dir = "%s"\n' "$CD" > "$RH/config.toml"
RETRO_HOME="$RH" ./target/debug/retro doctor; echo "exit=$?"     # failures expected (no repo/index/hooks), exit 1
RETRO_HOME="$RH" ./target/debug/retro reindex && RETRO_HOME="$RH" ./target/debug/retro doctor; echo "exit=$?"  # index check flips
RETRO_HOME="$RH" ./target/debug/retro status                      # v3 block prints, no v2 bail
RETRO_HOME=$(mktemp -d) ./target/debug/retro doctor; echo "exit=$?"  # v3 disabled: single failing check pointing at init --v3
```

- [ ] **Step 5: Run tests and commit**

```bash
cargo test && git add crates/retro-cli/src/commands/doctor.rs crates/retro-cli/src/commands/status.rs crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs && git commit -m "feat(v3): retro doctor and v3 status"
```

---

### Task 5: `lint` core module

**Files:**
- Create: `crates/retro-core/src/lint.rs`
- Modify: `crates/retro-core/src/lib.rs` (`pub mod lint;`)
- Possibly modify: `crates/retro-core/src/analysis/merge.rs` (visibility of the Levenshtein helper ONLY)

Two stages: (1) a FREE pass — normalized-Levenshtein near-duplicate detection (the v2 0.8 threshold; carry-over #2) across active nodes within each scope, plus structural staleness (nodes whose `sources` sessions no longer exist on disk is NOT checkable cheaply — skip; instead flag nodes older than `staleness_days` with confidence < threshold as "candidates"); (2) an OPTIONAL AI pass (behind a flag, budget-counted) is OUT OF SCOPE for this plan — design the API so Plan 4/later can add it (`LintReport` is the seam). YAGNI: ship the free pass only.

- [ ] **Step 1: Check `analysis::merge`'s Levenshtein surface.** Read merge.rs. If the normalized-similarity fn is private, make it `pub(crate)` (one-line visibility change, no behavior change) and note it. 

- [ ] **Step 2: Write failing tests.** Create `crates/retro-core/src/lint.rs`:

```rust
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
            .write_node(&node("a", Scope::Global, 0.8, 1, "Always run the smoke tests before full runs"))
            .unwrap();
        store
            .write_node(&node("b", Scope::Global, 0.8, 1, "Always run the smoke tests before full runs!"))
            .unwrap();
        store
            .write_node(&node("c", Scope::Global, 0.8, 1, "Use uv for python environments"))
            .unwrap();
        // same body in a DIFFERENT scope must not pair with global ones
        store
            .write_node(&node("a2", Scope::Project("p".to_string()), 0.8, 1, "Always run the smoke tests before full runs"))
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
            .write_node(&node("old-weak", Scope::Global, 0.5, 60, "some tentative pattern"))
            .unwrap();
        store
            .write_node(&node("old-strong", Scope::Global, 0.9, 60, "an established rule"))
            .unwrap();
        store
            .write_node(&node("new-weak", Scope::Global, 0.5, 2, "a fresh observation"))
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
```

- [ ] **Step 3: Implement.**

```rust
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
            if crate::analysis::merge::similarity(&a.body, &b.body) > 0.8 {
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
```

Adaptation duties: `analysis::merge::similarity` — verify the actual fn name/signature in merge.rs (it may be `normalized_similarity` or private inside `find_similar_pattern`); expose minimally as noted in Step 1 and use the real name. Verify `config.analysis.staleness_days`'s type (u32?) and the exact field names (`knowledge.confidence_threshold`).

- [ ] **Step 4: Run, format, commit**

```bash
cargo test -p retro-core lint && rustfmt --edition 2024 crates/retro-core/src/lint.rs && cargo test && git add crates/retro-core/src/lint.rs crates/retro-core/src/lib.rs crates/retro-core/src/analysis/merge.rs && git commit -m "feat(v3): lint module — free near-duplicate and staleness pass"
```

Expected: +3 tests (workspace ~391).

---

### Task 6: `retro lint` CLI

**Files:**
- Create: `crates/retro-cli/src/commands/lint.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs`, `crates/retro-cli/src/main.rs`

- [ ] **Step 1: Command.**

```rust
use anyhow::Result;
use colored::Colorize;
use retro_core::config::{Config, retro_dir};
use retro_core::store::{Store, state::RunnerState};
use retro_core::lint;

/// Free lint pass (no AI calls). Without --dry-run, findings are also pushed
/// as briefing notifications (capped) so they surface in the next session.
pub fn run(dry_run: bool) -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml"))?;
    if !config.v3.enabled {
        anyhow::bail!("v3 is disabled — run `retro init --v3` first");
    }
    let store = Store::open(&dir);
    let report = lint::run_lint(&store, &config)?;
    println!(
        "Scanned {} active node(s): {} finding(s)",
        report.nodes_scanned,
        report.findings.len()
    );
    for f in &report.findings {
        println!("  {} {}", format!("[{}]", f.kind).yellow(), f.detail);
    }
    if !dry_run && !report.findings.is_empty() {
        let mut state = RunnerState::load(&dir)?;
        for f in report.findings.iter().take(3) {
            state.notifications.push(format!("Lint: {}", f.detail));
        }
        state.save(&dir)?;
        println!("\n(Top findings queued for your next session briefing.)");
    }
    Ok(())
}
```

- [ ] **Step 2: Wiring.** main.rs variant:

```rust
    /// (v3) Store-wide lint: near-duplicates and stale candidates (no AI calls)
    Lint {
        /// Report only; don't queue findings as briefing notifications
        #[arg(long)]
        dry_run: bool,
    },
```

arm: `Commands::Lint { dry_run } => commands::lint::run(dry_run),`. `pub mod lint;` alphabetical in commands/mod.rs.

- [ ] **Step 3: Behavior check (paste)** — seed two near-identical nodes in a temp store (hand-written .md files), run `retro lint --dry-run` then `retro lint`; verify findings print, dry-run leaves state.json untouched, non-dry-run queues ≤3 notifications.

- [ ] **Step 4: Test + commit**

```bash
cargo test && git add crates/retro-cli/src/commands/lint.rs crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs && git commit -m "feat(v3): retro lint command"
```

---

### Task 7: Dashboard server plumbing (`tiny_http` + routes skeleton)

**Files:**
- Modify: `crates/retro-cli/Cargo.toml` (add `tiny_http = "0.12"`)
- Create: `crates/retro-cli/src/ui/mod.rs`
- Create: `crates/retro-cli/src/ui/assets/index.html` (placeholder shell THIS task; full frontend in Task 10)
- Modify: `crates/retro-cli/src/main.rs` (add `mod ui;` next to `mod tui;` — check how tui is declared and mirror it)

- [ ] **Step 1: Dependency.** Add to `[dependencies]` in `crates/retro-cli/Cargo.toml`:

```toml
tiny_http = "0.12"
```

Run `cargo build` once and note the added transitive deps in your report (expected: tiny_http + a small set: ascii, chunked_transfer, httpdate — no async runtime; if anything async/tokio appears, STOP and report).

- [ ] **Step 2: Server skeleton.** Create `crates/retro-cli/src/ui/mod.rs`:

```rust
//! v3 dashboard: sync tiny_http server, localhost-only, single embedded page.
//! Read APIs serve store/index/health/state; write APIs go through the store
//! (file edit -> commit -> reindex -> reproject).

pub mod api;

use anyhow::Result;
use retro_core::config::Config;
use std::path::PathBuf;

const INDEX_HTML: &str = include_str!("assets/index.html");

/// Serve until the process is killed (Ctrl+C). Binds 127.0.0.1 only.
pub fn serve(store_root: PathBuf, config: Config) -> Result<()> {
    let addr = format!("127.0.0.1:{}", config.ui.port);
    let server = tiny_http::Server::http(&addr)
        .map_err(|e| anyhow::anyhow!("cannot bind {addr}: {e}"))?;
    println!("retro dashboard: http://{addr}  (Ctrl+C to stop)");

    for request in server.incoming_requests() {
        let url = request.url().to_string();
        let method = request.method().clone();
        let response = api::route(&store_root, &config, &method, &url, request);
        if let Err(e) = response {
            eprintln!("ui: request error: {e}");
        }
    }
    Ok(())
}

pub(crate) fn html_response(body: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let mut resp = tiny_http::Response::from_string(body);
    resp.add_header(
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
            .unwrap(),
    );
    resp
}

pub(crate) fn json_response(
    value: &serde_json::Value,
    status: u16,
) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let mut resp = tiny_http::Response::from_string(value.to_string());
    resp.add_header(
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
    );
    resp.with_status_code(status)
}

pub(crate) fn index_html() -> &'static str {
    INDEX_HTML
}
```

- [ ] **Step 3: Route skeleton.** Create `crates/retro-cli/src/ui/api.rs` (this task: `/` serving the page + `/api/ping`; the real handlers land in Tasks 8–9):

```rust
//! Dashboard JSON API. All handlers are synchronous and read the same
//! retro-core modules the CLI uses.

use std::path::Path;

use anyhow::Result;
use retro_core::config::Config;
use serde_json::json;
use tiny_http::{Method, Request};

use super::{html_response, index_html, json_response};

pub fn route(
    store_root: &Path,
    config: &Config,
    method: &Method,
    url: &str,
    request: Request,
) -> Result<()> {
    let path = url.split('?').next().unwrap_or(url);
    match (method, path) {
        (Method::Get, "/") => request.respond(html_response(index_html()))?,
        (Method::Get, "/api/ping") => {
            request.respond(json_response(&json!({"ok": true}), 200))?
        }
        _ => request.respond(json_response(&json!({"error": "not found"}), 404))?,
    }
    Ok(())
}
```

- [ ] **Step 4: Placeholder page.** Create `crates/retro-cli/src/ui/assets/index.html`:

```html
<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>retro</title></head>
<body><h1>retro dashboard</h1><p>loading…</p></body></html>
```

- [ ] **Step 5: Wire `mod ui;` in main.rs** (private module, like `mod tui;` if that's the pattern — read main.rs first; commands access it via `crate::ui`).

- [ ] **Step 6: Build + manual check (paste):** `cargo build` clean; no server test yet (Task 11 adds the command). Commit:

```bash
git add crates/retro-cli/Cargo.toml Cargo.lock crates/retro-cli/src/ui/ crates/retro-cli/src/main.rs && git commit -m "feat(v3): dashboard server plumbing (tiny_http)"
```

NOTE: this is the ONE commit where Cargo.lock SHOULD be staged (new dependency).

---

### Task 8: Dashboard read APIs (X-ray, nodes, health, history, doctor)

**Files:**
- Modify: `crates/retro-cli/src/ui/api.rs`

Endpoints (all GET, all JSON):
- `/api/xray` — the flagship context inventory. For each registered project (PathMap + ProjectMeta): project CLAUDE.md, CLAUDE.local.md, auto-memory MEMORY.md presence/sizes/rough tokens (`chars/4`), plus node counts. Plus globals: `~/.claude/CLAUDE.md` (with managed-block line count marked as retro-owned), skills dir entry count (read_dir of `claude_dir()/skills`, count only).
- `/api/nodes?scope=&type=&q=&active=` — `store::index::query` passthrough (open() NotInitialized → 409 with `{"error":"index not built — run retro reindex"}`).
- `/api/node?scope=<scope>&id=<id>` — full node via `store.get` (body, frontmatter fields, file path).
- `/api/health` — `Health::load` verbatim + `RunnerState` summary (queue len, budget, notifications pending count).
- `/api/history?limit=50` — `git -C <store> log --format=%h|%ad|%s --date=iso -n <limit>` parsed into JSON.
- `/api/doctor` — `doctor::run_checks(store_root, config, false)` (no claude probe from the web).

- [ ] **Step 1: Implement handlers.** Extend `route()` with the six endpoints. Representative implementations (write ALL of them following these shapes):

```rust
fn query_param(url: &str, key: &str) -> Option<String> {
    url.split('?').nth(1)?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k == key {
            // minimal percent-decode: %20 and + as space (ids/scopes are kebab + '/')
            Some(v.replace('+', " ").replace("%20", " ").replace("%2F", "/"))
        } else {
            None
        }
    })
}

fn api_nodes(store_root: &Path, url: &str) -> (serde_json::Value, u16) {
    let conn = match retro_core::store::index::open(store_root) {
        Ok(c) => c,
        Err(e) => return (json!({"error": e.to_string()}), 409),
    };
    let filter = retro_core::store::index::NodeFilter {
        scope: query_param(url, "scope"),
        node_type: query_param(url, "type"),
        active_only: query_param(url, "active").as_deref() == Some("true"),
        text: query_param(url, "q"),
    };
    match retro_core::store::index::query(&conn, &filter) {
        Ok(rows) => (
            json!(rows
                .iter()
                .map(|r| json!({
                    "id": r.id, "scope": r.scope, "type": r.node_type,
                    "confidence": r.confidence, "active": r.active,
                    "updated": r.updated,
                    "body": retro_core::util::truncate_str(&r.body, 200),
                    "sources": r.sources,
                }))
                .collect::<Vec<_>>()),
            200,
        ),
        Err(e) => (json!({"error": e.to_string()}), 500),
    }
}

fn api_xray(store_root: &Path, config: &Config) -> (serde_json::Value, u16) {
    use retro_core::store::{projects::PathMap, Store};
    let est = |bytes: u64| bytes / 4; // rough tokens
    let file_info = |p: &Path| -> serde_json::Value {
        match std::fs::metadata(p) {
            Ok(m) => json!({"present": true, "bytes": m.len(), "tokens_est": est(m.len())}),
            Err(_) => json!({"present": false}),
        }
    };
    let store = Store::open(store_root);
    let loaded = store.load_all().unwrap_or(retro_core::store::LoadResult {
        nodes: vec![],
        warnings: vec![],
    });
    let map = PathMap::load(store_root).unwrap_or_default();
    let claude_dir = config.claude_dir();

    let mut projects_json = Vec::new();
    for (slug, path) in &map.paths {
        let root = Path::new(path);
        let node_count = loaded
            .nodes
            .iter()
            .filter(|(_, n)| matches!(&n.scope, retro_core::store::Scope::Project(s) if s == slug))
            .filter(|(_, n)| n.is_active())
            .count();
        // Auto-memory lives under claude_dir/projects/<encoded-path>/memory/
        let encoded = path.replace('/', "-");
        let memory = claude_dir
            .join("projects")
            .join(&encoded)
            .join("memory")
            .join("MEMORY.md");
        projects_json.push(json!({
            "slug": slug, "path": path,
            "claude_md": file_info(&root.join("CLAUDE.md")),
            "claude_local_md": file_info(&root.join("CLAUDE.local.md")),
            "memory_md": file_info(&memory),
            "active_nodes": node_count,
        }));
    }
    let skills_count = std::fs::read_dir(claude_dir.join("skills"))
        .map(|d| d.count())
        .unwrap_or(0);
    (
        json!({
            "global_claude_md": file_info(&claude_dir.join("CLAUDE.md")),
            "global_active_nodes": loaded.nodes.iter()
                .filter(|(_, n)| n.is_active() && n.scope == retro_core::store::Scope::Global).count(),
            "skills_count": skills_count,
            "projects": projects_json,
            "store_warnings": loaded.warnings,
        }),
        200,
    )
}
```

History via `std::process::Command::new("git")` with `-C store_root` (args array, never shell strings). Doctor: `serde_json::to_value(doctor::run_checks(...))`.

Adaptation duties: verify `LoadResult` field visibility for the fallback construction (if constructing it is awkward, use `match`/`unwrap_or_else` returning early); the auto-memory encoded path — confirm the encoding scheme against `observer.rs`/`ingest` (leading dash: `/Users/x` → `-Users-x`); truncate_str is `retro_core::util::truncate_str` (pub — verify).

- [ ] **Step 2: HTTP-level tests.** Add a test module to `api.rs` that tests the HANDLER functions directly (not the server): seed a temp store + index, call `api_nodes`/`api_xray`/handler fns, assert JSON shapes. 3 tests minimum: nodes_endpoint_filters_and_409s_without_index, xray_lists_projects_and_globals, history_parses_git_log.

- [ ] **Step 3: Test + commit**

```bash
cargo test -p retro-cli && cargo test && git add crates/retro-cli/src/ui/api.rs && git commit -m "feat(v3): dashboard read APIs — xray, nodes, health, history, doctor"
```

---

### Task 9: Dashboard write APIs (edit, invalidate, exclude) 

**Files:**
- Modify: `crates/retro-cli/src/ui/api.rs`

All writes go through the store and follow the pipeline's discipline: mutate file → `commit_all` with a descriptive message → `index::build` → reproject the affected scope. POST only, JSON bodies.

- `POST /api/node/invalidate` body `{"scope": "...", "id": "..."}` → `store.invalidate(scope, id, "user")` → commit `"user: invalidate <id> (dashboard)"` → reindex → reproject scope.
- `POST /api/node/update` body `{"scope","id","body","confidence"}` → `store.get` → apply → `write_node` → commit `"user: edit <id> (dashboard)"` → reindex → reproject.
- `POST /api/project/exclude` body `{"slug"}` → look up path via PathMap → append the path to `config.privacy.exclude_projects` + `config.save` → `projects::cleanup_excluded` → commit `"retro: exclude <slug>"` → reindex.

- [ ] **Step 1: Implement** with a shared helper:

```rust
fn after_write(store_root: &Path, config: &Config, scope: &retro_core::store::Scope, message: &str) -> Result<(), retro_core::errors::CoreError> {
    use retro_core::store::{git as store_git, index, projects::PathMap, Store};
    let store = Store::open(store_root);
    store_git::commit_all(store_root, message).map(|_| ())?;
    if let Err(e) = index::build(&store) {
        retro_core::health::record(store_root, "index", false, &e.to_string())?;
    }
    let threshold = config.knowledge.confidence_threshold;
    match scope {
        retro_core::store::Scope::Global => {
            let path = config.claude_dir().join("CLAUDE.md");
            retro_core::projection::local_md::project_global_md(&store, &path, threshold, Some(&store_root.join("backups")))?;
        }
        retro_core::store::Scope::Project(slug) => {
            let map = PathMap::load(store_root)?;
            if let Some(p) = map.paths.get(slug) {
                retro_core::projection::local_md::project_local_md(&store, slug, Path::new(p), threshold)?;
            }
        }
    }
    Ok(())
}
```

Request bodies: `let mut body = String::new(); request.as_reader().read_to_string(&mut body)?;` BEFORE responding (tiny_http consumes the reader from the request). Validate scope/id with `Scope::parse` (rejects hostile input; ids additionally via the store's get — nonexistent → 404). Confidence clamped [0,1].

- [ ] **Step 2: Tests (3+):** invalidate flow end-to-end on a temp store (node inactive after + commit exists + CLAUDE.local.md regenerated without the rule); update flow (body change reflected in file + projection); exclude flow (config gains path, knowledge dir gone). Handler-level, no HTTP server needed — factor handlers as `fn(store_root, config, body_json) -> (Value, u16)` so tests call them directly.

- [ ] **Step 3: Test + commit**

```bash
cargo test && git add crates/retro-cli/src/ui/api.rs && git commit -m "feat(v3): dashboard write APIs — edit, invalidate, exclude"
```

---

### Task 10: Dashboard frontend (single embedded file)

**Files:**
- Modify: `crates/retro-cli/src/ui/assets/index.html` (replace the placeholder)

A complete, minimal, dependency-free single-file app (~250 lines): vanilla JS + fetch, four tabs (X-ray / Knowledge / Health / History), dark-friendly minimal CSS. Functional over fancy — this is v1 of the dashboard; polish comes later. Full intended content:

```html
<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>retro</title>
<style>
  :root { --bg:#111; --fg:#ddd; --dim:#888; --accent:#7aa2f7; --warn:#e0af68; --err:#f7768e; --ok:#9ece6a; --card:#1a1a1f; }
  * { box-sizing: border-box; }
  body { margin:0; font:14px/1.5 -apple-system, "SF Mono", Menlo, monospace; background:var(--bg); color:var(--fg); }
  header { display:flex; gap:1rem; align-items:baseline; padding:0.8rem 1.2rem; border-bottom:1px solid #2a2a30; }
  header h1 { font-size:1rem; margin:0; color:var(--accent); }
  nav button { background:none; border:none; color:var(--dim); font:inherit; cursor:pointer; padding:0.2rem 0.6rem; }
  nav button.active { color:var(--fg); border-bottom:2px solid var(--accent); }
  main { padding:1rem 1.2rem; max-width:1000px; margin:0 auto; }
  .card { background:var(--card); border-radius:8px; padding:0.8rem 1rem; margin:0.6rem 0; }
  .row { display:flex; justify-content:space-between; gap:1rem; flex-wrap:wrap; }
  .dim { color:var(--dim); } .ok { color:var(--ok); } .warn { color:var(--warn); } .err { color:var(--err); }
  .badge { font-size:0.75rem; padding:0.05rem 0.45rem; border-radius:8px; background:#26262e; }
  .retro-owned { color:var(--accent); }
  input, select { background:#0d0d10; color:var(--fg); border:1px solid #2a2a30; border-radius:6px; padding:0.3rem 0.5rem; font:inherit; }
  textarea { width:100%; min-height:8rem; background:#0d0d10; color:var(--fg); border:1px solid #2a2a30; border-radius:6px; padding:0.5rem; font:inherit; }
  button.action { background:#26262e; color:var(--fg); border:1px solid #3a3a44; border-radius:6px; padding:0.25rem 0.7rem; cursor:pointer; font:inherit; }
  button.action:hover { border-color:var(--accent); }
  button.danger:hover { border-color:var(--err); color:var(--err); }
  table { width:100%; border-collapse:collapse; }
  td, th { text-align:left; padding:0.3rem 0.5rem; border-bottom:1px solid #22222a; vertical-align:top; }
  pre { white-space:pre-wrap; word-break:break-word; }
</style>
</head>
<body>
<header>
  <h1>retro</h1>
  <nav id="tabs">
    <button data-tab="xray" class="active">X-ray</button>
    <button data-tab="knowledge">Knowledge</button>
    <button data-tab="health">Health</button>
    <button data-tab="history">History</button>
  </nav>
</header>
<main id="main"></main>
<script>
const $ = (s, el=document) => el.querySelector(s);
const esc = s => String(s ?? "").replace(/[&<>"]/g, c => ({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;"}[c]));
const get = async p => { const r = await fetch(p); if (!r.ok) throw new Error((await r.json()).error || r.status); return r.json(); };
const post = async (p, body) => { const r = await fetch(p, {method:"POST", body: JSON.stringify(body)}); const j = await r.json(); if (!r.ok) throw new Error(j.error || r.status); return j; };
const fileCell = f => f.present ? `${(f.bytes/1024).toFixed(1)} KB <span class="dim">(~${f.tokens_est} tok)</span>` : `<span class="dim">—</span>`;

const views = {
  async xray() {
    const x = await get("/api/xray");
    let html = `<div class="card"><div class="row">
      <div><b>Global CLAUDE.md</b> ${fileCell(x.global_claude_md)}
        <span class="badge retro-owned">${x.global_active_nodes} retro rules</span></div>
      <div><b>Skills</b> ${x.skills_count}</div></div></div>`;
    if (x.store_warnings.length)
      html += `<div class="card err">⚠ ${x.store_warnings.map(esc).join("<br>")}</div>`;
    html += x.projects.map(p => `<div class="card">
      <div class="row"><b>${esc(p.slug)}</b><span class="dim">${esc(p.path)}</span></div>
      <table><tr>
        <td>CLAUDE.md ${fileCell(p.claude_md)}</td>
        <td>CLAUDE.local.md ${fileCell(p.claude_local_md)} <span class="badge retro-owned">retro</span></td>
        <td>MEMORY.md ${fileCell(p.memory_md)}</td>
        <td><span class="badge retro-owned">${p.active_nodes} nodes</span></td>
        <td><button class="action danger" onclick="excludeProject('${esc(p.slug)}')">exclude</button></td>
      </tr></table></div>`).join("");
    return html;
  },
  async knowledge() {
    const q = $("#kq")?.value ?? "", scope = $("#kscope")?.value ?? "";
    const params = new URLSearchParams(); if (q) params.set("q", q); if (scope) params.set("scope", scope);
    const nodes = await get("/api/nodes?" + params);
    return `<div class="card row">
        <input id="kq" placeholder="search…" value="${esc(q)}" onchange="render()">
        <select id="kscope" onchange="render()">
          <option value="">all scopes</option><option ${scope==="global"?"selected":""} value="global">global</option>
        </select></div>` +
      nodes.map(n => `<div class="card" ${n.active ? "" : 'style="opacity:.5"'}>
        <div class="row"><b>${esc(n.id)}</b>
          <span><span class="badge">${esc(n.scope)}</span> <span class="badge">${esc(n.type)}</span>
          <span class="badge">${n.confidence.toFixed(2)}</span>
          ${n.active ? `<button class="action danger" onclick="invalidate('${esc(n.scope)}','${esc(n.id)}')">invalidate</button>` : `<span class="dim">invalidated</span>`}</span></div>
        <pre class="dim">${esc(n.body)}</pre>
        <span class="dim">sources: ${n.sources.map(esc).join(", ") || "—"} · updated ${esc(n.updated)}</span>
      </div>`).join("") || `<div class="card dim">no nodes</div>`;
  },
  async health() {
    const h = await get("/api/health"), d = await get("/api/doctor");
    return `<div class="card"><b>Doctor</b><table>` +
      d.checks.map(c => `<tr><td class="${c.ok?"ok":"err"}">${c.ok?"✓":"✗"}</td><td>${esc(c.name)}</td><td class="dim">${esc(c.detail)}</td></tr>`).join("") +
      `</table></div><div class="card"><b>Stages</b><table>` +
      Object.entries(h.stages).map(([n,s]) => `<tr><td class="${s.ok?"ok":"err"}">${s.ok?"✓":"✗"}</td><td>${esc(n)}</td><td class="dim">${esc(s.detail)}</td><td class="dim">${esc(s.at)}</td></tr>`).join("") +
      `</table></div><div class="card dim">queue: ${h.queue_len} · budget left today: ${h.budget_remaining} · pending notifications: ${h.notifications_pending}</div>`;
  },
  async history() {
    const commits = await get("/api/history?limit=50");
    return `<div class="card"><table>` +
      commits.map(c => `<tr><td class="dim">${esc(c.hash)}</td><td>${esc(c.subject)}</td><td class="dim">${esc(c.date)}</td></tr>`).join("") +
      `</table></div>`;
  },
};

let tab = "xray";
async function render() {
  try { $("#main").innerHTML = await views[tab](); }
  catch (e) { $("#main").innerHTML = `<div class="card err">${esc(e.message)}</div>`; }
}
async function invalidate(scope, id) {
  if (!confirm(`Invalidate ${id}? (recoverable via store git history)`)) return;
  try { await post("/api/node/invalidate", {scope, id}); render(); } catch (e) { alert(e.message); }
}
async function excludeProject(slug) {
  if (!confirm(`Stop watching ${slug} and DELETE its knowledge? (recoverable via store git history)`)) return;
  try { await post("/api/project/exclude", {slug}); render(); } catch (e) { alert(e.message); }
}
$("#tabs").addEventListener("click", e => {
  if (e.target.dataset.tab) {
    tab = e.target.dataset.tab;
    document.querySelectorAll("nav button").forEach(b => b.classList.toggle("active", b === e.target));
    render();
  }
});
render();
</script>
</body>
</html>
```

Adaptation duties: the `/api/health` response shape must match Task 8's handler (add `queue_len`, `budget_remaining`, `notifications_pending` fields there if Task 8's shape differs — align the two, choosing the shape above). Node update (edit body) UI is deliberately deferred — the invalidate + exclude actions are the v1 write surface (the API from Task 9 exists for later UI use; note this in the report as intentional).

- [ ] **Step: Build, then commit**

```bash
cargo build && git add crates/retro-cli/src/ui/assets/index.html && git commit -m "feat(v3): dashboard frontend — xray, knowledge, health, history"
```

---

### Task 11: `retro ui` command + live check

**Files:**
- Create: `crates/retro-cli/src/commands/ui.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs`, `crates/retro-cli/src/main.rs`

- [ ] **Step 1: Command.**

```rust
use anyhow::Result;
use retro_core::config::{Config, retro_dir};

/// Start the dashboard server and open the browser. Blocks until Ctrl+C.
pub fn run(no_open: bool) -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml"))?;
    if !config.v3.enabled {
        anyhow::bail!("v3 is disabled — run `retro init --v3` first");
    }
    let url = format!("http://127.0.0.1:{}", config.ui.port);
    if !no_open {
        // macOS `open`; failure is non-fatal (headless/SSH)
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    crate::ui::serve(dir, config)
}
```

main.rs variant:

```rust
    /// (v3) Open the dashboard (local web UI)
    Ui {
        /// Don't auto-open the browser
        #[arg(long)]
        no_open: bool,
    },
```

arm: `Commands::Ui { no_open } => commands::ui::run(no_open),`. Exempt `Ui` from the nudge? No — nudge on stderr is fine for an interactive command; leave as-is.

- [ ] **Step 2: Live check (paste outputs).** With an isolated RETRO_HOME (v3 enabled, temp claude_dir, seeded store + one node + reindex):

```bash
cargo build
RETRO_HOME="$RH" ./target/debug/retro ui --no-open & UIPID=$!
sleep 1
curl -s http://127.0.0.1:7777/api/ping
curl -s http://127.0.0.1:7777/ | head -3
curl -s "http://127.0.0.1:7777/api/nodes?scope=global" | head -c 300; echo
curl -s http://127.0.0.1:7777/api/doctor | head -c 300; echo
curl -s -X POST http://127.0.0.1:7777/api/node/invalidate -d '{"scope":"global","id":"<seeded-id>"}'
kill $UIPID
```

Verify: ping ok; HTML served; nodes JSON lists the seeded node; doctor JSON; invalidate returns ok and the node file gains `invalidated_by: user`; a `user: invalidate` commit exists in the store log.

- [ ] **Step 3: Test + commit**

```bash
cargo test && git add crates/retro-cli/src/commands/ui.rs crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs && git commit -m "feat(v3): retro ui command"
```

---

### Task 12: Documentation and plan completion

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1:** Append to the v3 subsection after the Plan 2 bullet:

```markdown
- **Plan 3: DONE** — Surfaces. `retro ui` local dashboard (tiny_http, single embedded page:
  context X-ray with retro-owned marking, knowledge browser with search/invalidate,
  health + doctor view, store history; write actions commit → reindex → reproject),
  `retro doctor` (structural checks + optional claude-CLI probe), v3-aware `retro status`,
  `retro lint` (free near-duplicate + stale-candidate pass, Levenshtein 0.8 net),
  queue-age nudge, store self-exclusion guard, subagent-transcript skip. One new
  dependency: tiny_http (sync).
```

Add command rows: `retro ui [--no-open]`, `retro doctor`, `retro lint [--dry-run]` to the Core Commands table with (v3) prefixes.

- [ ] **Step 2:** `cargo test` all green; `cargo run -- --help | grep -E "ui|doctor|lint"` lists all three. Commit `docs: v3 plan 3 surfaces status`.

---

## Rollout (manual, after merge)

The store is already live on this machine (Plan 2 smoke test). After merging Plan 3: rebuild release, `retro doctor` (expect all green except any stale queue), then `retro ui` for the first real X-ray. Rerunning `retro init --v3` is NOT needed (hooks unchanged) — but IS safe (idempotent) and updates hook binary paths if the release binary moved.

## Out of scope (Plan 4 — Lifecycle)

- `retro migrate` (v2 SQLite → v3 nodes, launchd removal, `[v3]` default flip, poisoned-store `git rm --cached` cleanup), v3 `retro uninstall`
- Deletion of v1/v2 commands, modules, and tables; scenario-test rewrite; 3.0.0 release via publish workflow
- Dashboard polish: node editing UI (API already exists), one-click history revert (spec §8 — history view is read-only in this plan), coverage list of seen-but-unwatched projects, AI-assisted lint stage, confidence-based knowledge filtering (spec §8 names it; confidence is displayed but not filterable in this plan)
