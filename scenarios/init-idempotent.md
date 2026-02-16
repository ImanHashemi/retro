# Scenario: Init is idempotent

## Description
Running `retro init` twice should not duplicate git hooks or error out. The second run should show "Exists" for already-created resources rather than creating duplicates.

## Setup
None needed — init is the operation under test.

## Steps
1. Run `./target/debug/retro init 2>&1`
2. Run `./target/debug/retro init 2>&1`

## Expected
- Both commands exit successfully (exit code 0)
- Second run output contains "Exists" for config.toml and/or database (already created)
- Second run shows "already installed" or "Exists" for git hooks (not "Installed" again)
- Both runs end with "initialized successfully"

## Not Expected
- No errors or panics on second run
- No "duplicate" hook warnings
- Second run should NOT show "Installed" for hooks (they were already installed by first run)
