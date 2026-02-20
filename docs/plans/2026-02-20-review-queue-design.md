# Review Queue Design

**Date:** 2026-02-20
**Status:** Approved

## Problem

Retro has no feedback loop. When `retro apply` runs (especially in auto-mode), it generates content and immediately writes files / creates PRs. Users cannot:

- Reject unwanted patterns or skills
- Review what retro wants to do before it does it
- Provide feedback on closed PRs (patterns get stuck as `Active` forever)
- Undo personal projections (global agents auto-apply with no review)

The `Dismissed` pattern status exists in the enum but is never used.

## Solution

Add a review queue between content generation and execution. `retro apply` becomes generation-only. A new `retro review` command is the gate where users approve, skip, or dismiss items. A new `retro sync` command detects closed PRs and resets patterns.

## Projection Lifecycle

### Current

```
Pattern (Discovered) → apply → Projection created + Pattern (Active)
                                 ↓
                              files written + PR created (all at once)
```

### New

```
Pattern (Discovered) → apply → Projection (PendingReview) + content saved in DB
                                        ↓
                            retro review → user picks items
                                        ↓
                    ┌───────────────┬────────────────┐
                    ↓               ↓                ↓
              Apply (accepted)   Skip (stay pending)  Dismiss
                    ↓                                    ↓
           files written / PR created          Pattern → Dismissed
           Pattern → Active                   (never suggested again)
```

- **Apply**: files written (personal) or PR created (shared), pattern → `Active`
- **Skip**: item stays `PendingReview`, shown again next `retro review`
- **Dismiss**: pattern → `Dismissed`, projection deleted, never suggested again

## `retro review` Command

```
retro review [--auto] [--global] [--dry-run]
```

1. Run `retro sync` first (check PR status, reset closed PRs)
2. Query DB for projections with `status = pending_review`
3. Display numbered list:

```
Pending review (3 items):

  1. [skill]  Always use bun instead of npm for package management
     Target: .claude/skills/use-bun.md
     Source: seen 4 times across 3 sessions (confidence: 0.85)

  2. [rule]   Run clippy with -- -D warnings before committing Rust code
     Target: CLAUDE.md (managed section)
     Source: seen 6 times across 5 sessions (confidence: 0.92)

  3. [agent]  Pre-commit linting agent for Python projects
     Target: ~/.claude/global_agents/pre-commit-lint.md
     Source: seen 3 times across 2 sessions (confidence: 0.78)

Actions: apply (a), skip (s), dismiss (d), preview (p)
Enter selections (e.g., "1a 2a 3d" or "all:a"):
```

4. **preview (p)**: show full generated content for that item
5. After selections:
   - **apply** items → execute (write files, create PR for shared track)
   - **skip** items → stay `PendingReview`, shown again next time
   - **dismiss** items → pattern marked `Dismissed`, projection deleted
6. Shared approved items batched into single PR (same as today's apply)
7. Personal approved items written directly

`--dry-run`: show the pending list without prompting for action.

## Changes to `retro apply`

`retro apply` becomes generation-only. It no longer writes files or creates PRs.

1. Same qualifying pattern selection (`get_qualifying_patterns`)
2. Same content generation (AI calls for skills, direct for rules, AI for agents)
3. Creates `Projection` records with `status = pending_review`
4. Pattern stays `Discovered` (not `Active`)
5. Prints: "Generated N items for review. Run `retro review` to approve."

`--dry-run` still skips AI calls entirely.

## Auto-Mode Pipeline

```
post-commit hook
  → retro ingest --auto
    → retro analyze --auto (if cooldown + data gate pass)
      → retro apply --auto (if cooldown + data gate pass)
        → generates content, saves as PendingReview
        → does NOT call retro review
  → nudge on next interactive command: "N items pending review"
```

Auto-mode does the expensive work (ingestion, analysis, generation) in the background. The human decision stays with the user via `retro review`.

## `retro sync` Command

```
retro sync
```

1. Query projections with `status = applied` and `pr_url IS NOT NULL`
2. For each unique PR URL: `gh pr view <url> --json state`
3. If `CLOSED` (not merged):
   - Delete projection record
   - Reset pattern → `Discovered`
   - Log audit: `sync_reset`
4. If `MERGED`: no action
5. Print summary: "Reset N patterns from closed PR #X back to discoverable"

Runs automatically at the start of `retro apply --auto` and `retro review`.

## Database Schema Changes

Schema version bumps from 2 → 3.

```sql
ALTER TABLE projections ADD COLUMN status TEXT NOT NULL DEFAULT 'applied';
```

Values: `pending_review`, `applied`, `dismissed`.

Existing rows default to `applied` — no disruption to already-projected items.

No new tables needed.

### Query Changes

| Query | Current | New |
|-------|---------|-----|
| `get_qualifying_patterns` | Excludes patterns with any projection | Excludes patterns with `applied` or `pending_review` projection, AND excludes `dismissed` patterns |
| `get_projections` | Returns all | Accepts optional status filter |
| New: `get_pending_reviews` | N/A | `SELECT * FROM projections WHERE status = 'pending_review'` |
| New: `update_projection_status` | N/A | Sets status to `applied` or `dismissed` |
| New: `delete_projection` | N/A | Remove record and reset pattern to `Discovered` |

## Nudge Updates

New nudge message type:

```
retro: 3 items pending review — run `retro review`
```

Replaces "Pull request created" nudge for new items. Existing PR nudges still work for already-applied projections.

## Audit Entries

Four new audit actions:

| Action | When | Details |
|--------|------|---------|
| `apply_generated` | `retro apply` generates content | `{patterns_generated: N, project: "..."}` |
| `review_applied` | User approves in review | `{patterns: [...], pr_url: "..." (if shared)}` |
| `review_dismissed` | User dismisses in review | `{patterns: [...]}` |
| `sync_reset` | PR closed, patterns reset | `{patterns: [...], pr_url: "..."}` |

## What Doesn't Change

- Ingestion pipeline
- Analysis pipeline and prompts
- Content generation logic (two-phase skills, rules, agents)
- `retro patterns`, `retro status`, `retro log`, `retro clean`, `retro audit`
- `--dry-run` behavior on apply and audit
- Lockfile, cooldowns, session cap
- CLAUDE.md managed section format
- Skill file format
- Backup strategy

## Future Path (Not In This Design)

- **PR-based review**: `retro apply` creates PR directly, comments on PR drive accept/reject/revise via AI
- **AI-assisted revision**: "make this skill more specific" feedback loop from review
- **`retro dismiss <pattern-id>`**: standalone command shortcut without going through review
