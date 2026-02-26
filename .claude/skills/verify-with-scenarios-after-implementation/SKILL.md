---
name: verify-with-scenarios-after-implementation
description: Use when completing implementation work, after fixing bugs, after merging branches, when making changes to core functionality, or when the user asks to verify changes. Ensures regression testing before considering work complete.
---

After completing any implementation work, always verify with scenario tests:

1. Run the full scenario test suite: invoke the `run-scenarios` skill
2. Wait for all tests to complete before proceeding
3. If any scenarios fail, investigate and fix before marking work complete
4. Only after all scenarios pass should you consider the implementation verified

This applies to:
- Bug fixes
- New features
- Refactoring work
- Dependency updates
- Any changes to core functionality

Do NOT skip scenario verification even if unit tests pass. Scenarios test real-world CLI behavior and catch integration issues that unit tests miss.