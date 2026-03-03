# Full CLAUDE.md Management — Design Doc

**Date:** 2026-02-26
**Status:** Approved

## Problem

Retro currently only writes to a managed section within CLAUDE.md (`<!-- retro:managed:start/end -->` delimiters). Users want retro to also improve, reorganize, and deduplicate the rest of their CLAUDE.md — the user-written content outside the delimiters.

## Solution: Two-Mode System ("Audit + Rewrite")

A config-gated opt-in (`full_management = true`) that unlocks two complementary modes:

1. **Granular edits** — the existing `retro apply` → `retro review` pipeline gains the ability to propose edits (add/remove/reword/move) to user-written CLAUDE.md content. Each edit is a separate review queue item.
2. **Full rewrite** — a new `retro curate` command that generates a complete CLAUDE.md rewrite using agentic AI with full codebase access. Creates a PR for review.

## Config

New section in `~/.retro/config.toml`:

```toml
[claude_md]
full_management = false  # default: managed-section only
```

When `false` (default): everything works exactly as today.

When `true`: both granular edits and `retro curate` are enabled. Managed delimiters are dissolved on first run.

## Mode 1: Granular Edits (via `retro apply` → `retro review`)

### New Edit Types

When `full_management = true`, the analysis AI examines the full CLAUDE.md and can propose:

- **Add** — new rule/section (same as today)
- **Remove** — delete a stale or redundant rule
- **Reword** — improve clarity/accuracy of an existing rule
- **Move** — relocate a rule to a better section

### Analysis Schema Extension

The analysis JSON response gains an optional `claude_md_edits` array:

```json
{
  "patterns": [...],
  "claude_md_edits": [
    {
      "edit_type": "reword",
      "original_text": "No async",
      "suggested_content": "Sync only — no tokio, no async",
      "reasoning": "Original is too terse"
    },
    {
      "edit_type": "remove",
      "original_text": "Use cargo fmt before committing",
      "reasoning": "Redundant — pre-commit hook enforces this"
    }
  ]
}
```

Only present when `full_management = true` and the prompt requests it. Backward-compatible.

### Review Queue Integration

Each edit appears as a separate item in `retro review`:

```
1. [rule+]  Add: "Use thiserror in library crates, anyhow in binary crates"
2. [rule~]  Reword: "No async" -> "Sync only — no tokio, no async..."
3. [rule-]  Remove: "Use cargo fmt before committing" (redundant with pre-commit hook)
4. [rule>]  Move: "SQLite WAL mode" from Architecture -> Key Design Decisions
```

Same `1a 2a 3d` / `all:a` approval syntax.

### Execution

- **Add**: Append rule to CLAUDE.md
- **Remove**: Find `original_text` in file, remove it
- **Reword**: Find `original_text`, replace with `suggested_content`
- **Move**: Remove from old location, insert at target section

All edits batched into single file write. Shared track — delivered via PR.

### Data Model

Granular edits stored as JSON in existing `projections.content` column:

```json
{"edit_type": "reword", "original": "No async", "replacement": "Sync only — no tokio..."}
```

No DB schema changes needed.

## Mode 2: Full Rewrite (via `retro curate`)

### Command

```
retro curate [--dry-run] [--verbose]
```

Requires `full_management = true` — errors with helpful message if not set.

### Context Gathering — Two-Phase

**Phase 1: Seed context (always included)**
1. Existing CLAUDE.md — full file content
2. Retro patterns — all discovered patterns at confidence >= threshold
3. MEMORY.md — Claude Code's memory file (read-only)
4. Session insights — recent analysis reasoning summaries
5. Project tree — full directory listing (filtered: no target/, .git/, node_modules/, etc.)

**Phase 2: AI-driven exploration**

The audit AI call is a multi-turn agentic call. The AI receives seed context and the project tree, then decides what to read. It has access to file-reading tools (via Claude CLI's built-in tool use). This makes retro fully language-agnostic — the AI figures out what matters.

### AI Call Parameters

- No `--max-turns` — unlimited, let the AI decide when it's done
- No `--tools ""` — the model needs tool access to read files
- No `--json-schema` — output is raw markdown

### User Flow

```
$ retro curate

Gathering context...
  CLAUDE.md: 142 lines
  Patterns: 12 qualifying (>=0.7 confidence)
  MEMORY.md: 45 lines
  Project tree: 87 files

Exploring codebase and generating rewrite...
(this is an agentic AI call — it may read files and take a minute or two)

Done. Proposed rewrite: 128 lines (was 142)

--- CLAUDE.md (current)
+++ CLAUDE.md (proposed)
@@ -1,8 +1,6 @@
-# Retro — Active Context Curator for AI Coding Agents
-
-Rust CLI tool that does stuff.
+# Retro — Active Context Curator
+Rust CLI tool that analyzes Claude Code session history...
...

Create a PR with this rewrite? You can review and edit the PR on GitHub
before merging if you want to tweak anything. [y/N]
> y

Creating branch retro/curate-2026-02-26...
Pushing and creating PR...

PR created: https://github.com/user/repo/pulls/43
Edit the PR on GitHub if needed, then merge when ready.
```

### Rejection Flow

```
Create a PR with this rewrite? You can review and edit the PR on GitHub
before merging if you want to tweak anything. [y/N]
> n

Rewrite discarded. No changes made.
```

### Dry Run

```
$ retro curate --dry-run

Would gather context:
  CLAUDE.md: 142 lines
  Patterns: 12 qualifying
  MEMORY.md: 45 lines
  Project tree: 87 files

Dry run — skipping AI call. No changes made.
```

### Delivery

PR on `retro/curate-{YYYYMMDD-HHMMSS}` branch. The PR is the review mechanism — user reviews diff on GitHub, edits files in the PR if needed, merges when ready.

### Tracking

No projection stored. Audit log entry tracks it: `curate_applied` or `curate_rejected` with `pr_url`, token counts, before/after line counts.

## Safety & Migration

### Backups

Before any write (granular or full rewrite), backup current CLAUDE.md to `~/.retro/backups/CLAUDE.md.<timestamp>`.

### Enabling `full_management`

On first run after enabling:
1. Detect managed delimiters in CLAUDE.md
2. Dissolve them: strip `<!-- retro:managed:start/end -->` markers and `## Retro-Discovered Patterns` header, keep rule content in place
3. First `retro curate` naturally integrates those rules into the reorganized document

### Disabling `full_management`

When the user sets `full_management = false`:
- Granular edits stop — analysis no longer proposes edits to user content
- `retro curate` refuses to run (with helpful message)
- Next `retro apply` recreates managed delimiters if they don't exist, returns to append-only behavior
- No destructive changes to current CLAUDE.md

## Explicitly NOT Building

- No token budget config — AI manages its own context
- No max-turns config for curate — unlimited by default
- No staging file / local editing workflow — PR is the review mechanism
- No interactive diff editor — GitHub handles this
- No auto-scheduling of audits — user runs `retro curate` when they want
- No `retro curate --auto` — always user-initiated
- No changes to existing `retro audit` command (context staleness/archiving)

## Summary

| | Granular | Full Rewrite |
|---|---|---|
| **Command** | `retro apply` -> `retro review` | `retro curate` |
| **Trigger** | Automatic (part of normal pipeline) | User-initiated |
| **AI approach** | Extended analysis prompt, schema call | Agentic multi-turn, unlimited turns, tool access |
| **Context** | Session history + existing CLAUDE.md | Everything: CLAUDE.md, patterns, MEMORY.md, codebase (AI explores) |
| **Output** | Individual edits (add/remove/reword/move) | Complete CLAUDE.md rewrite |
| **Review** | Review queue (`retro review`, per-item approve) | Diff preview -> PR (edit on GitHub) |
| **Delivery** | PR via shared track (same as today) | PR on `retro/curate-{date}` branch |
