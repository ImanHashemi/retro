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
- **Pattern merging** — AI-assisted (primary) + Levenshtein similarity > 0.8 safety net.

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
| dialoguer | Confirmation dialogs |
| regex | Sensitive data scrubbing |
| libc | Portable process-alive check (kill signal 0) |

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
- Test strategy: unit tests with fixtures (no AI), integration tests with `MockBackend`

## Implementation Status

- **Phase 1: DONE** — Skeleton + Ingestion. `retro init`, `retro ingest`, `retro status` working. 18 sessions ingested from real data.
- **Phase 2: DONE** — Analysis Backend + Pattern Discovery. `retro analyze`, `retro patterns` working. ClaudeCliBackend (stdin), prompt builder, pattern merging with Levenshtein dedup, audit log. 19 unit tests.
- **Phase 3: TODO** — Projection + Apply. `projection/{skill,claude_md,global_agent}.rs`, `retro apply [--dry-run]`, `retro diff`. Two-phase skill gen (draft+validate). `projections` table exists but needs CRUD.
- **Phase 4: TODO** — Full Apply + Clean + Audit + Git. `git.rs`, `curator.rs`, `retro clean`, `retro audit`, `retro log`, `retro hooks remove`.
- **Phase 5: TODO** — Hooks + Polish. Git hook installation, `--auto` mode, `--verbose`, colored output polish.

## Full Plan

See `PLAN.md` for the complete implementation plan with database schema, CLI commands, session JSONL format details, prompt strategy, and phased implementation steps.
