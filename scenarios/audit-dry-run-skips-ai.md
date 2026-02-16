# Scenario: Audit dry-run skips AI calls

## Description
`retro audit --dry-run` should show a context summary (CLAUDE.md size, skill count, MEMORY.md size, agent count) without making any AI calls. It should complete nearly instantly since no Claude CLI invocation happens.

## Setup
1. Run `./target/debug/retro init`

## Steps
1. Run `time ./target/debug/retro audit --dry-run 2>&1`

## Expected
- Command exits successfully (exit code 0)
- Output contains "Dry run" (confirming dry-run mode)
- Output contains "Context to audit" or similar context summary heading
- Output mentions CLAUDE.md (present or not present)
- Output mentions Skills
- Completes in under 5 seconds (no AI call)

## Not Expected
- No "This may take a minute" message (that indicates an AI call is starting)
- No "claude CLI not found" error (dry-run skips CLI check)
- No panic or crash
- No "Tokens:" line (no AI call means no token usage)
