# Scenario: retro apply creates a real PR on GitHub

## Description
Tests that `retro apply` actually creates a PR on GitHub when there are shared (CLAUDE.md) patterns to project. Seeds a CLAUDE.md pattern directly in the DB, runs `retro apply` with confirmation, and verifies the PR exists on GitHub with the correct base branch. Cleans up by closing the PR, deleting the remote branch, and restoring the original branch.

This tests the real `execute_shared_with_pr` code path — not a manual reimplementation.

## Setup
1. Run `./target/debug/retro init`
2. Run `./target/debug/retro ingest`
3. Run `sqlite3 ~/.retro/retro.db "INSERT INTO patterns (id, pattern_type, description, confidence, times_seen, first_seen, last_seen, last_projected, status, source_sessions, related_files, suggested_content, suggested_target, project, generation_failed) VALUES ('e2e-test-pattern-' || strftime('%s','now'), 'recurring_instruction', 'E2E test: always run tests before committing', 0.95, 5, datetime('now'), datetime('now'), NULL, 'discovered', '[]', '[]', 'Always run tests before committing changes', 'claude_md', NULL, 0);"`

## Steps
1. Run `echo "y" | ./target/debug/retro apply 2>&1`
2. Run `RETRO_BRANCH=$(git branch -r --list 'origin/retro/updates-*' --sort=-committerdate | head -1 | tr -d ' ') && echo "remote_branch=$RETRO_BRANCH"`
3. Run `RETRO_PR_URL=$(gh pr list --head "$(echo $RETRO_BRANCH | sed 's|origin/||')" --json url -q '.[0].url' 2>&1) && echo "pr_url=$RETRO_PR_URL"`
4. Run `RETRO_PR_NUMBER=$(echo "$RETRO_PR_URL" | grep -oE '[0-9]+$') && gh pr view "$RETRO_PR_NUMBER" --json number,title,baseRefName,headRefName,state 2>&1`
5. Run `gh pr close "$RETRO_PR_NUMBER" --delete-branch 2>&1 && echo "cleanup_pr_ok"`
6. Run `git checkout main 2>&1; git branch -D "$(echo $RETRO_BRANCH | sed 's|origin/||')" 2>/dev/null; echo "cleanup_local_ok"`

## Expected
- Step 1 output contains "Apply complete" (retro apply succeeded)
- Step 1 output contains "Pull request created:" with a URL
- Step 1 output contains "Files written:" with a number greater than 0
- Step 2 finds a remote branch matching "origin/retro/updates-"
- Step 3 finds a PR URL
- Step 4 shows PR details: baseRefName should be "main" (the default branch), headRefName should contain "retro/updates-", state should be "OPEN" or "MERGED"
- Step 5 shows "cleanup_pr_ok"
- Step 6 shows "cleanup_local_ok"

## Not Expected
- Step 1 should NOT contain "No patterns qualify" (we seeded a pattern)
- Step 1 should NOT contain "Aborted" (we piped "y" for confirmation)
- Step 4 baseRefName should NOT be anything other than the default branch
- No panics or crashes
