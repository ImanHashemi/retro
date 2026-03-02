# Scenario: PR branch is created from default branch, not current HEAD

## Description
When `retro apply` creates a branch for shared changes, it must branch from the repo's default branch (e.g., `origin/main`), NOT from the user's current HEAD. This prevents retro PRs from accidentally including unrelated feature branch commits.

This scenario tests the git plumbing by simulating what `execute_shared_with_pr` does: detect the default branch, fetch it, create a branch from `origin/<default>`, and verify it doesn't contain commits from the user's working branch. Uses a git worktree so the user's working tree is never touched.

## Setup
1. Run `./target/debug/retro init`
2. Run `git worktree remove /tmp/retro-worktree-branch-test 2>/dev/null; git branch -D retro/scenario-worktree-test 2>/dev/null; echo "pre-clean done"`

## Steps
1. Run `RETRO_DEFAULT_BRANCH=$(gh repo view --json defaultBranchRef -q '.defaultBranchRef.name') && echo "default_branch=$RETRO_DEFAULT_BRANCH"`
2. Run `git fetch origin $RETRO_DEFAULT_BRANCH 2>&1 && echo "fetch_ok"`
3. Run `git worktree add -b retro/scenario-worktree-test /tmp/retro-worktree-branch-test "origin/$RETRO_DEFAULT_BRANCH" 2>&1 && echo "worktree_created"`
4. Run `git -C /tmp/retro-worktree-branch-test log --oneline -5 HEAD 2>&1`
5. Run `MAIN_TIP=$(git rev-parse "origin/$RETRO_DEFAULT_BRANCH") && BRANCH_MERGE_BASE=$(git -C /tmp/retro-worktree-branch-test merge-base HEAD "origin/$RETRO_DEFAULT_BRANCH") && echo "main_tip=$MAIN_TIP" && echo "merge_base=$BRANCH_MERGE_BASE" && if [ "$MAIN_TIP" = "$BRANCH_MERGE_BASE" ]; then echo "BRANCH_IS_BASED_ON_DEFAULT=true"; else echo "BRANCH_IS_BASED_ON_DEFAULT=false"; fi`
6. Run `git worktree remove /tmp/retro-worktree-branch-test 2>&1; git branch -D retro/scenario-worktree-test 2>&1; echo "cleanup_ok"`

## Expected
- Step 1 outputs a default branch name (e.g., "default_branch=main")
- Step 2 shows "fetch_ok" (fetch succeeded)
- Step 3 creates the worktree successfully and shows "worktree_created"
- Step 5 shows "BRANCH_IS_BASED_ON_DEFAULT=true" (the branch's merge-base with origin/default equals origin/default's tip, proving it was branched from there)
- Step 6 shows "cleanup_ok" (worktree removed, test branch deleted)

## Not Expected
- No errors or failures in any step
- Step 5 should NOT show "BRANCH_IS_BASED_ON_DEFAULT=false"
