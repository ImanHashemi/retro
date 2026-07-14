# Scenario Tests

End-to-end test scenarios for the `retro` CLI. Each `.md` file describes one test scenario that the `/run-scenarios` skill executes.

The v2-era scenarios (`v2-backward-compat.md`, `v2-init-and-lifecycle.md`, `v2-pipeline-dry-run.md`) were replaced at 3.0.0 by the v3 suite (`v3-init-and-lifecycle.md`, `v3-pipeline-dry-run.md`, `v3-migrate.md`).

## HARD RULE: isolation preamble

Scenarios run real lifecycle commands (`retro init`, `retro migrate`, `retro uninstall --purge`) that would otherwise mutate the live machine. Every scenario MUST begin its Setup with this preamble, and every retro command in a scenario MUST run under it:

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
```

Non-negotiable rules for the agent executing scenarios:

- **NEVER run any retro command without `RETRO_HOME` set** to a temp dir whose `config.toml` points `[paths] claude_dir` at another temp dir.
- **Override `HOME`** for every step: `uninstall --purge` writes its backup-rescue dir to `$HOME`, and doctor/migrate/uninstall probe `$HOME/Library/LaunchAgents` for the v2 plist.
- **Stub `launchctl` on `PATH`**: migrate and uninstall call `launchctl bootout gui/$UID/com.retro.runner` for v2 cleanup — without the stub that escapes the sandbox and can unload a real runner.
- **Use `./target/release/retro`** (or a debug build) — never a `retro` binary from `PATH`.
- **Answer interactive prompts via stdin pipes**, never a TTY: `printf 'n\n' | $RETRO init` (backup-remote y/N prompt) and `printf 'yes\n' | $RETRO uninstall --purge` (typed-yes confirmation).
- **Abort immediately** if any output mentions a real home path (`/Users/<you>/.retro`, `/Users/<you>/.claude`).
- **Clean up** every temp dir afterwards and kill only processes the scenario itself spawned (e.g. the `retro ui` server).

## Format

```markdown
# Scenario: Short Title

## Description
What this scenario tests and why.

## Setup
Commands to ensure preconditions (run before test steps).
MUST start with the isolation preamble above.

## Steps
1. Run `command here`
2. Run `another command`

## Expected
- Output contains "some text"
- Command exits successfully

## Not Expected
- No "error" or panic in output
```

## Sections

| Section | Required | Purpose |
|---------|----------|---------|
| Description | Yes | Context for the test |
| Setup | Yes | Isolation preamble + preconditions |
| Steps | Yes | Commands to execute |
| Expected | Yes | Conditions that must be true |
| Not Expected | No | Conditions that must NOT be true |

## Adding a scenario

1. Create a new `.md` file in this directory
2. Follow the format above — isolation preamble first
3. Run `/run-scenarios scenarios/your-file.md` to test it
4. Run `/run-scenarios` to run all scenarios

## Tips

- Scenarios that use `--dry-run` don't make AI calls and are fast/free
- Setup should be idempotent (safe to run multiple times — mktemp makes reruns naturally fresh)
- Expected/Not Expected are evaluated by the AI agent using natural language judgment
- For idempotency tests, the Steps section should run the command twice (see `v3-migrate.md`)
