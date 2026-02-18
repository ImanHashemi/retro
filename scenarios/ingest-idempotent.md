# Scenario: Ingest is idempotent

## Description
Running `retro ingest` twice should skip sessions whose files haven't changed. Retro uses file size and mtime to detect changes — if a session's JSONL file grew between runs (e.g., an active Claude Code session appended entries), it will correctly be re-ingested. In multi-agent environments many files may change between runs, so we only check that *some* sessions were skipped (proving the dedup logic works) rather than asserting an exact count.

## Setup
1. Run `./target/debug/retro init`

## Steps
1. Run `./target/debug/retro ingest 2>&1`
2. Run `./target/debug/retro ingest 2>&1`

## Expected
- Both commands exit successfully (exit code 0)
- Second run's "Sessions skipped" count is greater than 0 (unchanged sessions were recognized and skipped)
- Second run's "Sessions found" count is equal to or greater than first run's "Sessions found" count (no sessions disappeared)

## Not Expected
- No errors or panics on second run
- No duplicate session warnings
- Second run should NOT show "Sessions skipped: 0" (at least some sessions must be unchanged between two rapid runs)
