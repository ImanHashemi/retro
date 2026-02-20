# Review Queue Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a review queue between content generation and execution, so users can approve/skip/dismiss retro's suggestions before anything is written to disk or pushed as a PR.

**Architecture:** New `ProjectionStatus` enum (`PendingReview`/`Applied`/`Dismissed`) added to DB schema v3. `retro apply` saves generated content as `PendingReview` instead of writing files. New `retro review` command lets users batch-approve items. New `retro sync` command checks closed PRs and resets patterns. Nudge updated to show pending review count.

**Tech Stack:** Rust, rusqlite, clap, colored, serde_json. No new dependencies.

**Design Doc:** `docs/plans/2026-02-20-review-queue-design.md`

---

### Task 1: Add `ProjectionStatus` enum and update `Projection` model

**Files:**
- Modify: `crates/retro-core/src/models.rs:559-571`

**Step 1: Write the failing test**

Add to `crates/retro-core/src/models.rs` at the bottom of the file (there are no existing tests in this file, so add a test module):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_projection_status_display() {
        assert_eq!(ProjectionStatus::PendingReview.to_string(), "pending_review");
        assert_eq!(ProjectionStatus::Applied.to_string(), "applied");
        assert_eq!(ProjectionStatus::Dismissed.to_string(), "dismissed");
    }

    #[test]
    fn test_projection_status_from_str() {
        assert_eq!(ProjectionStatus::from_str("pending_review"), Some(ProjectionStatus::PendingReview));
        assert_eq!(ProjectionStatus::from_str("applied"), Some(ProjectionStatus::Applied));
        assert_eq!(ProjectionStatus::from_str("dismissed"), Some(ProjectionStatus::Dismissed));
        assert_eq!(ProjectionStatus::from_str("unknown"), None);
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_projection_status -- --nocapture`
Expected: Compilation error — `ProjectionStatus` doesn't exist yet.

**Step 3: Write the implementation**

In `crates/retro-core/src/models.rs`, add after the `Projection` struct (after line 571):

```rust
/// Status of a projection in the review queue.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionStatus {
    PendingReview,
    Applied,
    Dismissed,
}

impl std::fmt::Display for ProjectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PendingReview => write!(f, "pending_review"),
            Self::Applied => write!(f, "applied"),
            Self::Dismissed => write!(f, "dismissed"),
        }
    }
}

impl ProjectionStatus {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending_review" => Some(Self::PendingReview),
            "applied" => Some(Self::Applied),
            "dismissed" => Some(Self::Dismissed),
            _ => None,
        }
    }
}
```

Also update the `Projection` struct to include the status field:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Projection {
    pub id: String,
    pub pattern_id: String,
    pub target_type: String,
    pub target_path: String,
    pub content: String,
    pub applied_at: DateTime<Utc>,
    pub pr_url: Option<String>,
    pub status: ProjectionStatus,
}
```

**Step 4: Fix all compilation errors**

Every place that constructs a `Projection` must now include `status`. Search for `Projection {` across the codebase. Key locations:
- `crates/retro-core/src/projection/mod.rs:268-276` (`record_projection` fn) — set `status: ProjectionStatus::PendingReview`
- `crates/retro-core/src/db.rs:652-665` (`get_projections_for_active_patterns`) — read status from DB
- `crates/retro-core/src/db.rs` — all test functions that create `Projection` values (around lines 814, 867, 915, 924, 1007, 1072)

**Step 5: Run tests to verify they pass**

Run: `cargo test -p retro-core`
Expected: All tests pass including new `test_projection_status_*` tests.

**Step 6: Commit**

```bash
git add crates/retro-core/src/models.rs
git commit -m "feat: add ProjectionStatus enum and status field to Projection"
```

---

### Task 2: DB schema v3 migration — add `status` column to projections

**Files:**
- Modify: `crates/retro-core/src/db.rs:9` (SCHEMA_VERSION)
- Modify: `crates/retro-core/src/db.rs:85-95` (add v3 migration block)
- Modify: `crates/retro-core/src/db.rs:574-590` (insert_projection — include status)
- Modify: `crates/retro-core/src/db.rs:640-671` (get_projections — read status)

**Step 1: Write the failing test**

Add to `crates/retro-core/src/db.rs` tests module:

```rust
#[test]
fn test_projection_status_column_exists() {
    let conn = test_db();
    let pattern = test_pattern("pat-1", "Test");
    insert_pattern(&conn, &pattern).unwrap();

    let proj = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "skill".to_string(),
        target_path: "/test/skill.md".to_string(),
        content: "content".to_string(),
        applied_at: Utc::now(),
        pr_url: None,
        status: ProjectionStatus::PendingReview,
    };
    insert_projection(&conn, &proj).unwrap();

    let status: String = conn
        .query_row(
            "SELECT status FROM projections WHERE id = 'proj-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "pending_review");
}

#[test]
fn test_existing_projections_default_to_applied() {
    // Simulate a v2 database with existing projections
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();

    // Create v1 schema manually
    conn.execute_batch(
        "CREATE TABLE patterns (
            id TEXT PRIMARY KEY, pattern_type TEXT NOT NULL, description TEXT NOT NULL,
            confidence REAL NOT NULL, times_seen INTEGER NOT NULL DEFAULT 1,
            first_seen TEXT NOT NULL, last_seen TEXT NOT NULL, last_projected TEXT,
            status TEXT NOT NULL DEFAULT 'discovered', source_sessions TEXT NOT NULL,
            related_files TEXT NOT NULL, suggested_content TEXT NOT NULL,
            suggested_target TEXT NOT NULL, project TEXT,
            generation_failed INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE projections (
            id TEXT PRIMARY KEY, pattern_id TEXT NOT NULL REFERENCES patterns(id),
            target_type TEXT NOT NULL, target_path TEXT NOT NULL, content TEXT NOT NULL,
            applied_at TEXT NOT NULL, pr_url TEXT, nudged INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE analyzed_sessions (session_id TEXT PRIMARY KEY, project TEXT NOT NULL, analyzed_at TEXT NOT NULL);
        CREATE TABLE ingested_sessions (session_id TEXT PRIMARY KEY, project TEXT NOT NULL, session_path TEXT NOT NULL, file_size INTEGER NOT NULL, file_mtime TEXT NOT NULL, ingested_at TEXT NOT NULL);
        PRAGMA user_version = 1;",
    ).unwrap();

    // Insert an old-style projection (no status column)
    conn.execute(
        "INSERT INTO projections (id, pattern_id, target_type, target_path, content, applied_at)
         VALUES ('proj-old', 'pat-1', 'skill', '/path', 'content', '2026-01-01T00:00:00Z')",
        [],
    ).unwrap();

    // Now run migration (open_db equivalent)
    migrate(&conn).unwrap();

    // Old projection should have status = 'applied'
    let status: String = conn
        .query_row("SELECT status FROM projections WHERE id = 'proj-old'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(status, "applied");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p retro-core test_projection_status_column -- --nocapture`
Expected: Fails because `status` column doesn't exist in schema yet.

**Step 3: Write the implementation**

In `crates/retro-core/src/db.rs`:

1. Change `SCHEMA_VERSION` from `2` to `3` (line 9)

2. Add v3 migration after the v2 block (after line 95):
```rust
if current_version < 3 {
    conn.execute_batch(
        "ALTER TABLE projections ADD COLUMN status TEXT NOT NULL DEFAULT 'applied';",
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
}
```

3. Update `insert_projection` (line 574-590) to include status:
```rust
pub fn insert_projection(conn: &Connection, proj: &Projection) -> Result<(), CoreError> {
    conn.execute(
        "INSERT INTO projections (id, pattern_id, target_type, target_path, content, applied_at, pr_url, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            proj.id,
            proj.pattern_id,
            proj.target_type,
            proj.target_path,
            proj.content,
            proj.applied_at.to_rfc3339(),
            proj.pr_url,
            proj.status.to_string(),
        ],
    )?;
    Ok(())
}
```

4. Update `get_projections_for_active_patterns` (line 640-671) to read status:
```rust
pub fn get_projections_for_active_patterns(
    conn: &Connection,
) -> Result<Vec<Projection>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.pattern_id, p.target_type, p.target_path, p.content, p.applied_at, p.pr_url, p.status
         FROM projections p
         INNER JOIN patterns pat ON p.pattern_id = pat.id
         WHERE pat.status = 'active'",
    )?;

    let projections = stmt
        .query_map([], |row| {
            let applied_at_str: String = row.get(5)?;
            let applied_at = DateTime::parse_from_rfc3339(&applied_at_str)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let status_str: String = row.get(7)?;
            let status = ProjectionStatus::from_str(&status_str)
                .unwrap_or(ProjectionStatus::Applied);
            Ok(Projection {
                id: row.get(0)?,
                pattern_id: row.get(1)?,
                target_type: row.get(2)?,
                target_path: row.get(3)?,
                content: row.get(4)?,
                applied_at,
                pr_url: row.get(6)?,
                status,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(projections)
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p retro-core`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add crates/retro-core/src/db.rs
git commit -m "feat: DB schema v3 — add status column to projections table"
```

---

### Task 3: New DB queries for review queue

**Files:**
- Modify: `crates/retro-core/src/db.rs`

**Step 1: Write the failing tests**

Add to `crates/retro-core/src/db.rs` tests:

```rust
#[test]
fn test_get_pending_review_projections() {
    let conn = test_db();
    let p1 = test_pattern("pat-1", "Pattern one");
    let p2 = test_pattern("pat-2", "Pattern two");
    insert_pattern(&conn, &p1).unwrap();
    insert_pattern(&conn, &p2).unwrap();

    // One pending, one applied
    let proj1 = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "skill".to_string(),
        target_path: "/test/a.md".to_string(),
        content: "content a".to_string(),
        applied_at: Utc::now(),
        pr_url: None,
        status: ProjectionStatus::PendingReview,
    };
    let proj2 = Projection {
        id: "proj-2".to_string(),
        pattern_id: "pat-2".to_string(),
        target_type: "skill".to_string(),
        target_path: "/test/b.md".to_string(),
        content: "content b".to_string(),
        applied_at: Utc::now(),
        pr_url: None,
        status: ProjectionStatus::Applied,
    };
    insert_projection(&conn, &proj1).unwrap();
    insert_projection(&conn, &proj2).unwrap();

    let pending = get_pending_review_projections(&conn).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, "proj-1");
}

#[test]
fn test_update_projection_status() {
    let conn = test_db();
    let p = test_pattern("pat-1", "Pattern");
    insert_pattern(&conn, &p).unwrap();

    let proj = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "skill".to_string(),
        target_path: "/test.md".to_string(),
        content: "content".to_string(),
        applied_at: Utc::now(),
        pr_url: None,
        status: ProjectionStatus::PendingReview,
    };
    insert_projection(&conn, &proj).unwrap();

    update_projection_status(&conn, "proj-1", &ProjectionStatus::Applied).unwrap();

    let status: String = conn
        .query_row("SELECT status FROM projections WHERE id = 'proj-1'", [], |row| row.get(0))
        .unwrap();
    assert_eq!(status, "applied");
}

#[test]
fn test_delete_projection() {
    let conn = test_db();
    let p = test_pattern("pat-1", "Pattern");
    insert_pattern(&conn, &p).unwrap();

    let proj = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "skill".to_string(),
        target_path: "/test.md".to_string(),
        content: "content".to_string(),
        applied_at: Utc::now(),
        pr_url: None,
        status: ProjectionStatus::PendingReview,
    };
    insert_projection(&conn, &proj).unwrap();
    assert!(has_projection_for_pattern(&conn, "pat-1").unwrap());

    delete_projection(&conn, "proj-1").unwrap();
    assert!(!has_projection_for_pattern(&conn, "pat-1").unwrap());
}

#[test]
fn test_get_projections_with_pr_url() {
    let conn = test_db();
    let p1 = test_pattern("pat-1", "Pattern one");
    let p2 = test_pattern("pat-2", "Pattern two");
    insert_pattern(&conn, &p1).unwrap();
    insert_pattern(&conn, &p2).unwrap();

    let proj1 = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "skill".to_string(),
        target_path: "/a.md".to_string(),
        content: "a".to_string(),
        applied_at: Utc::now(),
        pr_url: Some("https://github.com/test/pull/1".to_string()),
        status: ProjectionStatus::Applied,
    };
    let proj2 = Projection {
        id: "proj-2".to_string(),
        pattern_id: "pat-2".to_string(),
        target_type: "skill".to_string(),
        target_path: "/b.md".to_string(),
        content: "b".to_string(),
        applied_at: Utc::now(),
        pr_url: None,
        status: ProjectionStatus::Applied,
    };
    insert_projection(&conn, &proj1).unwrap();
    insert_projection(&conn, &proj2).unwrap();

    let with_pr = get_applied_projections_with_pr(&conn).unwrap();
    assert_eq!(with_pr.len(), 1);
    assert_eq!(with_pr[0].pr_url, Some("https://github.com/test/pull/1".to_string()));
}

#[test]
fn test_get_projected_pattern_ids_by_status() {
    let conn = test_db();
    let p1 = test_pattern("pat-1", "Pattern one");
    let p2 = test_pattern("pat-2", "Pattern two");
    insert_pattern(&conn, &p1).unwrap();
    insert_pattern(&conn, &p2).unwrap();

    let proj1 = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "skill".to_string(),
        target_path: "/a.md".to_string(),
        content: "a".to_string(),
        applied_at: Utc::now(),
        pr_url: None,
        status: ProjectionStatus::Applied,
    };
    let proj2 = Projection {
        id: "proj-2".to_string(),
        pattern_id: "pat-2".to_string(),
        target_type: "skill".to_string(),
        target_path: "/b.md".to_string(),
        content: "b".to_string(),
        applied_at: Utc::now(),
        pr_url: None,
        status: ProjectionStatus::PendingReview,
    };
    insert_projection(&conn, &proj1).unwrap();
    insert_projection(&conn, &proj2).unwrap();

    let ids = get_projected_pattern_ids_by_status(&conn, &[ProjectionStatus::Applied, ProjectionStatus::PendingReview]).unwrap();
    assert_eq!(ids.len(), 2);

    let ids_applied_only = get_projected_pattern_ids_by_status(&conn, &[ProjectionStatus::Applied]).unwrap();
    assert_eq!(ids_applied_only.len(), 1);
    assert!(ids_applied_only.contains("pat-1"));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p retro-core test_get_pending_review -- --nocapture`
Expected: Compilation error — functions don't exist.

**Step 3: Write the implementation**

Add to `crates/retro-core/src/db.rs` in the projection operations section (after `get_projected_pattern_ids`):

```rust
/// Get all projections with pending_review status.
pub fn get_pending_review_projections(conn: &Connection) -> Result<Vec<Projection>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.pattern_id, p.target_type, p.target_path, p.content, p.applied_at, p.pr_url, p.status
         FROM projections p
         WHERE p.status = 'pending_review'
         ORDER BY p.applied_at ASC",
    )?;

    let projections = stmt
        .query_map([], |row| {
            let applied_at_str: String = row.get(5)?;
            let applied_at = DateTime::parse_from_rfc3339(&applied_at_str)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let status_str: String = row.get(7)?;
            let status = ProjectionStatus::from_str(&status_str)
                .unwrap_or(ProjectionStatus::PendingReview);
            Ok(Projection {
                id: row.get(0)?,
                pattern_id: row.get(1)?,
                target_type: row.get(2)?,
                target_path: row.get(3)?,
                content: row.get(4)?,
                applied_at,
                pr_url: row.get(6)?,
                status,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(projections)
}

/// Update a projection's status.
pub fn update_projection_status(
    conn: &Connection,
    projection_id: &str,
    status: &ProjectionStatus,
) -> Result<(), CoreError> {
    conn.execute(
        "UPDATE projections SET status = ?2 WHERE id = ?1",
        params![projection_id, status.to_string()],
    )?;
    Ok(())
}

/// Delete a projection record.
pub fn delete_projection(conn: &Connection, projection_id: &str) -> Result<(), CoreError> {
    conn.execute("DELETE FROM projections WHERE id = ?1", params![projection_id])?;
    Ok(())
}

/// Get applied projections that have a PR URL (for sync).
pub fn get_applied_projections_with_pr(conn: &Connection) -> Result<Vec<Projection>, CoreError> {
    let mut stmt = conn.prepare(
        "SELECT p.id, p.pattern_id, p.target_type, p.target_path, p.content, p.applied_at, p.pr_url, p.status
         FROM projections p
         WHERE p.status = 'applied' AND p.pr_url IS NOT NULL",
    )?;

    let projections = stmt
        .query_map([], |row| {
            let applied_at_str: String = row.get(5)?;
            let applied_at = DateTime::parse_from_rfc3339(&applied_at_str)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let status_str: String = row.get(7)?;
            let status = ProjectionStatus::from_str(&status_str)
                .unwrap_or(ProjectionStatus::Applied);
            Ok(Projection {
                id: row.get(0)?,
                pattern_id: row.get(1)?,
                target_type: row.get(2)?,
                target_path: row.get(3)?,
                content: row.get(4)?,
                applied_at,
                pr_url: row.get(6)?,
                status,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(projections)
}

/// Get pattern IDs that have projections with specific statuses.
pub fn get_projected_pattern_ids_by_status(
    conn: &Connection,
    statuses: &[ProjectionStatus],
) -> Result<std::collections::HashSet<String>, CoreError> {
    if statuses.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let placeholders: Vec<String> = statuses.iter().enumerate().map(|(i, _)| format!("?{}", i + 1)).collect();
    let sql = format!(
        "SELECT DISTINCT pattern_id FROM projections WHERE status IN ({})",
        placeholders.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<String> = statuses.iter().map(|s| s.to_string()).collect();
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
    let ids = stmt
        .query_map(param_refs.as_slice(), |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(ids)
}
```

Also add `ProjectionStatus` to the import at the top of `db.rs` (line 2):
```rust
use crate::models::{IngestedSession, Pattern, PatternStatus, PatternType, Projection, ProjectionStatus, SuggestedTarget};
```

**Step 4: Run tests**

Run: `cargo test -p retro-core`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add crates/retro-core/src/db.rs
git commit -m "feat: add DB queries for review queue (pending, update status, delete, sync)"
```

---

### Task 4: Update `get_qualifying_patterns` to exclude dismissed patterns and use status-aware projection check

**Files:**
- Modify: `crates/retro-core/src/projection/mod.rs:197-212`

**Step 1: Write the failing test**

Add a test to `crates/retro-core/src/db.rs` tests:

```rust
#[test]
fn test_has_unprojected_patterns_excludes_dismissed() {
    let conn = test_db();

    let mut pattern = test_pattern("pat-1", "Dismissed pattern");
    pattern.status = PatternStatus::Dismissed;
    insert_pattern(&conn, &pattern).unwrap();

    assert!(!has_unprojected_patterns(&conn, 0.0).unwrap());
}

#[test]
fn test_has_unprojected_patterns_excludes_pending_review() {
    let conn = test_db();

    let pattern = test_pattern("pat-1", "Pattern with pending review");
    insert_pattern(&conn, &pattern).unwrap();

    // Create a pending_review projection
    let proj = Projection {
        id: "proj-1".to_string(),
        pattern_id: "pat-1".to_string(),
        target_type: "skill".to_string(),
        target_path: "/test.md".to_string(),
        content: "content".to_string(),
        applied_at: Utc::now(),
        pr_url: None,
        status: ProjectionStatus::PendingReview,
    };
    insert_projection(&conn, &proj).unwrap();

    // Pattern already has a pending_review projection — should NOT be "unprojected"
    assert!(!has_unprojected_patterns(&conn, 0.0).unwrap());
}
```

**Step 2: Run tests**

Run: `cargo test -p retro-core test_has_unprojected_patterns_excludes_dismissed -- --nocapture`
Expected: First test fails — `has_unprojected_patterns` doesn't filter dismissed patterns.

**Step 3: Update `has_unprojected_patterns`**

In `crates/retro-core/src/db.rs`, update `has_unprojected_patterns` (line 242-255) to also exclude dismissed:

```rust
pub fn has_unprojected_patterns(conn: &Connection, confidence_threshold: f64) -> Result<bool, CoreError> {
    let count: u64 = conn.query_row(
        "SELECT COUNT(*) FROM patterns p
         LEFT JOIN projections pr ON p.id = pr.pattern_id
         WHERE pr.id IS NULL
         AND p.status IN ('discovered', 'active')
         AND p.generation_failed = 0
         AND p.suggested_target != 'db_only'
         AND p.confidence >= ?1",
        [confidence_threshold],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}
```

Note: The `dismissed` status is already excluded by `AND p.status IN ('discovered', 'active')`. The second test about `pending_review` projections already passes because the LEFT JOIN finds the projection row. No changes needed for the SQL — just verify the tests pass.

**Step 4: Update `get_qualifying_patterns` in `projection/mod.rs`**

In `crates/retro-core/src/projection/mod.rs` (line 197-212), update to use status-aware filtering:

```rust
fn get_qualifying_patterns(
    conn: &Connection,
    config: &Config,
    project: Option<&str>,
) -> Result<Vec<Pattern>, CoreError> {
    let patterns = db::get_patterns(conn, &["discovered", "active"], project)?;
    let projected_ids = db::get_projected_pattern_ids_by_status(
        conn,
        &[ProjectionStatus::Applied, ProjectionStatus::PendingReview],
    )?;
    Ok(patterns
        .into_iter()
        .filter(|p| p.confidence >= config.analysis.confidence_threshold)
        .filter(|p| p.suggested_target != SuggestedTarget::DbOnly)
        .filter(|p| !p.generation_failed)
        .filter(|p| !projected_ids.contains(&p.id))
        .collect())
}
```

Add `ProjectionStatus` to the imports at the top of `projection/mod.rs` (line 9-11):
```rust
use crate::models::{
    ApplyAction, ApplyPlan, ApplyTrack, Pattern, PatternStatus, Projection, ProjectionStatus, SuggestedTarget,
};
```

**Step 5: Run tests**

Run: `cargo test -p retro-core`
Expected: All tests pass.

**Step 6: Commit**

```bash
git add crates/retro-core/src/db.rs crates/retro-core/src/projection/mod.rs
git commit -m "feat: exclude dismissed patterns and pending_review projections from qualifying"
```

---

### Task 5: Update `retro apply` to save as PendingReview instead of writing files

**Files:**
- Modify: `crates/retro-core/src/projection/mod.rs:122-189` (split execute_plan, add save_plan_for_review)
- Modify: `crates/retro-cli/src/commands/apply.rs`

**Step 1: Add `save_plan_for_review` to projection/mod.rs**

In `crates/retro-core/src/projection/mod.rs`, add a new function after `build_apply_plan`:

```rust
/// Save an apply plan's actions as pending_review projections in the database.
/// Does NOT write files or create PRs — just records the generated content for later review.
pub fn save_plan_for_review(
    conn: &Connection,
    plan: &ApplyPlan,
    project: Option<&str>,
) -> Result<usize, CoreError> {
    let mut saved = 0;

    for action in &plan.actions {
        let target_path = if action.target_type == SuggestedTarget::ClaudeMd {
            match project {
                Some(proj) => format!("{proj}/CLAUDE.md"),
                None => "CLAUDE.md".to_string(),
            }
        } else {
            action.target_path.clone()
        };

        let proj = Projection {
            id: uuid::Uuid::new_v4().to_string(),
            pattern_id: action.pattern_id.clone(),
            target_type: action.target_type.to_string(),
            target_path,
            content: action.content.clone(),
            applied_at: Utc::now(),
            pr_url: None,
            status: ProjectionStatus::PendingReview,
        };
        db::insert_projection(conn, &proj)?;
        saved += 1;
    }

    Ok(saved)
}
```

**Step 2: Update apply.rs interactive mode**

In `crates/retro-cli/src/commands/apply.rs`, the interactive mode (lines 180-329) needs to change. Instead of executing the plan and writing files, it saves for review.

Replace the section from the confirmation prompt through the end (lines 240-329) with:

```rust
    // Save generated content for review
    let saved = projection::save_plan_for_review(&conn, &plan, project.as_deref())?;

    // Audit log
    let audit_details = serde_json::json!({
        "action": "apply_generated",
        "patterns_generated": saved,
        "project": project,
        "global": global,
    });
    audit_log::append(&audit_path, "apply_generated", audit_details)?;

    println!();
    println!("{}", "Content generated!".green().bold());
    println!(
        "  {} {}",
        "Items queued for review:".white(),
        saved.to_string().green()
    );
    println!();
    println!(
        "  {}",
        "Run `retro review` to approve, skip, or dismiss items.".cyan()
    );

    Ok(())
```

Remove the old confirmation prompt, two-phase execution, and PR creation code from the interactive path. The `execute_shared_with_pr` function and `SharedResult` struct should stay — they'll be called from `retro review` instead.

Make `execute_shared_with_pr` and `SharedResult` public so the review command can use them:

```rust
pub struct SharedResult {
    pub files_written: usize,
    pub patterns_activated: usize,
    pub pr_url: Option<String>,
}

pub fn execute_shared_with_pr(
    // ... same signature
```

**Step 3: Update apply.rs auto mode**

In the auto mode section (lines 43-178), replace the plan execution with save_for_review:

Replace lines 93-161 with:
```rust
            Ok(plan) => {
                if plan.is_empty() {
                    if verbose {
                        eprintln!("[verbose] apply: no actions in plan");
                    }
                    return Ok(());
                }

                match projection::save_plan_for_review(&conn, &plan, project.as_deref()) {
                    Ok(saved) => {
                        let audit_details = serde_json::json!({
                            "action": "apply_generated",
                            "patterns_generated": saved,
                            "project": project,
                            "global": global,
                            "auto": true,
                        });
                        let _ = audit_log::append(&audit_path, "apply_generated", audit_details);

                        if verbose {
                            eprintln!("[verbose] auto-apply: queued {} items for review", saved);
                        }
                    }
                    Err(e) => {
                        if verbose {
                            eprintln!("[verbose] apply save error: {e}");
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
                }
            }
```

**Step 4: Run tests**

Run: `cargo test -p retro-core && cargo build -p retro-cli`
Expected: All tests pass, CLI builds.

**Step 5: Commit**

```bash
git add crates/retro-core/src/projection/mod.rs crates/retro-cli/src/commands/apply.rs
git commit -m "feat: retro apply saves as PendingReview instead of writing files"
```

---

### Task 6: Add `pr_view` to git.rs

**Files:**
- Modify: `crates/retro-core/src/git.rs`

**Step 1: Write the implementation**

No test needed for this (it shells out to `gh`). Add after `create_pr` in `crates/retro-core/src/git.rs` (after line 218):

```rust
/// Check the state of a PR by its URL. Returns "OPEN", "CLOSED", or "MERGED".
pub fn pr_state(pr_url: &str) -> Result<String, CoreError> {
    let output = Command::new("gh")
        .args(["pr", "view", pr_url, "--json", "state", "-q", ".state"])
        .output()
        .map_err(|e| CoreError::Io(format!("gh pr view: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::Io(format!("gh pr view failed: {stderr}")));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
```

**Step 2: Commit**

```bash
git add crates/retro-core/src/git.rs
git commit -m "feat: add git::pr_state to check PR status via gh"
```

---

### Task 7: Implement `retro sync` command

**Files:**
- Create: `crates/retro-cli/src/commands/sync.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs` — add `pub mod sync;`
- Modify: `crates/retro-cli/src/main.rs` — add `Sync` variant to Commands enum

**Step 1: Create the sync command**

Create `crates/retro-cli/src/commands/sync.rs`:

```rust
use anyhow::Result;
use colored::Colorize;
use retro_core::audit_log;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::git;
use retro_core::models::{PatternStatus, ProjectionStatus};

/// Run sync: check PR status for applied projections and reset patterns from closed PRs.
pub fn run(verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let db_path = dir.join("retro.db");
    let audit_path = dir.join("audit.jsonl");

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let conn = db::open_db(&db_path)?;

    let reset_count = run_sync(&conn, &audit_path, verbose)?;

    if reset_count > 0 {
        println!(
            "{}",
            format!("Reset {} pattern(s) from closed PRs back to discoverable.", reset_count)
                .green()
        );
    } else if verbose {
        println!("{}", "No closed PRs found — nothing to sync.".dimmed());
    }

    Ok(())
}

/// Core sync logic — callable from other commands (review, apply).
/// Returns the number of patterns reset.
pub fn run_sync(
    conn: &db::Connection,
    audit_path: &std::path::Path,
    verbose: bool,
) -> Result<usize> {
    if !git::is_gh_available() {
        if verbose {
            eprintln!("[verbose] sync: gh CLI not available, skipping");
        }
        return Ok(0);
    }

    let projections = db::get_applied_projections_with_pr(conn)?;
    if projections.is_empty() {
        return Ok(0);
    }

    // Dedupe by PR URL
    let mut seen_urls = std::collections::HashSet::new();
    let mut pr_urls: Vec<String> = Vec::new();
    for proj in &projections {
        if let Some(ref url) = proj.pr_url {
            if seen_urls.insert(url.clone()) {
                pr_urls.push(url.clone());
            }
        }
    }

    let mut reset_count = 0;

    for url in &pr_urls {
        let state = match git::pr_state(url) {
            Ok(s) => s,
            Err(e) => {
                if verbose {
                    eprintln!("[verbose] sync: failed to check PR {url}: {e}");
                }
                continue;
            }
        };

        if state == "CLOSED" {
            // Find all projections for this PR URL
            let affected: Vec<_> = projections
                .iter()
                .filter(|p| p.pr_url.as_deref() == Some(url.as_str()))
                .collect();

            let mut pattern_ids = Vec::new();
            for proj in &affected {
                // Delete the projection
                db::delete_projection(conn, &proj.id)?;
                // Reset pattern to Discovered
                db::update_pattern_status(conn, &proj.pattern_id, &PatternStatus::Discovered)?;
                pattern_ids.push(proj.pattern_id.clone());
                reset_count += 1;
            }

            // Audit log
            let _ = audit_log::append(
                audit_path,
                "sync_reset",
                serde_json::json!({
                    "patterns": pattern_ids,
                    "pr_url": url,
                }),
            );

            if verbose {
                eprintln!("[verbose] sync: reset {} patterns from closed PR {url}", affected.len());
            }
        }
    }

    Ok(reset_count)
}
```

**Step 2: Register the command**

In `crates/retro-cli/src/commands/mod.rs`, add `pub mod sync;` to the module list.

In `crates/retro-cli/src/main.rs`, add to the `Commands` enum:
```rust
/// Sync PR status: reset patterns from closed PRs back to discoverable
Sync,
```

And in the match arm (around line 138):
```rust
Commands::Sync => commands::sync::run(verbose),
```

**Step 3: Run build**

Run: `cargo build -p retro-cli`
Expected: Builds successfully.

**Step 4: Commit**

```bash
git add crates/retro-cli/src/commands/sync.rs crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs
git commit -m "feat: add retro sync command to detect closed PRs and reset patterns"
```

---

### Task 8: Implement `retro review` command

**Files:**
- Create: `crates/retro-cli/src/commands/review.rs`
- Modify: `crates/retro-cli/src/commands/mod.rs`
- Modify: `crates/retro-cli/src/main.rs`

**Step 1: Create the review command**

Create `crates/retro-cli/src/commands/review.rs`:

```rust
use anyhow::Result;
use colored::Colorize;
use retro_core::audit_log;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::git;
use retro_core::models::{ApplyAction, ApplyPlan, ApplyTrack, PatternStatus, ProjectionStatus, SuggestedTarget};
use retro_core::projection;

use super::git_root_or_cwd;

/// User's decision for a pending review item.
#[derive(Debug, Clone, PartialEq)]
enum ReviewAction {
    Apply,
    Skip,
    Dismiss,
}

pub fn run(global: bool, dry_run: bool, verbose: bool) -> Result<()> {
    let dir = retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
    let audit_path = dir.join("audit.jsonl");

    if !db_path.exists() {
        anyhow::bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;
    let conn = db::open_db(&db_path)?;

    // Run sync first to clean up closed PRs
    let _ = super::sync::run_sync(&conn, &audit_path, verbose);

    // Get pending review items
    let pending = db::get_pending_review_projections(&conn)?;

    if pending.is_empty() {
        println!("{}", "No items pending review.".dimmed());
        return Ok(());
    }

    // Also fetch the patterns for display
    let all_patterns = db::get_all_patterns(&conn, None)?;
    let pattern_map: std::collections::HashMap<String, _> = all_patterns
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect();

    // Display numbered list
    println!();
    println!(
        "Pending review ({} items):",
        pending.len().to_string().cyan()
    );
    println!();

    for (i, proj) in pending.iter().enumerate() {
        let num = format!("  {}.", i + 1);
        let target_label = match proj.target_type.as_str() {
            "skill" => "[skill]",
            "claude_md" => "[rule] ",
            "global_agent" => "[agent]",
            _ => "[item] ",
        };

        let description = pattern_map
            .get(&proj.pattern_id)
            .map(|p| p.description.as_str())
            .unwrap_or("(unknown pattern)");

        let confidence = pattern_map
            .get(&proj.pattern_id)
            .map(|p| p.confidence)
            .unwrap_or(0.0);

        let times_seen = pattern_map
            .get(&proj.pattern_id)
            .map(|p| p.times_seen)
            .unwrap_or(0);

        println!(
            "{} {} {}",
            num.white().bold(),
            target_label.dimmed(),
            description.white()
        );
        println!(
            "     Target: {}",
            retro_core::util::shorten_path(&proj.target_path).dimmed()
        );
        println!(
            "     Seen {} times (confidence: {:.2})",
            times_seen.to_string().cyan(),
            confidence
        );
        println!();
    }

    if dry_run {
        println!(
            "{}",
            "Dry run — no actions taken. Run `retro review` to make decisions.".yellow().bold()
        );
        return Ok(());
    }

    // Parse user input
    println!(
        "{}",
        "Actions: apply (a), skip (s), dismiss (d), preview (p)".dimmed()
    );
    print!(
        "{} ",
        "Enter selections (e.g., \"1a 2a 3d\" or \"all:a\"):".yellow().bold()
    );
    use std::io::Write;
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        println!("{}", "No selections made.".dimmed());
        return Ok(());
    }

    // Handle preview requests first
    let tokens: Vec<&str> = input.split_whitespace().collect();
    for token in &tokens {
        if token.ends_with('p') || token.ends_with('P') {
            let num_str = &token[..token.len() - 1];
            if let Ok(num) = num_str.parse::<usize>() {
                if num >= 1 && num <= pending.len() {
                    let proj = &pending[num - 1];
                    println!();
                    println!("{}", format!("--- Preview: item {} ---", num).cyan().bold());
                    println!("{}", &proj.content);
                    println!("{}", "--- End preview ---".cyan());
                    println!();
                }
            }
        }
    }

    // Re-prompt after preview if only previews were requested
    let has_non_preview = tokens.iter().any(|t| {
        let last = t.chars().last().unwrap_or(' ');
        last == 'a' || last == 'A' || last == 's' || last == 'S' || last == 'd' || last == 'D'
    });

    if !has_non_preview {
        // Only previews were requested — re-prompt
        print!(
            "{} ",
            "Enter selections (e.g., \"1a 2a 3d\" or \"all:a\"):".yellow().bold()
        );
        std::io::stdout().flush()?;
        input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let input_ref = input.trim();
        if input_ref.is_empty() {
            println!("{}", "No selections made.".dimmed());
            return Ok(());
        }
        // Fall through to process actions below
    }

    // Parse actions
    let mut decisions: Vec<(usize, ReviewAction)> = Vec::new();

    for token in input.trim().split_whitespace() {
        if token.starts_with("all:") {
            let action_char = token.chars().last().unwrap_or(' ');
            let action = match action_char {
                'a' | 'A' => ReviewAction::Apply,
                's' | 'S' => ReviewAction::Skip,
                'd' | 'D' => ReviewAction::Dismiss,
                _ => continue,
            };
            for i in 0..pending.len() {
                decisions.push((i, action.clone()));
            }
            break;
        }

        if token.len() < 2 {
            continue;
        }

        let action_char = token.chars().last().unwrap_or(' ');
        let num_str = &token[..token.len() - 1];

        let action = match action_char {
            'a' | 'A' => ReviewAction::Apply,
            's' | 'S' => ReviewAction::Skip,
            'd' | 'D' => ReviewAction::Dismiss,
            'p' | 'P' => continue, // Already handled previews
            _ => continue,
        };

        if let Ok(num) = num_str.parse::<usize>() {
            if num >= 1 && num <= pending.len() {
                decisions.push((num - 1, action));
            }
        }
    }

    if decisions.is_empty() {
        println!("{}", "No valid selections.".dimmed());
        return Ok(());
    }

    // Execute decisions
    let project = if global {
        None
    } else {
        Some(git_root_or_cwd()?)
    };

    let mut applied_projections = Vec::new();
    let mut dismissed_patterns = Vec::new();
    let mut skipped = 0;

    for (idx, action) in &decisions {
        let proj = &pending[*idx];

        match action {
            ReviewAction::Apply => {
                applied_projections.push(proj.clone());
            }
            ReviewAction::Skip => {
                skipped += 1;
            }
            ReviewAction::Dismiss => {
                // Delete projection and mark pattern as Dismissed
                db::delete_projection(&conn, &proj.id)?;
                db::update_pattern_status(&conn, &proj.pattern_id, &PatternStatus::Dismissed)?;
                dismissed_patterns.push(proj.pattern_id.clone());
            }
        }
    }

    // Execute approved items
    if !applied_projections.is_empty() {
        // Build an ApplyPlan from the approved projections
        let actions: Vec<ApplyAction> = applied_projections
            .iter()
            .map(|proj| {
                let target_type = match proj.target_type.as_str() {
                    "skill" => SuggestedTarget::Skill,
                    "claude_md" => SuggestedTarget::ClaudeMd,
                    "global_agent" => SuggestedTarget::GlobalAgent,
                    _ => SuggestedTarget::DbOnly,
                };
                let track = match target_type {
                    SuggestedTarget::GlobalAgent => ApplyTrack::Personal,
                    _ => ApplyTrack::Shared,
                };
                let description = pattern_map
                    .get(&proj.pattern_id)
                    .map(|p| p.description.clone())
                    .unwrap_or_default();
                ApplyAction {
                    pattern_id: proj.pattern_id.clone(),
                    pattern_description: description,
                    target_type,
                    target_path: proj.target_path.clone(),
                    content: proj.content.clone(),
                    track,
                }
            })
            .collect();

        let plan = ApplyPlan { actions };

        let mut total_files = 0;
        let mut total_patterns = 0;
        let mut pr_url: Option<String> = None;

        // Phase 1: Personal actions
        let has_personal = !plan.personal_actions().is_empty();
        if has_personal {
            let result = projection::execute_plan(
                &conn,
                &config,
                &plan,
                project.as_deref(),
                Some(&ApplyTrack::Personal),
            )?;
            total_files += result.files_written;
            total_patterns += result.patterns_activated;
        }

        // Phase 2: Shared actions with PR
        let has_shared = !plan.shared_actions().is_empty();
        if has_shared {
            let shared_result = super::apply::execute_shared_with_pr(
                &conn, &config, &plan, project.as_deref(), false,
            )?;
            total_files += shared_result.files_written;
            total_patterns += shared_result.patterns_activated;
            pr_url = shared_result.pr_url;
        }

        // Update the pending_review projections to applied
        for proj in &applied_projections {
            db::update_projection_status(&conn, &proj.id, &ProjectionStatus::Applied)?;
            if let Some(ref url) = pr_url {
                // Update pr_url on shared projections
                let target_type = proj.target_type.as_str();
                if target_type == "skill" || target_type == "claude_md" {
                    // Update pr_url in DB
                    conn.execute(
                        "UPDATE projections SET pr_url = ?2 WHERE id = ?1",
                        rusqlite::params![proj.id, url],
                    )?;
                }
            }
        }

        // Audit
        let applied_pattern_ids: Vec<_> = applied_projections.iter().map(|p| p.pattern_id.clone()).collect();
        audit_log::append(
            &audit_path,
            "review_applied",
            serde_json::json!({
                "patterns": applied_pattern_ids,
                "files_written": total_files,
                "patterns_activated": total_patterns,
                "pr_url": pr_url,
            }),
        )?;

        println!();
        println!("{}", "Review complete!".green().bold());
        println!("  {} {}", "Applied:".white(), applied_projections.len().to_string().green());
        if total_files > 0 {
            println!("  {} {}", "Files written:".white(), total_files.to_string().green());
        }
        if let Some(url) = &pr_url {
            println!("  {} {}", "Pull request:".white(), url.cyan());
        }
    }

    if !dismissed_patterns.is_empty() {
        audit_log::append(
            &audit_path,
            "review_dismissed",
            serde_json::json!({ "patterns": dismissed_patterns }),
        )?;
        println!("  {} {}", "Dismissed:".white(), dismissed_patterns.len().to_string().yellow());
    }

    if skipped > 0 {
        println!("  {} {}", "Skipped:".white(), skipped.to_string().dimmed());
    }

    Ok(())
}
```

**Step 2: Register the command**

In `crates/retro-cli/src/commands/mod.rs`, add `pub mod review;` to the module list.

In `crates/retro-cli/src/main.rs`, add to the `Commands` enum:
```rust
/// Review pending suggestions: approve, skip, or dismiss generated items
Review {
    /// Review items for all projects, not just the current one
    #[arg(long)]
    global: bool,
    /// Show pending items without prompting for action
    #[arg(long)]
    dry_run: bool,
},
```

And in the match arm:
```rust
Commands::Review { global, dry_run } => commands::review::run(global, dry_run, verbose),
```

**Step 3: Build and verify**

Run: `cargo build -p retro-cli`
Expected: Builds successfully.

**Step 4: Commit**

```bash
git add crates/retro-cli/src/commands/review.rs crates/retro-cli/src/commands/mod.rs crates/retro-cli/src/main.rs
git commit -m "feat: add retro review command for batch approve/skip/dismiss"
```

---

### Task 9: Update nudge to show pending review count

**Files:**
- Modify: `crates/retro-cli/src/commands/mod.rs` (nudge system)

**Step 1: Update `check_and_display_nudge`**

In the nudge display function in `crates/retro-cli/src/commands/mod.rs`, add a check for pending review items. After the existing nudge aggregation logic, add:

```rust
// Check for pending review items
if let Ok(pending) = db::get_pending_review_projections(&conn) {
    if !pending.is_empty() {
        println!(
            "  {} {} — run {}",
            "retro:".dimmed(),
            format!("{} items pending review", pending.len()).yellow(),
            "`retro review`".cyan()
        );
    }
}
```

This should be added to `check_and_display_nudge` after the existing auto-run display logic, and before the `set_last_nudge_at` call.

Also add `apply_generated` to the recognized audit actions in `aggregate_auto_runs`. When action is `apply_generated`, read `patterns_generated` from the details and include it in the summary.

**Step 2: Build and verify**

Run: `cargo build -p retro-cli`
Expected: Builds.

**Step 3: Commit**

```bash
git add crates/retro-cli/src/commands/mod.rs
git commit -m "feat: nudge shows pending review count"
```

---

### Task 10: Run sync automatically before apply and review

**Files:**
- Modify: `crates/retro-cli/src/commands/apply.rs`

**Step 1: Add sync call at the start of apply**

In `crates/retro-cli/src/commands/apply.rs`, after the DB is opened (around line 41), add:

```rust
// Sync PR status before generating new content
let _ = super::sync::run_sync(&conn, &audit_path, verbose);
```

This should be in both the auto and interactive paths, but for auto mode it should only run if `gh` is available (which `run_sync` already handles).

**Step 2: Build and verify**

Run: `cargo build -p retro-cli`
Expected: Builds.

**Step 3: Commit**

```bash
git add crates/retro-cli/src/commands/apply.rs
git commit -m "feat: run sync before apply to clean up closed PRs"
```

---

### Task 11: Update CLAUDE.md with new conventions

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Add new conventions**

Add to the "Key Design Decisions" or "Conventions" section in CLAUDE.md:

- `ProjectionStatus` enum: `PendingReview`, `Applied`, `Dismissed` — tracks review queue state
- `retro apply` generates content and saves as `PendingReview` — does NOT write files or create PRs
- `retro review` is the gate: lists pending items, user batch-selects apply/skip/dismiss
- `retro sync` checks PR state via `gh pr view` — resets patterns from closed PRs to `Discovered`
- Nudge system shows pending review count alongside auto-run summaries
- DB schema v3: `projections` table has `status` column (default `applied` for migration)

Also update the Implementation Status section to note Phase 7 (Review Queue).

**Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md with review queue conventions"
```

---

### Task 12: Final integration test and cleanup

**Step 1: Run full test suite**

Run: `cargo test --workspace`
Expected: All tests pass.

**Step 2: Test the full flow manually (if possible)**

Run: `cargo run -p retro-cli -- review --dry-run`
Expected: Either "No items pending review" or a list of items.

Run: `cargo run -p retro-cli -- sync`
Expected: Either syncs or reports nothing to sync.

**Step 3: Verify build with no warnings**

Run: `cargo build --workspace 2>&1`
Expected: Clean build, no warnings.

**Step 4: Final commit if any cleanup needed**

```bash
git add -A
git commit -m "chore: review queue integration cleanup"
```
