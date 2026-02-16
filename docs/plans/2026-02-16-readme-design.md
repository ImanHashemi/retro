# README Documentation Design

## Context

Retro is an open-source Rust CLI tool (v0.1) that needs its first real README. Target audience: Claude Code power users who already understand CLAUDE.md, skills, MEMORY.md, and sessions.

## Research

Analyzed 6 popular Rust/CLI open-source READMEs (ripgrep, bat, delta, jujutsu, starship, claude-code). Key findings:

- Hook in first 10 seconds with visual proof or quantified value
- README length depends on docs site existence (~2,000 words for README-only, v0.1 projects)
- Universal ordering: title > one-liner > visual hook > quick start > features > install > config > contributing
- Honest status sections build trust (jujutsu pattern)
- Table format for command references is most scannable

## Approach

**Pipeline Demo (Approach A)** with a problem-statement opener (from Approach B). Lead with why context management matters, introduce retro as an automated retrospective, then show the pipeline in action.

## Decisions

- **Audience**: Claude Code power users (no need to explain CLAUDE.md, skills, etc.)
- **Visuals**: ASCII terminal output for inline examples, hero GIF/screenshot for top
- **Location**: README.md only (no docs site, no docs/ folder)
- **Depth**: Brief architecture section (pipeline diagram), no DB schema or prompt details
- **Tone**: Technical and direct, no emoji, no hype

## Section Design

### 1. Title + Tagline
`retro` with "Active context curator for AI coding agents."

### 2. Problem/Solution (3 paragraphs)
1. Why context matters (agents are powerful with good context)
2. The problem (curating context is manual, nobody does it well)
3. What retro is (automated retrospective on your sessions)
4. The wow (agent improves automatically from history)
5. The control (shared changes come as PRs)

### 3. Hero GIF Placeholder
Terminal recording showing the full pipeline: ingest > analyze > apply.

### 4. Quick Start
4-command happy path: init > ingest > analyze > apply. Then one paragraph about git hook automation.

### 5. How It Works
Three-stage pipeline ASCII diagram (Ingestion > Analysis > Projection) with one bullet per stage.

### 6. What Retro Generates
Three artifact types: CLAUDE.md rules, skills, global agents. Each gets 1-2 sentences. PR detail reinforces control.

### 7. Commands
Table format: command | description. Two-line note about --global and --dry-run flags.

### 8. Automatic Mode
Git hooks explanation: post-commit ingest, post-merge analyze. --auto mode behavior.

### 9. Configuration
Three key config.toml settings with example. Just enough to know it's configurable.

### 10. Installation
cargo install (primary), prebuilt binaries (placeholder), requirements (Rust, Claude Code, C compiler).

### 11. Status
Honest assessment: what works well (4 items), what's early (3 items). Inspired by jujutsu.

### 12. Contributing
Clone + build + test commands. Points to CLAUDE.md for architecture.

### 13. License
MIT.

## Estimated Length
~1,500-2,000 words plus hero GIF.
