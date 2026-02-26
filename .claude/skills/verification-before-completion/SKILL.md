---
name: verification-before-completion
description: Use when about to claim work is complete, fixed, or passing, before committing or creating PRs - requires running verification commands and confirming output before making any success claims; evidence before assertions always
---

Before claiming any work is complete, fixed, or passing:

## Required Verification Steps

1. **Run all automated tests first**
   - Execute scenario tests: `cargo test --test scenarios`
   - Run unit tests: `cargo test`
   - Verify all tests pass with clean output

2. **Provide clean local verification commands**
   - Full uninstall: `retro init --uninstall --purge`
   - Fresh install from current branch: `cargo install --path . --force`
   - List specific retro commands to verify the fix in a real project

3. **Execute and confirm real-world behavior**
   - Actually run the commands on a real project
   - Paste the command output showing success
   - Never claim "it should work" without evidence

## Evidence Before Assertions

- ❌ "The fix is complete and tests pass"
- ✅ "Tests pass (output: ...), verified with clean install: `retro <command>` shows <expected behavior>"

User performs final manual verification before merging.