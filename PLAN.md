 Retro: Implementation Plan

 Context

 AI coding agents like Claude Code accumulate knowledge through manual user instructions — "always run tests", "use uv not pip", "follow this commit style". Users repeat
 these instructions across sessions, and agents repeat the same mistakes. Meanwhile, context files (CLAUDE.md, skills, memory) grow unchecked and become stale.

 Retro is an open-source Rust CLI tool that solves this by:
 1. Analyzing agent history to discover repetitive instructions, recurring mistakes, and workflow patterns
 2. Projecting those patterns into skills and CLAUDE.md rules
 3. Curating context to keep it clean — archiving stale items, consolidating duplicates

 Retro acts as an "active curator" — it maintains its own database of ALL discovered patterns, but only projects a curated subset into the agent's active context.

 ---
 Architecture

 ┌─────────────────────────────────────────────┐
 │  INGESTION (pure Rust, no AI)               │
 │  Reads: ~/.claude/history.jsonl             │
 │         ~/.claude/projects/*/sessions       │
 │         CLAUDE.md, .claude/skills/, MEMORY  │
 │  Parses into structured Sessions in SQLite  │
 └────────────────┬────────────────────────────┘
                  │
 ┌────────────────▼────────────────────────────┐
 │  ANALYSIS (AI-powered, pluggable backend)   │
 │  Discovers: repetitive instructions,        │
 │  recurring mistakes, workflow patterns,     │
 │  stale/redundant context                    │
 │  Stores patterns with confidence scores     │
 └────────────────┬────────────────────────────┘
                  │
 ┌────────────────▼────────────────────────────┐
 │  PROJECTION (two-track)                     │
 │  Personal (auto-apply): global agents       │
 │  Shared (separate PR): CLAUDE.md, skills    │
 │  Read-only: MEMORY.md (Claude Code owns it) │
 │  Audit log: JSONL append-only               │
 └─────────────────────────────────────────────┘

 Storage (~/.retro/):
 - retro.db — SQLite in WAL mode (patterns, projections, metadata)
 - audit.jsonl — Append-only action log
 - config.toml — User configuration
 - backups/ — File backups before modification
 - retro.lock — PID lockfile for mutual exclusion

 ---
 Key Design Decisions
 Decision: Scope
 Choice: Claude Code first, extensible
 Rationale: Deep integration now, agent-agnostic analysis layer for later
 ────────────────────────────────────────
 Decision: Analysis scope
 Choice: Per-project default, --global opt-in
 Rationale: retro analyze runs for current project. retro analyze --global loads sessions from ALL projects within the rolling window. Session IDs are globally unique —
 same
    analyzed_sessions table is used.
 ────────────────────────────────────────
 Decision: Execution
 Choice: CLI + git hooks (no daemon)
 Rationale: Hooks for automatic ingestion/analysis at natural breakpoints
 ────────────────────────────────────────
 Decision: Hook strategy
 Choice: Ingest on post-commit, analyze on post-merge
 Rationale: Fast ingest (no AI) on every commit. Full AI analysis on merges (natural breakpoint, 1-5x/day).
 ───────────────────────────────────────
 Decision: Context philosophy
 Choice: Separate store + projection
 Rationale: DB stores everything; only curated subset projected to active context
 ────────────────────────────────────────
 Decision: Change flow
 Choice: Two-track
 Rationale: Personal (global agents) = auto-apply; Shared (CLAUDE.md, skills) = separate PRs on retro/updates-{date} branch
 ────────────────────────────────────────
 Decision: Memory overlap
 Choice: Complement, don't replace
 Rationale: Retro reads MEMORY.md as input but never writes to it. Memory-type patterns stay in Retro's DB (retro patterns). Claude Code owns MEMORY.md.
 ────────────────────────────────────────
 Decision: Storage
 Choice: SQLite (WAL mode) + JSONL hybrid
 Rationale: SQLite for queryable patterns; JSONL for inspectable audit log
 ────────────────────────────────────────
 Decision: Language
 Choice: Rust (sync, no tokio)
 Rationale: Single binary, fast. std::process::Command for process spawning — no async runtime needed.
 ────────────────────────────────────────
 Decision: AI backend
 Choice: Pluggable trait, Claude CLI first
 Rationale: claude -p "..." --output-format json. Future: Claude API, others
 ────────────────────────────────────────
 Decision: Analysis window
 Choice: Rolling (default 14 days)
 Rationale: Bounded cost/time; configurable
 ────────────────────────────────────────
 Decision: Pattern merging
 Choice: AI-assisted with post-processing safety net
 Rationale: Existing patterns included in analysis prompt; AI merges semantically; text-similarity check catches duplicates
 ────────────────────────────────────────
 Decision: MVP
 Choice: Full loop including PRs
 Rationale: v0.1 delivers the complete pipeline
 ────────────────────────────────────────
 Decision: Repo
 Choice: Cargo workspace (retro-core + retro-cli)
 Rationale: Library crate enables future integrations (MCP server, etc.)
 ---
 Repo Structure

 retro/                              # ~/repositories/retro/
 ├── Cargo.toml                      # Workspace root
 ├── CLAUDE.md
 ├── crates/
 │   ├── retro-core/
 │   │   ├── Cargo.toml
 │   │   └── src/
 │   │       ├── lib.rs
 │   │       ├── config.rs           # Config loading (~/.retro/config.toml)
 │   │       ├── db.rs               # SQLite schema, migrations, CRUD (WAL mode)
 │   │       ├── models.rs           # Domain types (Pattern, Session, Finding, etc.)
 │   │       ├── lock.rs             # PID lockfile for mutual exclusion
 │   │       ├── scrub.rs            # Sensitive data scrubbing (regex-based)
 │   │       ├── ingest/
 │   │       │   ├── mod.rs
 │   │       │   ├── history.rs      # Parse ~/.claude/history.jsonl
 │   │       │   ├── session.rs      # Parse main + subagent session JSONL files
 │   │       │   └── context.rs      # Snapshot CLAUDE.md, skills, MEMORY.md
 │   │       ├── analysis/
 │   │       │   ├── mod.rs
 │   │       │   ├── backend.rs      # AnalysisBackend trait (sync, not async)
 │   │       │   ├── claude_cli.rs   # Primary: spawns `claude -p`
 │   │       │   ├── prompts.rs      # Prompt templates for analysis & audit
 │   │       │   └── merge.rs        # Pattern merging/deduplication logic
 │   │       ├── projection/
 │   │       │   ├── mod.rs
 │   │       │   ├── skill.rs        # Write .claude/skills/*/SKILL.md
 │   │       │   ├── claude_md.rs    # Managed section in CLAUDE.md
 │   │       │   └── global_agent.rs # Update ~/.claude/agents/*.md (incl. model, color fields)
 │   │       ├── curator.rs          # Staleness detection, archiving
 │   │       ├── git.rs              # Hook installation/removal, branches, PRs
 │   │       └── audit_log.rs        # JSONL audit writer/reader
 │   └── retro-cli/
 │       ├── Cargo.toml
 │       └── src/
 │           ├── main.rs
 │           └── commands/
 │               ├── mod.rs
 │               ├── init.rs         # retro init [--uninstall]
 │               ├── ingest.rs       # retro ingest (fast, no AI)
 │               ├── analyze.rs      # retro analyze [--global]
 │               ├── apply.rs        # retro apply [--dry-run]
 │               ├── clean.rs        # retro clean [--dry-run] (fast, local, no AI)
 │               ├── audit.rs        # retro audit [--dry-run] (AI-powered context review)
 │               ├── status.rs       # retro status
 │               ├── log.rs          # retro log
 │               ├── patterns.rs     # retro patterns
 │               ├── diff.rs         # retro diff (alias for apply --dry-run with diff-style output)
 │               └── hooks.rs        # retro hooks remove
 └── tests/
     ├── fixtures/                   # Sample JSONL, CLAUDE.md, skills
     └── integration/

 ---
 Key Crates
 ┌────────────────────┬────────────────────────────────────────────────────┐
 │       Crate        │                      Purpose                       │
 ├────────────────────┼────────────────────────────────────────────────────┤
 │ clap (derive)      │ CLI parsing                                        │
 ├────────────────────┼────────────────────────────────────────────────────┤
 │ rusqlite (bundled) │ SQLite — bundled to avoid system dep               │
 ├────────────────────┼────────────────────────────────────────────────────┤
 │ serde + serde_json │ JSON/JSONL parsing                                 │
 ├────────────────────┼────────────────────────────────────────────────────┤
 │ anyhow + thiserror │ Error handling (thiserror for lib, anyhow for CLI) │
 ├────────────────────┼────────────────────────────────────────────────────┤
 │ chrono             │ Timestamps, rolling window                         │
 ├────────────────────┼────────────────────────────────────────────────────┤
 │ uuid               │ Pattern/projection IDs                             │
 ├────────────────────┼────────────────────────────────────────────────────┤
 │ glob               │ Finding session files                              │
 ├────────────────────┼────────────────────────────────────────────────────┤
 │ colored            │ Terminal output                                    │
 ├────────────────────┼────────────────────────────────────────────────────┤
 │ (removed)          │ Confirmation uses stdin y/N pattern (not dialoguer) │
 ├────────────────────┼────────────────────────────────────────────────────┤
 │ regex              │ Sensitive data scrubbing                           │
 └────────────────────┴────────────────────────────────────────────────────┘
 No tokio — all operations are synchronous. std::process::Command for spawning claude CLI and git/gh.
 No git2 — shell out to git and gh directly (simpler, more predictable for hooks/PRs).

 ---
 Configuration Schema

 ~/.retro/config.toml:

 [analysis]
 window_days = 14                    # Rolling window for analysis
 confidence_threshold = 0.7          # Min confidence to project patterns
 staleness_days = 28                 # Days without activity before a pattern is considered stale

 [ai]
 backend = "claude-cli"              # "claude-cli" | "claude-api" (future)
 model = "sonnet"                    # Model for analysis calls
 max_budget_per_call = 0.50          # Cost cap per AI invocation

 [hooks]
 ingest_cooldown_minutes = 5         # Minimum time between auto-ingests
 analyze_cooldown_minutes = 1440     # Minimum time between auto-analyses (24h)
 apply_cooldown_minutes = 1440       # Minimum time between auto-applies (24h)
 auto_apply = true                   # Enable full auto pipeline
 post_commit = "ingest"              # "ingest" (fast) | "none"

 [paths]
 claude_dir = "~/.claude"            # Override Claude Code data directory

 [privacy]
 scrub_secrets = true                # Regex-scrub common secret patterns before AI calls
 exclude_projects = []               # Project paths to skip entirely

 ---
 Database Schema

 -- Enable WAL mode for concurrent access
 PRAGMA journal_mode=WAL;

 CREATE TABLE patterns (
     id TEXT PRIMARY KEY,
     pattern_type TEXT NOT NULL,        -- repetitive_instruction | recurring_mistake | workflow_pattern | stale_context | redundant_context
     description TEXT NOT NULL,
     confidence REAL NOT NULL,          -- 0.0-1.0
     times_seen INTEGER NOT NULL DEFAULT 1,
     first_seen TEXT NOT NULL,          -- ISO 8601
     last_seen TEXT NOT NULL,
     last_projected TEXT,
     status TEXT NOT NULL DEFAULT 'discovered',  -- discovered | active | archived | dismissed
     source_sessions TEXT NOT NULL,     -- JSON array
     related_files TEXT NOT NULL,       -- JSON array
     suggested_content TEXT NOT NULL,
     suggested_target TEXT NOT NULL,    -- skill | claude_md | global_agent | db_only
     project TEXT,                      -- NULL = global pattern, else project path
     generation_failed INTEGER NOT NULL DEFAULT 0  -- 1 if skill generation failed after retries
 );

 CREATE TABLE projections (
     id TEXT PRIMARY KEY,
     pattern_id TEXT NOT NULL REFERENCES patterns(id),
     target_type TEXT NOT NULL,
     target_path TEXT NOT NULL,
     content TEXT NOT NULL,
     applied_at TEXT NOT NULL,
     pr_url TEXT
 );

 CREATE TABLE analyzed_sessions (
     session_id TEXT PRIMARY KEY,
     project TEXT NOT NULL,
     analyzed_at TEXT NOT NULL
 );

 CREATE TABLE ingested_sessions (
     session_id TEXT PRIMARY KEY,
     project TEXT NOT NULL,
     session_path TEXT NOT NULL,
     file_size INTEGER NOT NULL,        -- Track file size for re-ingestion detection
     file_mtime TEXT NOT NULL,          -- Track mtime; if file grows, re-ingest
     ingested_at TEXT NOT NULL
 );

 CREATE INDEX idx_patterns_status ON patterns(status);
 CREATE INDEX idx_patterns_type ON patterns(pattern_type);
 CREATE INDEX idx_patterns_target ON patterns(suggested_target);
 CREATE INDEX idx_patterns_project ON patterns(project);
 CREATE INDEX idx_projections_pattern ON projections(pattern_id);

 -- Schema versioning: track via SQLite user_version pragma
 -- PRAGMA user_version = 1;
 -- Future migrations applied sequentially on startup based on current user_version

 ---
 Claude Code Data Sources
 ┌──────────────────────────────────────────────────────────────────┬─────────────────────────────┬─────────────────────────────────────────────┐
 │                               File                               │           Format            │                   Content                   │
 ├──────────────────────────────────────────────────────────────────┼─────────────────────────────┼─────────────────────────────────────────────┤
 │ ~/.claude/history.jsonl                                          │ JSONL                       │ User messages with timestamp + project path │
 ├──────────────────────────────────────────────────────────────────┼─────────────────────────────┼─────────────────────────────────────────────┤
 │ ~/.claude/projects/{encoded-path}/{uuid}.jsonl                   │ JSONL                       │ Main session conversations (PRIMARY source) │
 ├──────────────────────────────────────────────────────────────────┼─────────────────────────────┼─────────────────────────────────────────────┤
 │ ~/.claude/projects/{encoded-path}/{uuid}/subagents/agent-*.jsonl │ JSONL                       │ Subagent conversations (secondary)          │
 ├──────────────────────────────────────────────────────────────────┼─────────────────────────────┼─────────────────────────────────────────────┤
 │ {repo}/CLAUDE.md                                                 │ Markdown                    │ Project documentation and rules             │
 ├──────────────────────────────────────────────────────────────────┼─────────────────────────────┼─────────────────────────────────────────────┤
 │ {repo}/.claude/skills/{name}/SKILL.md                            │ Markdown + YAML frontmatter │ Project-level skills                        │
 ├──────────────────────────────────────────────────────────────────┼─────────────────────────────┼─────────────────────────────────────────────┤
 │ ~/.claude/agents/*.md                                            │ Markdown + YAML frontmatter │ Global user-level skills                    │
 ├──────────────────────────────────────────────────────────────────┼─────────────────────────────┼─────────────────────────────────────────────┤
 │ ~/.claude/projects/{encoded-path}/memory/MEMORY.md               │ Markdown                    │ Per-project learned knowledge (read-only)   │
 └──────────────────────────────────────────────────────────────────┴─────────────────────────────┴─────────────────────────────────────────────┘
 Path encoding: /Users/foo/bar → -Users-foo-bar

 Session JSONL Entry Types

 Each line in a session JSONL file has a type field. Retro handles them as follows:
 Entry Type: user
 Action: Parse
 Rationale: User instructions — primary signal for repetitive instruction detection
 ────────────────────────────────────────
 Entry Type: assistant
 Action: Parse
 Rationale: Agent responses with content arrays (text, thinking, tool_use blocks). Thinking blocks contain reasoning — rich signal for mistake detection.
 ────────────────────────────────────────
 Entry Type: summary
 Action: Parse
 Rationale: Session summaries — efficient overview of what happened
 ────────────────────────────────────────
 Entry Type: file-history-snapshot
 Action: Skip
 Rationale: File tracking metadata, not useful for pattern analysis
 ────────────────────────────────────────
 Entry Type: progress
 Action: Skip
 Rationale: Hook events, tool progress — operational noise
 Content array handling: Assistant entries contain message.content as an array of blocks:
 - text blocks → agent's visible response
 - thinking blocks → agent's internal reasoning (valuable for detecting mistakes and decision patterns). Truncation: first 500 chars + keyword-extracted segments containing
  "error", "mistake", "wrong", "failed", "retry" (thinking blocks can be 32K+ tokens; must be bounded).
 - tool_use blocks → which tools were invoked and with what inputs

 Metadata extracted per entry: parentUuid, cwd, sessionId, version (Claude Code version), gitBranch, timestamp

 ---
 AI Backend

 pub trait AnalysisBackend: Send + Sync {
     fn analyze_sessions(&self, sessions: &[Session], existing_patterns: &[Pattern], prompt: &str) -> Result<Vec<PatternUpdate>>;
     fn audit_context(&self, context: &ContextSnapshot, sessions: &[Session], prompt: &str) -> Result<Vec<Finding>>;
     fn generate_skills(&self, patterns: &[Pattern], prompt: &str) -> Result<Vec<SkillDraft>>;
     fn validate_skills(&self, drafts: &[SkillDraft], prompt: &str) -> Result<Vec<ValidationResult>>;
 }

 Note: Sync trait (no async). PatternUpdate can be either a new pattern or an update to an existing one (with existing pattern ID).

 Primary implementation (ClaudeCliBackend):
 claude -p "<prompt>" --output-format json --model sonnet --max-budget-usd 0.50
 - Spawns claude CLI in non-interactive mode via std::process::Command
 - --output-format json returns a wrapper object with result field — parser extracts inner content
 - --json-schema constrains the inner result structure
 - Timeout: 120 seconds
 - Cost cap via --max-budget-usd

 Prompt strategy: Sessions are serialized as compact JSON (user messages truncated to 500 chars, tool names only, errors, thinking block summaries). Total prompt < 150k
 chars.

 Four prompt types:
 1. Pattern discovery — includes existing patterns for AI-assisted merging
 2. Context audit — AI-powered review of CLAUDE.md/skills/memory for redundancy/contradictions (used by retro audit)
 3. Skill generation — one skill per AI call for maximum quality (not batched)
 4. Skill validation — one validation per generated skill

 ---
 Pattern Merging & Deduplication

 When retro analyze runs repeatedly, patterns must be merged correctly:

 Strategy: AI-assisted merging with post-processing safety net

 1. Load existing patterns from DB (status = discovered or active)
 2. Include in analysis prompt: "Here are patterns already discovered. If you find evidence supporting an existing pattern, return its ID with updated evidence. Only create
  new patterns for genuinely new findings."
 3. AI returns PatternUpdate objects:
   - Update { id, new_sessions, new_confidence } — reinforces existing pattern
   - New { pattern } — genuinely new discovery
 4. Post-processing safety net: For any New pattern, compute text similarity (normalized Levenshtein) against existing patterns. If similarity > 0.8, merge instead of
 creating duplicate. Known limitation: Levenshtein misses semantic duplicates like "always use uv" vs "prefer uv over pip" — the AI-assisted merging is the primary
 mechanism; this is just a safety net for near-identical descriptions.
 5. Merge logic:
   - times_seen += new occurrences
   - confidence = max(existing, new)
   - source_sessions = existing ∪ new
   - last_seen = max(existing, new)

 ---
 CLI Commands

 retro init                    # Create ~/.retro/, init DB, install git hooks (if in repo)
 retro init --uninstall        # Remove hooks from current repo, preserve ~/.retro/ data
 retro init --uninstall --purge  # Remove hooks AND delete ~/.retro/ entirely
 retro ingest                  # Fast: parse new sessions into DB (no AI)
 retro analyze [--since 14d]   # Full: ingest + AI analysis + pattern discovery
 retro analyze --global        # Cross-project analysis
 retro apply [--dry-run]       # Project patterns → skills/CLAUDE.md
 retro clean [--dry-run]       # Fast local staleness check (no AI) — archive stale items
 retro audit [--dry-run]       # AI-powered redundancy/contradiction detection
 retro status                  # DB stats, last analysis, session count
 retro patterns [--status X]   # List patterns with confidence scores
 retro diff                    # Show pending changes (= apply --dry-run)
 retro log [--since 7d]        # Show audit log entries
 retro hooks remove            # Remove git hooks from current repo

 Git hooks installed by retro init:

 post-commit:
 # retro hook - do not remove
 retro ingest 2>/dev/null &

 post-merge:
 # retro hook - do not remove
 retro analyze --auto 2>/dev/null &

 --auto mode: silent, acquires lockfile (skips if locked), skips if analyzed within cooldown window, exits on any error.

 Hook safety: Appends to existing hook files (never overwrites). retro hooks remove scans for the # retro hook marker and removes those lines. Compatible with pre-commit,
 Husky, lefthook.

 ---
 Concurrency Protection

 - SQLite WAL mode: Enabled at DB creation. Handles concurrent reads, serializes writes safely.
 - PID lockfile (~/.retro/retro.lock): Acquired by retro analyze and retro apply (long-running commands). If locked, --auto mode skips silently; interactive mode warns and
 exits.
 - retro ingest: No lockfile needed — append-only inserts to SQLite are safe under WAL mode.
 - audit.jsonl: Uses file-level append with O_APPEND flag — atomic on POSIX systems.

 ---
 Sensitive Data Handling

 Session data may contain API keys, passwords, or PII. Before sending to AI:

 1. Regex scrubbing (scrub.rs): Strip common secret patterns:
   - AWS keys (AKIA...), GitHub tokens (ghp_..., gho_...), generic password=, secret=, token=
   - Replaced with [REDACTED]
 2. Project exclusion: privacy.exclude_projects in config.toml — skip sensitive repos entirely
 3. Documentation: README clearly states what data leaves the machine (session content → Claude CLI/API)
 4. retro analyze --dry-run: Shows preview of what would be analyzed (session list, message counts, batch estimate) without making AI calls. Implemented.

 ---
 CLAUDE.md Protection

 Retro owns a delimited section — never touches user-written content:

 <!-- everything above is user-managed -->

 <!-- retro:managed:start -->
 ## Retro-Discovered Patterns

 - Always use `uv` for Python package management, never `pip`
 - Run `cargo test` after modifying any `.rs` file

 <!-- retro:managed:end -->

 ---
 retro clean and retro audit Behavior

 retro clean = fast, local, no AI. Checks staleness heuristics and archives items that haven't been seen recently.
 retro audit = AI-powered. Analyzes context for redundancy, contradictions, and bloat. Has associated AI cost.

 Staleness Detection (retro clean)

 A pattern/projection is stale if ALL of these are true:
 - last_seen is older than analysis.staleness_days (configurable, default: 28 days)
 - The skill/rule it generated was NOT referenced in any session within the window
 - It was generated by Retro (tracked via projections table)

 Retro never touches user-authored content — only items it created (tracked in projections).

 Archiving Actions
 ┌──────────────────────────────────┬────────────────────────────────────────────────────────┐
 │              Target              │                     Archive Action                     │
 ├──────────────────────────────────┼────────────────────────────────────────────────────────┤
 │ Skill (.claude/skills/)          │ Backup to ~/.retro/backups/, delete SKILL.md directory │
 ├──────────────────────────────────┼────────────────────────────────────────────────────────┤
 │ CLAUDE.md managed section        │ Remove entry from <!-- retro:managed --> block         │
 ├──────────────────────────────────┼────────────────────────────────────────────────────────┤
 │ Global agent (~/.claude/agents/) │ Backup to ~/.retro/backups/, delete .md file           │
 ├──────────────────────────────────┼────────────────────────────────────────────────────────┤
 │ DB pattern                       │ Status change: active → archived                       │
 └──────────────────────────────────┴────────────────────────────────────────────────────────┘
 Redundancy & Contradiction Detection (retro audit)

 The context audit prompt (AI-powered, called via retro audit) identifies:
 - Redundant: Same information in CLAUDE.md and a skill → consolidate into one
 - Contradictory: A rule says "use pip" but a pattern says "use uv" → flag for user review
 - Oversized: CLAUDE.md managed section > 50 lines → suggest consolidation

 ---
 Skill Generation Quality

 Skill quality is critical — a bad auto-generated skill is worse than no skill. Based on analysis of the Superpowers plugin's skill-writing system:

 Skill Format (SKILL.md)

 ---
 name: letters-numbers-hyphens-only
 description: Use when [specific triggering conditions/symptoms]. Never summarize what the skill does here.
 ---

 The description field is the most important field — it's how Claude discovers the skill (CSO). Must:
 - Start with "Use when..."
 - Describe triggering conditions, NOT what the skill does
 - Include keywords: error messages, tool names, symptoms
 - Stay under 1024 chars total YAML

 Global Agent Format

 When generating global agents (~/.claude/agents/*.md), include all frontmatter fields:
 ---
 name: agent-name
 description: When/how to use
 model: sonnet
 color: blue
 ---

 Two-Phase Generation Pipeline

 Phase A — One Skill Per Prompt (during retro apply):
 - Collects all qualifying patterns (confidence ≥ threshold, suggested_target = skill)
 - Generates one skill per AI call — not batched, because skill quality is the highest priority
 - Each prompt includes: the specific pattern + evidence, Superpowers format rules, 2-3 example skills, CSO guidelines
 - This means N qualifying patterns = N AI calls. Quality over cost.

 Phase B — Quality Validation (during retro apply, after each generation):
 - Separate AI call validates the generated skill against quality criteria
 - Checks: CSO description, word limit, addresses actual pattern, appropriate specificity
 - If validation fails: retry with feedback (max 2 retries per skill)
 - After 2 failed retries: Store pattern as generation_failed = 1 in DB. Surface via retro patterns with note. User can retry later or write manually. Pattern is NOT
 silently dropped.

 What Retro Generates vs What It Doesn't
 ┌─────────────────────────────────────┬────────────────┬─────────────────────────────────────────────────────┐
 │            Pattern Type             │  Generated As  │                        Notes                        │
 ├─────────────────────────────────────┼────────────────┼─────────────────────────────────────────────────────┤
 │ Repetitive instruction (simple)     │ CLAUDE.md rule │ "Always use uv" — too simple for a skill            │
 ├─────────────────────────────────────┼────────────────┼─────────────────────────────────────────────────────┤
 │ Repetitive instruction (procedural) │ Skill          │ Multi-step procedure → skill with steps             │
 ├─────────────────────────────────────┼────────────────┼─────────────────────────────────────────────────────┤
 │ Recurring mistake                   │ CLAUDE.md rule │ "Never do X, do Y instead" — rules prevent mistakes │
 ├─────────────────────────────────────┼────────────────┼─────────────────────────────────────────────────────┤
 │ Workflow pattern                    │ Skill          │ Multi-step workflows are the ideal skill use case   │
 ├─────────────────────────────────────┼────────────────┼─────────────────────────────────────────────────────┤
 │ Stale/redundant context             │ Archive action │ Not a generation — it's a removal                   │
 └─────────────────────────────────────┴────────────────┴─────────────────────────────────────────────────────┘
 ---
 Implementation Phases

 Phase 1: Skeleton + Ingestion

 - Cargo workspace setup (retro-core + retro-cli)
 - config.rs — load/create ~/.retro/config.toml with full schema
 - models.rs — all domain types including session entry type handling
 - ingest/history.rs — parse history.jsonl, filter by rolling window
 - ingest/session.rs — parse main session JSONL ({uuid}.jsonl) AND subagent files; handle all entry types (user, assistant with content arrays, summary; skip
 file-history-snapshot, progress)
 - ingest/context.rs — snapshot CLAUDE.md, skills, MEMORY.md
 - scrub.rs — regex-based secret scrubbing
 - db.rs — SQLite schema + migrations + WAL mode + indexes
 - lock.rs — PID lockfile
 - retro init + retro status + retro ingest commands
 - Error handling from the start: missing claude CLI, empty history, malformed JSONL
 - Test fixtures based on real Claude Code data formats (all entry types)
 - Deliverable: retro init && retro ingest && retro status shows discovered sessions

 Phase 2: Analysis Backend + Pattern Discovery

 - analysis/backend.rs — sync trait definition (no async)
 - analysis/claude_cli.rs — spawn claude -p, parse JSON wrapper (extract inner result)
 - analysis/prompts.rs — pattern discovery prompt template (includes existing patterns for merging)
 - analysis/merge.rs — pattern merge/dedup logic (AI-assisted + text-similarity safety net)
 - retro analyze command — ingest → scrub → AI analysis → merge → store patterns
 - retro analyze --global — cross-project mode
 - retro patterns command — list discovered patterns
 - audit_log.rs — JSONL writer/reader
 - Deliverable: retro analyze && retro patterns discovers and lists patterns. Repeated runs merge correctly.

 Phase 3: Projection + Apply

 - projection/skill.rs — batch skill generation with two-phase pipeline (draft + validate)
 - projection/claude_md.rs — managed section with <!-- retro:managed:start/end -->
 - projection/global_agent.rs — create/update global agents (with model, color frontmatter)
 - Note: No memory.rs — Retro reads MEMORY.md as input but never writes to it.
 - retro apply --dry-run + retro diff
 - Two-track classification (personal vs shared)
 - Failed generation handling (generation_failed flag)
 - Deliverable: retro apply --dry-run shows proposed changes

 Phase 4: Full Apply + Clean + Audit + Git

 - retro apply — write personal changes, create branch + PR for shared. One skill per AI call.
 - git.rs — branch creation, gh pr create, hook installation/removal
 - curator.rs — staleness detection (configurable staleness_days), ownership tracking via projections table
 - retro clean [--dry-run] — fast local staleness check, backup before deletion
 - retro audit [--dry-run] — AI-powered redundancy/contradiction detection (context audit prompt)
 - retro log
 - retro hooks remove
 - retro init --uninstall [--purge]
 - Deliverable: Full loop works end-to-end

 Phase 5: Hooks + Polish (DONE)

 - Git hook installation in retro init (single post-commit hook: ingest --auto, chains analyze + apply when auto_apply=true)
 - --auto mode on ingest, analyze, and apply: LockFile::try_acquire() (skip if locked), per-stage cooldowns (ingest: 5m, analyze: 24h, apply: 24h), suppress all output, exit silently on errors
 - --verbose global flag: threads through all commands, [verbose] debug output to stderr
 - Progress indicators: "This may take a minute..." messages before AI calls in analyze, apply, audit
 - Colored terminal output: consistency pass on all commands
 - Deliverable: v0.1 complete

 ---
 Risks & Mitigations
 ┌────────────────────────────────────────┬─────────────────────────────────────────────────────────────────────────────────────────────────┐
 │                  Risk                  │                                           Mitigation                                            │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ Claude CLI not installed / auth issues │ Check in retro init, clear error message, graceful skip                                         │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ JSONL format changes between versions  │ #[serde(default)] on all optional fields, skip unparseable lines, track Claude Code version     │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ Large session volume                   │ Rolling window bounds it; batch in groups of 20; skip already-analyzed                          │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ Prompt injection from session content  │ Wrap data in JSON block; validate AI output against schema                                      │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ Sensitive data in sessions             │ Regex scrubbing of secrets; project exclude list; document data flow                            │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ Clobbering user files                  │ <!-- retro:managed --> sections; ownership tracking via projections table; backup before modify │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ Git hook conflicts                     │ Append-only; comment markers; retro hooks remove for clean uninstall                            │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ gh CLI not available for PRs           │ Fall back to creating branch + printing manual PR instructions                                  │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ Low-quality auto-generated skills      │ Two-phase generation; confidence threshold; retro clean; shared skills via PR review            │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ Pattern duplication across runs        │ AI-assisted merging + text-similarity safety net                                                │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ Concurrent execution (rapid commits)   │ WAL mode; PID lockfile; --auto skips if locked                                                  │
 ├────────────────────────────────────────┼─────────────────────────────────────────────────────────────────────────────────────────────────┤
 │ Skill generation fails repeatedly      │ After 2 retries: mark generation_failed, surface in retro patterns, don't silently drop         │
 └────────────────────────────────────────┴─────────────────────────────────────────────────────────────────────────────────────────────────┘
 ---
 Verification Plan

 After each phase, verify:

 1. Phase 1: cargo build succeeds. retro init creates ~/.retro/ with config.toml and WAL-mode DB. retro ingest parses main session JSONL + subagent files. retro status
 shows correct session count. Test with malformed JSONL (graceful skip). Verify secret scrubbing strips test patterns.
 2. Phase 2: retro analyze calls Claude CLI, parses JSON wrapper response, stores patterns. Run twice on same data — verify merging works (no duplicates, times_seen
 increments). retro analyze --global discovers cross-project patterns. Check audit.jsonl.
 3. Phase 3: retro apply --dry-run shows diffs for proposed skills/CLAUDE.md changes. Verify skill format matches Superpowers CSO rules. Verify one-skill-per-prompt
 generation. Test failed generation → generation_failed flag set.
 4. Phase 4: retro apply writes personal files correctly. For shared changes, creates retro/updates-* branch and PR. retro clean --dry-run identifies stale items (local, no
  AI). retro audit --dry-run finds redundancies (AI-powered). Verify neither touches user-authored skills. retro hooks remove cleans up hooks. Test backup creation. Test
 --uninstall --purge.
 5. Phase 5: Commit in a repo → retro ingest runs silently. Merge → retro analyze --auto runs. Verify lockfile prevents concurrent execution. Full retro init → ingest →
 analyze → apply → clean cycle works.

 Testing strategy: Unit tests with fixtures (no AI), integration tests with MockBackend, CLI tests with assert_cmd, optional #[ignore] tests that invoke real Claude CLI.
