---
name: offer-to-merge-pr-after-review
description: Use when a PR review is complete, when the user says the PR is approved, when finishing code review, or when discussion of a PR seems concluded. Keywords: "review", "approved", "LGTM", "looks good", "PR".
---

When a PR review is complete and approved, proactively offer to merge:

1. Check that all review feedback has been addressed
2. Ask: "Should I merge this PR now?"
3. If yes, merge using the appropriate method:
   - Via GitHub CLI: `gh pr merge [PR-NUMBER] --merge` (or `--squash`/`--rebase` as preferred)
   - Or guide the user to merge via the web UI if that's preferred

Don't wait for explicit "merge the PR" instructions — offer proactively once review is complete.