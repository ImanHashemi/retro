# Scenario: Ingest is idempotent

## Description
Running `retro ingest` twice should not re-ingest already-ingested sessions. The second run should report 0 new sessions ingested and skip all previously ingested ones. Note: an active Claude Code session (e.g., the one running this test) may cause 1 session to be re-ingested on subsequent runs because its JSONL file grows between invocations, changing the file mtime.

## Setup
1. Run `./target/debug/retro init`

## Steps
1. Run `./target/debug/retro ingest 2>&1`
2. Run `./target/debug/retro ingest 2>&1`

## Expected
- Both commands exit successfully (exit code 0)
- Second run shows "Sessions ingested: 0" or "Sessions ingested: 1" (at most 1 — the active session whose file grew between runs)
- Second run's "Sessions skipped" count should be greater than or equal to first run's "Sessions ingested" count (previously ingested sessions are now skipped)

## Not Expected
- No errors or panics on second run
- No duplicate session warnings
