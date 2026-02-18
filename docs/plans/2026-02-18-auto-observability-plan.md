# Auto-Mode Observability & Session Cap Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the auto-mode pipeline observable via an enhanced nudge system, and prevent multi-minute AI calls in hooks by capping auto-analyze sessions.

**Architecture:** Expand audit log coverage to capture all auto-mode events (success, skip, error). Add a DB metadata table to track `last_nudge_at`. Expand the nudge system to read audit entries since last nudge and display a multi-line status block. Add a session count check before auto-analyze. Redirect hook stderr to a file instead of /dev/null.

**Tech Stack:** Rust, rusqlite, serde_json, chrono, colored

---

### Task 1: Add `auto_analyze_max_sessions` Config Field

**Files:**
- Modify: `crates/retro-core/src/config.rs:52-65` (HooksConfig struct)
- Modify: `crates/retro-core/src/config.rs:97-106` (default_hooks factory)
- Modify: `crates/retro-core/src/config.rs:139-156` (default functions)

**Step 1: Write the failing test**

Add to `crates/retro-core/src/config.rs` in the existing `#[cfg(test)]` module:

```rust
#[test]
fn test_hooks_config_max_sessions_default() {
    let config = Config::default();
    assert_eq!(config.hooks.auto_analyze_max_sessions, 15);
}

#[test]
fn test_hooks_config_max_sessions_custom() {
    let toml_str = r#"
[hooks]
auto_analyze_max_sessions = 5
"#;
    let config: Config = toml::from_str(toml_str).unwrap();
    assert_eq!(config.hooks.auto_analyze_max_sessions, 5);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_hooks_config_max_sessions`
Expected: FAIL — field doesn't exist

**Step 3: Write minimal implementation**

In `config.rs`:

1. Add default function (after line 156):
```rust
fn default_auto_analyze_max_sessions() -> u32 {
    15
}
```

2. Add field to `HooksConfig` struct (after `post_merge` field, line 64):
```rust
    #[serde(default = "default_auto_analyze_max_sessions")]
    pub auto_analyze_max_sessions: u32,
```

3. Add to `default_hooks()` factory (after `post_merge` line, ~line 104):
```rust
        auto_analyze_max_sessions: default_auto_analyze_max_sessions(),
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_hooks_config_max_sessions`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/retro-core/src/config.rs
git commit -m "feat: add auto_analyze_max_sessions config field (default 15)"
```

---

### Task 2: Add DB Metadata Table for `last_nudge_at`

**Files:**
- Modify: `crates/retro-core/src/db.rs:8` (SCHEMA_VERSION)
- Modify: `crates/retro-core/src/db.rs:23-85` (migrate function)
- Add functions to `crates/retro-core/src/db.rs`

**Step 1: Write the failing tests**

Add to the `#[cfg(test)]` module in `db.rs`:

```rust
#[test]
fn test_get_last_nudge_at_empty() {
    let conn = test_db();
    assert!(get_last_nudge_at(&conn).unwrap().is_none());
}

#[test]
fn test_set_and_get_last_nudge_at() {
    let conn = test_db();
    let now = Utc::now();
    set_last_nudge_at(&conn, &now).unwrap();
    let result = get_last_nudge_at(&conn).unwrap().unwrap();
    // Compare to second precision (DB stores RFC 3339)
    assert_eq!(
        result.format("%Y-%m-%dT%H:%M:%S").to_string(),
        now.format("%Y-%m-%dT%H:%M:%S").to_string()
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_last_nudge`
Expected: FAIL — functions don't exist

**Step 3: Write minimal implementation**

1. Change `SCHEMA_VERSION` (line 8):
```rust
const SCHEMA_VERSION: u32 = 2;
```

2. Add migration block in `migrate()` after the `current_version < 1` block (after line 82):
```rust
    if current_version < 2 {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;
        conn.pragma_update(None, "user_version", 2)?;
    }
```

3. Add DB functions (near the other `last_*_at` functions, after line 200):
```rust
/// Get the last nudge timestamp from metadata.
pub fn get_last_nudge_at(conn: &Connection) -> Result<Option<DateTime<Utc>>, CoreError> {
    let result: Option<String> = conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'last_nudge_at'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    match result {
        Some(s) => match DateTime::parse_from_rfc3339(&s) {
            Ok(dt) => Ok(Some(dt.with_timezone(&Utc))),
            Err(_) => Ok(None),
        },
        None => Ok(None),
    }
}

/// Set the last nudge timestamp in metadata.
pub fn set_last_nudge_at(conn: &Connection, timestamp: &DateTime<Utc>) -> Result<(), CoreError> {
    conn.execute(
        "INSERT OR REPLACE INTO metadata (key, value) VALUES ('last_nudge_at', ?1)",
        params![timestamp.to_rfc3339()],
    )?;
    Ok(())
}
```

Note: Add `use rusqlite::OptionalExtension;` at the top of `db.rs` if not already imported (needed for `.optional()`).

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_last_nudge`
Expected: PASS

**Step 5: Run all tests to verify migration doesn't break existing**

Run: `cargo test`
Expected: All tests pass

**Step 6: Commit**

```bash
git add crates/retro-core/src/db.rs
git commit -m "feat: add metadata table with last_nudge_at tracking (schema v2)"
```

---

### Task 3: Add Unanalyzed Session Count Function

**Files:**
- Modify: `crates/retro-core/src/db.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_unanalyzed_session_count() {
    let conn = test_db();
    assert_eq!(unanalyzed_session_count(&conn).unwrap(), 0);

    // Add 3 sessions
    for i in 1..=3 {
        let session = IngestedSession {
            session_id: format!("sess-{i}"),
            project: "/proj".to_string(),
            session_path: format!("/path/sess-{i}"),
            file_size: 100,
            file_mtime: "2025-01-01T00:00:00Z".to_string(),
            ingested_at: Utc::now(),
        };
        record_ingested_session(&conn, &session).unwrap();
    }
    assert_eq!(unanalyzed_session_count(&conn).unwrap(), 3);

    // Analyze one
    record_analyzed_session(&conn, "sess-1", "/proj").unwrap();
    assert_eq!(unanalyzed_session_count(&conn).unwrap(), 2);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_unanalyzed_session_count`
Expected: FAIL

**Step 3: Write minimal implementation**

Add after `has_unanalyzed_sessions` (near line 212):

```rust
/// Count ingested sessions that haven't been analyzed yet.
pub fn unanalyzed_session_count(conn: &Connection) -> Result<u64, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM ingested_sessions i
         LEFT JOIN analyzed_sessions a ON i.session_id = a.session_id
         WHERE a.session_id IS NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(count)
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p retro-core test_unanalyzed_session_count`
Expected: PASS

**Step 5: Commit**

```bash
git add crates/retro-core/src/db.rs
git commit -m "feat: add unanalyzed_session_count() for auto-mode cap check"
```

---

### Task 4: Session Cap Check in Orchestration

**Files:**
- Modify: `crates/retro-cli/src/commands/ingest.rs:114-165`

**Step 1: Add session cap check before analyze call**

In `ingest.rs`, after the `should_analyze` check (line 122) and before the `if should_analyze {` block (line 124), add the cap check. Replace the existing analyze block:

```rust
            if should_analyze {
                // Check session cap for auto mode
                let unanalyzed_count = db::unanalyzed_session_count(&conn).unwrap_or(0);
                let cap = config.hooks.auto_analyze_max_sessions;

                if unanalyzed_count > cap as u64 {
                    if verbose {
                        eprintln!(
                            "[verbose] orchestrator: skipping analyze — {} unanalyzed sessions exceeds auto limit ({})",
                            unanalyzed_count, cap
                        );
                    }
                    let _ = audit_log::append(
                        &audit_path,
                        "analyze_skipped",
                        serde_json::json!({
                            "reason": "session_cap",
                            "unanalyzed_count": unanalyzed_count,
                            "cap": cap,
                            "auto": true,
                        }),
                    );
                } else {
                    // Existing analyze logic (lines 125-162)
                    if verbose {
                        eprintln!("[verbose] orchestrator: running analyze");
                    }
                    // ... (keep existing analyze call and audit logging unchanged)
                }
            }
```

**Step 2: Build and verify**

Run: `cargo build`
Expected: compiles

**Step 3: Test manually**

Run: `retro ingest --auto --verbose 2>&1`
Expected: If sessions > cap, shows skip message. If not, runs normally.

**Step 4: Commit**

```bash
git add crates/retro-cli/src/commands/ingest.rs
git commit -m "feat: add session cap check — skip auto-analyze when too many sessions"
```

---

### Task 5: Comprehensive Audit Logging in Orchestration

**Files:**
- Modify: `crates/retro-cli/src/commands/ingest.rs:59-205`

This task adds audit entries for every auto-mode decision. The key events:

**Step 1: Add ingest success audit entry**

After the successful ingest match arm (line 67-75), add:

```rust
                Ok(r) => {
                    if verbose {
                        eprintln!(
                            "[verbose] ingested {} sessions ({} skipped)",
                            r.sessions_ingested, r.sessions_skipped
                        );
                    }
                    // Audit: ingest success
                    let _ = audit_log::append(
                        &audit_path,
                        "ingest",
                        serde_json::json!({
                            "sessions_ingested": r.sessions_ingested,
                            "sessions_skipped": r.sessions_skipped,
                            "auto": true,
                            "project": project_path_for_audit,
                        }),
                    );
                }
```

Note: `audit_path` is currently defined inside the orchestration block (line 98). Move its definition earlier (before the ingest block) so it's accessible. Similarly, capture `project_path_for_audit` from `git_root_or_cwd()` before the ingest call.

**Step 2: Add analyze error audit entry**

In the analyze error match arm (currently line 157-161):

```rust
                    Err(e) => {
                        if verbose {
                            eprintln!("[verbose] analyze error: {e}");
                        }
                        let _ = audit_log::append(
                            &audit_path,
                            "analyze_error",
                            serde_json::json!({
                                "error": e.to_string(),
                                "auto": true,
                            }),
                        );
                    }
```

**Step 3: Add analyze cooldown skip audit entry**

In the `else if verbose` branch after `should_analyze` (line 163-165):

```rust
            } else {
                if verbose {
                    eprintln!("[verbose] orchestrator: skipping analyze (no unanalyzed sessions or within cooldown)");
                }
                let _ = audit_log::append(
                    &audit_path,
                    "analyze_skipped",
                    serde_json::json!({
                        "reason": "cooldown_or_no_data",
                        "auto": true,
                    }),
                );
            }
```

**Step 4: Add apply skip audit entry**

In the apply `else if verbose` branch (line 198-200):

```rust
            } else {
                if verbose {
                    eprintln!("[verbose] orchestrator: skipping apply (no unprojected patterns or within cooldown)");
                }
                let _ = audit_log::append(
                    &audit_path,
                    "apply_skipped",
                    serde_json::json!({
                        "reason": "no_qualifying_patterns",
                        "auto": true,
                    }),
                );
            }
```

**Step 5: Add apply error audit entry in apply.rs**

In `crates/retro-cli/src/commands/apply.rs`, in the auto-mode error branches (lines 110-113, 119-123, 138-142), add audit entries:

```rust
                Err(e) => {
                    if verbose {
                        eprintln!("[verbose] apply plan error: {e}");
                    }
                    let _ = audit_log::append(
                        &audit_path,
                        "apply_error",
                        serde_json::json!({
                            "error": e.to_string(),
                            "auto": true,
                        }),
                    );
                }
```

**Step 6: Enrich existing apply success audit entry**

In `apply.rs` line 126-132, the existing audit entry already includes `actions`. Add `files_written`, `patterns_activated`, and `pr_url` if available. This requires capturing results from the execute phases.

**Step 7: Build and verify**

Run: `cargo build`
Expected: compiles

**Step 8: Commit**

```bash
git add crates/retro-cli/src/commands/ingest.rs crates/retro-cli/src/commands/apply.rs
git commit -m "feat: comprehensive audit logging for all auto-mode events"
```

---

### Task 6: Enhanced Nudge System

**Files:**
- Modify: `crates/retro-cli/src/commands/mod.rs:36-69` (check_and_display_nudge)
- Reference: `crates/retro-core/src/audit_log.rs:38-75` (read_entries)

**Step 1: Write helper to aggregate audit entries into a status block**

Create a struct for the aggregated auto-run summary. Add to `commands/mod.rs`:

```rust
/// Summary of one auto-mode run (entries within 60s window).
struct AutoRunSummary {
    timestamp: DateTime<Utc>,
    sessions_ingested: Option<u64>,
    sessions_skipped: Option<u64>,
    sessions_analyzed: Option<u64>,
    new_patterns: Option<u64>,
    actions_applied: Option<u64>,
    pr_url: Option<String>,
    analyze_skipped_reason: Option<String>,
    unanalyzed_count: Option<u64>,
    session_cap: Option<u64>,
    errors: Vec<String>,
}
```

**Step 2: Write aggregation function**

```rust
/// Group audit entries into auto-run summaries (entries within 60s = one run).
fn aggregate_auto_runs(entries: &[AuditEntry]) -> Vec<AutoRunSummary> {
    // Filter to auto entries only
    let auto_entries: Vec<&AuditEntry> = entries
        .iter()
        .filter(|e| e.details.get("auto").and_then(|v| v.as_bool()).unwrap_or(false))
        .collect();

    if auto_entries.is_empty() {
        return Vec::new();
    }

    let mut runs: Vec<AutoRunSummary> = Vec::new();
    let mut current = AutoRunSummary { /* defaults */ };
    let mut current_start = auto_entries[0].timestamp;

    for entry in &auto_entries {
        // New run if gap > 60s
        if (entry.timestamp - current_start).num_seconds() > 60 {
            runs.push(current);
            current = AutoRunSummary { /* defaults */ };
            current_start = entry.timestamp;
        }
        current.timestamp = entry.timestamp;

        // Populate fields based on action type
        match entry.action.as_str() {
            "ingest" => {
                current.sessions_ingested = entry.details.get("sessions_ingested").and_then(|v| v.as_u64());
                current.sessions_skipped = entry.details.get("sessions_skipped").and_then(|v| v.as_u64());
            }
            "analyze" => {
                current.sessions_analyzed = entry.details.get("sessions_analyzed").and_then(|v| v.as_u64());
                current.new_patterns = entry.details.get("new_patterns").and_then(|v| v.as_u64());
            }
            "apply" => {
                current.actions_applied = entry.details.get("actions").and_then(|v| v.as_u64());
                current.pr_url = entry.details.get("pr_url").and_then(|v| v.as_str()).map(|s| s.to_string());
            }
            "analyze_skipped" => {
                current.analyze_skipped_reason = entry.details.get("reason").and_then(|v| v.as_str()).map(|s| s.to_string());
                current.unanalyzed_count = entry.details.get("unanalyzed_count").and_then(|v| v.as_u64());
                current.session_cap = entry.details.get("cap").and_then(|v| v.as_u64());
            }
            "analyze_error" | "apply_error" => {
                if let Some(err) = entry.details.get("error").and_then(|v| v.as_str()) {
                    current.errors.push(format!("{}: {}", entry.action, err));
                }
            }
            _ => {} // skip cooldown skips, etc.
        }
    }
    runs.push(current);

    // Filter out runs that only have cooldown skips (nothing interesting)
    runs.into_iter()
        .filter(|r| {
            r.sessions_ingested.is_some()
                || r.sessions_analyzed.is_some()
                || r.actions_applied.is_some()
                || r.analyze_skipped_reason.is_some()
                || !r.errors.is_empty()
        })
        .collect()
}
```

**Step 3: Write display function**

```rust
fn display_auto_run(run: &AutoRunSummary) {
    use colored::Colorize;
    let ago = format_time_ago(run.timestamp);

    println!("  {}", format!("--- retro auto-run ({ago}) ---").dimmed());

    if let Some(n) = run.sessions_ingested {
        println!("  {}  {} sessions", "Ingested:".white(), n.to_string().cyan());
    }

    if let Some(n) = run.sessions_analyzed {
        let pattern_info = match run.new_patterns {
            Some(p) if p > 0 => format!(" -> {} new patterns", p),
            _ => String::new(),
        };
        println!("  {} {} sessions{}", "Analyzed:".white(), n.to_string().cyan(), pattern_info.green());
    }

    if let Some(ref reason) = run.analyze_skipped_reason {
        if reason == "session_cap" {
            let count = run.unanalyzed_count.unwrap_or(0);
            let cap = run.session_cap.unwrap_or(0);
            println!(
                "  {} {} unanalyzed sessions exceeds auto limit ({})",
                "! Skipped:".yellow(),
                count.to_string().yellow(),
                cap
            );
            println!("  {}", "  Run `retro analyze` to process them.".dimmed());
        }
    }

    if let Some(n) = run.actions_applied {
        if n > 0 {
            println!("  {}  {} actions", "Applied:".white(), n.to_string().green());
        }
    }

    if let Some(ref url) = run.pr_url {
        println!("  {} {}", "PR created:".white(), url.cyan().underline());
    }

    for err in &run.errors {
        println!("  {} {}", "Error:".red(), err);
    }

    println!();
}

fn format_time_ago(timestamp: DateTime<Utc>) -> String {
    let diff = Utc::now() - timestamp;
    if diff.num_hours() > 0 {
        format!("{}h ago", diff.num_hours())
    } else if diff.num_minutes() > 0 {
        format!("{}m ago", diff.num_minutes())
    } else {
        "just now".to_string()
    }
}
```

**Step 4: Rewrite `check_and_display_nudge()`**

Replace the existing function in `commands/mod.rs`:

```rust
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

    // Get last nudge timestamp
    let since = match retro_core::db::get_last_nudge_at(&conn) {
        Ok(Some(ts)) => Some(ts),
        _ => None,
    };

    // Read audit entries since last nudge
    let audit_path = dir.join("audit.jsonl");
    let entries = match retro_core::audit_log::read_entries(&audit_path, since.as_ref()) {
        Ok(e) => e,
        Err(_) => return,
    };

    let runs = aggregate_auto_runs(&entries);
    if runs.is_empty() {
        return;
    }

    for run in &runs {
        display_auto_run(run);
    }

    // Update last nudge timestamp
    let _ = retro_core::db::set_last_nudge_at(&conn, &Utc::now());
}
```

**Step 5: Build and verify**

Run: `cargo build`
Expected: compiles

**Step 6: Manual test**

Run `retro ingest --auto --verbose 2>&1` then `retro status` — should see the status block before the status output.

**Step 7: Commit**

```bash
git add crates/retro-cli/src/commands/mod.rs
git commit -m "feat: enhanced nudge system — multi-line auto-run status block"
```

---

### Task 7: Update Hook Script to Redirect stderr

**Files:**
- Modify: `crates/retro-core/src/git.rs:230`

**Step 1: Update the hook script string**

Change line 230 from:
```rust
&format!("{HOOK_MARKER}\nretro ingest --auto 2>/dev/null &\n"),
```
to:
```rust
&format!("{HOOK_MARKER}\nretro ingest --auto 2>>~/.retro/hook-stderr.log &\n"),
```

**Step 2: Update hook installation test**

Find the test that checks hook content (in `git.rs` tests) and update the expected string.

**Step 3: Build and run tests**

Run: `cargo test -p retro-core`
Expected: PASS

**Step 4: Commit**

```bash
git add crates/retro-core/src/git.rs
git commit -m "feat: redirect hook stderr to ~/.retro/hook-stderr.log instead of /dev/null"
```

---

### Task 8: Hook Stderr Log Cleanup in `retro init`

**Files:**
- Modify: `crates/retro-cli/src/commands/init.rs`

**Step 1: Add truncation of hook-stderr.log on init**

In the `retro init` command, after creating the `~/.retro/` directory, add:

```rust
// Truncate hook stderr log on fresh init
let hook_stderr_path = dir.join("hook-stderr.log");
if hook_stderr_path.exists() {
    let _ = std::fs::write(&hook_stderr_path, "");
}
```

**Step 2: Build and verify**

Run: `cargo build`
Expected: compiles

**Step 3: Commit**

```bash
git add crates/retro-cli/src/commands/init.rs
git commit -m "feat: truncate hook-stderr.log on retro init"
```

---

### Task 9: Update Existing Hooks via `retro init`

**Files:**
- Modify: `crates/retro-cli/src/commands/init.rs`

Users with existing hooks still have the old `2>/dev/null` redirect. `retro init` already calls `install_hooks()` which uses `install_hook_lines()`. Check that `install_hook_lines` replaces the old hook content (marker-based detection) rather than appending. If it detects the marker already exists, it should replace the old retro lines with the new redirect.

**Step 1: Read `install_hook_lines` to verify replacement behavior**

Check `crates/retro-core/src/git.rs:252` — if it skips when marker exists, we need to add a force-update path (remove old lines + re-add new ones).

**Step 2: If needed, update to replace old hook lines**

The function should:
1. Read existing hook content
2. If marker found, remove old retro lines (`remove_hook_lines`)
3. Add new retro lines with updated redirect
4. Write back

**Step 3: Build and run tests**

Run: `cargo test`
Expected: All pass

**Step 4: Commit**

```bash
git add crates/retro-core/src/git.rs crates/retro-cli/src/commands/init.rs
git commit -m "feat: retro init updates existing hooks to new stderr redirect"
```

---

### Task 10: Final Integration Test & Cleanup

**Step 1: Run full test suite**

Run: `cargo test`
Expected: All pass

**Step 2: Install and manual end-to-end test**

```bash
cargo install --path crates/retro-cli
```

Test sequence:
1. `retro init` in a test repo — verify hook has `2>>~/.retro/hook-stderr.log`
2. Make a commit — verify `~/.retro/audit.jsonl` has ingest entry
3. Run `retro status` — verify nudge block appears
4. Run `retro status` again — verify nudge doesn't repeat

**Step 3: Commit any final fixes**

```bash
git add -A
git commit -m "fix: integration test fixes for auto-mode observability"
```
