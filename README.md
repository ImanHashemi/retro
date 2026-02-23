# retro

**Active context curator for AI coding agents.**

Coding agents work best when their context is right: when they know your conventions, remember past mistakes, and have the skills to work efficiently on your projects. But curating that context is manual work that most people never do well.

Retro runs retrospectives on your AI coding sessions. It analyzes your Claude Code conversation history, discovers patterns (repeated instructions, recurring mistakes, workflow conventions, explicit directives), and turns them into skills and CLAUDE.md rules automatically.

Your agent gets better over time, learning from every session, without you having to maintain its context by hand. You stay in control: all changes go through a review queue where you approve, skip, or dismiss each suggestion. Shared changes are then proposed as PRs.

![retro demo](docs/demo.gif)

## Quick Start

```sh
# Install
cargo install retro-cli

# Initialize (creates ~/.retro/, installs git hooks)
cd your-project
retro init

# Ingest your Claude Code session history (fast, no AI)
retro ingest

# Analyze sessions to discover patterns (AI-powered)
retro analyze

# See what retro wants to change
retro diff

# Generate content and queue for review
retro apply

# Review pending items: approve, skip, or dismiss
retro review
```

After `retro init`, a single post-commit hook orchestrates the full pipeline in the background: ingest → analyze → apply (queue for review), each with its own cooldown. Run `retro review` to approve items — shared changes become PRs, personal changes apply directly.

## How It Works

Retro operates as a three-stage pipeline:

```
  ┌─────────────────────────────────────────────┐
  │  INGESTION (pure Rust, no AI)               │
  │  Reads Claude Code session history          │
  │  Parses into structured sessions in SQLite  │
  └────────────────┬────────────────────────────┘
                   │
  ┌────────────────▼────────────────────────────┐
  │  ANALYSIS (AI-powered)                      │
  │  Discovers: repeated instructions,          │
  │  recurring mistakes, workflow patterns,     │
  │  explicit directives ("always"/"never")     │
  │  Stores patterns with confidence scores     │
  └────────────────┬────────────────────────────┘
                   │
  ┌────────────────▼────────────────────────────┐
  │  PROJECTION (two-track, via review queue)    │
  │  Personal: global agents (apply after review)│
  │  Shared: CLAUDE.md rules, skills (PR after  │
  │  review)                                     │
  └─────────────────────────────────────────────┘
```

- **Ingestion** is fast and runs on every commit via git hooks. No AI calls, just parsing.
- **Analysis** uses Claude to find patterns across your sessions within a rolling window (default: 14 days). Explicit directives ("always use X", "never do Y") are detected as high-confidence patterns from a single session.
- **Projection** turns high-confidence patterns into concrete artifacts: skills, CLAUDE.md rules, and global agents. All generated content is queued for your review before anything is written to disk or proposed as a PR.

## What Retro Generates

**CLAUDE.md rules** are conventions discovered from your sessions, added to a managed section in your project's CLAUDE.md. Retro never touches your manually-written content.

**Skills** are reusable workflow patterns extracted from your sessions, saved as `.claude/skills/` files.

**Global agents** are personal agent definitions at `~/.claude/agents/` for patterns that apply across all your projects.

All changes go through `retro review` first. Approved shared changes (CLAUDE.md, skills) are proposed via PR on a `retro/updates-*` branch. Approved personal changes (global agents) apply directly.

## Commands

| Command | Description |
|---------|-------------|
| `retro init` | Set up retro: creates `~/.retro/`, database, config, and git hooks |
| `retro ingest` | Parse new sessions from Claude Code history (fast, no AI) |
| `retro analyze` | Discover patterns across sessions (AI-powered) |
| `retro patterns` | List discovered patterns, filterable by status |
| `retro apply` | Generate content from patterns and queue for review |
| `retro review` | Review pending items: approve, skip, or dismiss |
| `retro sync` | Check PR status and reset patterns from closed PRs |
| `retro diff` | Preview what `apply` would change (alias for `apply --dry-run`) |
| `retro clean` | Archive stale patterns and remove their projections |
| `retro audit` | AI-powered review of your context for redundancy and contradictions |
| `retro status` | Show session counts, last analysis, pattern summary |
| `retro log` | Show audit log entries |
| `retro hooks remove` | Remove retro git hooks from the current repository |
| `retro init --uninstall` | Remove retro hooks (preserves `~/.retro/` data) |
| `retro init --uninstall --purge` | Remove hooks and delete all retro data |

Most commands are project-scoped by default. Use `--global` to operate across all projects. Use `--dry-run` on any AI-powered command to preview without making changes or API calls.

## Automatic Mode

After `retro init`, a single **post-commit** hook runs `retro ingest --auto` in the background. When `auto_apply = true` (the default), this chains through the full pipeline:

1. **Ingest** — parse new sessions (cooldown: 5 minutes)
2. **Analyze** — discover patterns via AI (cooldown: 24 hours)
3. **Apply** — generate content and queue for review (cooldown: 24 hours)

Each stage has its own cooldown and data trigger — no unnecessary AI calls. In `--auto` mode, retro skips if another instance is running and never produces output.

Auto mode does the expensive work (ingestion, analysis, content generation) in the background, but nothing is written to disk until you run `retro review`. You'll see a terminal nudge showing pending review count the next time you run any retro command interactively.

## Configuration

Config lives at `~/.retro/config.toml`:

```toml
[analysis]
window_days = 14           # How far back to analyze
rolling_window = true      # Re-analyze all sessions in window (cross-session patterns)
staleness_days = 28        # When to consider patterns stale
confidence_threshold = 0.7 # Minimum confidence to act on patterns

[ai]
model = "sonnet"           # AI model (sonnet, opus, haiku)

[hooks]
ingest_cooldown_minutes = 5    # Minimum time between auto-ingests
analyze_cooldown_minutes = 1440 # Minimum time between auto-analyses (24h)
apply_cooldown_minutes = 1440   # Minimum time between auto-applies (24h)
auto_apply = true               # Enable full auto pipeline
auto_analyze_max_sessions = 15  # Skip auto-analyze when backlog exceeds this
```

Run `retro init` to create the default config.

## Installation

Requires the [Rust toolchain](https://rustup.rs/) and a C compiler (`build-essential` on Ubuntu) for bundled SQLite.

```sh
cargo install retro-cli
```

### Requirements

- [Claude Code](https://claude.ai/download) for session history and the `claude` CLI (used for AI-powered analysis)
- Git (for hook integration and PR creation)
- `gh` CLI (optional, for automatic PR creation and `retro sync`)

## Status

Retro is v0.2. The core pipeline works end-to-end and has been tested on real Claude Code session history.

**What works well:**
- Session ingestion and pattern discovery across projects (including explicit directives)
- Rolling window analysis for cross-session pattern discovery
- CLAUDE.md rule generation with managed sections (never touches your content)
- Skill and global agent generation
- Review queue with batch approve/skip/dismiss workflow
- Automatic pipeline via git hooks (ingest → analyze → apply → review queue, per-stage cooldowns)
- Auto-mode observability with audit logging and terminal nudge
- Context auditing for redundancy and contradictions
- PR lifecycle management (`retro sync` detects closed PRs)
- Dry-run mode on all AI-powered commands

**What's early:**
- Skill generation quality varies (two-phase generate+validate helps but isn't perfect)
- Pattern merging occasionally creates near-duplicates
- Only supports Claude Code (designed to be extensible to other agents)

## Contributing

Contributions welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for setup instructions and [CLAUDE.md](CLAUDE.md) for architecture details.

## License

MIT
