# Scenario: Curate completes agentic rewrite with real AI call

## Description
`retro curate` (without `--dry-run`) should execute an agentic AI call that explores the codebase, generates a full CLAUDE.md rewrite, and present a diff to the user. This is the integration smoke test for the full curate pipeline including the agentic `execute_agentic` path (no `--json-schema`, no `--max-turns`, full tool access).

Since this is a non-interactive test, we pipe "n" to decline the PR creation prompt so no branches or PRs are created.

This scenario requires the `claude` CLI to be installed and available on PATH.

## Setup
1. Run `./target/debug/retro init`
2. Run `./target/debug/retro ingest`
3. Run `sqlite3 ~/.retro/retro.db "INSERT INTO patterns (id, pattern_type, description, confidence, times_seen, first_seen, last_seen, last_projected, status, source_sessions, related_files, suggested_content, suggested_target, project, generation_failed) VALUES ('curate-ai-test-' || strftime('%s','now'), 'recurring_instruction', 'Curate AI test: always run cargo test before committing', 0.90, 4, datetime('now'), datetime('now'), NULL, 'discovered', '[]', '[]', 'Always run cargo test before committing changes', 'claude_md', NULL, 0);"`
4. Run `grep -q 'full_management' ~/.retro/config.toml || printf '\n[claude_md]\nfull_management = true\n' >> ~/.retro/config.toml`
5. Run `sed -i '' 's/full_management = false/full_management = true/' ~/.retro/config.toml`

## Steps
1. Run `echo "n" | unset CLAUDECODE && ./target/debug/retro curate 2>&1`

## Expected
- Command exits successfully (exit code 0)
- Output contains "retro curate" (command heading)
- Output contains "CLAUDE.md:" with a line count
- Output contains "Patterns:" with a count
- Output contains "Running agentic CLAUDE.md rewrite" (AI call started)
- Output contains "CLAUDE.md (current)" or "CLAUDE.md (proposed)" (diff was shown)
- Output contains "Create a PR with this rewrite?" (confirmation prompt reached)
- Output contains "Discarded" (user declined with "n")

## Not Expected
- No "full_management" error (setup enables it)
- No "claude CLI not found" error (claude must be on PATH)
- No "empty result" error (would indicate agentic call failed)
- No "failed to spawn claude CLI" error
- No panic or crash
- No "Dry run" text (this is a real run)
- No "PR created" (user declined)
