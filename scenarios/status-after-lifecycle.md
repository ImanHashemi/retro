# Scenario: Status shows correct counts after lifecycle

## Description
After running init + ingest, `retro status` should show correct session counts, database info, and configuration. This validates the full read path of the status command.

## Setup
1. Run `./target/debug/retro init`
2. Run `./target/debug/retro ingest`

## Steps
1. Run `./target/debug/retro status 2>&1`

## Expected
- Command exits successfully (exit code 0)
- Output contains "retro status" heading
- Output contains "Database:" with a path
- Output contains "WAL mode:" showing "enabled"
- Output contains "Sessions" section
- Output contains "Ingested:" with a number (may be 0 if no Claude Code sessions exist)
- Output contains "Patterns" section
- Output contains "Configuration" section with "Analysis window" and "AI backend"

## Not Expected
- No errors or panics
- No "not initialized" error (setup runs init first)
- No "WAL mode: disabled" (WAL should always be enabled)
