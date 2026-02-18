# Auto-Mode Observability & Session Cap Design

**Date:** 2026-02-18
**Status:** Approved

## Problem

The auto-mode pipeline (post-commit hook → ingest → analyze → apply) runs silently in the background. When things go wrong — stuck processes, broken pipe errors, session overload — the user gets zero feedback. When things go right, there's also no indication that work was done. The only way to discover what happened is `retro status` or `retro log`.

Additionally, analyzing many sessions in auto mode (e.g., 41 on first install) triggers multi-minute AI calls that block the lock and prevent subsequent hook runs from doing useful work.

## Design Decisions

- **Feedback timing:** Next interactive `retro` command (expand existing nudge system)
- **Session cap behavior:** Skip analysis + nudge user to run manually
- **Nudge detail level:** Multi-line status block
- **Debug logging:** No separate debug log; audit.jsonl captures structured events, hook-stderr.log captures raw stderr
- **Warning suppression:** Not needed — stderr redirect to file captures everything without code changes

## 1. Enhanced Nudge System

### Current State

`check_and_display_nudge()` in `commands/mod.rs` only shows unnudged PR URLs. Called before interactive commands in `main.rs`.

### New Behavior

Expand the nudge to show a **multi-line status block** summarizing everything the auto pipeline did since the user last saw a nudge.

**Display format — successful run:**

```
  ┌─ retro auto-run (2 min ago) ─────────────────────
  │  Ingested:  3 sessions
  │  Analyzed:  3 sessions → 2 new patterns
  │  Applied:   1 skill, 1 CLAUDE.md rule
  │  PR created: https://github.com/user/repo/pull/42
  └───────────────────────────────────────────────────
```

**Display format — session cap hit:**

```
  ┌─ retro auto-run (5 min ago) ─────────────────────
  │  Ingested:  3 sessions
  │  ⚠ Skipped analyze: 41 unanalyzed sessions exceeds auto limit (15)
  │    Run `retro analyze` to process them.
  └───────────────────────────────────────────────────
```

**Display format — error:**

```
  ┌─ retro auto-run (1 min ago) ─────────────────────
  │  Ingested:  3 sessions
  │  ✗ Analyze failed: claude CLI not found on PATH
  └───────────────────────────────────────────────────
```

### Data Source

The nudge reads from `audit.jsonl`:

1. Track `last_nudge_at` timestamp in the DB (new metadata row or dedicated column)
2. On interactive command, read audit entries where `timestamp > last_nudge_at`
3. Group entries within 60 seconds as one auto-run
4. Aggregate into the status block
5. Update `last_nudge_at` after display
6. Skip entries with `reason: "cooldown"` (expected behavior, not interesting)

### Nudge Grouping

Multiple audit entries from one hook run (ingest + analyze + apply) share approximate timestamps. Group entries within a 60-second window as a single auto-run block.

## 2. Session Cap for Auto-Mode Analysis

### Config

```toml
[hooks]
auto_analyze_max_sessions = 15   # default: 15
```

New field in `HooksConfig` with `#[serde(default)]` for backwards compatibility.

### Logic

In the ingest orchestration chain (in `ingest.rs`), before calling `analysis::analyze()`:

1. Count unanalyzed sessions (lightweight count query, or use `get_sessions_for_analysis().len()`)
2. If count > `auto_analyze_max_sessions`:
   - Skip the AI call entirely
   - Write audit entry: `{ "action": "analyze_skipped", "reason": "session_cap", "unanalyzed_count": N, "cap": M }`
   - Fall through to apply check (which will also have nothing to do)

### Why 15

With `BATCH_SIZE = 20`, 15 sessions means one AI call (~30s). The user can tune this via config. No cap on interactive `retro analyze` — only auto mode.

## 3. Audit Log Improvements

The auto pipeline must write richer audit entries so the nudge system can reconstruct a complete picture.

### New Audit Events

| Event | Action | Key Fields |
|-------|--------|------------|
| Ingest success | `ingest` | `auto`, `sessions_ingested`, `sessions_skipped`, `project` |
| Ingest skipped (cooldown) | `ingest_skipped` | `reason: "cooldown"` |
| Analyze success | `analyze` | Already logged (no change) |
| Analyze skipped (cap) | `analyze_skipped` | `reason: "session_cap"`, `unanalyzed_count`, `cap` |
| Analyze skipped (cooldown) | `analyze_skipped` | `reason: "cooldown"` |
| Analyze error | `analyze_error` | `error: "..."` |
| Apply success | `apply` | Enrich: `files_written`, `patterns_activated`, `pr_url` |
| Apply skipped (no patterns) | `apply_skipped` | `reason: "no_qualifying_patterns"` |
| Apply error | `apply_error` | `error: "..."` |

### Principle

Every auto-mode decision gets an audit entry. The nudge system never needs to guess what happened.

## 4. Hook Script Update

### Current

```sh
# retro hook - do not remove
retro ingest --auto 2>/dev/null &
```

### Proposed

```sh
# retro hook - do not remove
retro ingest --auto 2>>~/.retro/hook-stderr.log &
```

### Rationale

- `audit.jsonl` captures structured events (read by nudge system)
- `hook-stderr.log` captures unstructured stderr (parse warnings, panics, unexpected errors)
- Nothing is lost; the user sees the clean nudge block, not raw stderr
- JSONL parse warnings (`missing field uuid`) naturally go to `hook-stderr.log` instead of vanishing

### Cleanup

- `retro init` truncates `hook-stderr.log` (fresh start)
- Size check: if `hook-stderr.log` exceeds ~100KB, rotate (truncate oldest half or rename to `.old`)

## 5. Changes Summary

| Component | File(s) | Change |
|-----------|---------|--------|
| Config | `config.rs` | Add `auto_analyze_max_sessions: u32` with default 15 |
| Ingest orchestration | `commands/ingest.rs` | Session count check before analyze; audit entries for ingest success/skip |
| Analyze orchestration | `commands/ingest.rs` | Audit entries for analyze skip (cap, cooldown) and error |
| Apply orchestration | `commands/ingest.rs`, `commands/apply.rs` | Audit entries for apply skip and error; enrich success entry |
| Nudge system | `commands/mod.rs` | Expand `check_and_display_nudge()` to read audit.jsonl, aggregate, display status block |
| DB | `db.rs` | Add `last_nudge_at` tracking |
| Hook installation | `git.rs` | Change redirect from `2>/dev/null` to `2>>~/.retro/hook-stderr.log` |
| Init | `commands/init.rs` | Update hook format; truncate hook-stderr.log on init |
| Audit log | `audit_log.rs` | No format change (just more callers writing entries) |

### Already Fixed (this session)

| Component | File(s) | Change |
|-----------|---------|--------|
| CLAUDECODE env var | `analysis/claude_cli.rs` | `.env_remove("CLAUDECODE")` on both spawn sites |
| Data gate mismatch | `db.rs` | `has_unprojected_patterns` now takes `confidence_threshold` parameter |

### What Doesn't Change

- `audit.jsonl` format (append-only JSONL, same schema — just more event types)
- `--verbose` flag behavior (still available for manual debugging)
- Lock mechanism
- Cooldown system
- Interactive mode output
