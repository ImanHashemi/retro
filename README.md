# retro

**Your personal context curator for Claude Code.**

Retro watches your Claude Code sessions through hooks, learns the rules and preferences you keep repeating, and keeps `CLAUDE.md` (global) and `CLAUDE.local.md` (per project) current — automatically, with no dashboard to babysit and no daemon to keep alive.

Your knowledge is plain markdown files under `~/.retro/knowledge/`, one file per rule, version-controlled with git. Files are the source of truth; everything else (the search index, the queue, run state) is disposable and rebuilt on demand.

## Quick Start

```sh
cargo install retro-cli

retro init
```

`retro init` creates `~/.retro` as a git repo, installs `SessionEnd`/`SessionStart` hooks into `~/.claude/settings.json`, and offers to add a private GitHub remote for backup. That's it — work normally in Claude Code. Sessions get captured as they end, analyzed in the background, and folded into your rules.

Open the dashboard any time to look around:

```sh
retro ui
```

Already running retro 2.x? See [Migrating from 2.x](#migrating-from-2x) below — run `retro migrate` instead of `retro init`.

## How It Works

```
SessionEnd hook  →  retro observe  →  queue/
                                         │
SessionStart hook → retro brief  ───────┤  (catch-up scan for missed sessions)
                                         ▼
                              retro run (spawned in the background)
                                         │
                          budget-gated `claude -p` analysis
                                         │
                                         ▼
                         knowledge store (~/.retro/knowledge/**.md)
                                         │
                                one-way projection
                                         ▼
                    ~/.claude/CLAUDE.md + <project>/CLAUDE.local.md
```

- **Capture** — a `SessionEnd` hook runs `retro observe`, which enqueues the finished session and spawns `retro run --background`. A `SessionStart` hook runs `retro brief`, which catches up on any sessions missed since the last watermark (crashes, other machines) and prints a short briefing of what changed.
- **Analysis** — each pending project's sessions get one `claude -p` call, gated by a daily AI-call budget (`[runner] max_ai_calls_per_day`, default 10). A failed call still consumes budget, so a persistently broken call can't spin forever.
- **Store** — knowledge lives as one markdown file per node, frontmatter + body:

  ```markdown
  ---
  id: use-uv-for-python-scripts
  scope: global
  type: rule
  confidence: 0.80
  sources: [session:a1b2c3d4]
  created: 2026-07-01
  updated: 2026-07-01
  invalidated_by: null
  ---
  Always use uv instead of pip for Python package management.
  ```

  Layout: `~/.retro/knowledge/global/*.md` for cross-project rules, `~/.retro/knowledge/projects/<slug>/*.md` for project-scoped ones. Node types are `rule`, `preference`, `pattern`, and `memory` (memory nodes are stored and browsable but never projected). Every mutation is a git commit in `~/.retro` — the commit log is your audit trail; nothing is silently overwritten.
- **Projection** — one-way, regenerated from the store every run: global rules (confidence above `knowledge.confidence_threshold`) go into a managed block in `~/.claude/CLAUDE.md`, project rules into `<project>/CLAUDE.local.md`. Retro only ever touches content between `<!-- retro:managed:start -->` and `<!-- retro:managed:end -->` — everything else in your CLAUDE.md is yours. `CLAUDE.local.md` is added to the project's `.git/info/exclude`, so it stays machine-local and out of the repo's history.

## Dashboard

`retro ui` starts a localhost-only web server (default `http://127.0.0.1:7777`) with four tabs:

- **X-ray** — what retro currently believes about the project you're in.
- **Knowledge** — browse and search every stored node, invalidate ones that no longer apply.
- **Health** — recent stage-by-stage run results (analyze, project, push, ...).
- **History** — the store's git commit log.

## Commands

| Command | Purpose |
|---------|---------|
| `retro init [--from <remote>]` | Initialize the store, install hooks. `--from` clones an existing knowledge repo instead of starting fresh |
| `retro migrate [--dry-run]` | One-time bridge from a retro 2.x install: import v2 knowledge, clean up v1/v2 remnants |
| `retro run [--verbose --dry-run --background]` | Run the pipeline once: drain the queue, analyze, project, commit, push |
| `retro observe` | SessionEnd hook entry — enqueues a finished session |
| `retro brief` | SessionStart hook entry — catch-up scan + briefing |
| `retro reindex` | Rebuild the search index from the knowledge files (safe anytime) |
| `retro status` | Store stats, queue depth, budget remaining, health |
| `retro doctor` | End-to-end, read-only health verification |
| `retro lint [--dry-run]` | Free near-duplicate and stale-candidate scan (no AI calls) |
| `retro ui [--no-open]` | Open the local dashboard |
| `retro uninstall [--purge]` | Remove hooks and projected content; `--purge` also deletes the store |

## Configuration

Config lives at `~/.retro/config.toml`; any key not set falls back to its default.

```toml
[analysis]
window_days = 14                # analysis window, in days
confidence_threshold = 0.7      # analysis-side default (the projection gate is [knowledge])
staleness_days = 28             # node age before `retro lint` flags it as a stale candidate

[ai]
backend = "claude-cli"          # the only backend today
model = "sonnet"                # sonnet, opus, or haiku

[paths]
claude_dir = "~/.claude"        # where CLAUDE.md, settings.json, and session transcripts live

[privacy]
scrub_secrets = true            # redact likely secrets before they reach the AI call
exclude_projects = []           # paths to never watch (or stop watching)

[runner]
max_ai_calls_per_day = 10       # hard cap; a failed call still counts against it

[knowledge]
confidence_threshold = 0.7      # minimum confidence to project into CLAUDE.md
global_promotion_threshold = 0.85

[ui]
port = 7777                     # retro ui bind port (127.0.0.1 only)
```

## Migrating from 2.x

`retro migrate` is idempotent and safe to re-run: it reads your 2.x `retro.db` **read-only** (never modifies it), imports active/pending-review knowledge into the v3 store with type mapping (`directive`→`rule`, `skill`→`pattern`) and dedup against anything already there, rescues any rules already sitting in a managed `CLAUDE.md` block that aren't in the store yet, then cleans up the old environment: v1 git post-commit/post-merge hooks, the 2.x launchd runner (macOS), and any machine-local files an old binary had committed into the store repo. Run it again any time — everything it does dedups.

Rollback: `retro.db` is left in place at `~/.retro/retro.db` (safe to delete once you trust the v3 store — the 2.x binary can still read it if you need to go back), and every store write is a git commit, so you can revert. Reading it may leave empty `retro.db-wal`/`retro.db-shm` files behind — harmless standard SQLite artifacts.

## Requirements

- [Claude Code](https://claude.ai/download), for session transcripts and the `claude` CLI used for analysis
- [Rust toolchain](https://rustup.rs/) and a C compiler, for building from source
- macOS or Linux (no launchd/systemd dependency — retro is driven entirely by Claude Code hooks)
- `git`, for the knowledge store's version history

## Installation

```sh
cargo install retro-cli
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for setup instructions and [CLAUDE.md](CLAUDE.md) for architecture details.

## License

MIT
