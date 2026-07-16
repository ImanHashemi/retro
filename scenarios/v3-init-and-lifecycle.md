# Scenario: v3 Init and Lifecycle

## Description

Tests the full v3 lifecycle end to end: `retro init` (store creation, git repo,
global hooks, settings backup, safety-import of pre-existing managed-block
rules), `retro status`, `retro doctor`, `retro uninstall` (hooks and managed
section removed, unrelated user content preserved, store kept), and
`retro uninstall --purge` (store deleted after typed confirmation, backups
rescued outside the store).

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
EOF
RETRO=./target/release/retro        # NEVER a PATH `retro` binary
# NEVER run any retro command without RETRO_HOME set in this scenario.
# Interactive prompts are answered via stdin pipes (see Steps) — never a TTY.
# ------------------------------------------------------------------------

# Pre-existing Claude settings (init must back this up and merge, not clobber):
mkdir -p "$FAKE_CLAUDE"
cat > "$FAKE_CLAUDE/settings.json" <<'EOF'
{
  "permissions": { "allow": ["Bash(echo:*)"] },
  "hooks": {
    "SessionStart": [
      { "matcher": "", "hooks": [{ "type": "command", "command": "echo unrelated-hook" }] }
    ]
  }
}
EOF

# Pre-existing global CLAUDE.md with user content plus a managed block of
# 2 bullets (init's safety-import must rescue both into the store):
cat > "$FAKE_CLAUDE/CLAUDE.md" <<'EOF'
# My own notes

Keep this user content.

<!-- retro:managed:start -->
## Retro-Discovered Patterns

- Always run cargo test before committing changes
- Never commit directly to the main branch

<!-- retro:managed:end -->
EOF
```

## Steps

1. Run `printf 'n\n' | $RETRO init` (answer "n" to the backup-remote prompt) and capture output
2. Run `ls "$RETRO_HOME/knowledge/global" "$RETRO_HOME/knowledge/projects"` and `git -C "$RETRO_HOME" log --oneline`
3. Run `cat "$FAKE_CLAUDE/settings.json"` and `ls "$RETRO_HOME/backups/"`
4. Run `$RETRO status` and capture output
5. Run `$RETRO doctor; echo "doctor exit: $?"` and capture output
6. Run `$RETRO uninstall` and capture output
7. Run `cat "$FAKE_CLAUDE/settings.json"` and `cat "$FAKE_CLAUDE/CLAUDE.md"` and `ls "$RETRO_HOME/knowledge"`
8. Run `printf 'yes\n' | $RETRO uninstall --purge` and capture output
9. Run `test -d "$RETRO_HOME" && echo "still there" || echo "gone"` and `ls "$HOME"` (rescued backups dir)

## Expected

- Step 1: init reports the store repo initialized at `$RETRO_HOME`, an indexed
  node count, "Imported 2 existing rule(s) from your CLAUDE.md managed section",
  and "Installed v3 hooks in $FAKE_CLAUDE/settings.json"
- Step 2: both `knowledge/global` and `knowledge/projects` exist; `knowledge/global`
  contains 2 rescued node files; git log shows the "retro: initialize store" commit
- Step 3: settings.json contains a SessionEnd hook running `retro observe` (absolute
  binary path) and a SessionStart hook running `retro brief`; the pre-existing
  `echo unrelated-hook` entry and the `permissions` block are still present;
  `$RETRO_HOME/backups/` contains a timestamped backup of the original settings.json
- Step 4: `retro status` shows the v3 block: "v3 knowledge store", 2 active nodes
  (2 global, 0 project), 0 pending sessions, and the daily AI-call budget
- Step 5: doctor prints per-check lines; store-present, store-repo, index, hooks,
  queue, and projection all pass. The claude-cli check is tolerated either way
  (the claude CLI may be absent in a sandbox); exit code is 0 when claude-cli
  passes too
- Step 6: uninstall reports removing the SessionEnd/SessionStart hooks from
  settings.json and removing the managed section from `$FAKE_CLAUDE/CLAUDE.md`,
  then "store kept at $RETRO_HOME (use --purge to delete it)" and "retro uninstalled."
- Step 7: settings.json no longer contains `retro observe`/`retro brief` but still
  contains `echo unrelated-hook` and the permissions block; CLAUDE.md still has
  "My own notes"/"Keep this user content." but no `retro:managed` markers and no
  bullet text; `$RETRO_HOME/knowledge` still exists (store kept)
- Step 8: `--purge` warns that deletion includes git history, names the rescue
  location for backups, asks to type 'yes', then reports "removed $RETRO_HOME"
  and "retro uninstalled."
- Step 9: `$RETRO_HOME` is gone; `$HOME` contains a `.retro-uninstall-backups-*`
  directory holding the rescued backups

## Not Expected

- No write outside `$RETRO_HOME`, `$FAKE_CLAUDE`, and the fake `$HOME`
- No mention of the real user home paths (`/Users/*/.retro`, `/Users/*/.claude`) in any output
- No launchd or plist output (a fresh v3 install has nothing to remove)
- No panics, stack traces, or "error:" lines
- No AI calls (`claude -p`); doctor's `claude --version` probe is the only claude invocation
