#!/usr/bin/env bash
# Seed synthetic data into ~/.retro/retro.db for demo recording.
# Usage: ./docs/demo-seed.sh [--clean]

DB="$HOME/.retro/retro.db"
NOW=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
TODAY=$(date -u +"%Y-%m-%d")

if [ "$1" = "--clean" ]; then
    echo "Cleaning synthetic demo data..."
    sqlite3 "$DB" "DELETE FROM nodes WHERE id LIKE 'demo-%';"
    sqlite3 "$DB" "DELETE FROM projects WHERE id LIKE 'demo-%' OR id IN ('my-rust-app', 'my-python-api', 'retro');"
    sqlite3 "$DB" "DELETE FROM edges WHERE source_id LIKE 'demo-%' OR target_id LIKE 'demo-%';"
    echo "Done."
    exit 0
fi

echo "Seeding demo data into $DB..."

# Projects
sqlite3 "$DB" <<SQL
INSERT OR REPLACE INTO projects (id, path, remote_url, agent_type, last_seen) VALUES
    ('my-rust-app', '/Users/iman/projects/my-rust-app', 'git@github.com:iman/my-rust-app.git', 'claude_code', '$NOW'),
    ('my-python-api', '/Users/iman/projects/my-python-api', 'git@github.com:iman/my-python-api.git', 'claude_code', '$NOW'),
    ('retro', '/Users/iman/repositories/retro', 'git@github.com:ImanHashemi/retro.git', 'claude_code', '$NOW');
SQL

# Pending review nodes (what the user will see in tab 1)
sqlite3 "$DB" <<SQL
INSERT OR REPLACE INTO nodes (id, type, scope, project_id, content, confidence, status, created_at, updated_at) VALUES
    ('demo-p1', 'rule', 'project', 'my-rust-app', 'Prefer thiserror over anyhow in library crates — reserve anyhow for binary/CLI code', 0.82, 'pending_review', '$NOW', '$NOW'),
    ('demo-p2', 'skill', 'global', NULL, 'rust-error-handling: two-crate pattern with thiserror for library errors and anyhow for CLI', 0.78, 'pending_review', '$NOW', '$NOW'),
    ('demo-p3', 'rule', 'project', 'my-python-api', 'Always type-hint function return values — use -> None explicitly for void functions', 0.71, 'pending_review', '$NOW', '$NOW'),
    ('demo-p4', 'directive', 'global', NULL, 'Never use print() for logging — always use the logging module with appropriate levels', 0.75, 'pending_review', '$NOW', '$NOW');
SQL

# Active knowledge nodes (tab 2)
sqlite3 "$DB" <<SQL
INSERT OR REPLACE INTO nodes (id, type, scope, project_id, content, confidence, status, created_at, updated_at) VALUES
    ('demo-a1', 'directive', 'global', NULL, 'Do not summarize what you just did at the end of every response', 0.92, 'active', '$NOW', '$NOW'),
    ('demo-a2', 'rule', 'project', 'my-rust-app', 'Run cargo test before suggesting a commit — never commit without passing tests', 0.85, 'active', '$NOW', '$NOW'),
    ('demo-a3', 'directive', 'global', NULL, 'Always use snake_case for Rust function and variable names', 0.90, 'active', '$NOW', '$NOW'),
    ('demo-a4', 'skill', 'global', NULL, 'pre-pr-checklist: run tests, clippy, format, then commit with conventional message', 0.78, 'active', '$NOW', '$NOW'),
    ('demo-a5', 'pattern', 'project', 'my-rust-app', 'Frequently forgets to update CLAUDE.md after changing project conventions', 0.65, 'active', '$NOW', '$NOW'),
    ('demo-a6', 'rule', 'project', 'my-python-api', 'Use uv instead of pip for all Python package management', 0.88, 'active', '$NOW', '$NOW'),
    ('demo-a7', 'preference', 'global', NULL, 'Prefers concise responses without preamble or filler', 0.80, 'active', '$NOW', '$NOW'),
    ('demo-a8', 'rule', 'project', 'retro', 'Use rusqlite bundled feature — never link to system SQLite', 0.83, 'active', '$NOW', '$NOW'),
    ('demo-a9', 'pattern', 'project', 'my-python-api', 'Often writes integration tests that mock the database instead of using test fixtures', 0.60, 'active', '$NOW', '$NOW'),
    ('demo-a10', 'directive', 'project', 'my-rust-app', 'Always run cargo clippy -- -D warnings before committing', 0.87, 'active', '$NOW', '$NOW');
SQL

# Set runner metadata for a realistic status bar
sqlite3 "$DB" <<SQL
INSERT OR REPLACE INTO metadata (key, value) VALUES
    ('last_run_at', '$NOW'),
    ('ai_calls_date', '$TODAY'),
    ('ai_calls_today', '3');
SQL

echo "Seeded: 3 projects, 4 pending nodes, 10 active nodes"
echo "Run: ./target/debug/retro dash"
