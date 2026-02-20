# Scenario: Review queue creates a real PR on GitHub

## Description
Tests the full apply → review workflow: `retro apply` generates content and saves it as PendingReview, then `retro review` approves all items and creates a PR on GitHub. Seeds a CLAUDE.md pattern directly in the DB, runs `retro apply` to generate, runs `retro review` with "all:a" to approve, and verifies the PR exists on GitHub with the correct base branch. Cleans up by closing the PR, deleting the remote branch, and restoring the original branch.

This tests the real two-step workflow: `save_plan_for_review` → `execute_shared_with_pr`.

## Setup
1. Run `./target/debug/retro init`
2. Run `./target/debug/retro ingest`
3. Run `sqlite3 ~/.retro/retro.db "INSERT INTO patterns (id, pattern_type, description, confidence, times_seen, first_seen, last_seen, last_projected, status, source_sessions, related_files, suggested_content, suggested_target, project, generation_failed) VALUES ('e2e-test-pattern-' || strftime('%s','now'), 'recurring_instruction', 'E2E test: always run tests before committing', 0.95, 5, datetime('now'), datetime('now'), NULL, 'discovered', '[]', '[]', 'Always run tests before committing changes', 'claude_md', NULL, 0);"`

## Steps
1. Run `unset CLAUDECODE && ./target/debug/retro apply 2>&1`
2. Run `./target/debug/retro review --dry-run 2>&1`
3. Run `echo "all:a" | ./target/debug/retro review 2>&1`
4. Run `RETRO_BRANCH=$(git branch -r --list 'origin/retro/updates-*' --sort=-committerdate | head -1 | tr -d ' ') && echo "remote_branch=$RETRO_BRANCH"`
5. Run `RETRO_PR_URL=$(gh pr list --head "$(echo $RETRO_BRANCH | sed 's|origin/||')" --json url -q '.[0].url' 2>&1) && echo "pr_url=$RETRO_PR_URL"`
6. Run `RETRO_PR_NUMBER=$(echo "$RETRO_PR_URL" | grep -oE '[0-9]+$') && gh pr view "$RETRO_PR_NUMBER" --json number,title,baseRefName,headRefName,state 2>&1`
7. Run `gh pr close "$RETRO_PR_NUMBER" --delete-branch 2>&1 && echo "cleanup_pr_ok"`
8. Run `git checkout main 2>&1; git branch -D "$(echo $RETRO_BRANCH | sed 's|origin/||')" 2>/dev/null; echo "cleanup_local_ok"`

## Expected
- Step 1 output contains "Content generated!" (retro apply saved items for review)
- Step 1 output contains "Items queued for review:" with a number greater than 0
- Step 1 output contains "retro review" (tells user to run review next)
- Step 2 output contains "pending review" and shows at least 1 item (dry-run lists pending items)
- Step 3 output contains "Review complete!" (retro review approved items)
- Step 3 output contains "Applied:" with a number greater than 0
- Step 3 output contains "Pull request:" with a URL
- Step 4 finds a remote branch matching "origin/retro/updates-"
- Step 5 finds a PR URL
- Step 6 shows PR details: baseRefName should be "main" (the default branch), headRefName should contain "retro/updates-", state should be "OPEN" or "MERGED"
- Step 7 shows "cleanup_pr_ok"
- Step 8 shows "cleanup_local_ok"

## Not Expected
- Step 1 should NOT contain "No patterns qualify" (we seeded a pattern)
- Step 2 should NOT contain "No items pending review" (apply just queued items)
- Step 3 should NOT contain "No items pending review" (items were queued by step 1)
- Step 6 baseRefName should NOT be anything other than the default branch
- No panics or crashes
