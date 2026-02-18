# Retro — Active Context Curator for AI Coding Agents

Rust CLI tool that analyzes Claude Code session history to discover repetitive instructions, recurring mistakes, and workflow patterns — then projects them into skills and CLAUDE.md rules.

## Architecture

Three-stage pipeline: **Ingestion** (pure Rust, no AI) → **Analysis** (AI-powered, pluggable backend) → **Projection** (two-track: personal auto-apply, shared via PR).

Storage lives in `~/.retro/` (SQLite WAL mode + JSONL audit log + config.toml).

## Repo Structure

Cargo workspace with two crates:
- `crates/retro-core/` — library crate (all logic)
- `crates/retro-cli/` — binary crate (clap commands)
- `tests/` — fixtures and integration tests

## Key Design Decisions

- **Rust, sync only** — no tokio, no async. `std::process::Command` for spawning `claude` CLI and `git`/`gh`.
- **No git2 crate** — shell out to `git` and `gh` directly.
- **SQLite bundled** — `rusqlite` with `bundled` feature. WAL mode always.
- **Error handling** — `thiserror` in retro-core, `anyhow` in retro-cli.
- **AI backend** — sync `AnalysisBackend` trait. Primary impl: `claude -p - --output-format json` (prompt piped via stdin).
- **CLAUDE.md protection** — only write within `<!-- retro:managed:start/end -->` delimiters, never touch user content.
- **MEMORY.md** — read-only input, never write. Claude Code owns it.
- **Skill generation** — one skill per AI call (quality over cost), two-phase: generate then validate.
- **Pattern merging** — AI-assisted (primary) with strong semantic dedup prompt guidance + Levenshtein similarity > 0.8 safety net.
- **Token tracking** — `BackendResponse` carries `input_tokens`/`output_tokens` (not dollar cost). `ClaudeCliOutput` extracts from nested `usage` object, summing `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` for total input.
- **Auto-apply pipeline** — single post-commit hook orchestrates ingest → analyze → apply. Per-stage cooldowns (5m/24h/24h). Data triggers prevent unnecessary runs. Session cap (`auto_analyze_max_sessions`, default 15) skips auto-analyze when backlog is too large. Hook stderr redirected to `~/.retro/hook-stderr.log`.
- **Observability** — every auto-mode decision gets a structured audit entry (ingest/analyze/apply success/skip/error). Enhanced nudge reads audit entries since `last_nudge_at` (stored in `metadata` table), groups within 60s as one run, displays colored multi-line status block on next interactive command.

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

## Build

Standard Rust build — no special flags needed:
```
cargo build
```

Requires: Rust toolchain (`rustup`) and a C compiler (`build-essential` on Ubuntu) for bundled SQLite.

## Conventions

- All domain types in `retro-core/src/models.rs`
- All DB operations in `retro-core/src/db.rs` — schema versioned via `PRAGMA user_version`
- Use `#[serde(default)]` on all optional fields for forward-compatibility with JSONL format changes
- Skip unparseable JSONL lines gracefully (log warning for known types, silent skip for unknown types)
- Pre-parse `type` field from JSONL before full deserialization to distinguish unknown entry types from parse errors in known types
- String truncation must use `char_boundary()` helper — never slice at arbitrary byte offsets (UTF-8 panic risk)
- `ToolResultContent` is an enum (`Text(String)` | `Blocks(Vec<Value>)`) — tool results can be string or array
- Path decoding uses `recover_project_path()` which reads `cwd` from session files — naive decode breaks on paths with hyphens
- Project-scoped commands resolve project path via `git rev-parse --show-toplevel`, falling back to cwd
- `CoreError` implements `std::error::Error` via thiserror — use `?` directly in CLI commands (no `.map_err(|e| anyhow!("{e}"))`)
- Process-alive checks use `libc::kill(pid, 0)` — portable across Linux and macOS (not `/proc/` which is Linux-only)
- Backup files to `~/.retro/backups/` before any modification
- Audit log: append-only JSONL at `~/.retro/audit.jsonl`
- AI prompts must be piped via stdin (`-p -`), never as CLI arguments (ARG_MAX risk with 150K prompts)
- When progressively fitting content into a prompt budget, drop items from the end — never truncate mid-JSON
- Shared helper `git_root_or_cwd()` lives in `retro-cli/src/commands/mod.rs` — use `super::git_root_or_cwd`
- Shared `strip_code_fences()` lives in `retro-core/src/util.rs` — use `crate::util::strip_code_fences`
- Confirmation prompts use `stdin` y/N pattern (not dialoguer) — keep it simple
- CLI commands that share logic should expose a shared entry point (e.g., `run_apply()` with `DisplayMode` enum) rather than duplicating code
- Batch DB queries into HashSets when filtering (avoid N+1 queries in loops)
- Re-export internal types from retro-core when CLI crate needs them (e.g., `pub use rusqlite::Connection` in `db.rs`) — avoid adding transitive deps to retro-cli
- Git/gh shell-outs use `Command::new().args()` (not shell strings) — each arg passed directly to `execve()`, safe from injection
- Git hook format: marker comment (`# retro hook - do not remove`) + command on next line; removal is line-pair based; `install_hook_lines` returns `HookInstallResult` (Installed/Updated/UpToDate)
- Hook stderr: `2>>~/.retro/hook-stderr.log` (not `/dev/null`) — captures parse warnings, panics. `retro init` truncates it. `retro init` also updates existing hooks to new redirect format (remove+re-add)
- Two-phase apply execution: personal actions on current branch, shared actions on a new `retro/updates-{YYYYMMDD-HHMMSS}` branch
- Claude CLI JSON output nests token counts inside `usage` object — never assume top-level fields exist (use nested struct with `#[serde(default)]`)
- `--dry-run` on all AI commands must skip AI calls entirely — snapshot context, show summary, return early (not just suppress writes)
- Test strategy: unit tests with fixtures (no AI), integration tests with `MockBackend`
- Auto-mode orchestration: `ingest --auto` chains analyze and apply when `auto_apply=true` and data triggers + cooldowns are satisfied
- Terminal nudge: `check_and_display_nudge()` runs before interactive commands, reads audit entries since `last_nudge_at` from metadata table, aggregates into `AutoRunSummary` structs (60s window grouping), displays colored status block, updates `last_nudge_at`
- Session cap: `auto_analyze_max_sessions` (default 15) — `unanalyzed_session_count()` checked before auto-analyze; if exceeded, writes `analyze_skipped` audit entry with `reason: "session_cap"` and skips AI call
- Audit coverage: every auto-mode decision gets an entry — `ingest` (success), `analyze_skipped` (session_cap or cooldown_or_no_data), `analyze_error`, `apply_skipped` (no_qualifying_patterns), `apply_error`, enriched `apply` (with `pr_url`)
- Per-stage cooldowns: `ingest_cooldown_minutes` (5), `analyze_cooldown_minutes` (1440), `apply_cooldown_minutes` (1440) — each stage has its own cooldown matching cost profile
- PR creation flow: detect default branch via `gh repo view` → `git fetch origin <default>` → `git checkout -b retro/... origin/<default>` → write/commit → `git push -u origin HEAD` → `gh pr create --base <default>`
- Always push before `gh pr create` — the remote branch must exist
- `stash_push()`/`stash_pop()` around branch switches in apply — `git checkout -b` fails if tracked files differ between branches when working tree is dirty

## Implementation Status

- **Phase 1: DONE** — Skeleton + Ingestion. `retro init`, `retro ingest`, `retro status` working. 18 sessions ingested from real data.
- **Phase 2: DONE** — Analysis Backend + Pattern Discovery. `retro analyze`, `retro patterns` working. ClaudeCliBackend (stdin), prompt builder, pattern merging with Levenshtein dedup, audit log. 19 unit tests.
- **Phase 3: DONE** — Projection + Apply. `projection/{mod,skill,claude_md,global_agent}.rs`, `util.rs`, `retro apply [--dry-run] [--global]`, `retro diff [--global]`. Two-phase skill gen (draft+validate), CLAUDE.md managed section, global agent generation, projection CRUD, file backups, two-track classification (personal/shared), y/N confirmation before writes. 47 unit tests.
- **Phase 4: DONE** — Full Apply + Clean + Audit + Git. `git.rs` (branch/PR/hook management), `curator.rs` (staleness detection, archiving), `retro clean [--dry-run]`, `retro audit [--dry-run]`, `retro log [--since]`, `retro hooks remove`, `retro init --uninstall [--purge]`. Apply now creates git branch + PR for shared track via `gh`. Two-phase apply (personal on current branch, shared on new branch). 63 unit tests.
- **Phase 5: DONE** — Hooks + Polish. `--auto` mode on `ingest` and `analyze` (lockfile skip, cooldown check, silent operation), `--verbose` global flag, progress indicators for AI calls, `LockFile::try_acquire()`, post-commit hook updated to `retro ingest --auto`. `analyze --dry-run` for previewing AI calls. 63 unit tests.
- **Post-v0.1 fixes**: Strengthened analysis prompt dedup (semantic merge guidance with examples). Replaced `cost_usd` with `input_tokens`/`output_tokens` across pipeline (extracts from nested `usage` in CLI JSON). `audit --dry-run` now skips AI calls entirely (shows context summary instead).
- **Phase 6: DONE** — Auto-Apply Pipeline. Single post-commit hook orchestrates full pipeline (`ingest --auto` chains analyze + apply). Per-stage cooldowns (`ingest_cooldown_minutes=5`, `analyze_cooldown_minutes=1440`, `apply_cooldown_minutes=1440`). `auto_apply` config (on by default). `apply --auto` with lockfile, cooldown, and data gate. Old post-merge hook cleanup on `retro init`. Orchestration lock prevents concurrent analyze. 87 unit tests.
- **Auto-mode observability: DONE** — `auto_analyze_max_sessions` config (default 15) skips auto-analyze when backlog exceeds cap. DB schema v2 adds `metadata` table for `last_nudge_at`. `unanalyzed_session_count()` for cap check. Comprehensive audit logging for every auto-mode decision (ingest success, analyze skip/error, apply skip/error, enriched apply with `pr_url`). Enhanced nudge system reads audit entries since last nudge, groups by 60s window, displays multi-line colored status block. Hook stderr redirected to `~/.retro/hook-stderr.log`. `retro init` updates existing hooks to new format (`HookInstallResult` enum: Installed/Updated/UpToDate). Old `get_unnudged_pr_urls`/`mark_projections_nudged` removed (replaced by audit-based nudge). 93 unit tests, 9 scenario tests.

## Full Plan

See `PLAN.md` for the complete implementation plan with database schema, CLI commands, session JSONL format details, prompt strategy, and phased implementation steps.
