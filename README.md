# retro

**Active context curator for AI coding agents.**

You've told your agent "always use uv, not pip" across a dozen sessions. You've corrected the same testing mistake three times this week. Your agent forgets everything between conversations, and you're the one doing the remembering.

Retro fixes this. It analyzes your Claude Code session history, discovers patterns (repeated instructions, recurring mistakes, workflow conventions, explicit directives) and turns them into skills and CLAUDE.md rules automatically. Your agent gets better after every session, without you maintaining its context by hand.

You stay in control: every change goes through a review queue where you approve, skip, or dismiss. Shared changes are proposed as PRs.

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
```

You'll see output like:

```
  Batch 1/1: 12 sessions, 48K chars -> 892 tokens out, 3 new + 1 updated
    Found recurring testing workflow and two explicit directives about package management...

Analysis complete!
  Sessions analyzed: 12
  New patterns:      3
  Tokens:            52340 in / 892 out
```

Then review and apply what retro found:

```sh
# See discovered patterns
retro patterns

# Generate content and queue for review
retro apply

# Review: approve, skip, or dismiss each suggestion
retro review
```

After `retro init`, a post-commit hook runs the full pipeline in the background (ingest, analyze, apply) with per-stage cooldowns. Run `retro review` when you're ready to approve changes.

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

**CLAUDE.md rules** are conventions discovered from your sessions, added to a managed section in your project's CLAUDE.md:

```
<!-- retro:managed:start -->
- Always use uv instead of pip for Python package management
- Run cargo clippy -- -D warnings before committing
- Use conventional commit messages with type prefix (feat:, fix:, docs:)
<!-- retro:managed:end -->
```

Retro never touches content outside the managed delimiters.

**Skills** are reusable workflow patterns saved as `.claude/skills/` files. For example, a "pre-pr-checklist" skill extracted from a workflow you guided your agent through across multiple sessions: run tests, lint, format commit message.

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

1. **Ingest**: parse new sessions (cooldown: 5 minutes)
2. **Analyze**: discover patterns via AI (cooldown: 24 hours)
3. **Apply**: generate content and queue for review (cooldown: 24 hours)

Each stage has its own cooldown and data trigger, so there are no unnecessary AI calls. In `--auto` mode, retro skips if another instance is running and never produces output.

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

Retro is v0.2. The core pipeline works end-to-end and has been tested on real Claude Code session history. 115 unit tests, 10 scenario tests.

**What works well:**
- Session ingestion and pattern discovery across projects
- Explicit directive detection ("always use X", "never do Y") from single sessions
- Rolling window analysis with per-batch reasoning summaries
- Structured output via `--json-schema` for reliable AI response parsing
- CLAUDE.md rule generation with managed sections (never touches your content)
- Skill and global agent generation (two-phase: generate then validate)
- Review queue with batch approve/skip/dismiss workflow
- Automatic pipeline via git hooks with per-stage cooldowns
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
