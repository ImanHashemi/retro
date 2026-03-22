# retro

**Your coding agent gets better after every session — automatically.**

You've told your agent "always use uv, not pip" across a dozen sessions. You've corrected the same testing mistake three times this week. Your agent forgets everything between conversations, and you're the one doing the remembering.

Retro fixes this. It watches your Claude Code sessions in the background, discovers patterns (repeated instructions, recurring mistakes, workflow conventions, explicit directives) and turns them into persistent context — skills and CLAUDE.md rules. Your agent improves session over session, with zero effort from you.

You stay in control: suggestions surface in a TUI dashboard where you approve or dismiss them. Shared changes are proposed as PRs.

![retro demo](docs/demo.gif)

## Quick Start

```sh
# Install
cargo install retro-cli

# Initialize (creates database, config, starts background watcher)
cd your-project
retro init

# That's it. Retro is now watching your sessions.
# Open the dashboard anytime to see what it's learned:
retro dash
```

After `retro init`, a background job runs every 5 minutes: ingesting new sessions, analyzing them for patterns, and queuing suggestions for your review. Open `retro dash` to approve or dismiss suggestions from a TUI dashboard.

## How It Works

Retro runs as a scheduled background job (launchd on macOS) that processes your Claude Code sessions through a five-layer pipeline:

```
  ┌──────────────────────────────────────────────────┐
  │  SURFACES                                        │
  │  TUI Dashboard (retro dash) · Session Briefing   │
  ├──────────────────────────────────────────────────┤
  │  PROJECTORS                                      │
  │  Claude Code: rules → CLAUDE.md, skills → files  │
  │  (Pluggable: Gemini, Cursor planned)             │
  ├──────────────────────────────────────────────────┤
  │  KNOWLEDGE STORE                                 │
  │  Graph in SQLite: nodes (rules, patterns, skills,│
  │  preferences, directives) + edges (supports,     │
  │  supersedes, derived_from)                       │
  ├──────────────────────────────────────────────────┤
  │  ANALYZERS                                       │
  │  AI-powered pattern discovery with scope         │
  │  classification (global vs project)              │
  ├──────────────────────────────────────────────────┤
  │  OBSERVERS                                       │
  │  Session file watcher (mtime-based polling)      │
  ├──────────────────────────────────────────────────┤
  │  SCHEDULED RUNNER                                │
  │  launchd periodic job (every 5 min)              │
  └──────────────────────────────────────────────────┘
```

- **Observers** detect modified session files by polling mtimes — no filesystem watcher needed.
- **Analyzers** use Claude to discover patterns, classify them by scope (global vs project-specific), and build a knowledge graph with confidence scores. Explicit directives ("always use X", "never do Y") are detected at high confidence from a single session.
- **Knowledge Store** is a graph of typed nodes (rules, patterns, skills, preferences, directives, memories) with edges (supports, contradicts, supersedes). Confidence accumulates across sessions.
- **Projectors** turn high-confidence knowledge into agent-specific output. The Claude Code projector generates CLAUDE.md rules and skill files.
- **Surfaces** present suggestions for review. The TUI dashboard shows pending suggestions and lets you browse all stored knowledge.

## TUI Dashboard

`retro dash` opens a terminal UI for reviewing suggestions and browsing what Retro has learned:

```
┌─ Retro Dashboard ─────────────────────────────────────────┐
│  Status: Active · Last run: 4 min ago · AI calls: 3/10   │
│  [Pending Review (3)]  [Knowledge (23)]                   │
├───────────────────────────────────────────────────────────┤
│  > [rule]  my-rust-app  "Prefer thiserror over…"    .82   │
│    [skill] global       "rust-error-handling"       .78   │
│    [rule]  my-python    "Always type-hint return…"  .71   │
│                                                           │
│  a: approve  d: dismiss  p: preview  Tab: switch  q: quit│
└───────────────────────────────────────────────────────────┘
```

- **Pending Review tab** — suggestions waiting for your approval. Press `a` to approve, `d` to dismiss, `p` to preview.
- **Knowledge tab** — browse all active knowledge with scope and type filters. Press `s` to cycle scope, `t` to cycle type, `/` to search.

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

**Full management mode** lets Retro manage your entire CLAUDE.md. Enable with `full_management = true`. In this mode, `retro apply` proposes granular edits anywhere in CLAUDE.md, and `retro curate` performs an AI-powered full rewrite via PR.

**Skills** are reusable workflow patterns saved as `.claude/skills/` files — extracted from patterns you've demonstrated across multiple sessions.

**Session briefings** are per-project files at `~/.retro/briefings/` that tell your agent what's new when it starts a session. Retro installs a skill that reads the briefing automatically.

## Commands

| Command | Description |
|---------|-------------|
| `retro init` | Set up Retro: database, config, background watcher, briefing skill |
| `retro dash` | Open TUI dashboard to review suggestions and browse knowledge |
| `retro start` | Start the background watcher (launchd on macOS) |
| `retro stop` | Stop the background watcher |
| `retro run` | Run the full pipeline once (what the watcher runs periodically) |
| `retro status` | Show session counts, last analysis, pattern summary |
| `retro ingest` | Parse new sessions from Claude Code history (fast, no AI) |
| `retro analyze` | Discover patterns across sessions (AI-powered) |
| `retro patterns` | List discovered patterns, filterable by status |
| `retro apply` | Generate content from patterns and queue for review |
| `retro review` | CLI-based review: approve, skip, or dismiss (alternative to `retro dash`) |
| `retro sync` | Check PR status and reset patterns from closed PRs |
| `retro diff` | Preview what `apply` would change |
| `retro clean` | Archive stale patterns |
| `retro audit` | AI-powered review for redundancy and contradictions |
| `retro curate` | AI-powered full CLAUDE.md rewrite via PR |
| `retro log` | Show audit log entries |

Use `--dry-run` on any AI-powered command to preview without making changes or API calls.

## Configuration

Config lives at `~/.retro/config.toml`:

```toml
[runner]
interval_seconds = 300         # How often the background watcher runs (5 min)
analysis_trigger = "sessions"  # Trigger: "sessions" (after N new) or "always"
analysis_threshold = 3         # Analyze after this many new sessions
max_ai_calls_per_day = 10     # Hard cap on AI calls per day

[trust]
mode = "review"                # "review" (default) or "auto"

[trust.auto_approve]           # Only applies when mode = "auto"
rules = true
skills = false
preferences = true
directives = true

[trust.scope]                  # Only applies when mode = "auto"
global_changes = "review"      # Always review global changes even in auto mode
project_changes = "auto"

[knowledge]
confidence_threshold = 0.7     # Minimum confidence to suggest
global_promotion_threshold = 0.85  # Suggest moving project knowledge to global

[analysis]
window_days = 14               # How far back to analyze
rolling_window = true          # Re-analyze all sessions in window
model = "sonnet"               # AI model (sonnet, opus, haiku)

[claude_md]
full_management = false        # Full CLAUDE.md management (enables curate + granular edits)
```

Run `retro init` to create the default config.

## Installation

Requires the [Rust toolchain](https://rustup.rs/) and a C compiler (`build-essential` on Ubuntu) for bundled SQLite.

```sh
cargo install retro-cli
```

### Requirements

- [Claude Code](https://claude.ai/download) for session history and the `claude` CLI (used for AI analysis)
- Git (for hook integration and PR creation)
- `gh` CLI (optional, for automatic PR creation and `retro sync`)
- macOS (for background watcher via launchd — Linux systemd support planned)

## Status

Retro 2.0 "The Watcher". The core pipeline works end-to-end with automatic background operation. 228 unit tests.

**What works:**
- Automatic background operation — `retro init` sets everything up, the watcher runs every 5 minutes
- TUI dashboard for reviewing suggestions and browsing knowledge
- Knowledge graph with typed nodes, scoped knowledge (global vs project), confidence accumulation
- Session ingestion and pattern discovery across projects
- Explicit directive detection ("always use X", "never do Y") from single sessions
- CLAUDE.md rule generation with managed sections
- Skill generation (two-phase: generate then validate)
- Full CLAUDE.md management with granular edits and agentic rewrite
- Trust-based auto-approve configuration
- Session briefings delivered via skill files
- Cost control (configurable daily AI call cap)
- PR lifecycle management
- Dry-run mode on all AI-powered commands

**What's planned:**
- Linux systemd support for the background watcher
- Additional projectors (Gemini, Cursor)
- Contradiction detection and resolution
- Knowledge graph visualization in TUI

## Contributing

Contributions welcome! See [CONTRIBUTING.md](CONTRIBUTING.md) for setup instructions and [CLAUDE.md](CLAUDE.md) for architecture details.

## License

MIT
