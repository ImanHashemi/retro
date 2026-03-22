# Scenario: v2 Pipeline Dry-Run

## Description

Tests the v2 pipeline (`retro run`) in dry-run mode, end-to-end. Verifies that the pipeline executes all four steps (observe, ingest, analyze, summary) without making AI calls, shows correct output format, and that `retro analyze --dry-run` still works (v1 backward compatibility). Also tests that `retro dash --help` works and that all new commands are registered.

This scenario makes no AI calls and creates no PRs — it's fast and free.

## Setup

```bash
# Ensure retro is initialized
cargo build
cargo run -- init 2>/dev/null || true
# Stop the scheduled runner to avoid interference
cargo run -- stop 2>/dev/null || true
```

## Steps

1. Run `cargo run -- run --dry-run` and capture output
2. Run `cargo run -- run --dry-run --verbose` and capture output (should show per-session details)
3. Run `cargo run -- analyze --dry-run` and capture output (v1 command still works)
4. Run `cargo run -- ingest` and capture output (fast, no AI)
5. Run `cargo run -- status` and capture output
6. Run `cargo run -- --help` and capture output (verify all commands listed)
7. Run `cargo run -- dash --help` and capture output
8. Run `cargo run -- start --help` and capture output
9. Run `cargo run -- stop --help` and capture output

## Expected

- `retro run --dry-run` shows "Step 1/4", "Step 2/4", "Step 3/4", "Step 4/4" pipeline progression
- `retro run --dry-run` shows "Dry run" or "dry-run" in the output
- `retro run --dry-run` mentions "modified session" in Step 1
- `retro run --dry-run --verbose` shows more detail than the non-verbose version (e.g., file paths or session counts)
- `retro analyze --dry-run` completes without error (v1 backward compatibility)
- `retro ingest` completes without error
- `retro status` shows session counts and database info
- `retro --help` lists "start", "stop", "dash", "run" among the available commands
- `retro dash --help` mentions "dashboard" or "TUI" or "Open"
- `retro start --help` mentions "runner" or "scheduled" or "launchd"
- `retro stop --help` mentions "runner" or "scheduled" or "Stop"

## Not Expected

- No panics, crashes, or stack traces
- No "AI call" or "API" activity during dry-run steps
- `retro run --dry-run` should NOT show "retro run complete" with real data — it should show dry-run markers
