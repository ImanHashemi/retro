---
name: check-docs-after-merge
description: Use when a PR is merged, when completing a significant feature implementation, when user says "update documentation", or after changes to core functionality. Keywords: merge, documentation, CLAUDE.md, README.
---

After merging PRs or completing significant changes, proactively audit documentation for updates:

1. **Check CLAUDE.md** — Does it reflect new commands, architecture changes, or implementation status?
2. **Check README.md** — Does it need updated usage examples, feature descriptions, or installation steps?
3. **Check PLAN.md** (if present) — Should completed phases be marked DONE?
4. **Check any domain-specific docs** — API docs, architecture diagrams, etc.

Suggest specific documentation updates rather than waiting to be asked. For example:
- "The new `--json-schema` behavior should be documented in CLAUDE.md's Key Design Decisions section"
- "README.md's Quick Start should mention the new `retro review` command"