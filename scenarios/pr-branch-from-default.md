# Scenario: PR branch is created from default branch, not current HEAD

## Description
When `retro apply` creates a branch for shared changes, it must branch from the repo's default branch (e.g., `origin/main`), NOT from the user's current HEAD. This prevents retro PRs from accidentally including unrelated feature branch commits.

This scenario tests the git plumbing by simulating what `execute_shared_with_pr` does: detect the default branch, fetch it, create a branch from `origin/<default>`, and verify it doesn't contain commits from the user's working branch.

## Setup
1. Run `./target/debug/retro init`

## Steps
1. Run `RETRO_DEFAULT_BRANCH=$(gh repo view --json defaultBranchRef -q '.defaultBranchRef.name') && echo "default_branch=$RETRO_DEFAULT_BRANCH"`
2. Run `git fetch origin $RETRO_DEFAULT_BRANCH 2>&1 && echo "fetch_ok"`
3. Run `RETRO_TEST_BRANCH="retro/scenario-test-$(date +%s)" && git checkout -b "$RETRO_TEST_BRANCH" "origin/$RETRO_DEFAULT_BRANCH" 2>&1 && echo "branch=$RETRO_TEST_BRANCH"`
4. Run `git log --oneline -5 HEAD 2>&1`
5. Run `MAIN_TIP=$(git rev-parse "origin/$RETRO_DEFAULT_BRANCH") && BRANCH_MERGE_BASE=$(git merge-base HEAD "origin/$RETRO_DEFAULT_BRANCH") && echo "main_tip=$MAIN_TIP" && echo "merge_base=$BRANCH_MERGE_BASE" && if [ "$MAIN_TIP" = "$BRANCH_MERGE_BASE" ]; then echo "BRANCH_IS_BASED_ON_DEFAULT=true"; else echo "BRANCH_IS_BASED_ON_DEFAULT=false"; fi`
6. Run `git checkout - 2>&1 && git branch -D "$RETRO_TEST_BRANCH" 2>&1 && echo "cleanup_ok"`

## Expected
- Step 1 outputs a default branch name (e.g., "default_branch=main")
- Step 2 shows "fetch_ok" (fetch succeeded)
- Step 3 creates the branch successfully
- Step 5 shows "BRANCH_IS_BASED_ON_DEFAULT=true" (the branch's merge-base with origin/default equals origin/default's tip, proving it was branched from there)
- Step 6 shows "cleanup_ok" (test branch deleted)

## Not Expected
- No errors or failures in any step
- Step 5 should NOT show "BRANCH_IS_BASED_ON_DEFAULT=false"
