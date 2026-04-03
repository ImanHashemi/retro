# Retro — Active Context Curator for AI Coding Agents

Rust CLI tool that watches your coding agent sessions, discovers what you keep repeating, and turns those patterns into persistent context — automatically. Session over session, your agent gets better without you maintaining its context by hand.

## Architecture

### v2 "The Watcher" (primary)

Five-layer pipeline, data flows upward:

```
Surfaces        (TUI dashboard, in-session briefing, CLI)
Projectors      (Claude Code — pluggable for other agents)
Knowledge Store (graph-modeled in SQLite: nodes + edges)
Analyzers       (pattern discovery, scope classification)
Observers       (session watcher via mtime polling)
Scheduled Runner (launchd periodic job)
```

`retro run` executes the full pipeline. `retro start` installs a launchd job that runs it every 5 minutes. `retro dash` opens the TUI dashboard for reviewing suggestions and browsing knowledge.

### v1 (legacy, still functional)

Three-stage pipeline: **Ingestion** → **Analysis** → **Projection**. Driven by post-commit hooks (`--auto` flag, now deprecated). All v1 commands still work.

### Storage

`~/.retro/` contains: SQLite DB (WAL mode, schema v4), JSONL audit log, config.toml, `briefings/` directory, `runner.log`.

## Repo Structure

Cargo workspace with two crates:
- `crates/retro-core/` — library crate (all logic, models, DB, analysis, knowledge graph, observers, runner helpers)
- `crates/retro-cli/` — binary crate (clap commands, TUI dashboard, launchd integration)
  - `src/tui/` — TUI module (app state, rendering, event handling)
  - `src/launchd.rs` — macOS launchd plist generation and management
- `tests/` — fixtures and integration tests
- `scenarios/` — scenario-based integration tests (see [scenarios/README.md](scenarios/README.md))

## Build & Test

```bash
# Build (requires Rust toolchain and C compiler for bundled SQLite)
cargo build

# Run unit tests
cargo test

# Run scenario tests
./scenarios/README.md  # see file for test runner usage

# Always run tests before committing
cargo test && cargo run -- --help  # verify build

# Clean install testing
retro init --uninstall --purge && cargo build --release && ./target/release/retro init
```

## Commands Overview

### v2 Commands (new)

| Command | Purpose |
|---------|---------|
| `retro run [--dry-run]` | Run the full v2 pipeline (observe → ingest → analyze → project) |
| `retro start` | Start the scheduled runner (launchd on macOS) |
| `retro stop` | Stop the scheduled runner |
| `retro dash` | Open the TUI dashboard (review suggestions, browse knowledge) |

### Core Commands

| Command | Purpose |
|---------|---------|
| `retro init` | Initialize retro (creates DB, config, launchd job, briefing skill) |
| `retro ingest [--auto]` | Scan Claude Code session files and save to DB |
| `retro analyze [--dry-run] [--auto]` | AI-powered pattern discovery from sessions |
| `retro patterns` | List discovered patterns |
| `retro apply [--dry-run] [--auto] [--global]` | Generate skills/CLAUDE.md from patterns (saved as PendingReview) |
| `retro review` | Review and approve/skip/dismiss pending projections |
| `retro sync` | Sync PR state, reset patterns from closed PRs |
| `retro curate [--dry-run]` | AI-assisted CLAUDE.md editing (direct file write) |
| `retro diff [--global]` | Preview changes to CLAUDE.md or global agents |
| `retro status` | Show summary of sessions, patterns, projections |
| `retro clean [--dry-run]` | Archive stale patterns |
| `retro audit [--dry-run]` | Context audit (detects inconsistencies) |
| `retro log [--since <days>]` | View audit log entries |
| `retro hooks remove` | Remove git hooks |
| `retro init --uninstall [--purge]` | Uninstall retro (removes launchd plist, hooks, optionally data) |

Note: `--auto` flag is deprecated in v2. Use `retro start` for automatic background operation.

## Key Design Decisions

### Core Architecture

- **Rust, sync only** — no tokio, no async. `std::process::Command` for spawning `claude` CLI and `git`/`gh`.
- **No git2 crate** — shell out to `git` and `gh` directly for simplicity and reliability.
- **SQLite bundled** — `rusqlite` with `bundled` feature. WAL mode always. Schema versioned via `PRAGMA user_version`.
- **Error handling** — `thiserror` in retro-core, `anyhow` in retro-cli. `CoreError` implements `std::error::Error` — use `?` directly in CLI commands.

### AI Backend

- **Sync trait** — `AnalysisBackend` trait with `json_schema: Option<&str>` parameter.
- **Primary impl** — `ClaudeCliBackend` uses `claude -p - --output-format json` (prompt piped via stdin to avoid ARG_MAX issues).
- **Structured output** — JSON-producing calls pass `--json-schema` for constrained decoding (guaranteed valid JSON, no sanitization needed). Schema constants: `ANALYSIS_RESPONSE_SCHEMA` (analysis/mod.rs), `SKILL_VALIDATION_SCHEMA` (projection/skill.rs), `AUDIT_RESPONSE_SCHEMA` (curator.rs), `GRAPH_ANALYSIS_RESPONSE_SCHEMA` (analysis/mod.rs).
- **CLI quirks**:
  - `--json-schema` conflicts with `--tools ""` on large prompts — only pass `--tools ""` when NOT using `--json-schema`.
  - `--json-schema` uses an internal tool call for constrained decoding, so model needs extra turns — `--max-turns 5` gives headroom (observed max 4 turns).
  - Without `--tools ""`, model sometimes makes tool calls consuming turns — `--max-turns 5` prevents turn exhaustion.
  - Non-schema calls use `--tools "" --max-turns 1` (safe, no tool calls possible).
  - Output appears in `structured_output` field (parsed JSON), NOT `result` (empty string). `ClaudeCliOutput` checks `structured_output` first, serializes to string, falls back to `result`.
  - Token counts nest inside `usage` object — never assume top-level fields exist (use nested struct with `#[serde(default)]`). Sum `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` for total input.
- **YAML-producing calls** — skill/agent generation passes `None` for json_schema (free-form output).

### Knowledge Store (v2)

- **Graph-modeled in SQLite** — `nodes` table (knowledge items) + `edges` table (relationships) + `projects` table.
- **Node types** — `preference`, `pattern`, `rule`, `skill`, `memory`, `directive`. Memory nodes are context-only (not projected to output files).
- **Edge types** — `supports`, `contradicts`, `supersedes`, `derived_from`, `applies_to`.
- **Scopes** — `global` (travels across projects) vs `project` (local). Project scope overrides global on conflicts.
- **Confidence model** — single-session observations at 0.4–0.5, recurrence bumps confidence, explicit directives at 0.7–0.85. Threshold (default 0.7) gates projection.
- **Node status** — `Active`, `PendingReview`, `Dismissed`, `Archived`. `supersedes` edges auto-archive the old node.
- **Project identity** — human-readable slug from repo directory name (e.g., `my-rust-app`), stable once assigned. `remote_url` for reconnection after repo moves.

### Session Observer (v2)

- **Mtime-based polling** — `find_modified_sessions()` scans `~/.claude/projects/` for session files modified since last check.
- **No filesystem watching** — simple, like the existing `glob`-based approach. The scheduled runner polls periodically.

### Scheduled Runner (v2)

- **launchd (macOS only)** — plist at `~/Library/LaunchAgents/com.retro.runner.plist`. systemd (Linux) deferred.
- **Modern launchctl API** — uses `launchctl bootstrap`/`bootout` (not deprecated `load`/`unload`).
- **Default interval** — 300 seconds (5 minutes). Configurable via `runner.interval_seconds`.
- **`retro run` global mode** — when invoked without project context (launchd), iterates all known projects. Error in one project doesn't block others.
- **Cost control** — `max_ai_calls_per_day` (default 10), tracked in metadata table, resets daily.
- **Log rotation** — `runner.log` rotated at 1 MB, keeps 1 backup (`runner.log.1`).
- **Schema check** — `retro run` requires schema v4. Bails with migration prompt if v3 detected.

### TUI Dashboard (v2)

- **`retro dash`** — ratatui + crossterm terminal UI.
- **Two tabs** — Pending Review (approve/dismiss nodes) and Knowledge (browse all active nodes with scope/type filters and search).
- **Status bar** — runner active/stopped, last run time, AI calls today.
- **Keyboard-driven** — vim-style (j/k/g/G), Tab to switch panels, `/` to search, `a`/`d` to approve/dismiss, `s`/`t` to cycle filters.
- **Minimum terminal size** — 60x15. Prints error and exits if too small.
- **DB updates** — approve/dismiss write to DB immediately, remove from list, show transient message.

### Pattern Discovery

- **Pattern merging** — AI-assisted (primary) with strong semantic dedup prompt guidance + Levenshtein similarity > 0.8 safety net.
- **Pattern accumulation** — single-session observations stored at 0.4–0.5 confidence; confirmed when behavior recurs across sessions (AI emits "update" action bumping confidence). Explicit directives ("always"/"never"/"must") get 0.7–0.85 confidence from a single session.
- **Projection gating** — confidence threshold (default 0.7) is the sole gate for projection. No `times_seen` minimum — explicit directives can project from a single session.
- **Session filtering** — sessions with < 2 user messages are low-signal (retro's own `claude -p` calls, compacted sessions) and filtered before AI analysis. They are still recorded as analyzed to prevent reprocessing. `analyze --dry-run` shows skipped count.
- **User message truncation** — `MAX_USER_MSG_LEN = 500` chars in prompt serialization (balances signal quality vs token budget).
- **Rolling window analysis** — `rolling_window` config (default `true`) re-analyzes all sessions within the time window each run, enabling cross-session pattern discovery. When `false`, sessions are analyzed once and excluded (legacy behavior). Dry-run always shows only unanalyzed sessions regardless of this setting.
- **Analysis response** — includes `reasoning` field (1-2 sentence summary of what the model observed) displayed truncated per batch, full with `--verbose`.

### Projection & Apply

- **CLAUDE.md protection** — only write within `<!-- retro:managed:start/end -->` delimiters, never touch user content.
- **MEMORY.md** — read-only input, never write. Claude Code owns it.
- **Skill generation** — one skill per AI call (quality over cost), two-phase: generate then validate.
- **Review queue** — `retro apply` generates content and saves as `PendingReview` (no file writes or PRs). `retro review` is the gate: displays numbered list, user batch-selects apply/skip/dismiss (e.g., `1a 2a 3d` or `all:a`). Preview with `{N}p`. In v2, `retro dash` provides TUI-based review as an alternative.
- **Sync** — `retro sync` checks PR state via `gh pr view --json state` — resets patterns from closed PRs to `Discovered`. Both `retro apply` and `retro review` run sync first.
- **Two-track classification** — personal actions (skills, MEMORY.md edits) apply on current branch; shared actions (CLAUDE.md edits) create new `retro/updates-{YYYYMMDD-HHMMSS}` branch.
- **PR creation flow** — detect default branch via `gh repo view` → `git fetch origin <default>` → `git checkout -b retro/... origin/<default>` → write/commit → `git push -u origin HEAD` → `gh pr create --base <default>`. Always push before `gh pr create` (remote branch must exist).
- **Stash wrapper** — `stash_push()`/`stash_pop()` around branch switches (`git checkout -b` fails if tracked files differ when working tree is dirty).
- **Backup** — files backed up to `~/.retro/backups/` before modification.

### Full CLAUDE.md Management

- **Opt-in mode** — `[claude_md] full_management = true` in config. Default is `false` (managed section only).
- **Granular edits** — when enabled, `retro apply` uses extended analysis schema (`full_management_analysis_schema()`) that includes `claude_md_edits` (add/remove/reword/move). Edits flow through the standard apply → review pipeline.
- **Agentic rewrite** — `retro curate` runs `execute_agentic()` with full tool access to explore codebase, proposes complete CLAUDE.md rewrite via PR on `retro/curate-{YYYYMMDD-HHMMSS}` branch.
- **Agentic AI calls** — `execute_agentic()` uses `claude -p` with unlimited turns, full tool access, no `--json-schema` (raw markdown output), optional `cwd`, 600s timeout. Shared `run_claude_child()` helper handles stdin/stdout/stderr piping and timeout for both `execute()` and `execute_agentic()`.
- **Delimiter dissolution** — `dissolve_if_needed()` removes `<!-- retro:managed:start/end -->` markers when full management is first enabled, preserving rule content in place. Backs up to `~/.retro/backups/`.
- **Edit types** — `ClaudeMdEdit` (Add/Remove/Reword/Move) in `models.rs`. `apply_edit()`/`apply_edits()` in `projection/claude_md.rs`. Review command shows icons: `[rule+]`, `[rule-]`, `[rule~]`, `[rule>]`.

### Runtime Models

**v2 Scheduled Runner (primary):** `retro start` installs a launchd periodic job. `retro run` executes the full pipeline each invocation. No long-running daemon — launchd handles scheduling, lifecycle. Lockfile prevents concurrent runs.

**v1 Git Hooks (deprecated):** Post-commit hook runs `retro ingest --auto` which chains analyze + apply. Per-stage cooldowns (`ingest_cooldown_minutes`, `analyze_cooldown_minutes`, `apply_cooldown_minutes`). Session cap (`auto_analyze_max_sessions`). `--auto` flag prints deprecation warning in v2 directing users to `retro start`.

### Observability

- **Audit log** — append-only JSONL at `~/.retro/audit.jsonl`. Every auto-mode decision gets an entry.
- **Terminal nudge** — `check_and_display_nudge()` runs before interactive commands, shows colored status block with pending review count.
- **Token tracking** — `BackendResponse` carries `input_tokens`/`output_tokens` (not dollar cost).
- **Runner log** — `~/.retro/runner.log` captures stdout/stderr from scheduled `retro run` invocations. Rotated at 1 MB.

### Data Models

- **Domain types** — all in `retro-core/src/models.rs`. Both v1 (`Pattern`, `Projection`) and v2 (`KnowledgeNode`, `KnowledgeEdge`, `KnowledgeProject`, `GraphOperation`) types coexist.
- **DB schema** — v4. Adds `nodes`, `edges`, `projects` tables alongside existing v1 tables. `PRAGMA user_version = 4`. Migration from v3 is deterministic (no AI calls).
- **v2 node types** — `NodeType` (Rule, Directive, Pattern, Skill, Memory, Preference), `NodeScope` (Global, Project), `NodeStatus` (Active, PendingReview, Dismissed, Archived).
- **v1 types preserved** — `Pattern`, `Projection`, `ProjectionStatus`, `ApplyAction`, `ApplyTrack`, `ApplyPlan`. Used by existing v1 commands.
- **ToolResultContent enum** — `Text(String)` | `Blocks(Vec<Value>)` (tool results can be string or array).

## Dependencies

| Crate | Purpose |
|-------|---------|
| clap (derive) | CLI parsing |
| rusqlite (bundled) | SQLite |
| serde + serde_json | JSON/JSONL parsing |
| anyhow + thiserror | Error handling |
| chrono | Timestamps, rolling window |
| uuid | Pattern/projection IDs, knowledge node IDs |
| glob | Finding session files |
| colored | Terminal output |
| regex | Sensitive data scrubbing |
| libc | Process-alive check (kill signal 0), launchd UID |
| toml | Config file parsing |
| ratatui | TUI dashboard rendering (retro-cli only) |
| crossterm | Terminal backend for ratatui (retro-cli only) |
| tempfile | Test-only: temporary directories |

## Coding Conventions

### File Organization

- All domain types in `retro-core/src/models.rs`
- All DB operations in `retro-core/src/db.rs`
- Platform-independent runner helpers in `retro-core/src/runner.rs` (log rotation, state queries)
- Platform-specific launchd integration in `retro-cli/src/launchd.rs`
- TUI module in `retro-cli/src/tui/` (app.rs, ui.rs, event.rs)
- Shared helpers:
  - `git_root_or_cwd()` lives in `retro-cli/src/commands/mod.rs` — use `super::git_root_or_cwd`
  - `truncate_str()` lives in `retro-core/src/util.rs` — safe UTF-8 truncation
  - `build_curate_prompt()` lives in `analysis/prompts.rs`
  - `run_claude_child()` shared helper in `analysis/claude_cli.rs`
- CLI commands that share logic should expose a shared entry point (e.g., `run_apply()` with `DisplayMode` enum) rather than duplicating code

### Error Handling

- `CoreError` implements `std::error::Error` via thiserror — use `?` directly in CLI commands (no `.map_err(|e| anyhow!("{e}"))`)
- `thiserror` in retro-core, `anyhow` in retro-cli

### JSON/JSONL Parsing

- Use `#[serde(default)]` on all optional fields for forward-compatibility with JSONL format changes
- Skip unparseable JSONL lines gracefully (log warning for known types, silent skip for unknown types)
- Pre-parse `type` field from JSONL before full deserialization to distinguish unknown entry types from parse errors in known types
- JSON-producing AI calls use `--json-schema` constrained decoding — response is guaranteed valid JSON, no sanitization needed

### String Handling

- String truncation must use `truncate_str()` helper — never slice at arbitrary byte offsets (UTF-8 panic risk)
- Path decoding uses `recover_project_path()` which reads `cwd` from session files — naive decode breaks on paths with hyphens

### Process Management

- Process-alive checks use `libc::kill(pid, 0)` — portable across Linux and macOS (not `/proc/` which is Linux-only)
- AI prompts must be piped via stdin (`-p -`), never as CLI arguments (ARG_MAX risk with 150K prompts)
- Git/gh shell-outs use `Command::new().args()` (not shell strings) — each arg passed directly to `execve()`, safe from injection
- Launchd management uses modern `launchctl bootstrap`/`bootout` API (not deprecated `load`/`unload`)

### Git Hooks

- Hook format: marker comment (`# retro hook - do not remove`) + command on next line; removal is line-pair based
- `install_hook_lines` returns `HookInstallResult` (Installed/Updated/UpToDate)
- `retro init` updates existing hooks to new redirect format (remove+re-add)
- v2: hooks are supplementary. The scheduled runner (`retro start`) is the primary automation mechanism.

### Database

- Batch DB queries into HashSets when filtering (avoid N+1 queries in loops)
- Re-export internal types from retro-core when CLI crate needs them (e.g., `pub use rusqlite::Connection` in `db.rs`) — avoid adding transitive deps to retro-cli
- `init_db()` public wrapper over `migrate()` for in-memory test DBs

### User Interaction

- Confirmation prompts use `stdin` y/N pattern (not dialoguer) — keep it simple
- TUI uses ratatui alternate screen + raw mode with proper cleanup on exit

### Testing

- Test strategy: unit tests with fixtures (no AI), integration tests with `MockBackend`
- Scenario tests in `scenarios/` directory — see `scenarios/README.md` for usage
- `--dry-run` on all AI commands must skip AI calls entirely — snapshot context, show summary, return early (not just suppress writes)
- `analyze --dry-run` shows skipped count in summary and per-session in `--verbose` mode

### Performance

- When progressively fitting content into a prompt budget, drop items from the end — never truncate mid-JSON
- Project-scoped commands resolve project path via `git rev-parse --show-toplevel`, falling back to cwd

## Implementation Status

### v1 (retro 0.1–0.3)

All core features complete and tested.

- **Phase 1–7: DONE** — Full v1 pipeline: ingestion, analysis, projection, apply, clean, audit, review queue, hooks, auto-apply.
- **Pattern discovery quality: DONE** — Structured output, session filtering, explicit directives, confidence-based gating, rolling window.
- **Full CLAUDE.md management: DONE** — Granular edits, agentic rewrite via `retro curate`, delimiter dissolution.

### v2 "The Watcher" (retro 2.0)

- **Plan 1: DONE** — Foundation. v2 domain types (KnowledgeNode, KnowledgeEdge, etc.), config evolution (runner, trust, knowledge sections), schema v4 (nodes, edges, projects tables), node/edge/project CRUD, v3→v4 data migration.
- **Plan 2: DONE** — Pipeline. Session observer, graph analysis (prompt builder + response parser), Claude Code projector (rules → CLAUDE.md), briefing file generation, trust-based auto-approve, `retro run` command.
- **Plan 3: DONE** — Surfaces. TUI dashboard (`retro dash`), launchd scheduled runner (`retro start`/`retro stop`), `retro init` evolution (launchd + briefing skill), `--auto` deprecation, runner log rotation, default interval 300s, `retro run` global mode.

Test coverage: 228 unit tests.

## Testing

Always run tests before committing:
```bash
cargo test
```

- Before completing any implementation work, run all scenario tests to verify nothing broke. Use the run-scenarios skill.
- When debugging issues, always investigate and identify the root cause before proposing fixes. Do not implement symptom-based patches or workarounds without understanding why the problem occurs.
- After completing implementation work, always check if documentation (CLAUDE.md, README.md) needs updates to reflect the changes.
- When AI operations return unexpected or counterintuitive results (e.g., 0 patterns found, empty responses), include a `reasoning` field in the response schema and display it to the user.
- For major changes, provide commands for clean install testing: `retro init --uninstall --purge && cargo build --release && ./target/release/retro init`
- To release: bump version numbers in both Cargo.toml files, merge PR, then `git tag vX.Y.Z && git push origin vX.Y.Z`. The `.github/workflows/publish.yml` workflow handles testing, crates.io publishing, and GitHub release creation automatically.

<!-- retro:managed:start -->
## Retro-Discovered Patterns

- User follows a strict PR workflow: implement changes, create branch, push and create PR, run /review, address review items, run /security-review, then iteratively check PR comments (often 5+ rounds). In session ade70b4d (PR #245): user checked comments 7+ times over 3 days, asking 'check the comments again' and 'check the pr state once more'. Also validates that previous review comments are resolved before checking for new ones: 'Check the comments again to see if those that were there are resolved and if there are any new ones.'
- Always run scenario/e2e tests before claiming work is complete. User asks 'Did you do an end-to-end test to confirm everything is working?', 'Did you test the change? Do you run the retro test to see if everything runs correctly?', 'Of course we need to run the scenario tests first to make sure that they work'. Seen in 10+ sessions. Also: 'no need to run all scenarios, just one or two that are most relevant to this change' — sometimes selective testing is acceptable.
- After merging PRs, user typically wants to release a new version. A GitHub Actions workflow auto-publishes to crates.io on tag push (created in session 1f43e081, using dtolnay/rust-toolchain). User says 'Merge the PR and let's release a new version', 'lets publish a new version', 'merge it, make sure the version numbers are bumped correctly so a new release is done instantly'. Actively monitors whether the CI pipeline successfully published after merge.
- User cares deeply about documentation quality. After features land, they ask 'did you update claude.md?' and 'Do we need to update any documentation or README?'. Proactively requests: 'Lets do a full run through of the documentation now', 'take proper time thinking and designing the documentation updates (claude and readme) to make sure the quality of the docs stay really good'. Seen in sessions 1f43e081, e78a1a7d, 2b626ce8.
- User wants retro built primarily for Claude Code but with an open architecture that can extend to other agents (Gemini, Open Code, etc). Quote: 'build for Claude Code but keep it open so we can easily extend in the future to Gemini or Open Code or whatever'. Made this an explicit design decision during retro 2.0 brainstorming.
- User wants retro to feel automatic (not like a chore to run manually) while maintaining a sense of user control and insight. Key quotes: 'I want to be in control but have it automatically become better', 'It still feels like a chore I need to do', 'I just want a place where I can see all knowledge that's stored. I think that's an important feature. Helps with the feeling in control.' Also cares about dashboard/TUI UX quality: 'How can we make this tui nicer?'
- For fixes and small features, user prefers branching from main: 'branch from main and do everything on a separate branch', 'fix it, in a branch from main and create a pr for it'. Feature work uses feature branches. Consistently seen across 6+ sessions.

<!-- retro:managed:end -->
