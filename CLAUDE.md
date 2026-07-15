# Retro — Personal Context Curator for Claude Code

Rust CLI tool that watches your Claude Code sessions via hooks, learns the rules you keep repeating, and keeps CLAUDE.md / CLAUDE.local.md current — automatically. Session over session, your agent gets better without you maintaining its context by hand.

## Architecture (v3, the only pipeline as of 3.0.0)

Hook-driven, no daemon:

```
Hooks       (SessionEnd → retro observe; SessionStart → retro brief)
Queue       (~/.retro/queue/ — one JSON entry per pending session)
Analysis    (budget-gated `claude -p`, one call per project group)
Store       (markdown nodes under ~/.retro/knowledge/, git-backed)
Projection  (one-way: global CLAUDE.md managed block + per-project CLAUDE.local.md)
Surfaces    (retro ui web dashboard, retro status/doctor/lint, session briefing)
```

`retro observe` enqueues a finished session and spawns `retro run --background`. `retro brief` catch-up-scans for missed sessions (60s watermark safety margin, processed-session dedup), prints a briefing, and spawns a run if it enqueued anything. `retro run` drains the queue: analyze → write nodes → commit → project → reindex → push.

### Storage

`~/.retro/` is itself the knowledge git repo: `knowledge/global/*.md` and `knowledge/projects/<slug>/*.md` (source of truth), `config.toml`, plus machine-local gitignored state — `index.db` (disposable FTS5 index), `queue/`, `state/`, `health.json`, `run.lock`, `backups/`.

## Repo Structure

Cargo workspace with two crates:
- `crates/retro-core/` — library crate (store, analysis, projection, runner, migrate, doctor, lint, health, hooks/settings editing)
- `crates/retro-cli/` — binary crate (clap commands, `src/ui/` web dashboard: tiny_http server + embedded single page)
- `scenarios/` — scenario-based integration tests (see [scenarios/README.md](scenarios/README.md))

## Build & Test

```bash
# Build (requires Rust toolchain and C compiler for bundled SQLite)
cargo build

# Run unit tests
cargo test

# Always run tests before committing
cargo test && cargo run -- --help  # verify build

# Clean install testing
# ⚠️ WARNING: during development this must ONLY ever be run with RETRO_HOME
# isolation (RETRO_HOME pointing at a temp dir whose config.toml redirects
# [paths] claude_dir to another temp dir) — run bare, it deletes the real
# ~/.retro and rewrites the real ~/.claude/settings.json and CLAUDE.md.
retro uninstall --purge && cargo build --release && ./target/release/retro init
```

## Commands Overview

| Command | Purpose |
|---------|---------|
| `retro init [--from <remote>]` | Initialize the personal store (git-backed `~/.retro`, global hooks); `--from` clones an existing knowledge repo |
| `retro migrate [--dry-run]` | Migrate v2 knowledge and environment to v3 (idempotent, v2 db read-only and preserved) |
| `retro run [--verbose --dry-run --background]` | Run the pipeline: drain queue, analyze, project, commit, push |
| `retro observe` | SessionEnd hook entry: enqueue session, spawn background worker |
| `retro brief` | SessionStart hook entry: catch-up scan + session briefing |
| `retro reindex` | Rebuild the store index from knowledge files (safe anytime) |
| `retro status` | Store stats, queue, budget, health |
| `retro doctor` | End-to-end health verification (read-only structural checks) |
| `retro lint [--dry-run]` | Near-duplicate + stale-candidate pass (no AI calls) |
| `retro ui [--no-open]` | Local web dashboard (X-ray, knowledge, health, history) |
| `retro uninstall [--purge]` | Remove hooks, projections, v1/v2 remnants; `--purge` also deletes the store |

## Key Design Decisions

### Core Architecture

- **Rust, sync only** — no tokio, no async. `std::process::Command` for spawning `claude` CLI and `git`/`gh`.
- **No git2 crate** — shell out to `git` and `gh` directly for simplicity and reliability.
- **SQLite bundled** — `rusqlite` with `bundled` feature. Used only for the disposable store index (`index.db`, FTS5) and read-only access to the v2 `retro.db` during migrate.
- **Error handling** — `thiserror` in retro-core, `anyhow` in retro-cli. `CoreError` implements `std::error::Error` — use `?` directly in CLI commands.

### AI Backend

- **Sync trait** — `AnalysisBackend` trait with `json_schema: Option<&str>` parameter.
- **Primary impl** — `ClaudeCliBackend` uses `claude -p - --output-format json` (prompt piped via stdin to avoid ARG_MAX issues).
- **Structured output** — analysis passes `--json-schema` for constrained decoding (guaranteed valid JSON, no sanitization needed). Schema constant: `GRAPH_ANALYSIS_RESPONSE_SCHEMA` (analysis/mod.rs).
- **CLI quirks**:
  - `--json-schema` conflicts with `--tools ""` on large prompts — only pass `--tools ""` when NOT using `--json-schema`.
  - `--json-schema` uses an internal tool call for constrained decoding, so model needs extra turns — `--max-turns 5` gives headroom (observed max 4 turns).
  - Without `--tools ""`, model sometimes makes tool calls consuming turns — `--max-turns 5` prevents turn exhaustion.
  - Non-schema calls use `--tools "" --max-turns 1` (safe, no tool calls possible).
  - Output appears in `structured_output` field (parsed JSON), NOT `result` (empty string). `ClaudeCliOutput` checks `structured_output` first, serializes to string, falls back to `result`.
  - Token counts nest inside `usage` object — never assume top-level fields exist (use nested struct with `#[serde(default)]`). Sum `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` for total input.

### Knowledge Store

- **Files as truth** — one markdown file per node under `~/.retro/knowledge/`, strict frontmatter (`id, scope, type, confidence, sources, created, updated, invalidated_by`) between `---` delimiters, then the body. Unknown frontmatter keys are a parse error (catches typos); parsing normalizes on rewrite (CRLF→LF, confidence written back at two decimals).
- **Node types** — `rule`, `preference`, `pattern`, `memory` (v2's six types collapse: `directive`→`rule`, `skill`→`pattern`, handled at migration). Memory nodes are context-only — stored and browsable, never projected.
- **Scopes** — `global` (`knowledge/global/`) vs `project/<slug>` (`knowledge/projects/<slug>/`). Slugs and node ids must pass `is_valid_slug` (lowercase ASCII alphanumerics + dashes, starting alphanumeric) — validated on every LLM-supplied id before path construction.
- **Invalidation, not deletion** — nodes get `invalidated_by` set; git history preserves everything.
- **Git layer** — every mutation is a commit in `~/.retro` (`store::git`); the commit log is the audit trail. Best-effort push to an optional private remote; unpushed between-run commits are pushed on the next run.
- **Disposable index** — `index.db` (SQLite + FTS5) is rebuilt from files by `retro reindex` / `index::build`; files always win. User search input is sanitized so raw FTS5 operators can't error.
- **Machine-local state** — `queue/`, `state/`, `health.json`, `run.lock`, `backups/`, `index.db` are gitignored via `IGNORED_ENTRIES` (store/mod.rs), the single source of truth for both the store `.gitignore` and `.git/info/exclude`.
- **Confidence model** — analysis assigns 0.4–0.85 (explicit directives high, single observations low); `knowledge.confidence_threshold` (default 0.7) gates projection.

### Pipeline (runner_v3)

- **No daemon** — hooks spawn `retro run --background`; `run.lock` (`lock::LockFile`) makes concurrent runs a silent no-op.
- **Budget gate** — `runner.max_ai_calls_per_day` (default 10), tracked in `state/`, reset daily. Failed AI calls still consume budget (a persistently failing group must not become unbounded spend).
- **One AI call per project group** — queued sessions are grouped by project; each group is one `claude -p` call.
- **Session filtering** — sessions with < 2 user messages are low-signal (retro's own `claude -p` calls) and dropped; subagent transcripts are never enqueued; excluded projects and the store dir itself are skipped; secrets scrubbed when `privacy.scrub_secrets` (default true).
- **Visible failure accounting** — stale/unparseable queue entries are pruned with health records; LLM ops rejected by slug/shape validation are counted (`ops_skipped`) and surfaced as briefing notifications (≤3 per group); store parse warnings surface via health.
- **Project registration** — automatic on first session (remote-url identity, canonical paths, `store::projects::PathMap`), with a notify-on-register briefing notification; exclusion via `privacy.exclude_projects` removes the project's knowledge and CLAUDE.local.md on the next run.
- **Notification cap** — `RunnerState` keeps only the newest 50 notifications (they only drain when a session starts).

### Projection

- **One-way** — managed blocks are build output, regenerated from the store every run; edits belong in the store. Global rules → `~/.claude/CLAUDE.md` managed block; project rules → `<project>/CLAUDE.local.md`.
- **CLAUDE.md protection** — only write within `<!-- retro:managed:start/end -->` delimiters, never touch user content. Files backed up to `~/.retro/backups/` before modification.
- **Single-line bullets** — projected rules are one bullet each.
- **CLAUDE.local.md is machine-local** — ignored via the project's common git dir `info/exclude`, never committed.

### Lifecycle (migrate / uninstall)

- **Migrate is self-contained** — raw read-only rusqlite queries against the v2 `retro.db` (never via a db layer, never mutated); its own v1-hook and launchd removal helpers. Idempotent: knowledge import dedups by normalized similarity (> 0.8) per scope; safety-import rescues managed-block rules not yet in the store (the guard against first-projection wiping pre-v3 rules — also runs in `retro init`).
- **Environment cleanup** — migrate/uninstall sweep v1 git hooks (marker line pair), remove the v2 launchd runner (macOS, tolerate absence), and untrack machine-local files an older binary committed into the store repo.
- **Uninstall strips, never deletes wholesale** — managed blocks are stripped from CLAUDE.md / CLAUDE.local.md (user content kept; a CLAUDE.local.md empty after stripping is removed); hooks removed from settings.json with backup; store kept unless `--purge` (typed-yes confirmation, backups rescued outside the store first). Atomic writes (tmp + rename) for settings.json and CLAUDE.md files.

### Dashboard (retro ui)

- **tiny_http, sync, localhost-only** — binds `127.0.0.1:{ui.port}` (default 7777), single embedded HTML page.
- **Four tabs** — X-ray (what retro believes about a project), Knowledge (browse/search/invalidate/edit), Health (stage records + doctor), History (store git log).
- **Write actions** — go through the store: file edit → commit → reindex → reproject, guarded by `run.lock` and boundary slug validation on all client-supplied ids.

### Runtime Model

- **`RETRO_HOME` env var** — overrides the default `~/.retro/` data directory. Used for test/scenario isolation to prevent touching production data. `[paths] claude_dir` in config.toml likewise redirects everything under `~/.claude` (settings.json, CLAUDE.md, session transcripts).
- **Hook entries never fail** — `retro observe`/`retro brief` swallow errors into `health.json` and always exit 0; stdout stays clean (brief's stdout IS the briefing).

### Observability

- **Health records** — per-stage results in `~/.retro/health.json` (machine-local); warnings feed the briefing, the terminal nudge, `retro status`, and the dashboard.
- **Terminal nudge** — `check_and_display_nudge()` runs before interactive commands (not hook entries or background runs).
- **Token tracking** — `BackendResponse` carries `input_tokens`/`output_tokens` (not dollar cost).

### Data Models

- **Session/JSONL types** in `retro-core/src/models.rs` (`Session`, `SessionEntry`, `ClaudeCliOutput`, …) plus a v2-shim `NodeType`/graph-response layer reused by the analysis prompt machinery (`analysis/mod.rs` ↔ `analysis/v3.rs` maps it onto store types).
- **Store types** in `retro-core/src/store/` — `Node`, `NodeType`, `Scope` (see Knowledge Store above).
- **ToolResultContent enum** — `Text(String)` | `Blocks(Vec<Value>)` (tool results can be string or array).

## Dependencies

| Crate | Purpose |
|-------|---------|
| clap (derive) | CLI parsing |
| rusqlite (bundled) | Store index (FTS5), read-only v2 db access in migrate |
| serde + serde_json | JSON/JSONL parsing, hook events, settings.json |
| anyhow + thiserror | Error handling |
| chrono | Timestamps, node dates, budget day-keys |
| glob | Finding session files |
| colored | Terminal output |
| regex | Sensitive data scrubbing |
| libc | Process-alive check (kill signal 0), uid for launchd cleanup |
| toml | Config file parsing |
| tiny_http | Dashboard server (retro-cli only) |
| tempfile | Test-only: temporary directories |

## Coding Conventions

### File Organization

- Session/JSONL domain types in `retro-core/src/models.rs`; store types in `retro-core/src/store/`
- Pipeline in `retro-core/src/runner_v3.rs`; migration in `retro-core/src/migrate.rs`
- Dashboard in `retro-cli/src/ui/` (mod.rs server, api.rs routes, assets/index.html embedded via `include_str!`)
- Shared helpers:
  - `truncate_str()` lives in `retro-core/src/util.rs` — safe UTF-8 truncation
  - `normalized_similarity()`/`levenshtein()` live in `retro-core/src/util.rs` — near-duplicate detection (> 0.8)
  - `run_claude_child()` shared helper in `analysis/claude_cli.rs` — stdin/stdout/stderr piping + timeout for both `execute()` and `execute_agentic()`
  - `check_and_display_nudge()` lives in `retro-cli/src/commands/mod.rs`
- CLI commands that share logic should expose a shared entry point rather than duplicating code

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
- Background workers are spawned detached (`std::process::Stdio::null()` all around) so hook entries return immediately

### Database

- Batch DB queries into HashSets when filtering (avoid N+1 queries in loops)
- Re-export internal types from retro-core when CLI crate needs them — avoid adding transitive deps to retro-cli
- The index is disposable — any code may assume `retro reindex` recreates it from files

### User Interaction

- Confirmation prompts use `stdin` y/N pattern (not dialoguer) — keep it simple; destructive confirmation (`uninstall --purge`) requires a typed `yes`
- Atomic writes (tmp sibling + rename) for settings.json and CLAUDE.md-family files — a crash mid-write must not truncate them

### Testing

- Test strategy: unit tests with fixtures (no AI), integration tests with `MockBackend`, everything on `TempDir`
- Scenario tests in `scenarios/` directory — see `scenarios/README.md` for usage
- `--dry-run` on AI commands must skip AI calls entirely — and must not mutate anything (no layout creation, no `git init`, no queue pruning)

### Performance

- When progressively fitting content into a prompt budget, drop items from the end — never truncate mid-JSON
- Project-scoped commands resolve project path via `git rev-parse --show-toplevel`, falling back to cwd

## Implementation Status

### v3 "Personal" (retro 3.0.0)

Spec: `docs/superpowers/specs/2026-07-06-retro-v3-personal-redesign-design.md`.

- **Plan 1: DONE** — Store foundation. File-based knowledge store (`retro-core/src/store/`): markdown nodes with strict frontmatter under `~/.retro/knowledge/`, git layer, disposable SQLite index with FTS5, `retro reindex`.
- **Plan 2: DONE** — Pipeline. Hook-based capture (`retro observe`/`retro brief`), automatic project registration, hardened markdown-store analysis sink (`analysis/v3.rs`), one-way projection, budget-gated `runner_v3`, `retro init [--from <remote>]`.
- **Plan 3: DONE** — Surfaces. `retro ui` local dashboard, `retro doctor`, v3 `retro status`, `retro lint`, queue-age nudge, store self-exclusion guard, subagent-transcript skip.
- **Plan 4: DONE** — Lifecycle. `retro migrate` (idempotent v2 import, safety-import, environment cleanup), `retro uninstall [--purge]`, notification cap, full v1/v2 code deletion (two crates remain), v3 scenario suite, 3.0.0.

Test coverage: 189 tests across the workspace.

## Testing

Always run tests before committing:
```bash
cargo test
```

- Before completing any implementation work, run all scenario tests to verify nothing broke. Use the run-scenarios skill.
- Scenario tests (`scenarios/v3-init-and-lifecycle.md`, `scenarios/v3-pipeline-dry-run.md`, `scenarios/v3-migrate.md`) exercise real lifecycle commands. **HARD RULE:** every scenario — and any manual live check — must run under the isolation preamble in `scenarios/README.md`: `RETRO_HOME` at a temp dir, config redirecting `[paths] claude_dir` to another temp dir, overridden `HOME`, a stubbed `launchctl` on `PATH`, and `./target/release/retro` (never a PATH binary). Never run `retro init`, `retro migrate`, or `retro uninstall` against the real environment.
- When debugging issues, always investigate and identify the root cause before proposing fixes. Do not implement symptom-based patches or workarounds without understanding why the problem occurs.
- After completing implementation work, always check if documentation (CLAUDE.md, README.md) needs updates to reflect the changes.
- When AI operations return unexpected or counterintuitive results (e.g., 0 patterns found, empty responses), include a `reasoning` field in the response schema and display it to the user.
- For major changes, provide commands for clean install testing: `retro uninstall --purge && cargo build --release && ./target/release/retro init` — **only ever under RETRO_HOME isolation during development** (see Build & Test).
- To release: bump version numbers in both Cargo.toml files, merge PR, then `git tag vX.Y.Z && git push origin vX.Y.Z`. The `.github/workflows/publish.yml` workflow handles testing, crates.io publishing, and GitHub release creation automatically.
