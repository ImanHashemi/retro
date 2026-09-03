# Retro v3 — Context Curator (keep projected context lean)

- **Date:** 2026-08-10 (revised 2026-08-24 after adversarial review; code references
  checked against `main` at 3.1.0)
- **Status:** Design — adversarial review complete, 14 findings addressed. Ready for planning.
- **Scope:** v3 store + pipeline (`retro run`, `curate`, `lint`, `doctor`, `ui`)

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

- Merge lexical near-duplicate nodes so a rule appears once, **without ever
  rewriting a surviving node's text**.
- Archive stale, sub-threshold, one-off nodes that never matured.
- Extract clusters of **process** patterns (how a procedure is handled, its
  steps/sequence) into on-demand **skills**, pulling those bullets out of the
  always-loaded rules file — with the skill itself held in the store and
  projected, like every other retro output.
- Reduce intake bloat at the source (reject the model-obvious; prefer
  merge/update over create).
- Make each behaviour configurable between silent auto-apply and review-gated.
- **Measure** whether the analyzer can detect genuine rule conflicts, before
  designing a feature on the assumption that it can.

## Non-goals (explicitly deferred)

- **Conflict resolution.** Cut from the MVP; this pass ships *instrumentation
  only* (see "Conflict detection — instrumentation only"). The previous draft
  specced a full resolve flow on the false premise that the analyzer already
  emits contradiction edges. It does not.
- **Softening rigid rules into judgment-style phrasing.** A separate content
  concern; out of scope here.
- **Coordinating with the platform's native memory system.** Separable; deferred.
- **A persisted relationship graph.** The flat model stays; see "Data model".
- **Project-scoped skill extraction.** MVP extracts global-scope process
  clusters only (project skills raise a "commit to the repo?" question we do
  not need to answer yet).
- **Semantic de-duplication.** Deterministic merge is lexical only; semantic
  dedup relies on the analysis model (intake filter) rather than a new pass.
- **Cross-machine review state.** The review queue is machine-local
  (`state/`), like `queue/` and `health.json`. Judgment items do not sync.

## Key decisions

| Decision | Choice | Rationale |
|---|---|---|
| Autonomy | **Hybrid, configurable per-op.** Safe ops auto-apply under a per-run cap; judgment ops queue for review. Each op is independently settable to `auto` / `review` / `off`. The **first** curator pass on a store is review-only regardless of config. | Matches "just does it" for safe work while keeping a gate on risky changes; the first-pass gate means the user sees the accumulated backlog before it is applied. |
| Merge semantics | **Body-preserving.** The survivor keeps *its own* body — never adopts an absorbed node's text. Merges where the absorbed body is not substantially contained in the survivor's go to `review`. | Removes the run-over-run grind (a survivor whose text changes creates new similarity relationships next pass), makes the pass genuinely idempotent, and means an auto-merge can never silently revert a body the user hand-edited. |
| Merge eligibility | **Same scope *and* same node type.** Cross-type near-duplicates are `review`-only, never auto. | A `memory` node is never projected; letting one absorb a `rule` would silently delete a projected bullet. `lint` already declines to recommend cross-type merges for this reason. |
| Skills = ? | **Process clusters.** A cluster becomes a skill only when the patterns describe *how a procedure is handled* (steps/sequence), not merely related atomic preferences. | Atomic prefs/gotchas stay as lean bullets; procedures move to on-demand skills. |
| Skill location | **Store-held and projected.** Generated into `~/.retro/skills/<name>/SKILL.md` (git-backed), then projected into `<claude_dir>/skills/` every run. | Keeps skills inside the one-way projection model retro uses everywhere else: git-backed, cloned by `init --from`, restorable, regenerable. A skill living only outside the store desynchronises from the archive record that replaced it. |
| Conflicts | **Instrumentation only for the MVP.** Ask the prompt for contradictions, split the ignored-edge counter by type, log the yield. No detection surface, no resolution flow, no mutation. | The design cannot be priced until we know whether the analyzer emits contradiction edges at all. Today it never does. |
| Review state | **Persisted, structured, machine-local** (`state/review.json`) with `pending` / `applied` / `dismissed`. | Judgment items produced by a background run must survive until the user acts on them, and a dismissed item must never be re-raised. Notifications are capped strings that drain on session start — they cannot carry a review queue. |
| Data model | **Stay flat.** Relationships are derived/transient. The only schema change is `archived` + `archived_reason`, written **only on nodes that are archived**. | Preserves v3's legible, hand-editable, git-versioned flat-file model and its strict closed schema, while keeping untouched nodes byte-compatible with older binaries reading the same synced store. |

## Architecture

One new core module — the **Curator** (`retro-core/src/store/curate.rs`) — owns
context hygiene. It splits along the Hybrid choice:

```
                          ┌──────────────────────────────┐
  STATIC CORE (free,      │   Curator (store::curate)     │
  deterministic) ────────▶│  • merge near-duplicates      │ → auto (capped)
                          │  • archive stale nodes        │ → auto (capped)
                          │  • absorb re-discovered nodes │ → auto
  AI OVERLAY (on approval,│  • propose process→skill      │ → review (default)
  budget-gated) ─────────▶│  • generate skill (agentic)   │ → runs on approval
                          └──────────────────────────────┘
```

Detection is shared: `lint` (report-only) and the curator (apply) call the same
detection functions, so a lint finding and a curator action can never disagree
about what counts as a duplicate or as stale.

### Command surface

| Surface | Role | Change |
|---|---|---|
| `retro run` (v3) | Curator runs as a stage after analysis: safe ops auto-applied under cap, judgment ops queued | new stage |
| `retro curate [--dry-run] [--undo-last]` | On-demand curator pass; `--dry-run` is the preview surface (`run --dry-run` returns before analysis, so a post-analysis curator is unreachable there); `--undo-last` reverts the most recent `chore(curator)` store commit | new command |
| `retro lint` | Report-only, unchanged in shape, but now calls the curator's detection functions so report and action share one definition | extend |
| `retro doctor` | Read-only hygiene report: projected bullet count + size, # near-dupes, # stale, # skill candidates, per-project projection freshness, orphaned skill checks | extend |
| `retro ui` review | Approve/resolve queued judgment items via `GET /api/review` + `POST /api/review/resolve`; applying reuses the existing commit → reindex → reproject path under `run.lock` | extend |

`retro lint --fix` is **not** the apply surface. Lint stays a free report; the
curator owns mutation, so there is one place where "safe op" is defined.

### Data flow (one `retro run`)

```
observe → analyze (+ intake filter)
        → CURATOR: merge dupes ✓auto | archive stale ✓auto | absorb re-discovered ✓auto
                   cross-type dupes → review queue | skill cluster → review queue
        → commit "chore(curator): …"        ← curator's own commit, isolated
        → commit "retro: learn N node(s)"   ← analysis writes, as today
        → reindex → project (global + every scope the curator touched) → push
```

The curator runs **before** projection so the projected files reflect the
cleaned set the same run, and commits **separately** so its mutations are an
identifiable, revertable point in history.

## Static core (deterministic, no AI)

Both reuse the existing similarity primitive (`util::normalized_similarity` in
`retro-core/src/util.rs` — normalized character-level Levenshtein, already the
primitive behind `lint`).

### Merge near-duplicates — `merge_duplicates`, default `auto`

- **Eligibility:** same scope **and** same `node_type`. Pairs that match on
  content but differ in type are queued as a `cross-type-duplicate` review item,
  never auto-merged.
- **Detection:** a length-ratio pre-filter derived from the active threshold —
  skip when `len_diff_ratio > 1.0 - merge_similarity` (Levenshtein distance is
  at least the length difference, so a wider gap cannot reach the threshold) —
  then `normalized_similarity(a.body, b.body) > merge_similarity`
  (default **0.8**). `merge_similarity` is threaded into `lint::run_lint` so the
  hardcoded `0.2` / `0.8` literals disappear from both call sites.
- **Single deterministic sweep** (not "repeat to a fixpoint"): order active
  nodes by `(confidence desc, updated desc, slug asc)`; walk the list; the next
  not-yet-consumed node becomes an anchor; every eligible node `> threshold`
  **to that anchor** is consumed into it; consumed nodes are removed from the
  candidate set and never become anchors. The sweep runs exactly once per pass.
- **Survivor** keeps the anchor's slug (stable id) **and the anchor's body,
  unchanged**, takes `confidence` = max, `sources` = union, and **keeps the
  anchor's `updated`** (a merge is not fresh evidence that the rule is in use,
  and resetting the clock would exempt survivors from staleness forever). Each
  absorbed node gets `invalidated_by = survivor_slug` — the existing
  supersession mechanism, so no new "merged" state is needed.
- **Containment check.** If the absorbed body is not substantially contained in
  the survivor's (i.e. the absorbed node may carry a nuance the survivor lacks),
  the pair is queued as a `divergent-duplicate` review item instead of merged,
  with both bodies shown side by side so the user picks the wording. This is the
  band the previous draft's open question was reaching for, expressed as a
  property of the pair rather than a magic similarity window.
- **Idempotent by construction:** no surviving body ever changes, so similarity
  relationships among the remaining nodes are identical next pass, and absorbed
  nodes are inactive and skipped. The required test asserts the store is
  **byte-identical** after a second pass.
- **Accepted consequence:** `sources` = union means a survivor has ≥ 2 sources
  and therefore no longer satisfies the archive-stale `≤ 1 source` condition.
  That is intended — a rule independently observed in two sessions is genuinely
  not a one-off — but it is a real exemption and is documented, not incidental.

### Archive stale — `archive_stale`, default `auto`

- Conservative criteria — **all** must hold: node is active, **and**
  `confidence < confidence_threshold` (never matured past the 0.7 projection
  gate), **and** `updated < today − staleness_days` (default **28**), **and**
  `sources.len() ≤ 1` (only ever seen in a single session).
- Targets only sub-threshold, old, one-off noise. It never archives a
  high-confidence rule for being old (e.g. "pin actions to SHAs" does not
  decay), and never archives a node reinforced across multiple sessions.
- Archiving sets `archived` + `archived_reason = "stale"` (see "Data model").

### Absorb re-discovered nodes — always on when `skills` is not `off`

A pattern extracted into a skill is archived, so the analyzer never sees it
again — but a *new* session can still surface the same pattern as a fresh node
that duplicates the skill. After analysis, the curator compares each newly
created node against archived-into-skill nodes (`archived_reason` starting
`extracted:skill:`) in the same scope with the same similarity primitive and
threshold. On a hit the new node is absorbed into the archived one (union
`sources`, max `confidence`, target **stays archived**) rather than projecting a
bullet that restates a skill. This is deterministic, needs no new field, and
closes the loop the previous draft left open.

### Volume control (both auto ops)

- **Per-run cap:** `max_auto_merges_per_run` and `max_auto_archives_per_run`
  (default **3** each). Detections beyond the cap are deferred to the next run,
  and the deferral is `log()`-ed with the number withheld — never silently
  truncated.
- **First-pass gate:** if `RunnerState` has no record of a completed curator
  pass, this pass is **review-only** regardless of config, and it records the
  full detected backlog as review items. The design's own premise is a store
  carrying months of accumulated duplicates; the user sees that backlog before
  anything is applied to it. Losing machine-local state re-arms the gate, which
  is the safe direction.

### Safety / audit (both)

- All writes go through the existing atomic temp-rename path, then a **dedicated
  curator commit** (`store_git::commit_all` with `chore(curator): merged 3
  duplicates, archived 2 stale`) issued immediately after the curator's writes
  and **before** the analysis learn-commit — the runner already does exactly this
  for the exclusion sweep. Because `commit_all` is `git add -A`, a curator stage
  that committed nothing would otherwise have its mutations swept into
  `retro: learn N node(s)`, leaving no revertable point and no evidence in the
  log that the curator ran at all.
- `retro curate --undo-last` reverts the most recent `chore(curator)` commit and
  reprojects, so recovery does not require the user to read `~/.retro`'s git log.
  Recoverability is the justification for defaulting these ops to `auto`, so it
  has to be a command, not a theory.
- Every auto-applied action is recorded in the audit log and summarised in one
  line in the briefing / `retro doctor`.
- Under `review` the op produces a queue item instead of writing; under `off`
  detection is skipped entirely.

## Judgment calls (review by default)

### Skill extraction — `skills`, default `review`

- **Detection — no new AI call.** The existing analysis response is extended
  with an optional `skill_suggestion { topic, node_ids[], rationale }`. The
  analyzer emits one only when a set of active nodes describes a **process**
  (steps/sequence for handling something), not merely topically-related atomic
  preferences. At most one suggestion per run.
- Queued as a review item: *"These N patterns form a process about `<topic>`.
  Extract into on-demand skill `<name>` and pull them out of the rules file?"*
- **Backend trait change (required, not incidental).** The generator needs an
  agentic call, but `execute_agentic` is an inherent method on the concrete
  `ClaudeCliBackend` while every pipeline seam holds `&dyn AnalysisBackend`.
  So `AnalysisBackend` gains
  `fn execute_agentic(&self, prompt: &str, cwd: Option<&str>) -> Result<BackendResponse, CoreError>`
  and `MockBackend` gains a scripted implementation — without which the specced
  mock-backed test cannot be written at all.
- **On approval — one agentic AI call.** The generator itself must be **built,
  not reused**: v2's `projection::skill::generate_skill_agentic` (superpowers
  writing-skills instructions injected, snapshot-diff to find the created
  `SKILL.md`, plus a validation schema and retry loop) was deleted with the rest
  of the v2 core in Plan 4 (`a563a0d`, ~900 lines). It is a usable reference —
  `git show a563a0d^:crates/retro-core/src/projection/skill.rs` — but this is the
  largest single piece of work in the design, not a thin adapter over existing
  code.
- **Write target:** `~/.retro/skills/<name>/SKILL.md` inside the store (git-backed,
  not in `IGNORED_ENTRIES`), then projected into `<claude_dir>/skills/<name>/`
  on every run. Projection writes only skill directories the store owns, backs
  up any pre-existing file to `~/.retro/backups/` first, and never touches
  unrelated user skills.
- **Validated before archiving.** The archive is gated on the generated skill
  being *valid*, not merely written: frontmatter parses, `name` is present and
  `is_valid_slug`, description non-empty, body non-trivial. A skill whose
  frontmatter is malformed is never loaded by the agent, so archiving its source
  nodes on a bare successful write would strand the guidance. On failure the
  nodes stay active and the item stays pending, with a health record.
- **Name is validated before it reaches frontmatter.** `archived_reason =
  "extracted:skill:<name>"` interpolates a model-derived string into a
  line-oriented, unescaped frontmatter serialiser. `<name>` must pass
  `is_valid_slug`, and the serialiser rejects any `archived_reason` containing
  `\n` or `\r` — otherwise a stray newline injects frontmatter keys and the node
  becomes unparseable, which `load_all` handles by *silently skipping it*.
- Then **archive the source nodes** with `archived_reason =
  "extracted:skill:<name>"` — they stop projecting (rules file shrinks) and,
  being inactive, are not shown to the analyzer again. Commit → reindex →
  reproject.
- **Budget-gated** by the existing `max_ai_calls_per_day`; if exhausted, an
  approved extraction waits until the budget resets.
- **Under `auto`:** generate + extract without asking (spends AI).
- MVP: **global-scope clusters → global skills only.**

### Conflict detection — instrumentation only

The previous draft specced this as "stop dropping edges we already get". That
premise is false, and the correction is the reason the feature is cut rather
than merely deferred:

- The analysis prompt asks only for `supports, derived_from` edges
  (`analysis/prompts.rs:63`) and explicitly routes contradictions to *prose*
  rather than an edge (`prompts.rs:45`: *"note it but still create the new
  node"*).
- A missing or unrecognised `edge_type` is silently coerced to
  `EdgeType::Supports` (`analysis/mod.rs:93-94`), so contradiction edges are not
  merely dropped — they are indistinguishable from supporting ones.
- `GRAPH_ANALYSIS_RESPONSE_SCHEMA` is closed (`additionalProperties: false`) with
  no field to carry the *why* a resolve prompt would have to render.
- `edges_ignored` (`analysis/v3.rs`) is one undifferentiated counter, so there is
  no evidence about how often anything relevant arrives.

**What ships instead:** add explicit contradiction guidance to the prompt, add a
`reason` string to the response schema, make an unrecognised `edge_type` a
counted skipped op rather than a `Supports` default, and split `edges_ignored`
by type so `retro doctor` can report the real yield. Nothing is detected,
surfaced, or mutated. Once there is data showing contradiction edges actually
arrive with usable reasons, the resolve flow gets designed against measured
behaviour — and it will need the persisted review queue below, which is why that
lands now.

### Review queue (persisted)

Judgment items are produced inside a background `retro run` and must survive
until the user acts. `RunnerState.notifications` cannot carry them: it is an
untyped `Vec<String>`, capped at 50, and drained on the next session start.

- **Storage:** `state/review.json` (machine-local, gitignored via the existing
  `state/` entry in `IGNORED_ENTRIES`).
- **Item:** `{ id, kind, node_ids[], detail, status, created }` where `kind` ∈
  `cross-type-duplicate` | `divergent-duplicate` | `skill-extraction` |
  `deferred-backlog`, and `status` ∈ `pending` | `applied` | `dismissed`. `id` is
  a stable hash of `kind` + sorted `node_ids`, so the same finding always maps to
  the same item.
- **Surfaces:** `GET /api/review` lists pending items; `POST /api/review/resolve`
  applies or dismisses one, reusing the dashboard's existing
  commit → reindex → reproject path under `run.lock`. The briefing keeps
  announcing counts (its existing notification role), it just no longer *is* the
  queue.
- **Dedup on intake:** a finding whose `id` is already `dismissed` is never
  re-raised, and one already `pending` is not duplicated. Without this, a
  decision the user consciously made is re-surfaced on every run forever.

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

v3 stays **flat**. The only change:

- Add two optional fields to `Node` (`store/node.rs`): `archived:
  Option<NaiveDate>` and `archived_reason: Option<String>`, registered in the
  strict `from_markdown` parser (unknown keys are a hard error, so they must be).
- **Emit only when set.** Unlike `invalidated_by`, these keys are written
  *only* on nodes that are actually archived — never as `archived: null` on
  every rewrite. This is a correctness requirement, not tidiness: the store is
  git-synced across machines that may run different versions, an older binary
  treats an unknown frontmatter key as a hard parse error
  (`store/node.rs:199-203`), and `load_all` responds by **skipping that node with
  a warning** (`store/mod.rs:191-193`) while the 3.0.1 empty-wipe guard only
  fires at *zero* parsed nodes (`projection/local_md.rs:392-404`). Emitting the
  keys on every node would therefore let an older binary project a silently
  shrunken managed block — the 2026-07-23 data-loss class through a new door.
  With emit-only-when-set, the only nodes carrying the new keys are the ones an
  older binary should already be excluding, so the two versions agree on what
  projects.
- `is_active()` becomes `invalidated_by.is_none() && archived.is_none()`. This is
  the only behavioural change to the model; projection, reindex, and lint
  already gate on `is_active()` and inherit archived-exclusion for free.
- **Distinguish archived from vetoed in the surfaces.** The dashboard's xray
  breakdown currently labels every inactive node as *vetoed*, which would report
  "vetoed: 14" after the first curator pass when the user vetoed two. Add an
  `archived` bucket to the breakdown, an `archived` column/filter to the index
  and `/api/nodes`, and a `POST /api/node/unarchive` write action (the same
  `after_write` commit → reindex → reproject path the existing actions use).
  Without an unarchive route, recovery from a single unwanted archive means
  hand-editing `~/.retro/knowledge/**.md`.
- **Store-format marker.** Write a store-format version (`~/.retro/meta.toml`) so
  a future binary can refuse to operate on a store written by a newer one instead
  of silently degrading. Honest limitation: this does nothing for already-released
  3.1.x binaries, which cannot know to look for it — for the 3.1.x → 3.2.0 window
  the emit-only-when-set rule above is the whole protection.
- **Considered and rejected:** encoding archival inside the existing
  `invalidated_by` vocabulary (e.g. `invalidated_by: archived:stale`). It parses
  on every existing binary — that field is free-form, not slug-validated — so it
  carries zero cross-version risk. Rejected because it overloads "superseded by
  node X" with a non-node sentinel that the index, the UI, and every future query
  would have to prefix-sniff to tell an archive from a veto, which is exactly the
  distinction the surfaces above need to make.

## Config

New `[curator]` section (unknown TOML sections are ignored, so adding it is safe
in both version directions):

```toml
[curator]
merge_duplicates          = "auto"    # auto | review | off   (default: auto)
archive_stale             = "auto"    # auto | review | off   (default: auto)
skills                    = "review"  # auto | review | off   (default: review)
merge_similarity          = 0.8       # near-duplicate threshold (also drives the pre-filter)
max_auto_merges_per_run   = 3
max_auto_archives_per_run = 3
```

Reuses existing `analysis.staleness_days` (28) and
`knowledge.confidence_threshold` (0.7). Policy is a `CuratorPolicy` enum
`{ Auto, Review, Off }` (serde lowercase). There is no `conflicts` knob — the
conflict work in this pass is instrumentation with no user-facing behaviour.

## Error handling / safety

- The curator is a stage in `runner_v3`, wrapped so a failure **logs and
  continues** — it never aborts the run, blocks projection, or corrupts the
  store (the same per-stage isolation v3 already uses per-project).
- Order within a run: analysis → curator → **curator commit** → learn commit →
  reindex → project → push.
- **Reprojection covers what the curator touched.** Global projection is
  unconditional today, but local projection loops over `touched` — the projects
  that had a session analysed this run (`runner_v3.rs:362-367`). A curator that
  archives nodes in a dormant project would otherwise leave that project's
  `CLAUDE.local.md` projecting absorbed and archived bullets indefinitely, which
  is precisely the case `archive_stale` targets. The curator therefore returns
  the set of scopes it mutated, and the runner unions those into `touched` before
  the projection stage. `retro doctor`'s projection-freshness check is extended
  to iterate project scopes from `PathMap` as well as global, so this failure is
  visible rather than reported healthy.
- Skill extraction archives source nodes **only after** the `SKILL.md` writes
  *and validates*; a failed, invalid, or budget-blocked generation leaves nodes
  active and the item pending. No orphaning.
- All node ids from the review surface / model are `is_valid_slug`-checked
  before any path use (existing hardening), and so is any model-derived skill
  name before it reaches frontmatter.
- Idempotency is a required, tested property: a second curator pass over an
  already-clean store leaves the store byte-identical.
- No silent caps: the per-run cap, the first-pass gate, and any bounded window
  are `log()`-ed with what was withheld.

## Testing

- **Unit (fixtures, no AI):** single-sweep merge (survivor keeps its own body,
  union sources, max confidence, anchor's `updated` preserved); same-type and
  same-scope eligibility (a Memory node must never absorb a Rule); containment
  check routing divergent pairs to review; pre-filter derived from threshold (a
  lowered `merge_similarity` must actually widen detection); archive criteria
  (all four conditions; must not archive high-confidence-old or multi-source
  nodes); `is_active()` honouring `archived`; re-discovery absorption into an
  archived-extracted node; per-run caps and the first-pass gate; markdown
  round-trip **emitting the new keys only when set**, and an old-format file
  round-tripping unchanged when not archived; `archived_reason` containing a
  newline rejected at the serialiser.
- **Idempotency:** second pass over a curated store → store bytes identical, no
  new commit.
- **`MockBackend`:** intake filter rejects an "obvious" pattern; a
  `skill_suggestion` → review item → on approval, mocked generation writes a
  store-held skill, validation passes, source nodes archive, rules file shrinks;
  a mocked *invalid* skill leaves nodes active and the item pending. Requires the
  `execute_agentic` trait method and its mock implementation.
- **Review queue:** an item dismissed once is never re-raised; a pending item is
  not duplicated; resolve applies through commit → reindex → reproject.
- **Reprojection:** curator archives a node in a project with no session this
  run → that project's `CLAUDE.local.md` is still reprojected.
- **Undo:** `retro curate --undo-last` restores the pre-curator store state and
  reprojects.
- **Conflict instrumentation:** an unrecognised `edge_type` is counted as a
  skipped op rather than coerced to `Supports`; `edges_ignored` is reported by
  type.
- **Scenario test:** seed a store with same-type dupes, a cross-type dupe, stale
  nodes, and a process cluster → run → assert merged/archived/queued counts, a
  distinct `chore(curator)` commit, and a dropped projected bullet count
  (run-scenarios skill).
- **Config-policy tests:** each op under `auto` / `review` / `off`.
- Clean-install verification is deferred to completion per project rules
  (`retro init`/`start`/`stop` are never run during a dev session).
- Docs updated as part of the work: checked-in `CLAUDE.md` (command table +
  Key Design Decisions) and `README` where projection is described.

## Review log

Adversarial review 2026-08-24; 14 findings accepted and addressed here, 1
declined.

| # | Finding | Where addressed |
|---|---|---|
| 1 | Auto-merge had no node-type guard (a Memory node could absorb a projected Rule) | Merge eligibility: same scope **and** type; cross-type → review |
| 2 | New frontmatter keys hard-error on older binaries reading a synced store | Data model: emit only when set; store-format marker; rejected `invalidated_by` alternative documented |
| 3 | "Git preserves everything" undelivered — curator writes swept into the learn commit | Safety/audit: dedicated `chore(curator)` commit + `curate --undo-last` |
| 4 | Curator changes to dormant projects never reprojected | Error handling: curator returns mutated scopes, unioned into `touched`; doctor covers project scopes |
| 5 | Idempotency false; anchoring deferred chaining by one run | Merge semantics: body-preserving, single deterministic sweep, byte-identical idempotency test, anchor's `updated` kept |
| 6 | "Analyzer already emits Contradicts" false | Conflicts cut to instrumentation only |
| 7 | Review-gated findings had no persistence layer | Review queue (persisted) |
| 8 | Skill state split across the store boundary | Skill location: store-held and projected; doctor orphan checks |
| 9 | Length pre-filter hardcoded to 0.8, breaking a configurable threshold | Detection: pre-filter derived as `1.0 - merge_similarity`, threaded into lint |
| 10 | `execute_agentic` not on the trait; archive gated on write not validation | Backend trait change; validated-before-archiving gate |
| 11 | Model-supplied skill name unvalidated into frontmatter | Name validated with `is_valid_slug`; `\n`/`\r` rejected at the serialiser |
| 12 | Archived nodes rendered as user-vetoed, no unarchive path | Data model: `archived` bucket, index column, `POST /api/node/unarchive` |
| 13 | `lint` and curator disagreed on "stale"/"duplicate"; no dry-run preview | Shared detection functions; `retro curate --dry-run`; `lint --fix` dropped |
| 14 | No cap and no first-run gate on the initial pass | Volume control: per-run caps + review-only first pass |
| 15 | Sort key not unique across scopes (nondeterministic anchor order) | **Declined.** The single-sweep rewrite already clusters within a scope by the same-scope eligibility rule; the residual cross-scope tie affects only which cluster is written first, and body-preserving merge makes write order irrelevant to the outcome. |

## Open questions

All three previous open questions were resolved by the review and are folded
into the design above:

1. **Merge body selection** — resolved by making auto-merge body-preserving
   rather than by banding similarity. The survivor never adopts another node's
   text; pairs where the absorbed body is not contained in the survivor's go to
   review with both bodies shown.
2. **Conflict detection reach** — resolved by cutting conflicts from the MVP and
   shipping instrumentation, so the piggyback-vs-dedicated-pass decision is made
   against measured yield instead of an assumption.
3. **Re-discovery after extraction** — resolved by the "Absorb re-discovered
   nodes" pass plus store-held skills, which turns re-discovery into a normal
   case of the one-way projection model.

Remaining for planning (scoping, not design):

- Whether the skill generator lands as its own plan given it is the largest
  piece of work here, with the static core shipping first.
