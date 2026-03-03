# Scenario: Curate dry-run shows context summary without AI calls

## Description
`retro curate --dry-run` should show a context summary (CLAUDE.md size, pattern count, MEMORY.md availability, file tree size) without making any AI calls. It requires `full_management = true` in config. This validates the config gate, context gathering, and dry-run exit path.

## Setup
1. Run `./target/debug/retro init`
2. Run `./target/debug/retro ingest`
3. Run `sqlite3 ~/.retro/retro.db "INSERT INTO patterns (id, pattern_type, description, confidence, times_seen, first_seen, last_seen, last_projected, status, source_sessions, related_files, suggested_content, suggested_target, project, generation_failed) VALUES ('curate-test-' || strftime('%s','now'), 'recurring_instruction', 'Curate test pattern', 0.85, 3, datetime('now'), datetime('now'), NULL, 'discovered', '[]', '[]', 'Test content for curate', 'claude_md', NULL, 0);"`
4. Run `grep -q 'full_management' ~/.retro/config.toml || printf '\n[claude_md]\nfull_management = true\n' >> ~/.retro/config.toml`
5. Run `sed -i '' 's/full_management = false/full_management = true/' ~/.retro/config.toml`

## Steps
1. Run `./target/debug/retro curate --dry-run 2>&1`

## Expected
- Command exits successfully (exit code 0)
- Output contains "retro curate" (command heading)
- Output contains "CLAUDE.md:" with a line count
- Output contains "Patterns:" with a count and confidence threshold
- Output contains "File tree:" with an entry count
- Output contains "Dry run" (confirming dry-run mode)
- Completes in under 5 seconds (no AI call)

## Not Expected
- No "full_management" error (setup enables it)
- No "Running agentic" message (dry-run skips AI)
- No "This may take several minutes" message
- No panic or crash
- No "claude CLI not found" error (dry-run skips CLI check)
