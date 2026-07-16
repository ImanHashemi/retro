# Scenario: v3 Migrate from v2

## Description

Tests `retro migrate` against a fixture v2 SQLite database: dry-run previews
counts without writing anything, the real run imports active/pending nodes with
type mapping (directive→rule, skill→pattern), skips dismissed/invalid rows
visibly, safety-imports novel managed-block rules, sweeps v1 git hooks and the
v2 launchd plist (both faked inside the sandbox), commits to the store repo,
and projects the global CLAUDE.md. `retro.db` stays byte-identical throughout
(checksum-verified), and a second run is a documented no-op (idempotent).

The launchd steps are macOS-specific; on Linux the plist lines simply do not
appear. No AI calls are made — fast and free.

## Setup

```bash
# --- MANDATORY ISOLATION PREAMBLE (never touch the real environment) ---
export RETRO_HOME=$(mktemp -d)      # sandbox for ~/.retro
export FAKE_CLAUDE=$(mktemp -d)     # sandbox for ~/.claude
export HOME=$(mktemp -d)            # sandbox $HOME-derived paths (launchd plist probe, purge backup rescue)
STUB_BIN=$(mktemp -d)               # neutralize launchctl — uninstall/migrate invoke it for v2 cleanup
printf '#!/bin/sh\nexit 0\n' > "$STUB_BIN/launchctl" && chmod +x "$STUB_BIN/launchctl"
export PATH="$STUB_BIN:$PATH"
cat > "$RETRO_HOME/config.toml" <<EOF
[paths]
claude_dir = "$FAKE_CLAUDE"
EOF
RETRO=./target/release/retro        # NEVER a PATH `retro` binary
# NEVER run any retro command without RETRO_HOME set in this scenario.
# ------------------------------------------------------------------------

# A fake registered project with a v1 post-commit hook (migrate must sweep it):
FAKE_PROJECT=$(mktemp -d)
mkdir -p "$FAKE_PROJECT/.git/hooks"
cat > "$FAKE_PROJECT/.git/hooks/post-commit" <<'EOF'
#!/bin/sh
# retro hook - do not remove
retro ingest --auto &
EOF
chmod +x "$FAKE_PROJECT/.git/hooks/post-commit"

# A fake v2 launchd plist in the fake $HOME (migrate must remove it — macOS only):
mkdir -p "$HOME/Library/LaunchAgents"
echo '<plist/>' > "$HOME/Library/LaunchAgents/com.retro.runner.plist"

# Fixture v2 database — schema and rows mirror the unit-test fixture in
# crates/retro-core/src/migrate.rs: 8 nodes covering active, pending_review,
# dismissed, unknown type, and project scope with NULL project_id, plus one
# projects row pointing at the fake project.
sqlite3 "$RETRO_HOME/retro.db" <<EOF
CREATE TABLE nodes (id TEXT PRIMARY KEY, type TEXT, scope TEXT, project_id TEXT,
    content TEXT, confidence REAL, status TEXT, created_at TEXT, updated_at TEXT,
    projected_at TEXT, pr_url TEXT);
CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT, remote_url TEXT,
    agent_type TEXT DEFAULT 'claude_code', last_seen TEXT);
INSERT INTO projects VALUES ('my-app', '$FAKE_PROJECT', NULL, 'claude_code', '2026-01-01T00:00:00Z');
INSERT INTO nodes VALUES
  ('n1', 'rule',      'global',  NULL,     'Always run smoke tests before full runs',  0.8,  'active',         '2026-05-01T10:00:00Z', '2026-06-01T10:00:00Z', NULL, NULL),
  ('n2', 'directive', 'global',  NULL,     'Never commit secrets',                     0.85, 'active',         '2026-05-01T10:00:00Z', '2026-06-01T10:00:00Z', NULL, NULL),
  ('n3', 'skill',     'global',  NULL,     'Use uv for python scripts',                0.75, 'active',         '2026-05-01T10:00:00Z', '2026-06-01T10:00:00Z', NULL, NULL),
  ('n4', 'pattern',   'project', 'my-app', 'Deploys go through staging first',         0.6,  'pending_review', '2026-05-01T10:00:00Z', '2026-06-01T10:00:00Z', NULL, NULL),
  ('n5', 'rule',      'global',  NULL,     'A dismissed rule',                         0.9,  'dismissed',      '2026-05-01T10:00:00Z', '2026-06-01T10:00:00Z', NULL, NULL),
  ('n6', 'memory',    'global',  NULL,     'Context-only memory item',                 0.7,  'active',         '2026-05-01T10:00:00Z', '2026-06-01T10:00:00Z', NULL, NULL),
  ('n7', 'wizardry',  'global',  NULL,     'Unknown type must be skipped visibly',     0.7,  'active',         '2026-05-01T10:00:00Z', '2026-06-01T10:00:00Z', NULL, NULL),
  ('n8', 'rule',      'project', NULL,     'Project rule with no project id',          0.7,  'active',         '2026-05-01T10:00:00Z', '2026-06-01T10:00:00Z', NULL, NULL);
EOF

# Global CLAUDE.md with one novel managed-block bullet (safety-import target):
cat > "$FAKE_CLAUDE/CLAUDE.md" <<'EOF'
# My global notes

<!-- retro:managed:start -->
## Retro-Discovered Patterns

- Use conventional commit messages in every repository

<!-- retro:managed:end -->
EOF
```

## Steps

1. Run `shasum "$RETRO_HOME/retro.db"` and record the checksum
2. Run `$RETRO migrate --dry-run` and capture output
3. Run `test -d "$RETRO_HOME/knowledge" && echo "store exists" || echo "store still empty"` and `shasum "$RETRO_HOME/retro.db"` (must match step 1)
4. Run `$RETRO migrate` and capture output
5. Run `shasum "$RETRO_HOME/retro.db"` (must match step 1), `find "$RETRO_HOME/knowledge" -name '*.md' | wc -l`, and `git -C "$RETRO_HOME" log --oneline`
6. Run `cat "$FAKE_PROJECT/.git/hooks/post-commit" 2>/dev/null; test -f "$HOME/Library/LaunchAgents/com.retro.runner.plist" && echo "plist still there" || echo "plist removed"` and `cat "$FAKE_CLAUDE/CLAUDE.md"`
7. Run `$RETRO migrate` a second time and capture output
8. Run `shasum "$RETRO_HOME/retro.db"` (must still match step 1) and `find "$RETRO_HOME/knowledge" -name '*.md' | wc -l` (must match step 5)

## Expected

- Step 2 (dry run): header says "dry run — nothing written"; counts line
  "knowledge: 5 imported, 0 already present (deduped), 1 skipped
  (dismissed/archived), 2 skipped (invalid)"; "safety-import: 1 rule(s) rescued
  from managed blocks"; "would sweep v1 git hooks from 1 known project(s)";
  on macOS a "would remove the v2 launchd runner" line; a note that the v2
  database is preserved and a rollback note that migrate never modifies retro.db
- Step 3: the store is still empty (no `knowledge/` directory was created by
  the dry run) and the retro.db checksum is unchanged
- Step 4 (real run): same counts — 5 imported, 1 skipped (dismissed/archived),
  2 skipped (invalid); "safety-import: 1 rule(s) rescued"; "removed 1 v1 git
  hook(s) across 1 known project(s)"; on macOS "removed the v2 launchd runner
  (com.retro.runner)"; "projected 4 rule(s) to $FAKE_CLAUDE/CLAUDE.md"
  (n1 + n2 directive→rule + n3 skill→pattern + the rescued bullet; the memory
  node and the below-threshold project pattern are not projected); "v2 database
  preserved at $RETRO_HOME/retro.db"
- Step 5: retro.db checksum is byte-identical to step 1; the store holds
  6 node files (5 imported + 1 safety-imported); git log shows a
  "retro: migrate v2 knowledge" commit on top of "retro: initialize store"
- Step 6: the v1 post-commit hook no longer contains the retro marker (a
  hook that held only the retro lines is deleted outright); the fake plist is
  removed (macOS); CLAUDE.md keeps "My global notes" and now has a managed
  block containing the 4 projected rules
- Step 7 (rerun): "0 imported, 5 already present (deduped), 1 skipped
  (dismissed/archived), 2 skipped (invalid)"; "safety-import: 0 rule(s)
  rescued" — fully idempotent
- Step 8: retro.db checksum still identical; still exactly 6 node files (no
  duplicates from the rerun)

## Not Expected

- `retro.db` modified, deleted, or renamed at any point (checksum must never change)
- Duplicate nodes after the rerun (node-file count must not grow)
- Any write outside `$RETRO_HOME`, `$FAKE_CLAUDE`, `$FAKE_PROJECT`, and the fake `$HOME`
- No mention of the real user home paths (`/Users/*/.retro`, `/Users/*/.claude`) in any output
- No AI calls (`claude -p`), no panics, stack traces, or "error:" lines
