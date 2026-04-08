# Retro 2.0: Bidirectional CLAUDE.md ↔ DB Reconciliation — Design Spec

## Overview

A reconciliation step that syncs the retro-managed section of CLAUDE.md files with the knowledge graph DB. Ensures the DB and file stay consistent when rules are added externally (team members, manual edits, DB recovery) or removed manually from the file.

**Depends on:** Plan 2 (Pipeline) — runs within `retro run` and `retro init`.

## Problem Statement

The DB and CLAUDE.md can drift apart in several scenarios:

1. **Team collaboration**: Team member A's retro discovers a pattern, creates a PR adding it to the project CLAUDE.md. After merge, team member B's retro DB doesn't know about this rule and may re-discover and re-project it, creating duplicates.
2. **DB recovery**: If the DB is lost or recreated (e.g., `retro init --uninstall --purge`), the CLAUDE.md file retains all rules but the DB has no corresponding nodes. The next analysis cycle would re-discover and attempt to re-project them.
3. **Manual removal**: A user removes a rule from the managed section of CLAUDE.md. The DB still thinks it's projected, creating a stale node that never gets cleaned up.

## Scope

### In Scope

- Bidirectional reconciliation between CLAUDE.md managed section and DB nodes
- Project CLAUDE.md (repo root, within `<!-- retro:managed:start/end -->` delimiters)
- Global `~/.claude/CLAUDE.md` (same delimiters)
- Integration into `retro run` (lightweight diff each cycle) and `retro init` (full seed)
- Dry-run support

### Out of Scope

- Skills reconciliation (`~/.claude/skills/`) — separate concern, can come later
- Semantic/fuzzy matching — exact string match only for now
- CLAUDE.md files without managed section delimiters (full management mode)
- Reconciliation of MEMORY.md or other files

## Reconciliation Logic

### Direction 1: File → DB (Import)

Rules present in the CLAUDE.md managed section but not matching any active projected node in the DB get imported as new nodes.

**Imported node properties:**
- `node_type`: `Rule`
- `confidence`: 0.8 (above default projection threshold of 0.7, indicating established rule)
- `status`: `Active`
- `projected_at`: current timestamp (already projected — it's in the file)
- `scope`: `Project` for project CLAUDE.md, `Global` for `~/.claude/CLAUDE.md`
- `project_id`: project slug for project-scoped, `None` for global
- `pr_url`: `None`

### Direction 2: DB → File (Archive)

Nodes in the DB that are marked as projected (status Active, `projected_at IS NOT NULL`) whose content no longer appears in the CLAUDE.md managed section get archived.

**Archive behavior:**
- Set `status` to `Archived`
- Preserve all other fields (content, confidence, projected_at, etc.)
- The node won't be re-projected (archived nodes are excluded from unprojected queries)
- If the same pattern is re-discovered from future sessions, it creates a new node

### Matching

Exact string match between managed section bullet text (parsed by `read_managed_section()`) and `node.content`.

An edited rule is treated as "old removed + new added":
- Old node gets archived (content no longer in file)
- New node gets imported (new text found in file)

This is predictable and debuggable. Semantic/fuzzy matching can be added later if churn becomes a problem.

### Which nodes participate in reconciliation

Only nodes with ALL of these properties:
- `status = Active`
- `projected_at IS NOT NULL` (has been projected)
- `node_type` in (`Rule`, `Directive`, `Preference`) — these are the types that project to CLAUDE.md
- Correct scope: `Global` for `~/.claude/CLAUDE.md`, `Project` for project CLAUDE.md
- Correct `project_id` for project-scoped nodes

## Integration Points

### `retro run` — Step 0 (before observe/ingest/analyze)

New step "Reconciling CLAUDE.md state" runs before the existing pipeline:

```
Step 0/6: Reconciling CLAUDE.md...
  Reconciled: 3 imported, 1 archived
Step 1/6: Observing session changes...
...
```

**Flow:**
1. Read managed section from project CLAUDE.md → `file_rules: Vec<String>`
2. Query DB for active projected nodes scoped to this project (Rule/Directive/Preference types) → `db_nodes: Vec<KnowledgeNode>`
3. **Import**: For each rule in `file_rules` not matching any `db_nodes[].content` → insert node
4. **Archive**: For each node in `db_nodes` whose content doesn't match any `file_rules` → set status to Archived
5. Repeat steps 1-4 for global `~/.claude/CLAUDE.md` with global-scoped nodes
6. Print summary

**Dry-run**: Shows what would be reconciled without modifying the DB.

### `retro init` — After project registration

Same reconciliation logic runs after the DB is created and the project is registered. This handles:
- Fresh installs where CLAUDE.md already has rules from a team member
- Recovery after DB purge

```
  Registered project: my-app (/path/to/my-app)
  Reconciled: 15 imported from CLAUDE.md
```

## Modified Files

| File | Change |
|------|--------|
| `crates/retro-core/src/reconcile.rs` | New module: `reconcile_claude_md()`, `reconcile_for_scope()` |
| `crates/retro-core/src/lib.rs` | Add `pub mod reconcile;` |
| `crates/retro-core/src/db.rs` | Add `get_projected_nodes_for_scope()`, `archive_node()` |
| `crates/retro-cli/src/commands/run.rs` | Call reconciliation as Step 0 |
| `crates/retro-cli/src/commands/init.rs` | Call reconciliation after project registration |

## Edge Cases

| Condition | Behavior |
|-----------|----------|
| No managed section in CLAUDE.md | Skip file → DB for that file |
| CLAUDE.md doesn't exist | Skip entirely |
| Node was projected via PR (has `pr_url`) | Still reconcile — if rule gone from file after PR merge, archive it |
| Multiple nodes match same rule text | Don't import duplicate — one match is sufficient |
| Empty managed section (delimiters exist but no rules) | Archive all projected nodes for that scope |
| Node type is Memory or Skill | Not included in reconciliation (these don't project to managed section) |
| Rule text has leading/trailing whitespace | `read_managed_section()` already trims — matching is on trimmed text |

## Testing Strategy

### Unit Tests
- `reconcile_for_scope()` with mock data: file has rules A, B, C; DB has nodes B, C, D → imports A, archives D
- Empty file rules with existing DB nodes → all archived
- Empty DB with existing file rules → all imported
- No managed section → no changes
- Duplicate rule text in file → only one node created

### Integration Tests
- Full `reconcile_claude_md()` with temp files and in-memory DB
- Verify imported nodes have correct type, confidence, status, projected_at
- Verify archived nodes retain their content and metadata

### Scenario Tests
- `retro run --dry-run` shows reconciliation summary without modifying DB
