# Retro v3 — Personal Context Curator (Redesign)

**Date:** 2026-07-06
**Status:** Approved design, pending implementation plan
**Supersedes:** v2 "The Watcher" architecture

## 1. Context & Motivation

A dogfood review (2026-06-25, across two production work repositories) found:

- **The personal track worked.** 55 active global rules projected friction-free into `~/.claude/CLAUDE.md`, with visible behavior change.
- **The team track failed.** 27 `retro/updates-*` PRs, 33% merge rate, 5 abandoned open PRs, 14 closed unmerged. One-branch-per-approval piled up; `pr_url` was not recorded on most nodes so sync could not reconcile; closed-PR rules were re-projected into fresh PRs (churn loop).
- **The background runner failed silently for ~6 weeks.** launchd-scheduled `claude` calls hit auth/network errors (`ConnectionRefused`, empty exit-1 stderr); `session_cap` config skipped analysis 251 times; the backlog grew (30 → 35+ unanalyzed, 67 total) while every run reported success. Nothing surfaced any of this.
- **Coverage was invisible.** Two repos the user believed were watched (infra, cc-service) were never registered.
- **No visibility surface.** There is no way to see "everything my agent loads, and which part is retro's."

Research inputs (2026-07-06): Karpathy's LLM-wiki gist (ingest/query/lint, compounding artifact, citations), Graphify (derived artifact with provenance instead of review), Graphiti/Zep (temporal invalidation, episode provenance, ingest-time dedup), and a landscape survey (review queues are nearly extinct — auto-apply with visibility and easy undo is the market answer; context visibility is an open gap everywhere).

## 2. Product Definition

**Retro v3 is a personal context curator for Claude Code.** It watches your sessions, learns your rules/preferences/patterns, stores them as a git-backed personal knowledge base under `~/.retro`, and automatically projects them into context files Claude Code natively loads. One user; multi-machine via a private git remote; zero review friction; full visibility via a local web dashboard.

### Non-goals (deleted from v2, not deferred bugs)

- **Team/shared context.** No PRs, no writes to shared project `CLAUDE.md`, no branches, no stash wrapper, no `gh` PR machinery. Team context curation is a team process, not a personal tool's job. (Possible future: opt-in "export rule for team" as an explicit user action.)
- **Review queue.** No `PendingReview`, no `retro review`, no trust config. Auto-apply + after-the-fact curation + git undo replaces gating.
- **Skill generation/projection.** Cut for scope. Existing skills remain untouched; the dashboard shows them read-only.
- **Multi-agent projection.** Claude Code only, behind a `Projector` trait so others can come later.
- **Wiki-style topic pages / cross-links.** Atomic nodes first; the markdown store makes pages a v3.x layer, not a migration.
- **Daemon / launchd.** Deleted entirely (see Capture).
- **v1 pipeline.** All v1 commands, tables, and git-hook automation removed. v3.0.0 is a breaking release.

## 3. Architecture Overview

```
Surfaces      retro ui (web dashboard) · terminal nudge · session-start briefing
Projection    Projector trait → ClaudeCodeProjector (CLAUDE.md managed / CLAUDE.local.md)
Store         ~/.retro (git repo): markdown nodes = truth · SQLite = disposable index
Pipeline      observe (hook) → queue → run (analyze + reconcile + lint + project)
Capture       Claude Code hooks: SessionEnd (enqueue + spawn worker), SessionStart (catch-up)
Backend       AnalysisBackend trait → ClaudeCliBackend (claude -p, subscription auth)
```

Data flows one way: sessions → store → projections. There is no bidirectional reconciliation anywhere in v3.

## 4. Storage — Files as Truth, Git as History

### Layout

```
~/.retro/                          # git repository (the knowledge base)
├── knowledge/
│   ├── global/<slug>.md           # one node per file
│   └── projects/<project-slug>/<slug>.md
├── config.toml                    # committed
├── index.db                       # SQLite index — gitignored, rebuildable
├── health.json                    # pipeline status — gitignored
├── queue/                         # pending sessions — gitignored
└── .gitignore
```

### Node format

```markdown
---
id: ab-paired-observations          # kebab-case slug, stable
scope: project/my-api-service       # global | project/<slug>
type: rule                          # rule | preference | pattern | memory
confidence: 0.9
sources: [session:1a2b3c4d, session:5e6f7a8b]
created: 2026-05-19
updated: 2026-06-02
invalidated_by: null                # node id, or null
---
A/B comparisons must always use paired observations (matched cohort filter).

**Why:** Unpaired comparisons mix traffic distributions across time windows.
**How to apply:** In experiment analysis scripts, filter to traces with both
A and B observations for the same input.
```

### Rules

- **Node status collapses to two states:** active (`invalidated_by: null`) and invalidated. Nothing is ever deleted; contradiction/supersession sets `invalidated_by` (Graphiti's model, single-writer simplification). Invalidated nodes stop projecting but stay browsable.
- `memory` nodes are context-only: stored and browsable, never projected.
- **Every mutation is a git commit.** Retro auto-commits its own changes (`retro: learn ab-paired-observations`); before each run it commits any manual edits it finds (`user: edit …`). Undo = `git revert`; audit trail = `git log`. The v2 JSONL audit log is removed.
- **`index.db` is a cache.** Built from the files (FTS5 search, scope/type/status queries, source lookups). `retro reindex` rebuilds it from scratch; files always win. Schema carries no state that is not derivable from the files.
- **Secret scrubbing runs at ingest**, before anything is written to the store — the store (and therefore the backup remote) never contains raw session content, only distilled knowledge.

### Backup & multi-machine

- `retro init` offers: create a **private** GitHub repo (e.g. `retro-knowledge`) via `gh`, add as `origin`, push. Optional; local-only is supported.
- After each commit, best-effort `git push` (failures recorded in health, retried next run — offline is fine).
- New machine: `retro init --from <remote-url>` clones instead of initializing fresh.

## 5. Capture — Hooks, No Daemon

`retro init` installs two Claude Code hooks (in `~/.claude/settings.json`):

- **SessionEnd → `retro observe`.** Fast, no AI: validates and enqueues the session file into `~/.retro/queue/`, then spawns a detached `retro run --background` worker (skips spawn if a lockfile shows one alive). The worker inherits the user's shell environment — network up, subscription auth warm — because it runs at a moment Claude was demonstrably reachable. This structurally removes the launchd failure class.
- **SessionStart → `retro brief`.** Cheap catch-up: scans `~/.claude/projects/` for session files modified since last observation (covers crashed sessions, force-quit, other machines), enqueues them; emits the session briefing (recent learnings + health warnings) as hook output.

Additional rules:

- **Opportunistic sweep:** every retro CLI invocation checks queue age and health, and nudges if the backlog is stale or errors repeat.
- **Registration is automatic, noticed, and one-step reversible:**
  - A project is registered when a session from its cwd is first observed. Identity: `remote_url` first (stable across machines and moves), path second; slug from the directory name. Slug + remote URL are committed in the store; the slug→local-path map is per-machine, gitignored, rebuildable. Retro only ever reads Claude Code's own session store (`~/.claude/projects/`) — it never scans user directories.
  - **Notification:** every new registration is announced in the next SessionStart briefing ("retro is now watching <slug> — exclude via dashboard or config") and highlighted in the dashboard's coverage list.
  - **Exclusion = removal:** excluding a project (dashboard toggle, or `privacy.exclude_projects` in config) stops watching it, deletes its `knowledge/projects/<slug>/` subtree (recoverable via git history), and removes its `CLAUDE.local.md` managed block. Excluded projects are remembered and never re-register automatically.
- **Cost controls:** `max_ai_calls_per_day` retained. The v2 `session_cap` hard-skip is replaced by a **visible queue with backpressure** — sessions are never dropped; an undrained queue is a health warning, not a silent skip.
- Stale queue entries (session file deleted before processing) are pruned and recorded in health — never retried forever.

## 6. Analysis — Same Engine, New Triggers and Sink

The v2 analysis engine is kept: `claude -p` with `--json-schema` constrained decoding, prompt-via-stdin, low-signal session filtering (< 2 user messages), confidence model (0.4–0.5 single observation, recurrence bumps, 0.7–0.85 explicit directives), reasoning field, `MAX_USER_MSG_LEN` truncation.

Changes:

- **Sink is the file store.** Graph operations (create/update/supersede) write markdown files + commit, then reindex.
- **Ingest-time reconciliation (dedup):** candidate matching (Levenshtein > 0.8 safety net; embeddings possible later) plus the analysis prompt's semantic-merge guidance, as in v2. Merges union `sources` and bump confidence.
- **Contradiction → supersession:** single-writer, so recency wins: the new node sets `invalidated_by` on the old one. Both remain in the store.
- **`retro lint` (new, Karpathy's third operation):** a periodic AI-assisted pass over the whole store — duplicates that slipped past ingest, contradictions, stale nodes (citing files/tools/flags that no longer exist — checked with tool access), orphan invalidations. Findings surface in the dashboard; safe fixes (dedup merges) auto-apply, judgment calls are flagged. Runs opportunistically (e.g. at most weekly), counted against the daily AI budget.
- **Backend abstraction kept:** `AnalysisBackend` trait; `ClaudeCliBackend` is the only v3 implementation (subscription auth, zero marginal cost, agentic tool access for lint). A direct-API backend is a possible later addition; not built now.

## 7. Projection — One-Way, Regenerable, Auto-Applied

- `Projector` trait; `ClaudeCodeProjector` only.
- **Global nodes** → managed block (`<!-- retro:managed -->`) in `~/.claude/CLAUDE.md` — unchanged from v2.
- **Project nodes** → managed block in **`CLAUDE.local.md`** at the project root. Claude Code loads it natively; it is personal. Retro ensures it is ignored via **`.git/info/exclude`** (personal ignore; no shared `.gitignore` edits, no force-adds, invisible to teammates).
- **Projection is idempotent full regeneration** of managed blocks from the store on every run. No diffing, no import, no reconciliation. Managed blocks are build output; edits belong in the store (dashboard or files). Content outside managed blocks is never touched.
- Confidence gate ≥ 0.7 (configurable) decides projection; sub-threshold knowledge accumulates in the store and shows in the dashboard as "not yet projected."
- Fresh clone / deleted `CLAUDE.local.md` → regenerated on next run. Projections carry zero durability weight.
- `MEMORY.md` and auto-memory remain read-only inputs (shown in the X-ray, never written).
- Node bodies keep the `Why / How to apply` shape; projection emits them as the bullet-with-bold-annotations format already proven in the current global CLAUDE.md.

## 8. Visibility — `retro ui` Dashboard

- **Server:** `tiny_http` (sync, honoring the no-tokio rule), localhost-only, port from `[ui] port` (default 7777), assets embedded in the binary (no Node runtime, no separate install). `retro ui` starts it and opens the browser.
- **Views:**
  1. **Context X-ray** (per project, the flagship): everything Claude Code loads — global `CLAUDE.md`, project `CLAUDE.md`, `CLAUDE.local.md`, `MEMORY.md`/auto-memory, skills inventory — with sizes, rough token estimates, and retro-owned parts marked. Includes the project coverage list (watched / newly registered / excluded) with a one-click exclude toggle.
  2. **Knowledge browser:** search (FTS) and filter by scope/type/confidence/status (incl. invalidated); node detail with provenance (source sessions) and history; inline edit / adjust confidence / invalidate. Writes go through the store: file edit → commit → reindex → reproject.
  3. **Health:** queue depth and age, last run result, AI calls today, hook installation status, backup push status, recent errors.
  4. **History:** rendered `git log` of the store; one-click revert of a commit.
- The v2 ratatui dashboard (`retro dash`) is retired; `retro status` remains for terminal-native summary.

## 9. Reliability Contract

- Every pipeline stage writes `health.json` (stage, timestamp, result, error).
- **`retro doctor`** verifies the chain end-to-end: hooks installed and pointing at the current binary → queue draining → `claude -p` callable (cheap probe) → index consistent with files → projections current → backup push ok.
- The SessionStart briefing and every interactive command surface health warnings (repeated failures, stale queue, missing hooks).
- Design invariant: **no scheduled background process exists**, and unprocessed work is a visible number in the dashboard and the nudge. The v2 failure mode (silent for six weeks) is structurally impossible: capture runs only in user context, and its absence is itself detectable (queue age).

## 10. CLI Surface (v3.0.0)

| Command | Purpose |
|---|---|
| `retro init [--from <remote>]` | Create/clone store, git init, install hooks, offer backup remote |
| `retro observe` | Hook entry: enqueue session, spawn worker |
| `retro brief` | Hook entry: catch-up scan + emit briefing |
| `retro run [--background] [--dry-run]` | Drain queue: analyze → reconcile → project |
| `retro lint [--dry-run]` | Store-wide dedup/contradiction/staleness pass |
| `retro ui` | Start dashboard, open browser |
| `retro status` | Terminal summary (knowledge counts, queue, health) |
| `retro doctor` | End-to-end health verification |
| `retro reindex` | Rebuild index.db from files |
| `retro migrate` | One-time v2 → v3 migration |
| `retro uninstall [--purge]` | Remove hooks; optionally the store |

Removed: `ingest`, `analyze`, `apply`, `review`, `sync`, `patterns`, `curate`, `diff`, `clean`, `audit`, `log`, `hooks`, `dash`, `start`, `stop`, `--auto` everywhere.

## 11. Migration (v2 → v3)

`retro migrate`, deterministic, no AI calls:

1. Back up `~/.retro/retro.db` to `retro.db.v2.backup`; back up current global CLAUDE.md.
2. `git init` the store; write `.gitignore`; initial commit.
3. Export v2 `nodes`: Active → active node files; Archived → invalidated node files (best-effort `invalidated_by` from `supersedes` edges, else a sentinel `migrated-archive`); Dismissed → invalidated. Type mapping: `directive` → `rule`, `skill` → `pattern`; others 1:1. Provenance edges (`derived_from`) → `sources`.
4. Projects table → project slugs (drop the stray worktree entry).
5. `launchctl bootout` the runner; remove plist; remove v1 git hooks from registered repos; update `~/.claude/settings.json` hooks to v3 entries.
6. One-time safety import: any rules present in existing managed blocks (global CLAUDE.md, project CLAUDE.md) but absent from the v2 DB become rule nodes (confidence 0.8) — the only import that ever happens; after this, flow is strictly store → projections. Rules in *project* CLAUDE.md managed blocks move to the personal store and the shared-file managed block is emptied (with a note in the migration summary, since teammates may see that diff — user decides whether to commit it).
7. Project `CLAUDE.local.md` for each registered project; regenerate the global managed block.
8. Report a summary; leave the v2 DB backup in place.

The 67-session unanalyzed backlog is enqueued after migration and drained under the normal daily budget (smoke-tested first — see Testing).

## 12. Testing

- **Unit tests + fixtures** (no AI) for store, node parsing/serialization, queue, projection, migration.
- **`MockBackend`** integration tests for the pipeline (observe → run → project) and lint.
- **Scenario tests** rewritten for v3 flows: clean init, init --from, observe/run cycle, manual-edit-then-run (commit of user edits), invalidation, reindex-after-index-deletion, migrate-from-v2-fixture, doctor failure modes, ui API smoke. `RETRO_HOME` isolation as today.
- **Dashboard API** tested at the HTTP layer (sync server makes this trivial); UI itself kept thin.
- **Rollout discipline:** after implementation, smoke-run the pipeline on 3 real sessions and present results before draining the full backlog; then dogfood v3 on the retro repo itself plus the maintainer's active work repositories — explicitly registering the two that were previously missed (closing the coverage gap).

## 13. Decisions Log

| Decision | Choice | Why |
|---|---|---|
| Audience | Personal-only | Dogfood: personal track delivered all the value; team track failed; team context is a team process |
| Source of truth | Markdown files, git-backed | Readable, editable, versioned, greppable; kills DB opacity and file↔DB reconciliation |
| SQLite role | Disposable index | Fast search without a second truth |
| Capture | Claude Code hooks, no daemon | Runs in user auth context at provably-online moments; work proportional to usage; nothing to fail silently |
| AI backend | `claude -p` (trait kept) | Subscription auth, zero keys, zero marginal cost, agentic tool access; API backend possible later |
| Review | None — auto-apply + undo | Review queues are burden (user) and nearly extinct (landscape); provenance + git revert + dashboard replace gating |
| Project-scope delivery | `CLAUDE.local.md` + `.git/info/exclude` | Native personal per-project surface; zero team/git footprint |
| Projection | One-way full regeneration | Deletes v2's buggiest subsystem (bidirectional reconciliation) |
| Control surface | Local web dashboard | Richest at-a-glance visibility; the X-ray answers "what does my agent see" |
| Knowledge shape | Atomic nodes now, wiki later | Ships fastest; markdown store makes pages a layer, not a migration |
| Skills | Cut from v3 | Scope control; least-proven value |
| Backup | Private GitHub repo, auto-push | Dotfiles pattern: boring, proven, multi-machine for free |
| Registration | Automatic + notify + one-step exclude | Zero setup and full coverage, with the registration always announced and trivially reversible (exclude deletes the project's knowledge and projection) |
