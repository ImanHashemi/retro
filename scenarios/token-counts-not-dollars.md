# Scenario: Token counts displayed instead of dollar costs

## Description
After the post-v0.1 fix, `retro analyze` should display token counts ("Tokens: X in / Y out") instead of dollar costs ("AI cost: $0.0000"). This verifies the token tracking pipeline is correctly wired through BackendResponse → AnalyzeResult → CLI display.

## Setup
1. Run `./target/debug/retro init`
2. Run `./target/debug/retro ingest`

## Steps
1. Run `./target/debug/retro analyze --dry-run 2>&1`
2. Run `grep -r "cost" ./crates/retro-cli/src/commands/analyze.rs || echo "NO_COST_REFS"`
3. Run `grep -r "Tokens" ./crates/retro-cli/src/commands/analyze.rs || echo "NO_TOKEN_REFS"`

## Expected
- Step 1 exits successfully
- Step 2 output shows "NO_COST_REFS" (no dollar cost references in analyze command)
- Step 3 output contains "Tokens" (token display code exists)
- The analyze.rs source contains "input_tokens" and "output_tokens" field references

## Not Expected
- No "AI cost" or "cost_usd" in analyze.rs source code
- No "$" dollar sign formatting in analyze.rs token display
- No panic or crash
