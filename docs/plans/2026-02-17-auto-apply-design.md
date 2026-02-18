# Auto-Apply Pipeline Design

## Problem

Today's retro pipeline requires manual intervention to complete:

```
post-commit → ingest --auto (automatic)
post-merge  → analyze --auto (automatic)
              ↓
              patterns sit in DB as "discovered"
              ↓
              user must manually run `retro apply`
```

The "wow moment" — a PR appearing with discovered patterns, ready for review — never happens automatically. Users must remember to run `retro apply`, which defeats the purpose of an autonomous context curator.

## Design

### Single-Hook Opportunistic Pipeline

Replace the two-hook system (post-commit + post-merge) with a single post-commit hook that runs the entire pipeline opportunistically. Each stage has its own cooldown and data trigger. No daemon, no extra processes — just a smart hook that checks what's overdue.

```
post-commit hook
  ↓
retro ingest --auto (5 min cooldown)
  ↓
  has un-analyzed sessions? AND last analyze > 24h?
  → yes → analyze
  ↓
  has un-projected patterns? AND last apply > 24h?
  → yes → apply (personal writes + shared PR creation)
  ↓
  store PR URL in DB for nudge
```

All three stages run in a single background process. The post-merge hook is dropped entirely.

### Cooldown Strategy

Each stage has a separate cooldown matching its cost profile:

| Stage | Cost | Default Cooldown | Trigger |
|-------|------|-----------------|---------|
| Ingest | Free (pure Rust) | 5 minutes | Always (cooldown only prevents rapid-fire) |
| Analyze | Expensive (AI calls) | 24 hours | Un-analyzed sessions exist in DB |
| Apply | Expensive (AI calls for skill gen) | 24 hours | Un-projected discovered patterns exist in DB |

Cooldowns are **minimum intervals** — the stage won't run more frequently than configured. Data triggers are **prerequisites** — the stage won't run if there's nothing to process. Both conditions must be met.

Config:

```toml
[hooks]
ingest_cooldown_minutes = 5       # prevent rapid-fire during rebase
analyze_cooldown_minutes = 1440   # once per day
apply_cooldown_minutes = 1440     # once per day
auto_apply = true                 # on by default with retro init
```

The old single `auto_cooldown_minutes` field is replaced by these three granular settings. Backwards compatibility: if only the old field exists, use it as the ingest cooldown and default the others to 1440.

### `apply --auto` Behavior

Follows the established `--auto` pattern from ingest and analyze:

- **Lockfile**: `LockFile::try_acquire()` — skip silently if another process holds it
- **Cooldown**: check `last_applied_at` timestamp vs `apply_cooldown_minutes`
- **Data gate**: check for un-projected discovered patterns (`has_unprojected_patterns()`)
- **No confirmation prompt**: skip the y/N stdin prompt entirely
- **Silent output**: no terminal output unless `--verbose`
- **Graceful failure**: catch all errors, log if verbose, exit 0
- **Git fallbacks**: no git repo → local file writes only; no `gh` → branch only, log manual PR instructions
- **PR URL storage**: save to `projections.pr_url` for the nudge system
- **Audit log**: record auto-apply actions in `~/.retro/audit.jsonl`

### Terminal Nudge

When any interactive retro command runs (status, patterns, apply, analyze, audit, clean, log), query the DB for recent auto-applied projections the user hasn't been nudged about:

```sql
SELECT DISTINCT pr_url FROM projections
WHERE pr_url IS NOT NULL AND nudged = 0
```

Display a one-liner:

```
  retro auto-created a PR with 3 new patterns: https://github.com/you/repo/pull/42
```

After displaying, mark those projections as `nudged = 1` so the message doesn't repeat. The nudge is a single line printed before the command's own output — non-intrusive.

### Hook Changes

**`retro init` installs a single post-commit hook:**

```sh
# retro hook - do not remove
retro ingest --auto 2>/dev/null &
```

The hook itself stays simple. All chaining logic (analyze, apply) lives inside `ingest --auto` — when `auto_apply = true` in config, ingest checks whether analyze and apply should run after completing ingestion.

**Migration**: `retro init` on an existing repo removes the old post-merge hook (if retro-managed) and replaces the post-commit hook. `retro init --uninstall` removes the post-commit hook cleanly.

### Internal Flow: `ingest --auto` Becomes the Orchestrator

When `--auto` is set:

1. Acquire lockfile (skip if held)
2. Check ingest cooldown (5 min) → run ingest if due
3. Check config: `auto_apply` enabled?
4. Check analyze conditions: un-analyzed sessions exist? + cooldown elapsed (24h)?
   - Yes → run analyze silently
5. Check apply conditions: un-projected patterns exist? + cooldown elapsed (24h)?
   - Yes → run apply silently (two-phase: personal on current branch, shared on new branch + PR)
6. Release lockfile

All within a single background process spawned by the post-commit hook. The lockfile scope covers the entire pipeline, preventing concurrent runs.

### What Stays the Same

- Manual `retro apply` works exactly as before (confirmation prompt, verbose output, two-phase)
- Manual `retro analyze` works as before
- `--dry-run` on all commands still works
- Pattern confidence threshold unchanged (0.7)
- Two-phase apply logic unchanged (personal on current branch, shared on new branch + PR)
- Audit log records all actions (both manual and auto)
- Skill generation is still two-phase (generate + validate) with up to 2 retries
- CLAUDE.md managed section delimiters unchanged
- `retro clean` staleness detection unchanged

### DB Changes

1. **New column**: `projections.nudged` (INTEGER DEFAULT 0) — tracks whether user has been nudged about this projection
2. **New function**: `last_applied_at()` — returns timestamp of most recent apply (from `projections.applied_at`)
3. **New function**: `has_unprojected_patterns()` — returns bool, checks for discovered patterns without projections and without `generation_failed`
4. **New function**: `mark_projections_nudged()` — sets `nudged = 1` on projections with non-null `pr_url`

Schema migration: `user_version` bumps from 1 to 2, adding the `nudged` column.

### Config Changes

New fields in `HooksConfig`:

```rust
pub struct HooksConfig {
    pub ingest_cooldown_minutes: u64,    // default 5
    pub analyze_cooldown_minutes: u64,   // default 1440
    pub apply_cooldown_minutes: u64,     // default 1440
    pub auto_apply: bool,                // default true
}
```

Backwards compatibility: the old `auto_cooldown_minutes` field is accepted and mapped to `ingest_cooldown_minutes`. New fields default to their values if absent from config.toml.

### Error Handling

In `--auto` mode, all errors are swallowed (logged only with `--verbose`). This includes:
- AI backend failures (Claude CLI not available, rate limit, etc.)
- Git failures (not a repo, branch conflicts, etc.)
- `gh` failures (not installed, auth issues, etc.)
- DB errors (locked by another process, etc.)

The pipeline degrades gracefully at each stage. If analyze fails, apply doesn't run (no new patterns). If apply fails, patterns remain "discovered" and will be retried next cycle.
