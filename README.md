# retro

**Active context curator for AI coding agents.**

Coding agents become powerful when their context is right — when they know your conventions, remember past mistakes, and have the skills to work on your projects efficiently. But curating that context is manual work that most people never do well.

Retro is like having a teammate who runs a retrospective on your AI coding sessions. It analyzes your Claude Code conversation history, discovers patterns — repeated instructions, recurring mistakes, workflow conventions — and turns them into skills and CLAUDE.md rules automatically.

The result: your agent gets better over time, learning from every session, without you having to manually maintain its context. And you stay in control — shared changes come as PRs for you to review.

<!-- TODO: Add hero GIF showing retro analyze → retro apply pipeline -->

## Quick Start

```sh
# Install
cargo install retro

# Initialize (creates ~/.retro/, installs git hooks)
cd your-project
retro init

# Ingest your Claude Code session history (fast, no AI)
retro ingest

# Analyze sessions to discover patterns (AI-powered)
retro analyze

# See what retro wants to change
retro diff

# Apply — personal changes auto-apply, shared changes open a PR
retro apply
```

That's it. After `retro init`, git hooks handle ingestion and analysis automatically — `retro ingest` runs on every commit, `retro analyze` runs on every merge. Run `retro apply` whenever you want to act on discovered patterns.

## How It Works

Retro operates as a three-stage pipeline:

```
  ┌─────────────────────────────────────────────┐
  │  INGESTION (pure Rust, no AI)               │
  │  Reads Claude Code session history           │
  │  Parses into structured sessions in SQLite   │
  └────────────────┬────────────────────────────┘
                   │
  ┌────────────────▼────────────────────────────┐
  │  ANALYSIS (AI-powered)                       │
  │  Discovers: repeated instructions,           │
  │  recurring mistakes, workflow patterns       │
  │  Stores patterns with confidence scores      │
  └────────────────┬────────────────────────────┘
                   │
  ┌────────────────▼────────────────────────────┐
  │  PROJECTION (two-track)                      │
  │  Personal (auto-apply): global agents        │
  │  Shared (via PR): CLAUDE.md rules, skills    │
  └─────────────────────────────────────────────┘
```

- **Ingestion** is fast and runs on every commit via git hooks. No AI calls, just parsing.
- **Analysis** uses Claude to find patterns across your sessions within a rolling window (default: 14 days).
- **Projection** turns high-confidence patterns into concrete artifacts — skills, CLAUDE.md rules, and global agents.

## What Retro Generates

**CLAUDE.md rules** — Conventions discovered from your sessions, added to a managed section in your project's CLAUDE.md. Retro never touches your manually-written content.

**Skills** — Reusable workflow patterns extracted from your sessions, saved as `.claude/skills/` files.

**Global agents** — Personal agent definitions at `~/.claude/agents/` for patterns that apply across all your projects.

All shared changes (CLAUDE.md, skills) are proposed via PR on a `retro/updates-*` branch. Personal changes (global agents) apply directly.

## Commands

| Command | Description |
|---------|-------------|
| `retro init` | Set up retro: creates `~/.retro/`, database, config, and git hooks |
| `retro ingest` | Parse new sessions from Claude Code history (fast, no AI) |
| `retro analyze` | Discover patterns across sessions (AI-powered) |
| `retro patterns` | List discovered patterns, filterable by status |
| `retro apply` | Generate skills, CLAUDE.md rules, and agents from patterns |
| `retro diff` | Preview what `apply` would change (alias for `apply --dry-run`) |
| `retro clean` | Archive stale patterns and remove their projections |
| `retro audit` | AI-powered review of your context for redundancy and contradictions |
| `retro status` | Show session counts, last analysis, pattern summary |
| `retro log` | Show audit log entries |

Most commands are project-scoped by default. Use `--global` to operate across all projects. Use `--dry-run` on any AI-powered command to preview without making changes or API calls.

## Automatic Mode

After `retro init`, git hooks run retro in the background:

- **post-commit**: `retro ingest --auto` — parses new sessions silently
- **post-merge**: `retro analyze --auto` — discovers patterns silently

In `--auto` mode, retro skips if another instance is running, respects a cooldown period, and never produces output. It quietly keeps your pattern database up to date.

Run `retro apply` when you're ready to act on what it found.

## Configuration

Retro stores its config at `~/.retro/config.toml`. Key settings:

```toml
[analysis]
model = "sonnet"           # AI model for analysis (sonnet, opus, haiku)
rolling_window_days = 14   # How far back to analyze

[hooks]
auto_cooldown_minutes = 60 # Minimum time between auto-runs

[curator]
staleness_days = 30        # When to consider patterns stale
```

Run `retro init` to create the default config.

## Installation

### From source

Requires the [Rust toolchain](https://rustup.rs/) and a C compiler (`build-essential` on Ubuntu) for bundled SQLite.

```sh
cargo install retro
```

### Prebuilt binaries

Download from [Releases](https://github.com/user/retro/releases) for Linux and macOS.

### Requirements

- [Claude Code](https://claude.ai/download) — retro reads its session history and uses the `claude` CLI for AI-powered analysis
- Git (for hook integration and PR creation)
- `gh` CLI (optional, for automatic PR creation via `retro apply`)

## Status

Retro is v0.1 — the core pipeline works and has been tested on real Claude Code session history.

**What works well:**
- Session ingestion and pattern discovery across projects
- CLAUDE.md rule generation with managed sections (never touches your content)
- Skill and global agent generation
- Automatic git hook integration (ingest on commit, analyze on merge)
- Context auditing for redundancy and contradictions
- Dry-run mode on all AI-powered commands

**What's early:**
- Skill generation quality varies — two-phase generate+validate helps but isn't perfect
- Pattern merging occasionally creates near-duplicates
- Only supports Claude Code (designed to be extensible to other agents)

## Contributing

Contributions welcome. See `CLAUDE.md` for architecture details and conventions.

```sh
git clone https://github.com/user/retro
cd retro
cargo build
cargo test
```

## License

MIT
