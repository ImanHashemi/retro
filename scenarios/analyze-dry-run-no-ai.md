# Scenario: Analyze dry-run makes no AI calls

## Description
`retro analyze --dry-run` should show a preview of sessions that would be analyzed (count, message stats, batch count) without making any AI calls. It should complete nearly instantly.

## Setup
1. Run `./target/debug/retro init`
2. Run `./target/debug/retro ingest`

## Steps
1. Run `time ./target/debug/retro analyze --dry-run 2>&1`

## Expected
- Command exits successfully (exit code 0)
- Output contains "Dry run" (confirming dry-run mode)
- Output contains either session preview information OR "No new sessions to analyze"
- Completes in under 5 seconds (no AI call)

## Not Expected
- No "This may take a minute" message (that indicates an AI call is about to happen)
- No "Tokens:" line (no AI call means no token display)
- No panic or crash
- No "Analysis complete!" message (that only appears after real analysis)
