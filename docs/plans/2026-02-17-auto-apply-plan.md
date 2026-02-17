# Auto-Apply Pipeline Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Automate the full retro pipeline (ingest → analyze → apply) via a single post-commit hook with per-stage cooldowns, data triggers, and a terminal nudge for auto-created PRs.

**Architecture:** The post-commit hook runs `retro ingest --auto`, which after ingesting, opportunistically chains analyze and apply when conditions are met (cooldown elapsed + new data exists). A terminal nudge system queries the DB on interactive commands to inform users of pending PRs.

**Tech Stack:** Rust, rusqlite, clap, chrono, colored

---

### Task 1: DB Schema Migration — Add `nudged` Column

**Files:**
- Modify: `crates/retro-core/src/db.rs:8` (SCHEMA_VERSION)
- Modify: `crates/retro-core/src/db.rs:23-84` (migrate function)

**Step 1: Write the failing test**

Add to the existing test module in `crates/retro-core/src/db.rs`:

```rust
#[test]
fn test_migration_v2_adds_nudged_column() {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    migrate(&conn).unwrap();

    // Insert a projection and verify nudged defaults to 0
    conn.execute(
        "INSERT INTO patterns (id, pattern_type, description, confidence, first_seen, last_seen, status, source_sessions, related_files, suggested_content, suggested_target)
         VALUES ('p1', 'repetitive_instruction', 'test', 0.9, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z', 'discovered', '[]', '[]', 'content', 'Skill')",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO projections (id, pattern_id, target_type, target_path, content, applied_at, pr_url, nudged)
         VALUES ('proj1', 'p1', 'Skill', '/path', 'content', '2025-01-01T00:00:00Z', 'https://github.com/test/repo/pull/1', 0)",
        [],
    ).unwrap();

    let nudged: i32 = conn.query_row(
        "SELECT nudged FROM projections WHERE id = 'proj1'",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(nudged, 0);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_migration_v2_adds_nudged_column`
Expected: FAIL — `nudged` column doesn't exist yet

**Step 3: Implement the migration**

In `crates/retro-core/src/db.rs`, change SCHEMA_VERSION and add v2 migration:

```rust
const SCHEMA_VERSION: u32 = 2;
```

Add after the `if current_version < 1` block (line 81), before `Ok(())`:

```rust
    if current_version < 2 {
        conn.execute_batch(
            "ALTER TABLE projections ADD COLUMN nudged INTEGER NOT NULL DEFAULT 0;"
        )?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    }
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_migration_v2_adds_nudged_column`
Expected: PASS

**Step 5: Run all tests to verify no regressions**

Run: `cargo test --workspace`
Expected: All 63+ tests pass

**Step 6: Commit**

```bash
git add crates/retro-core/src/db.rs
git commit -m "feat: add nudged column to projections (schema v2)"
```

---

### Task 2: New DB Functions — `last_applied_at`, `has_unanalyzed_sessions`, `has_unprojected_patterns`, nudge queries

**Files:**
- Modify: `crates/retro-core/src/db.rs` (add functions after line 189)

**Step 1: Write the failing tests**

Add to the test module in `crates/retro-core/src/db.rs`:

```rust
#[test]
fn test_last_applied_at_empty() {
    let conn = test_db();
    assert!(last_applied_at(&conn).unwrap().is_none());
}

#[test]
fn test_last_applied_at_returns_max() {
    let conn = test_db();
    let p = test_pattern("pat-1", "test pattern");
    insert_pattern(&conn, &p).unwrap();

    let proj1 = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "Skill".to_string(),
        target_path: "/path/a".to_string(),
        content: "content".to_string(),
        applied_at: chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z").unwrap().with_timezone(&Utc),
        pr_url: None,
    };
    insert_projection(&conn, &proj1).unwrap();

    let result = last_applied_at(&conn).unwrap();
    assert!(result.is_some());
}

#[test]
fn test_has_unanalyzed_sessions_empty() {
    let conn = test_db();
    assert!(!has_unanalyzed_sessions(&conn).unwrap());
}

#[test]
fn test_has_unanalyzed_sessions_with_new_session() {
    let conn = test_db();
    insert_ingested_session(&conn, "sess-1", "/proj", "/path/sess", 100, "2025-01-01T00:00:00Z").unwrap();
    assert!(has_unanalyzed_sessions(&conn).unwrap());
}

#[test]
fn test_has_unanalyzed_sessions_after_analysis() {
    let conn = test_db();
    insert_ingested_session(&conn, "sess-1", "/proj", "/path/sess", 100, "2025-01-01T00:00:00Z").unwrap();
    record_session_analyzed(&conn, "sess-1", "/proj").unwrap();
    assert!(!has_unanalyzed_sessions(&conn).unwrap());
}

#[test]
fn test_has_unprojected_patterns_empty() {
    let conn = test_db();
    assert!(!has_unprojected_patterns(&conn).unwrap());
}

#[test]
fn test_has_unprojected_patterns_with_discovered() {
    let conn = test_db();
    let p = test_pattern("pat-1", "test");
    insert_pattern(&conn, &p).unwrap();
    assert!(has_unprojected_patterns(&conn).unwrap());
}

#[test]
fn test_has_unprojected_patterns_after_projection() {
    let conn = test_db();
    let p = test_pattern("pat-1", "test");
    insert_pattern(&conn, &p).unwrap();

    let proj = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "Skill".to_string(),
        target_path: "/path".to_string(),
        content: "content".to_string(),
        applied_at: Utc::now(),
        pr_url: None,
    };
    insert_projection(&conn, &proj).unwrap();
    assert!(!has_unprojected_patterns(&conn).unwrap());
}

#[test]
fn test_has_unprojected_patterns_excludes_generation_failed() {
    let conn = test_db();
    let p = test_pattern("pat-1", "test");
    insert_pattern(&conn, &p).unwrap();
    set_generation_failed(&conn, "pat-1", true).unwrap();
    assert!(!has_unprojected_patterns(&conn).unwrap());
}

#[test]
fn test_has_unprojected_patterns_excludes_dbonly() {
    let conn = test_db();
    let mut p = test_pattern("pat-1", "test");
    p.suggested_target = SuggestedTarget::DbOnly;
    insert_pattern(&conn, &p).unwrap();
    assert!(!has_unprojected_patterns(&conn).unwrap());
}

#[test]
fn test_get_unnudged_pr_urls() {
    let conn = test_db();
    let p = test_pattern("pat-1", "test");
    insert_pattern(&conn, &p).unwrap();

    let proj = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "Skill".to_string(),
        target_path: "/path".to_string(),
        content: "content".to_string(),
        applied_at: Utc::now(),
        pr_url: Some("https://github.com/test/pull/1".to_string()),
    };
    insert_projection(&conn, &proj).unwrap();

    let urls = get_unnudged_pr_urls(&conn).unwrap();
    assert_eq!(urls.len(), 1);
    assert_eq!(urls[0], "https://github.com/test/pull/1");
}

#[test]
fn test_mark_projections_nudged() {
    let conn = test_db();
    let p = test_pattern("pat-1", "test");
    insert_pattern(&conn, &p).unwrap();

    let proj = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "Skill".to_string(),
        target_path: "/path".to_string(),
        content: "content".to_string(),
        applied_at: Utc::now(),
        pr_url: Some("https://github.com/test/pull/1".to_string()),
    };
    insert_projection(&conn, &proj).unwrap();

    mark_projections_nudged(&conn).unwrap();
    let urls = get_unnudged_pr_urls(&conn).unwrap();
    assert!(urls.is_empty());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p retro-core last_applied_at has_unanalyzed has_unprojected get_unnudged mark_projections_nudged`
Expected: FAIL — functions don't exist

**Step 3: Implement the DB functions**

Add after `last_analyzed_at()` (around line 189) in `crates/retro-core/src/db.rs`:

```rust
/// Get the most recent projection (apply) timestamp.
pub fn last_applied_at(conn: &Connection) -> Result<Option<String>, CoreError> {
    let result = conn.query_row(
        "SELECT MAX(applied_at) FROM projections",
        [],
        |row| row.get::<_, Option<String>>(0),
    )?;
    Ok(result)
}

/// Check if there are ingested sessions that haven't been analyzed yet.
pub fn has_unanalyzed_sessions(conn: &Connection) -> Result<bool, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM ingested_sessions i
         LEFT JOIN analyzed_sessions a ON i.session_id = a.session_id
         WHERE a.session_id IS NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Check if there are discovered patterns that haven't been projected yet.
/// Excludes patterns with generation_failed=true or suggested_target='DbOnly'.
pub fn has_unprojected_patterns(conn: &Connection) -> Result<bool, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM patterns p
         LEFT JOIN projections pr ON p.id = pr.pattern_id
         WHERE pr.id IS NULL
         AND p.status IN ('discovered', 'active')
         AND p.generation_failed = 0
         AND p.suggested_target != 'DbOnly'",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Get distinct PR URLs from projections that haven't been nudged.
pub fn get_unnudged_pr_urls(conn: &Connection) -> Result<Vec<String>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT pr_url FROM projections WHERE pr_url IS NOT NULL AND nudged = 0",
    )?;
    let urls = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(urls)
}

/// Mark all projections with non-null PR URLs as nudged.
pub fn mark_projections_nudged(conn: &Connection) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE projections SET nudged = 1 WHERE pr_url IS NOT NULL AND nudged = 0",
        [],
    )?;
    Ok(())
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p retro-core -- last_applied_at has_unanalyzed has_unprojected get_unnudged mark_projections_nudged`
Expected: All PASS

**Step 5: Run all tests**

Run: `cargo test --workspace`
Expected: All tests pass

**Step 6: Commit**

```bash
git add crates/retro-core/src/db.rs
git commit -m "feat: add DB functions for auto-apply pipeline (data triggers + nudge)"
```

---

### Task 3: Config Changes — Granular Cooldowns + `auto_apply`

**Files:**
- Modify: `crates/retro-core/src/config.rs:51-59` (HooksConfig struct)
- Modify: `crates/retro-core/src/config.rs:91-97` (default_hooks function)
- Modify: `crates/retro-core/src/config.rs:130-131` (default_cooldown function)

**Step 1: Write the failing test**

Add a test module to `crates/retro-core/src/config.rs` (there are no existing tests here — add at the end of the file):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hooks_config_defaults() {
        let config = HooksConfig::default();
        assert_eq!(config.ingest_cooldown_minutes, 5);
        assert_eq!(config.analyze_cooldown_minutes, 1440);
        assert_eq!(config.apply_cooldown_minutes, 1440);
        assert!(config.auto_apply);
    }

    #[test]
    fn test_hooks_config_backwards_compat() {
        // Old config with only auto_cooldown_minutes should still deserialize
        let toml_str = r#"
        [hooks]
        auto_cooldown_minutes = 30
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        // Old field should map to ingest cooldown
        assert_eq!(config.hooks.ingest_cooldown_minutes, 30);
    }

    #[test]
    fn test_hooks_config_new_fields() {
        let toml_str = r#"
        [hooks]
        ingest_cooldown_minutes = 10
        analyze_cooldown_minutes = 720
        apply_cooldown_minutes = 2880
        auto_apply = false
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.hooks.ingest_cooldown_minutes, 10);
        assert_eq!(config.hooks.analyze_cooldown_minutes, 720);
        assert_eq!(config.hooks.apply_cooldown_minutes, 2880);
        assert!(!config.hooks.auto_apply);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p retro-core test_hooks_config`
Expected: FAIL — fields don't exist

**Step 3: Implement the config changes**

Replace `HooksConfig` and its defaults in `crates/retro-core/src/config.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Backwards-compat: old single cooldown. Maps to ingest_cooldown_minutes.
    #[serde(default = "default_ingest_cooldown")]
    pub auto_cooldown_minutes: u32,
    /// Cooldown for ingest (default 5 min). If auto_cooldown_minutes is also set,
    /// this field takes precedence when explicitly provided.
    #[serde(default = "default_ingest_cooldown")]
    pub ingest_cooldown_minutes: u32,
    #[serde(default = "default_analyze_cooldown")]
    pub analyze_cooldown_minutes: u32,
    #[serde(default = "default_apply_cooldown")]
    pub apply_cooldown_minutes: u32,
    #[serde(default = "default_auto_apply")]
    pub auto_apply: bool,
    #[serde(default = "default_post_commit")]
    pub post_commit: String,
    #[serde(default = "default_post_merge")]
    pub post_merge: String,
}

impl Default for HooksConfig {
    fn default() -> Self {
        default_hooks()
    }
}
```

Add the new default functions:

```rust
fn default_ingest_cooldown() -> u32 {
    5
}
fn default_analyze_cooldown() -> u32 {
    1440
}
fn default_apply_cooldown() -> u32 {
    1440
}
fn default_auto_apply() -> bool {
    true
}
```

Update `default_hooks()`:

```rust
fn default_hooks() -> HooksConfig {
    HooksConfig {
        auto_cooldown_minutes: default_ingest_cooldown(),
        ingest_cooldown_minutes: default_ingest_cooldown(),
        analyze_cooldown_minutes: default_analyze_cooldown(),
        apply_cooldown_minutes: default_apply_cooldown(),
        auto_apply: default_auto_apply(),
        post_commit: default_post_commit(),
        post_merge: default_post_merge(),
    }
}
```

Remove the old `default_cooldown()` function (line 130-131) — replaced by `default_ingest_cooldown()`.

**Backwards compatibility:** When `auto_cooldown_minutes` is set in old config files, serde will deserialize it into the field. The `ingest_cooldown_minutes` field will get `default_ingest_cooldown()` (5). For true backwards compat, add a post-deserialize fixup: if `auto_cooldown_minutes != default_ingest_cooldown()` and `ingest_cooldown_minutes == default_ingest_cooldown()`, copy `auto_cooldown_minutes` → `ingest_cooldown_minutes`. Implement this in a `Config::load()` post-processing step.

**Step 4: Update ingest command to use new field**

In `crates/retro-cli/src/commands/ingest.rs`, line 43, change:
```rust
// Old:
if within_cooldown(last, config.hooks.auto_cooldown_minutes) {
// New:
if within_cooldown(last, config.hooks.ingest_cooldown_minutes) {
```

And update the verbose message on line 47:
```rust
config.hooks.ingest_cooldown_minutes
```

**Step 5: Update analyze command to use new field**

In `crates/retro-cli/src/commands/analyze.rs`, find the cooldown check line and change `auto_cooldown_minutes` → `analyze_cooldown_minutes`.

**Step 6: Run tests to verify they pass**

Run: `cargo test --workspace`
Expected: All tests pass

**Step 7: Commit**

```bash
git add crates/retro-core/src/config.rs crates/retro-cli/src/commands/ingest.rs crates/retro-cli/src/commands/analyze.rs
git commit -m "feat: granular per-stage cooldowns and auto_apply config"
```

---

### Task 4: `apply --auto` Flag in CLI

**Files:**
- Modify: `crates/retro-cli/src/main.rs:58-65` (Apply command args)
- Modify: `crates/retro-cli/src/main.rs:119` (dispatch)
- Modify: `crates/retro-cli/src/commands/apply.rs:26` (run_apply signature)

**Step 1: Add `--auto` flag to Apply command**

In `crates/retro-cli/src/main.rs`, modify the `Apply` variant:

```rust
    Apply {
        /// Show what would be changed without writing files
        #[arg(long)]
        dry_run: bool,
        /// Apply patterns for all projects, not just the current one
        #[arg(long)]
        global: bool,
        /// Silent mode for git hooks: skip if locked, check cooldown, suppress output
        #[arg(long)]
        auto: bool,
    },
```

Update the dispatch (line 119):

```rust
Commands::Apply { global, dry_run, auto } => commands::apply::run(global, dry_run, auto, verbose),
```

**Step 2: Update apply command module**

Update `crates/retro-cli/src/commands/apply.rs` — change `run()` to accept `auto`:

```rust
pub fn run(global: bool, dry_run: bool, auto: bool, verbose: bool) -> Result<()> {
    if dry_run && auto {
        anyhow::bail!("--dry-run and --auto are mutually exclusive");
    }
    run_apply(global, dry_run, auto, DisplayMode::Plan { dry_run }, verbose)
}
```

And update `run_apply()` signature to accept `auto: bool`. For now, pass `false` from the `diff` command's call to `run_apply`.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles. The `auto` param is accepted but not yet used in `run_apply`.

**Step 4: Commit**

```bash
git add crates/retro-cli/src/main.rs crates/retro-cli/src/commands/apply.rs crates/retro-cli/src/commands/diff.rs
git commit -m "feat: add --auto flag to apply command"
```

---

### Task 5: Implement `apply --auto` Logic

**Files:**
- Modify: `crates/retro-cli/src/commands/apply.rs:26-189` (run_apply function)

**Step 1: Implement auto mode in `run_apply()`**

Add the auto-mode branch after the dry-run/diff early returns, before the interactive confirmation prompt. This follows the exact same pattern as `ingest.rs:30-78` and `analyze.rs:40-117`:

```rust
    // Auto mode: silent apply with cooldown + data gate
    if auto {
        let lock_path = dir.join("retro.lock");
        let _lock = match LockFile::try_acquire(&lock_path) {
            Some(lock) => lock,
            None => {
                if verbose {
                    eprintln!("[verbose] skipping apply: another process holds the lock");
                }
                return Ok(());
            }
        };

        // Cooldown check
        if let Ok(Some(ref last)) = db::last_applied_at(&conn) {
            if within_cooldown(last, config.hooks.apply_cooldown_minutes) {
                if verbose {
                    eprintln!(
                        "[verbose] skipping apply: within cooldown ({}m)",
                        config.hooks.apply_cooldown_minutes
                    );
                }
                return Ok(());
            }
        }

        // Data gate: any un-projected patterns?
        if !db::has_unprojected_patterns(&conn)? {
            if verbose {
                eprintln!("[verbose] skipping apply: no un-projected patterns");
            }
            return Ok(());
        }

        // Build and execute plan silently
        let backend = retro_core::analysis::claude_cli::ClaudeCliBackend::new(
            &config.ai.model,
            config.ai.max_budget_per_call,
        );

        match projection::build_apply_plan(&conn, &config, &backend, project.as_deref()) {
            Ok(plan) => {
                if plan.is_empty() {
                    if verbose {
                        eprintln!("[verbose] apply: no actions in plan");
                    }
                    return Ok(());
                }

                // Phase 1: Personal actions on current branch
                if let Err(e) = projection::execute_plan(
                    &conn, &config, &plan, project.as_deref(),
                    Some(&ApplyTrack::Personal),
                ) {
                    if verbose {
                        eprintln!("[verbose] apply personal error: {e}");
                    }
                }

                // Phase 2: Shared actions on new branch + PR
                if plan.shared_actions().len() > 0 {
                    if let Err(e) = execute_shared_with_pr(
                        &conn, &config, &plan, project.as_deref(), verbose,
                    ) {
                        if verbose {
                            eprintln!("[verbose] apply shared error: {e}");
                        }
                    }
                }

                if verbose {
                    eprintln!("[verbose] auto-apply complete: {} actions", plan.actions.len());
                }
            }
            Err(e) => {
                if verbose {
                    eprintln!("[verbose] apply plan error: {e}");
                }
            }
        }

        return Ok(());
    }
```

Note: this requires adding `use retro_core::lock::LockFile;` and `use super::within_cooldown;` at the top of apply.rs.

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles

**Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: All pass (no new tests here — the auto logic reuses existing tested functions)

**Step 4: Commit**

```bash
git add crates/retro-cli/src/commands/apply.rs
git commit -m "feat: implement apply --auto with lockfile, cooldown, and data gate"
```

---

### Task 6: Ingest Orchestration — Chain Analyze and Apply

**Files:**
- Modify: `crates/retro-cli/src/commands/ingest.rs:12-78` (auto mode section)

**Step 1: Write the orchestration logic**

After the existing ingest auto-mode code (line 76, after the `match result` block), add the chaining logic before `return Ok(())`:

```rust
        // --- Orchestration: chain analyze and apply if conditions met ---
        if config.hooks.auto_apply {
            // Check analyze conditions: un-analyzed sessions + cooldown elapsed
            let should_analyze = db::has_unanalyzed_sessions(&conn).unwrap_or(false)
                && match db::last_analyzed_at(&conn) {
                    Ok(Some(ref last)) => !within_cooldown(last, config.hooks.analyze_cooldown_minutes),
                    Ok(None) => true, // never analyzed before
                    Err(_) => false,
                };

            if should_analyze {
                if verbose {
                    eprintln!("[verbose] orchestrator: running analyze");
                }
                let project_path = if !global {
                    Some(git_root_or_cwd().unwrap_or_default())
                } else {
                    None
                };

                let backend = retro_core::analysis::claude_cli::ClaudeCliBackend::new(
                    &config.ai.model,
                    config.ai.max_budget_per_call,
                );

                let since = Utc::now() - chrono::Duration::days(config.analysis.window_days as i64);
                match retro_core::analysis::analyze(
                    &conn, &config, &backend, project_path.as_deref(), &since,
                ) {
                    Ok(result) => {
                        if verbose {
                            eprintln!(
                                "[verbose] analyze complete: {} patterns ({} new)",
                                result.total_patterns, result.new_patterns
                            );
                        }
                    }
                    Err(e) => {
                        if verbose {
                            eprintln!("[verbose] analyze error: {e}");
                        }
                    }
                }
            }

            // Check apply conditions: un-projected patterns + cooldown elapsed
            let should_apply = db::has_unprojected_patterns(&conn).unwrap_or(false)
                && match db::last_applied_at(&conn) {
                    Ok(Some(ref last)) => !within_cooldown(last, config.hooks.apply_cooldown_minutes),
                    Ok(None) => true, // never applied before
                    Err(_) => false,
                };

            if should_apply {
                if verbose {
                    eprintln!("[verbose] orchestrator: running apply");
                }
                // Delegate to apply command in auto mode
                match super::apply::run(global, false, true, verbose) {
                    Ok(()) => {}
                    Err(e) => {
                        if verbose {
                            eprintln!("[verbose] apply error: {e}");
                        }
                    }
                }
            }
        }

        return Ok(());
```

This requires adding imports at the top of `ingest.rs`:

```rust
use chrono::Utc;
use retro_core::db;
```

Note: `db` is already imported. Add `chrono::Utc` and the analysis/projection imports.

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles

**Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: All pass

**Step 4: Commit**

```bash
git add crates/retro-cli/src/commands/ingest.rs
git commit -m "feat: ingest --auto orchestrates analyze and apply when conditions met"
```

---

### Task 7: Terminal Nudge System

**Files:**
- Modify: `crates/retro-cli/src/commands/mod.rs` (add nudge function)
- Modify: `crates/retro-cli/src/main.rs:109-128` (call nudge before each interactive command)

**Step 1: Implement the nudge function**

Add to `crates/retro-cli/src/commands/mod.rs`:

```rust
/// Check for auto-created PRs and display a one-time nudge.
/// Silently does nothing if DB doesn't exist or any error occurs.
pub fn check_and_display_nudge() {
    let dir = retro_core::config::retro_dir();
    let db_path = dir.join("retro.db");
    if !db_path.exists() {
        return;
    }

    let conn = match retro_core::db::open_db(&db_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let urls = match retro_core::db::get_unnudged_pr_urls(&conn) {
        Ok(u) => u,
        Err(_) => return,
    };

    if urls.is_empty() {
        return;
    }

    use colored::Colorize;
    for url in &urls {
        println!(
            "  {} {}",
            "retro auto-created a PR:".yellow(),
            url.cyan().underline()
        );
    }
    println!();

    // Mark as nudged so we don't show again
    let _ = retro_core::db::mark_projections_nudged(&conn);
}
```

**Step 2: Call nudge from main dispatch**

In `crates/retro-cli/src/main.rs`, add the nudge call before the command dispatch, but only for interactive commands (not auto mode):

```rust
fn main() {
    let cli = Cli::parse();
    let verbose = cli.verbose;

    // Show nudge for interactive commands (not ingest/analyze --auto)
    let is_auto = matches!(
        &cli.command,
        Commands::Ingest { auto: true, .. } | Commands::Analyze { auto: true, .. } | Commands::Apply { auto: true, .. }
    );
    if !is_auto {
        commands::check_and_display_nudge();
    }

    let result = match cli.command {
        // ... existing dispatch ...
    };
    // ...
}
```

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles

**Step 4: Run all tests**

Run: `cargo test --workspace`
Expected: All pass

**Step 5: Commit**

```bash
git add crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs
git commit -m "feat: terminal nudge for auto-created PRs on interactive commands"
```

---

### Task 8: Hook Changes — Single Post-Commit, Drop Post-Merge

**Files:**
- Modify: `crates/retro-core/src/git.rs:133-163` (install_hooks function)

**Step 1: Write the failing test**

Add to the test module in `crates/retro-core/src/git.rs`:

```rust
#[test]
fn test_install_hooks_only_post_commit() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();

    let installed = install_hooks(dir.path().to_str().unwrap()).unwrap();

    // Should only install post-commit, not post-merge
    assert_eq!(installed, vec!["post-commit".to_string()]);

    let post_commit = std::fs::read_to_string(hooks_dir.join("post-commit")).unwrap();
    assert!(post_commit.contains("retro ingest --auto"));

    // post-merge should NOT exist
    assert!(!hooks_dir.join("post-merge").exists());
}
```

Note: add `tempfile` to retro-core's dev-dependencies if not already present:
```toml
[dev-dependencies]
tempfile = "3"
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_install_hooks_only_post_commit`
Expected: FAIL — currently installs both post-commit and post-merge

**Step 3: Update install_hooks()**

In `crates/retro-core/src/git.rs`, modify `install_hooks()` to only install post-commit:

```rust
pub fn install_hooks(repo_root: &str) -> Result<Vec<String>, CoreError> {
    let hooks_dir = Path::new(repo_root).join(".git").join("hooks");
    let mut installed = Vec::new();

    // Single post-commit hook: ingest + opportunistic analyze/apply
    let post_commit_path = hooks_dir.join("post-commit");
    if install_hook_lines(
        &post_commit_path,
        &format!("{HOOK_MARKER}\nretro ingest --auto 2>/dev/null &\n"),
    )? {
        installed.push("post-commit".to_string());
    }

    // Remove old post-merge hook if it was retro-managed
    let post_merge_path = hooks_dir.join("post-merge");
    if post_merge_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&post_merge_path) {
            if content.contains(HOOK_MARKER) {
                let cleaned = remove_hook_lines(&content);
                if cleaned.trim() == "#!/bin/sh" || cleaned.trim().is_empty() {
                    std::fs::remove_file(&post_merge_path).ok();
                } else {
                    std::fs::write(&post_merge_path, cleaned).ok();
                }
            }
        }
    }

    Ok(installed)
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_install_hooks_only_post_commit`
Expected: PASS

**Step 5: Add migration test — old post-merge gets cleaned up**

```rust
#[test]
fn test_install_hooks_removes_old_post_merge() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();

    // Simulate old retro post-merge hook
    let old_content = "#!/bin/sh\n# retro hook - do not remove\nretro analyze --auto 2>/dev/null &\n";
    std::fs::write(hooks_dir.join("post-merge"), old_content).unwrap();

    install_hooks(dir.path().to_str().unwrap()).unwrap();

    // post-merge should be removed (was retro-only)
    assert!(!hooks_dir.join("post-merge").exists());
}

#[test]
fn test_install_hooks_preserves_non_retro_post_merge() {
    let dir = tempfile::tempdir().unwrap();
    let hooks_dir = dir.path().join(".git").join("hooks");
    std::fs::create_dir_all(&hooks_dir).unwrap();

    // post-merge with retro + other content
    let mixed = "#!/bin/sh\nother-tool run\n# retro hook - do not remove\nretro analyze --auto 2>/dev/null &\n";
    std::fs::write(hooks_dir.join("post-merge"), mixed).unwrap();

    install_hooks(dir.path().to_str().unwrap()).unwrap();

    // post-merge should still exist with other-tool preserved
    let content = std::fs::read_to_string(hooks_dir.join("post-merge")).unwrap();
    assert!(content.contains("other-tool run"));
    assert!(!content.contains("retro"));
}
```

**Step 6: Run all tests**

Run: `cargo test --workspace`
Expected: All pass

**Step 7: Commit**

```bash
git add crates/retro-core/src/git.rs crates/retro-core/Cargo.toml
git commit -m "feat: single post-commit hook, remove old post-merge on init"
```

---

### Task 9: Update `retro init` Output

**Files:**
- Modify: `crates/retro-cli/src/commands/init.rs` (update user-facing messages)

**Step 1: Read current init.rs**

Read `crates/retro-cli/src/commands/init.rs` to find the hook installation output messages.

**Step 2: Update messages**

Change any references to "post-merge" hook in the init output. The message should indicate that a single post-commit hook handles the full pipeline:

```
  Installed hook: post-commit (ingest + analyze + apply)
```

Instead of the old two separate hook messages.

**Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles

**Step 4: Commit**

```bash
git add crates/retro-cli/src/commands/init.rs
git commit -m "feat: update init output to reflect single-hook pipeline"
```

---

### Task 10: Integration Test — Full Auto Pipeline

**Files:**
- Create: `crates/retro-core/src/db.rs` (add integration-style test to existing test module)

**Step 1: Write an integration-style test for the data trigger flow**

Add to the db.rs test module:

```rust
#[test]
fn test_auto_apply_data_triggers_full_flow() {
    let conn = test_db();

    // Initially: no data, no triggers
    assert!(!has_unanalyzed_sessions(&conn).unwrap());
    assert!(!has_unprojected_patterns(&conn).unwrap());
    assert!(get_unnudged_pr_urls(&conn).unwrap().is_empty());

    // Step 1: Ingest creates sessions → triggers analyze
    insert_ingested_session(&conn, "sess-1", "/proj", "/path/sess", 100, "2025-01-01T00:00:00Z").unwrap();
    assert!(has_unanalyzed_sessions(&conn).unwrap());

    // Step 2: After analysis → sessions marked, patterns created → triggers apply
    record_session_analyzed(&conn, "sess-1", "/proj").unwrap();
    assert!(!has_unanalyzed_sessions(&conn).unwrap());

    let p = test_pattern("pat-1", "Always use cargo fmt");
    insert_pattern(&conn, &p).unwrap();
    assert!(has_unprojected_patterns(&conn).unwrap());

    // Step 3: After apply → projection created with PR URL → triggers nudge
    let proj = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "Skill".to_string(),
        target_path: "/skills/cargo-fmt.md".to_string(),
        content: "skill content".to_string(),
        applied_at: Utc::now(),
        pr_url: Some("https://github.com/test/pull/42".to_string()),
    };
    insert_projection(&conn, &proj).unwrap();
    assert!(!has_unprojected_patterns(&conn).unwrap());

    // Step 4: Nudge shows PR URL, then marks as nudged
    let urls = get_unnudged_pr_urls(&conn).unwrap();
    assert_eq!(urls, vec!["https://github.com/test/pull/42"]);

    mark_projections_nudged(&conn).unwrap();
    assert!(get_unnudged_pr_urls(&conn).unwrap().is_empty());
}
```

**Step 2: Run the test**

Run: `cargo test -p retro-core test_auto_apply_data_triggers_full_flow`
Expected: PASS (all functions implemented in earlier tasks)

**Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: All pass

**Step 4: Commit**

```bash
git add crates/retro-core/src/db.rs
git commit -m "test: integration test for auto-apply data trigger flow"
```

---

### Task 11: Update CLAUDE.md and Documentation

**Files:**
- Modify: `CLAUDE.md` (update Implementation Status, Key Design Decisions)
- Modify: `PLAN.md` (add Phase 6 status if it exists)

**Step 1: Update CLAUDE.md**

Add to the Key Design Decisions section:
```
- **Auto-apply pipeline** — single post-commit hook orchestrates ingest → analyze → apply. Per-stage cooldowns (5m/24h/24h). Data triggers prevent unnecessary runs. Terminal nudge for auto-created PRs.
```

Add to Implementation Status:
```
- **Phase 6: DONE** — Auto-Apply Pipeline. Single post-commit hook, per-stage cooldowns (`ingest_cooldown_minutes`, `analyze_cooldown_minutes`, `apply_cooldown_minutes`), `auto_apply` config, `apply --auto`, terminal nudge for PR URLs, schema v2 (`projections.nudged`), old post-merge hook migration.
```

Update conventions:
```
- Auto-mode orchestration: `ingest --auto` chains analyze and apply when `auto_apply=true` and data triggers + cooldowns are satisfied
- Terminal nudge: `check_and_display_nudge()` runs before interactive commands, queries `projections.nudged=0`, marks after display
```

**Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md with auto-apply pipeline details"
```

---

### Task 12: Final Verification

**Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass (63 original + ~15 new ≈ 78+ tests)

**Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

**Step 3: Build release**

Run: `cargo build --release`
Expected: Compiles successfully

**Step 4: Smoke test**

Run: `./target/debug/retro status`
Expected: Shows status output with no errors. If there are auto-created PRs, the nudge should appear.

**Step 5: Verify --auto flag works**

Run: `./target/debug/retro apply --auto --verbose`
Expected: Either applies patterns or shows verbose skip message (cooldown/no patterns/etc.)

**Step 6: Final commit if any fixups needed**

```bash
git add -A
git commit -m "chore: final fixups for auto-apply pipeline"
```
