# Retro — Active Context Curator for AI Coding Agents

Rust CLI tool that analyzes Claude Code session history to discover repetitive instructions, recurring mistakes, workflow patterns, and explicit directives — then projects them into skills and CLAUDE.md rules.

## Architecture

Three-stage pipeline: **Ingestion** (pure Rust, no AI) → **Analysis** (AI-powered, pluggable backend) → **Projection** (two-track: personal auto-apply, shared via PR).

Storage lives in `~/.retro/` (SQLite WAL mode + JSONL audit log + config.toml).

## Repo Structure

Cargo workspace with two crates:
- `crates/retro-core/` — library crate (all logic)
- `crates/retro-cli/` — binary crate (clap commands)
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
```

## Commands Overview

| Command | Purpose |
|---------|---------|
| `retro init` | Initialize retro (creates DB, installs hooks) |
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
| `retro init --uninstall [--purge]` | Uninstall retro |

## Key Design Decisions

### Core Architecture

- **Rust, sync only** — no tokio, no async. `std::process::Command` for spawning `claude` CLI and `git`/`gh`.
- **No git2 crate** — shell out to `git` and `gh` directly for simplicity and reliability.
- **SQLite bundled** — `rusqlite` with `bundled` feature. WAL mode always. Schema versioned via `PRAGMA user_version`.
- **Error handling** — `thiserror` in retro-core, `anyhow` in retro-cli. `CoreError` implements `std::error::Error` — use `?` directly in CLI commands.

### AI Backend

- **Sync trait** — `AnalysisBackend` trait with `json_schema: Option<&str>` parameter.
- **Primary impl** — `ClaudeCliBackend` uses `claude -p - --output-format json` (prompt piped via stdin to avoid ARG_MAX issues).
- **Structured output** — JSON-producing calls pass `--json-schema` for constrained decoding (guaranteed valid JSON, no sanitization needed). Schema constants: `ANALYSIS_RESPONSE_SCHEMA` (analysis/mod.rs), `SKILL_VALIDATION_SCHEMA` (projection/skill.rs), `AUDIT_RESPONSE_SCHEMA` (curator.rs).
- **CLI quirks**:
  - `--json-schema` conflicts with `--tools ""` on large prompts — only pass `--tools ""` when NOT using `--json-schema`.
  - `--json-schema` uses an internal tool call for constrained decoding, so model needs extra turns — `--max-turns 5` gives headroom (observed max 4 turns).
  - Without `--tools ""`, model sometimes makes tool calls consuming turns — `--max-turns 5` prevents turn exhaustion.
  - Non-schema calls use `--tools "" --max-turns 1` (safe, no tool calls possible).
  - Output appears in `structured_output` field (parsed JSON), NOT `result` (empty string). `ClaudeCliOutput` checks `structured_output` first, serializes to string, falls back to `result`.
  - Token counts nest inside `usage` object — never assume top-level fields exist (use nested struct with `#[serde(default)]`). Sum `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` for total input.
- **YAML-producing calls** — skill/agent generation passes `None` for json_schema (free-form output).

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
- **Review queue** — `retro apply` generates content and saves as `PendingReview` (no file writes or PRs). `retro review` is the gate: displays numbered list, user batch-selects apply/skip/dismiss (e.g., `1a 2a 3d` or `all:a`). Preview with `{N}p`.
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

### Auto-Apply Pipeline

- **Single hook** — post-commit hook orchestrates full pipeline (`retro ingest --auto` chains analyze + apply).
- **Per-stage cooldowns** — `ingest_cooldown_minutes` (5), `analyze_cooldown_minutes` (1440), `apply_cooldown_minutes` (1440) — each stage matches its cost profile.
- **Data triggers** — prevent unnecessary runs (e.g., skip analyze if no unanalyzed sessions).
- **Session cap** — `auto_analyze_max_sessions` (default 15) skips auto-analyze when backlog exceeds cap.
- **Hook stderr** — `2>>~/.retro/hook-stderr.log` (not `/dev/null`) — captures parse warnings, panics. `retro init` truncates it.
- **Auto-apply config** — `auto_apply` (default `true`) gates whether apply stage runs automatically.
- **Orchestration lock** — prevents concurrent analyze runs.

### Observability

- **Audit log** — append-only JSONL at `~/.retro/audit.jsonl`. Every auto-mode decision gets an entry: `ingest` (success), `analyze_skipped` (session_cap or cooldown_or_no_data), `analyze_error`, `apply_skipped` (no_qualifying_patterns), `apply_error`, enriched `apply` (with `pr_url`).
- **Terminal nudge** — `check_and_display_nudge()` runs before interactive commands, reads audit entries since `last_nudge_at` (stored in `metadata` table), groups within 60s as one run, displays colored multi-line status block, updates `last_nudge_at`. Shows pending review count alongside auto-run summaries.
- **Token tracking** — `BackendResponse` carries `input_tokens`/`output_tokens` (not dollar cost).

### Data Models

- **Domain types** — all in `retro-core/src/models.rs`.
- **DB schema** — v3, all operations in `retro-core/src/db.rs`. `projections` table has `status` column (`TEXT NOT NULL DEFAULT 'applied'` for migration compatibility). `metadata` table stores `last_nudge_at`.
- **ProjectionStatus enum** — `PendingReview`, `Applied`, `Dismissed` (tracks review queue lifecycle).
- **ToolResultContent enum** — `Text(String)` | `Blocks(Vec<Value>)` (tool results can be string or array).

## Dependencies

| Crate | Purpose |
|-------|---------|
| clap (derive) | CLI parsing |
| rusqlite (bundled) | SQLite |
| serde + serde_json | JSON/JSONL parsing |
| anyhow + thiserror | Error handling |
| chrono | Timestamps, rolling window |
| uuid | Pattern/projection IDs |
| glob | Finding session files |
| colored | Terminal output |
| regex | Sensitive data scrubbing |
| libc | Portable process-alive check (kill signal 0) |
| toml | Config file parsing |
| tempfile | Test-only: temporary directories for hook tests |

## Coding Conventions

### File Organization

- All domain types in `retro-core/src/models.rs`
- All DB operations in `retro-core/src/db.rs`
- Shared helpers:
  - `git_root_or_cwd()` lives in `retro-cli/src/commands/mod.rs` — use `super::git_root_or_cwd`
  - `strip_code_fences()` lives in `retro-core/src/util.rs` — use `crate::util::strip_code_fences`
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

- String truncation must use `char_boundary()` helper — never slice at arbitrary byte offsets (UTF-8 panic risk)
- Path decoding uses `recover_project_path()` which reads `cwd` from session files — naive decode breaks on paths with hyphens

### Process Management

- Process-alive checks use `libc::kill(pid, 0)` — portable across Linux and macOS (not `/proc/` which is Linux-only)
- AI prompts must be piped via stdin (`-p -`), never as CLI arguments (ARG_MAX risk with 150K prompts)
- Git/gh shell-outs use `Command::new().args()` (not shell strings) — each arg passed directly to `execve()`, safe from injection

### Git Hooks

- Hook format: marker comment (`# retro hook - do not remove`) + command on next line; removal is line-pair based
- `install_hook_lines` returns `HookInstallResult` (Installed/Updated/UpToDate)
- `retro init` updates existing hooks to new redirect format (remove+re-add)

### Database

- Batch DB queries into HashSets when filtering (avoid N+1 queries in loops)
- Re-export internal types from retro-core when CLI crate needs them (e.g., `pub use rusqlite::Connection` in `db.rs`) — avoid adding transitive deps to retro-cli

### User Interaction

- Confirmation prompts use `stdin` y/N pattern (not dialoguer) — keep it simple

### Testing

- Test strategy: unit tests with fixtures (no AI), integration tests with `MockBackend`
- Scenario tests in `scenarios/` directory — see `scenarios/README.md` for usage
- `--dry-run` on all AI commands must skip AI calls entirely — snapshot context, show summary, return early (not just suppress writes)
- `analyze --dry-run` shows skipped count in summary and per-session in `--verbose` mode

### Performance

- When progressively fitting content into a prompt budget, drop items from the end — never truncate mid-JSON
- Project-scoped commands resolve project path via `git rev-parse --show-toplevel`, falling back to cwd

## Implementation Status

All core features complete and tested. Current focus: quality improvements and user experience polish.

- **Phase 1: DONE** — Skeleton + Ingestion. `retro init`, `retro ingest`, `retro status` working.
- **Phase 2: DONE** — Analysis Backend + Pattern Discovery. `retro analyze`, `retro patterns` working. ClaudeCliBackend, prompt builder, pattern merging.
- **Phase 3: DONE** — Projection + Apply. Two-phase skill gen (draft+validate), CLAUDE.md managed section, global agent generation, projection CRUD.
- **Phase 4: DONE** — Full Apply + Clean + Audit + Git. Git branch/PR management, `retro clean`, `retro audit`, `retro log`, hook removal, uninstall.
- **Phase 5: DONE** — Hooks + Polish. `--auto` mode, `--verbose` flag, progress indicators, lockfile, `analyze --dry-run`.
- **Phase 6: DONE** — Auto-Apply Pipeline. Single hook orchestration, per-stage cooldowns, `auto_apply` config, old hook cleanup.
- **Phase 7: DONE** — Review Queue. `retro apply` → PendingReview, `retro review` command, `retro sync` PR state detection, nudge shows pending count.
- **Pattern discovery quality: DONE** — `--json-schema` structured output, analysis reasoning field, session filtering, explicit directives, confidence-based projection gate, rolling window analysis.
- **Full CLAUDE.md management: DONE** — `[claude_md] full_management` config. Granular edits (add/remove/reword/move) through apply pipeline with `full_management_analysis_schema()`. Agentic rewrite via `retro curate` with `execute_agentic()`. `dissolve_if_needed()` for delimiter removal. Edit type icons in review. Two scenario tests (curate-dry-run, curate-real-ai-call).

Test coverage: 160 unit tests, 12 scenario tests.

## Testing

Always run tests before committing:
```bash
cargo test
```

<!-- retro:managed:start -->
## Retro-Discovered Patterns

- Before completing any implementation work, run all scenario tests to verify nothing broke. Use the run-scenarios skill.
- When debugging issues, always investigate and identify the root cause before proposing fixes. Do not implement symptom-based patches or workarounds without understanding why the problem occurs.
- After completing implementation work, always check if documentation (CLAUDE.md, README.md) needs updates to reflect the changes
- When AI operations return unexpected or counterintuitive results (e.g., 0 patterns found, empty responses), include a `reasoning` field in the response schema and display it to the user. This helps debug AI behavior and understand why certain decisions were made.
- For major changes, provide commands for clean install testing: retro init --uninstall --purge && cargo build --release && ./target/release/retro init
- Before publishing a new release, bump version numbers in all Cargo.toml files (workspace root and crate manifests)

<!-- retro:managed:end -->
