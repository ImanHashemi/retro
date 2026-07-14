# Scenario: v3 Pipeline Dry-Run and Surfaces

## Description

Tests the read-only surfaces of the v3 pipeline against a seeded store:
`retro run --dry-run` (no AI calls, no store commit), `retro lint --dry-run`
(scan only, no state write), `retro doctor` (structural checks pass), and the
`retro ui` dashboard server (`/api/ping`, `/api/nodes?scope=global` shows the
seeded node).

No AI calls are made — fast and free.

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

[ui]
port = 7787
EOF
RETRO=./target/release/retro        # NEVER a PATH `retro` binary
# NEVER run any retro command without RETRO_HOME set in this scenario.
# Non-default UI port: never collide with (or curl into) a real dashboard.
# ------------------------------------------------------------------------

# Initialize the store (no pre-existing CLAUDE.md -> nothing to safety-import):
printf 'n\n' | $RETRO init

# Seed one global node file (frontmatter format from store/node.rs).
# Confidence 0.50 is below the 0.7 projection threshold on purpose: the node
# is visible to lint/index/UI but nothing needs projecting, so doctor's
# projection check stays green on a store that has never run a projection.
TODAY=$(date +%Y-%m-%d)
cat > "$RETRO_HOME/knowledge/global/scenario-seeded-rule.md" <<EOF
---
id: scenario-seeded-rule
scope: global
type: rule
confidence: 0.50
sources: [scenario-seed]
created: $TODAY
updated: $TODAY
invalidated_by: null
---
Prefer rebasing feature branches before merging
EOF

# Index the seeded node:
$RETRO reindex
```

## Steps

1. Run `git -C "$RETRO_HOME" rev-parse HEAD` and record the commit hash
2. Run `$RETRO run --dry-run` and capture output
3. Run `git -C "$RETRO_HOME" rev-parse HEAD` again (must equal step 1's hash) and `git -C "$RETRO_HOME" status --porcelain -- knowledge`
4. Run `$RETRO lint --dry-run` and capture output, then `test -f "$RETRO_HOME/state/state.json" && echo "state.json exists" || echo "no state.json"`
5. Run `$RETRO doctor; echo "doctor exit: $?"` and capture output
6. Start the dashboard in the background: `nohup $RETRO ui --no-open > "$RETRO_HOME/ui.log" 2>&1 & echo $! > "$RETRO_HOME/ui.pid"`, wait ~1s
7. Run `curl -s http://127.0.0.1:7787/api/ping`
8. Run `curl -s "http://127.0.0.1:7787/api/nodes?scope=global"`
9. Kill ONLY the server you started: `kill $(cat "$RETRO_HOME/ui.pid")`

## Expected

- Setup: `retro init` completes; `retro reindex` reports "Indexed 1 node(s)"
  from `$RETRO_HOME/knowledge`
- Step 2: a v3 dry-run summary line: "v3 dry run: 0 session(s) pending,
  0 skipped — no AI calls, no writes"
- Step 3: HEAD hash unchanged from step 1 (no store commit was made); the
  hand-seeded node still shows as untracked (`?? knowledge/`) — a dry run
  commits nothing, so it must NOT have been swept into a commit
- Step 4: lint reports "Scanned 1 active node(s)" with 0 findings; no
  `state/state.json` was created by the dry run
- Step 5: doctor prints per-check lines; store-present, store-repo, index,
  hooks, and queue all pass; projection reports nothing out of date. The
  claude-cli check is tolerated either way (the claude CLI may be absent in a
  sandbox); exit code is 0 when claude-cli passes too
- Step 7: `{"ok":true}`
- Step 8: JSON containing the seeded node — id `scenario-seeded-rule` and/or
  its body "Prefer rebasing feature branches before merging"
- Step 9: kill succeeds (the server was still running and is now stopped)

## Not Expected

- No AI analysis calls (`claude -p`); doctor's `claude --version` probe is the
  only claude invocation in the whole scenario
- No new git commits in the store and no writes under `$RETRO_HOME/knowledge`
  from any step
- No mention of the real user home paths (`/Users/*/.retro`, `/Users/*/.claude`) in any output
- No panics, stack traces, or "error:" lines
- The dashboard binds 127.0.0.1 only — nothing listens on other interfaces
