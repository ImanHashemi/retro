# Retro v3 — Context Curator (keep projected context lean)

- **Date:** 2026-08-10 (code references revised 2026-08-17 against `main` at 3.1.0)
- **Status:** Design — awaiting adversarial review before planning
- **Scope:** v3 store + pipeline (`retro run`, `lint`, `doctor`, `ui`)

## Motivation

Newer coding-agent models reward a different context discipline: less rigid
instruction, more judgment, and progressive disclosure over everything-upfront.
The practical consequences for a tool that *writes* an agent's persistent
context are:

1. **Bloat is a liability, not a freebie.** A long, ever-growing rules file
   forces the model to deliberate over overlapping and contradictory guidance.
2. **Automatic memory is now table stakes.** Platforms save memories on their
   own. The durable differentiator is *curation* — keeping that memory lean,
   non-conflicting, and correctly scoped.
3. **Related, procedural guidance belongs in on-demand skills**, loaded only
   when relevant, not in the always-loaded rules file.

Retro's v3 projection is today **purely additive**: every active node at or
above the confidence threshold becomes a bullet in `CLAUDE.md` /
`CLAUDE.local.md`, ordered arbitrarily (by slug), with no de-duplication at
projection time, no size budget, no conflict handling, and no notion of a
skill. In practice the projected sections accumulate near-duplicate and stale
bullets that nothing ever removes.

This design adds a **Context Curator**: a store-hygiene layer that keeps
projected context lean. It is retro's answer to a "rightsize my context"
command, run automatically each pipeline pass and on demand.

## Goals

- Merge lexical near-duplicate nodes so a rule appears once.
- Archive stale, sub-threshold, one-off nodes that never matured.
- Surface genuine conflicts between nodes for resolution.
- Extract clusters of **process** patterns (how a procedure is handled, its
  steps/sequence) into on-demand **skills**, pulling those bullets out of the
  always-loaded rules file.
- Reduce intake bloat at the source (reject the model-obvious; prefer
  merge/update over create).
- Make each behaviour configurable between silent auto-apply and review-gated.

## Non-goals (explicitly deferred)

- **Softening rigid rules into judgment-style phrasing.** A separate content
  concern; out of scope here.
- **Coordinating with the platform's native memory system.** Separable; deferred.
- **A persisted relationship graph.** See "Data model" — v3 stays flat; the
  conflict edge (`conflicts_with`) is documented as a future upgrade.
- **Project-scoped skill extraction.** MVP extracts global-scope process
  clusters only (project skills raise a "commit to the repo?" question we do
  not need to answer yet).
- **Semantic de-duplication.** Deterministic merge is lexical only; semantic
  dedup relies on the analysis model (intake filter) rather than a new pass.

## Key decisions

| Decision | Choice | Rationale |
|---|---|---|
| Autonomy | **Hybrid, configurable per-op.** Safe ops auto-apply; judgment ops queue for review. Each op is independently settable to `auto` / `review` / `off`. | Matches "just does it" for safe work while keeping a gate on risky changes; full control for either extreme. |
| Skills = ? | **Process clusters.** A cluster becomes a skill only when the patterns describe *how a procedure is handled* (steps/sequence), not merely related atomic preferences. | Atomic prefs/gotchas stay as lean bullets; procedures move to on-demand skills. |
| Approach | **Curate the store** (a dedicated hygiene pass), plus a cheap intake filter. Projection stays a dumb render of active nodes. | One clean, testable owner of "keep lean"; the store git history already preserves everything, so archiving/merging is safe and recoverable. |
| Data model | **Stay flat (decision A).** Relationships are derived/transient. The only schema change is an `archived` field. A `conflicts_with` frontmatter edge is the documented future upgrade (decision B). | Preserves v3's legible, hand-editable, git-versioned flat-file model and its strict closed schema; the curator needs a stored edge in only one spot, and detection (not storage) is the current bottleneck. |

## Architecture

One new core module — the **Curator** (`retro-core/src/store/curate.rs`) — owns
context hygiene. It splits along the Hybrid choice:

```
                          ┌─────────────────────────────┐
                          │   Curator (store::curate)    │
  STATIC CORE (free,      │  • merge near-duplicates     │ → auto (default)
  deterministic) ────────▶│  • archive stale nodes       │ → auto (default)
                          │  • detect conflicts          │ → review (default)
  AI OVERLAY (on approval,│  • propose process→skill     │ → review (default)
  budget-gated) ─────────▶│  • generate skill (agentic)  │ → runs on approval
                          └─────────────────────────────┘
```

### Command surface (all reuse existing surfaces)

| Surface | Role | Change |
|---|---|---|
| `retro run` (v3) | Curator runs as a stage after analysis: safe ops auto-applied, judgment ops queued | new stage |
| `retro lint --fix` | On-demand apply of the safe deterministic ops (merge dupes, archive stale); stays AI-free | extend (`lint` is flag-only today) |
| `retro doctor` | Read-only hygiene report: projected bullet count + size, # near-dupes, # conflicts, # stale, # skill candidates | extend |
| `retro ui` review | Approve/resolve judgment items (conflicts, skill extractions); applying reuses the existing commit → reindex → reproject path under `run.lock` | extend |

### Data flow (one `retro run`)

```
observe → analyze (+ intake filter, + optional conflict / skill signals)
        → CURATOR: merge dupes ✓auto | archive stale ✓auto | conflicts→queue | skill cluster→queue
        → commit store + reindex → project (now leaner) → push
```

Curator runs **before** projection so the projected files reflect the cleaned
set the same run.

## Static core (deterministic, no AI)

Both reuse the existing similarity primitive (`util::normalized_similarity` in
`retro-core/src/util.rs` — normalized character-level Levenshtein, already the
primitive behind `lint`) and are recoverable via the store's git history.

### Merge near-duplicates — `merge_duplicates`, default `auto`

- Detection reuses `lint`'s logic: same-scope pairs, a >20% length-ratio
  pre-filter, then `normalized_similarity(a.body, b.body) > merge_similarity`
  (default **0.8**).
- **Anchor-based clustering** (avoids transitive chaining of A→C when only
  A~B~C): order active nodes by `(confidence desc, updated desc, slug asc)`;
  take the top node as the anchor; absorb every same-scope node that is
  `> threshold` **to that anchor**; repeat to a fixpoint. Every absorbed node
  is genuinely near-identical to its survivor.
- **Survivor** keeps the anchor's slug (stable id), takes `body` = the longer of
  the merged bodies, `confidence` = max, `sources` = union, `updated` = today.
  Each absorbed node gets `invalidated_by = survivor_slug` — reusing the
  existing supersession mechanism, so no new "merged" state is needed.
- **Idempotent:** absorbed nodes are inactive and skipped next pass.

### Archive stale — `archive_stale`, default `auto`

- Conservative criteria — **all** must hold: node is active, **and**
  `confidence < confidence_threshold` (never matured past the 0.7 projection
  gate), **and** `updated < today − staleness_days` (default **28**), **and**
  `sources.len() ≤ 1` (only ever seen in a single session).
- Targets only sub-threshold, old, one-off noise. It never archives a
  high-confidence rule for being old (e.g. "pin actions to SHAs" does not
  decay), and never archives a node reinforced across multiple sessions.
- Representation: see "Data model". Archiving sets `archived` +
  `archived_reason = "stale"`.

### Safety / audit (both)

- All writes go through the existing atomic temp-rename + store git commit under
  `run.lock`. One honest commit label per run, e.g.
  `chore(curator): merged 3 duplicates, archived 2 stale`.
- Every auto-applied action is recorded in the audit log and summarised in one
  line in the briefing / `retro doctor`.
- Under `review` the op produces a proposal instead of writing; under `off`
  detection is skipped entirely.

## Judgment calls (review by default)

### Conflict detection — `conflicts`, default `review`

- The analysis model already emits `Contradicts` relationships each run; v3
  currently **drops** them (`edges_ignored` in `analysis/v3.rs`). The change is
  to stop dropping them.
- A contradiction between two active nodes becomes a **transient finding**
  (reusing the `LintFinding` shape) — not persisted, per decision A. Surfaces as
  a review item in `retro ui` + briefing:
  *"`slug-a` and `slug-b` conflict: <why>. Resolve?"*
- **On approval:** the user picks the winner; the loser's `invalidated_by`
  points at it (existing supersession) → reproject.
- **Under `auto`:** winner = higher confidence → more recent `updated` → more
  sources; loser invalidated automatically.
- **Limitation (accepted for MVP):** detection is bounded by the analyzer's
  50-node, 200-char context window, so it is *best-effort, not exhaustive*. The
  curator `log()`s that the window was capped rather than implying full
  coverage. Strengthening detection reach is future work, paired with decision B.

### Skill extraction — `skills`, default `review`

- **Detection — no new AI call.** The existing analysis response is extended
  with an optional `skill_suggestion { topic, node_ids[], rationale }`. The
  analyzer emits one only when a set of active nodes describes a **process**
  (steps/sequence for handling something), not merely topically-related atomic
  preferences. At most one suggestion per run.
- Surfaces as a review item:
  *"These N patterns form a process about `<topic>`. Extract into on-demand
  skill `<name>` and pull them out of the rules file?"*
- **On approval — one agentic AI call.** The generator has to be **built, not
  reused**: v2's `projection::skill::generate_skill_agentic` (superpowers
  writing-skills instructions injected, snapshot-diff to find the created
  `SKILL.md`) was deleted along with the rest of the v2 core in Plan 4
  (`a563a0d`), so only the transport survives —
  `AnalysisBackend::execute_agentic()` in `analysis/claude_cli.rs` (unlimited
  turns, full tool access, no `--json-schema`, optional `cwd`). Re-implement the
  generator against it, feeding it the cluster's node bodies, and write to the
  global skills directory. The deleted implementation is a usable reference for
  the prompt shape and the snapshot-diff trick:
  `git show a563a0d^:crates/retro-core/src/projection/skill.rs`. **Sizing note:**
  this makes skill extraction the largest single piece of work in the design, not
  a thin adapter over existing code. Then **archive the source nodes** with
  `archived_reason = "extracted:skill:<name>"` — they stop projecting (rules
  file shrinks) and, being inactive, are not shown to the analyzer again.
  Commit → reindex → reproject.
- **Budget-gated** by the existing `max_ai_calls_per_day`; if exhausted, an
  approved extraction waits until the budget resets.
- **Under `auto`:** generate + extract without asking (spends AI).
- MVP: **global-scope clusters → global skills only.**

## Intake filter (source-side complement)

Prompt-only changes to the v3 analysis prompt (`analysis/prompts.rs`):

1. **Reject the obvious** — do not create nodes for generic best-practices a
   capable model already follows (write tests, handle errors, meaningful
   names…); keep only non-obvious, user/team/repo-specific context.
2. **Merge harder** — strengthen the existing preference for update/merge/
   supersede over create when a similar node already exists.

Prompt guidance is soft, which is *why* the deterministic downstream merge still
exists — belt and suspenders.

## Data model

v3 stays **flat** (decision A). The only change:

- Add two optional fields to `Node` (`store/node.rs`):
  `archived: Option<NaiveDate>` and `archived_reason: Option<String>`.
- Teach the strict `from_markdown` parser these keys (unknown keys are a hard
  error, so they must be registered); `to_markdown` emits them like
  `invalidated_by` (`null` when absent) for stable round-trips. Files lacking
  the keys still parse (fields default to `None`) and gain them lazily on next
  rewrite.
- `is_active()` becomes `invalidated_by.is_none() && archived.is_none()`. This is
  the only behavioural change to the model; projection, reindex, and lint
  already gate on `is_active()` and inherit archived-exclusion for free.

**Deferred (decision B — future upgrade):** a list-valued `conflicts_with:
[slug]` frontmatter field, with adjacency materialised in the disposable SQLite
index for a queryable conflict view, plus edge garbage-collection on reindex for
links pointing at archived/hand-deleted nodes. Adopt only if transient conflict
detection proves insufficient in practice; it is additive and can be added
without reworking this design.

## Config

New `[curator]` section:

```toml
[curator]
merge_duplicates = "auto"    # auto | review | off   (default: auto)
archive_stale    = "auto"    # auto | review | off   (default: auto)
conflicts        = "review"  # auto | review | off   (default: review)
skills           = "review"  # auto | review | off   (default: review)
merge_similarity = 0.8       # near-duplicate threshold
```

Reuses existing `analysis.staleness_days` (28) and
`knowledge.confidence_threshold` (0.7). Policy is a `CuratorPolicy` enum
`{ Auto, Review, Off }` (serde lowercase).

## Error handling / safety

- The curator is a stage in `runner_v3`, wrapped so a failure **logs and
  continues** — it never aborts the run, blocks projection, or corrupts the
  store (the same per-stage isolation v3 already uses per-project).
- Order within a run: analysis → curator → commit → reindex → project.
- Skill extraction archives source nodes **only after** the `SKILL.md` writes
  successfully; a failed or budget-blocked generation leaves nodes active and
  the proposal pending. No orphaning.
- All node ids from the review surface / model are `is_valid_slug`-checked
  before any path use (existing hardening).
- Idempotency is a required, tested property: a second curator pass over an
  already-clean store is a no-op.
- No silent caps: bounded detection windows are `log()`-ed.

## Testing

- **Unit (fixtures, no AI):** anchor-based merge (survivor selection, union
  sources, max confidence, longer body), archive criteria (all four conditions;
  must not archive high-confidence-old or multi-source nodes), `is_active()`
  honouring `archived`, idempotency (second pass no-op), markdown round-trip with
  the new fields.
- **`MockBackend`:** intake filter rejects an "obvious" pattern; a `contradicts`
  signal surfaces as a review item (not persisted); a `skill_suggestion` →
  review item → on approval, mocked generation archives the source nodes and
  shrinks the rules file.
- **Scenario test:** seed a store with lexical dupes + stale + a conflict + a
  process cluster → run → assert merged/archived/queued and projected bullet
  count dropped (run-scenarios skill).
- **Config-policy tests:** each op under `auto` / `review` / `off`.
- Clean-install verification is deferred to completion per project rules
  (`retro init`/`start`/`stop` are never run during a dev session).
- Docs updated as part of the work: checked-in `CLAUDE.md` (command table +
  Key Design Decisions) and `README` where projection is described.

## Open questions

1. **Merge body selection.** For auto-merge, the survivor takes the *longer*
   body as a superset heuristic. At 0.8–0.95 similarity the shorter body may
   carry a nuance the longer one lacks. Should near-but-not-identical merges
   (e.g. similarity in a `[0.8, 0.95)` band) be downgraded to `review` rather
   than auto-applied, or is git recoverability enough to justify auto?
2. **Conflict detection reach.** Is best-effort detection (piggybacked on the
   analysis call, bounded by the 50-node window) acceptable for the MVP, or do
   we want a dedicated conflict pass — accepting one extra budget-gated AI call
   per run — for broader coverage?
3. **Re-discovery after extraction.** Archived-into-skill nodes are hidden from
   the analyzer, but a *new* session can still surface the same pattern as a
   fresh node that duplicates an extracted skill. Should the curator detect
   "this new node is already covered by skill X", and if so, how — without a
   persisted node→skill link?
```

