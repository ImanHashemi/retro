# Scenario: Analyze completes with real AI call

## Description
`retro analyze` (without `--dry-run`) should successfully call the Claude CLI with `--json-schema` structured output, parse the response from the `structured_output` field, and report results. This is the integration smoke test that verifies the full AI pipeline works end-to-end, including the `--json-schema` constrained decoding path.

This scenario requires the `claude` CLI to be installed and available on PATH.

## Setup
1. Run `./target/debug/retro init`
2. Run `./target/debug/retro ingest`

## Steps
1. Run `unset CLAUDECODE && ./target/debug/retro analyze 2>&1`

## Expected
- Command exits successfully (exit code 0)
- Output contains "Analysis complete!" (confirming full run, not dry-run)
- Output contains "Sessions analyzed:" with a number
- Output contains "Tokens:" with token counts (proves AI was called)
- Output contains "New patterns:" or "Updated patterns:" (analysis produced results)

## Not Expected
- No "empty result" error (would indicate structured_output field not handled)
- No "failed to parse AI response as JSON" error (would indicate schema mismatch)
- No "error_max_turns" in output (would indicate --max-turns too low)
- No panic or crash
- No "Dry run" text (this is a real run)
