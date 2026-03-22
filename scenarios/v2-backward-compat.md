# Scenario: v2 Backward Compatibility and Deprecation

## Description

Tests that v1 commands still work in v2 and that the `--auto` deprecation warning is correctly shown. Verifies that `retro ingest --auto`, `retro analyze --auto --dry-run`, and `retro apply --auto --dry-run` all still function but emit deprecation warnings. Also verifies that `retro init` run twice is idempotent (shows "Exists" for already-created resources).

This ensures users upgrading from v1 have a smooth transition path.

## Setup

```bash
# Ensure retro is initialized
cargo build
cargo run -- init 2>/dev/null || true
cargo run -- stop 2>/dev/null || true
```

## Steps

1. Run `script -q /dev/null cargo run -- ingest --auto 2>&1` and capture output (uses pseudo-TTY for deprecation warning)
2. Run `cargo run -- analyze --auto --dry-run 2>&1` and capture output
3. Run `cargo run -- apply --auto --dry-run 2>&1` and capture output
4. Run `cargo run -- ingest` and capture output (non-auto still works)
5. Run `cargo run -- patterns` and capture output (v1 command still works)
6. Run `cargo run -- init` a second time and capture output (idempotency test)

## Expected

- Step 1: output contains "deprecated" and "retro start" (deprecation warning for `--auto`)
- Step 2: `analyze --auto --dry-run` completes without error
- Step 3: `apply --auto --dry-run` completes without error
- Step 4: `retro ingest` (without --auto) completes without error or deprecation warning
- Step 5: `retro patterns` runs without error (may show patterns or "no patterns")
- Step 6: second `retro init` shows "Exists" for resources that were already created (config, database)

## Not Expected

- No panics or crashes in any v1 command
- `retro ingest` (without --auto) should NOT show the deprecation warning
- No "command not found" or "unknown command" errors
