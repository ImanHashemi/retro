# README Polish + Demo GIF Design

## Goal

Polish the README to show concrete output instead of abstract descriptions. Replace the broken demo GIF with a scripted VHS recording showing the full value loop. Target audience: Claude Code power users and open source discoverers (GitHub/HN).

## Core message

Frictionless pattern discovery + effortless improvement + user always in control.

## README Changes

### Tagline + Intro

Rewrite to lead with the problem (one sentence), then the solution, then the control guarantee. Three paragraphs, three beats. No em-dashes.

```
# retro

**Active context curator for AI coding agents.**

You've told your agent "always use uv, not pip" across a dozen sessions.
You've corrected the same testing mistake three times this week. Your agent
forgets everything between conversations, and you're the one doing the
remembering.

Retro fixes this. It analyzes your Claude Code session history, discovers
patterns (repeated instructions, recurring mistakes, workflow conventions,
explicit directives) and turns them into skills and CLAUDE.md rules
automatically. Your agent gets better after every session, without you
maintaining its context by hand.

You stay in control: every change goes through a review queue where you
approve, skip, or dismiss. Shared changes are proposed as PRs.
```

### Hero GIF

Replace with new VHS-scripted recording (~35s) showing:
1. `retro analyze` with reasoning summaries and pattern counts
2. `retro patterns` with discovered patterns and confidence scores
3. `retro review` with batch approve flow

Output is fully scripted via `docs/demo-output.sh` (no real DB or AI calls).

### Quick Start

Add a "what you'll see" static code block after `retro analyze` showing condensed output (reasoning, pattern count, tokens). Then the remaining commands (`patterns`, `apply`, `review`).

### How It Works

No changes. Pipeline diagram and bullets are clear.

### What Retro Generates

Add concrete examples:
- CLAUDE.md: show the managed section delimiters with real-looking rules
- Skills: describe a concrete example ("pre-pr-checklist" skill)

### Commands through Config

No changes.

### Install

No changes.

### Status

Update to reflect current state (reasoning field, structured output, current test count).

## Demo GIF Plan

### Approach: Scripted terminal output

Create `docs/demo-output.sh` that prints hardcoded colored output mimicking real retro commands. VHS tape calls this script. No DB, no AI, fully deterministic.

### Scripted patterns (relatable to broad audience)

1. **repetitive_instruction** (82%, 4x): "User consistently tells the agent to use uv instead of pip for all Python package operations" -> claude_md
2. **workflow_pattern** (75%, 3x): "User guides agent through run-tests-then-lint-then-commit workflow before every PR" -> skill
3. **repetitive_instruction** (78%, 1x): "Always use conventional commit messages with type prefix (feat:, fix:, docs:)" -> claude_md

### VHS tape flow

```
Scene 1: retro analyze (~15s)
- Type command, show output with:
  - Step 1/3: Ingesting
  - Step 2/3: Analyzing with batch reasoning
  - Step 3/3: Audit log
  - "Analysis complete!" summary

Scene 2: retro patterns (~10s)
- Type command, show 3 discovered patterns with confidence/target

Scene 3: retro review (~10s)
- Type command, show 2 pending items
- Type "1a 2a" to approve both
- Show confirmation
```

### VHS tape settings

- Theme: Catppuccin Mocha
- Font size: 15
- Width: 1000, Height: 600
- Typing speed: 60ms

### Recording steps

1. Write `docs/demo-output.sh` with hardcoded colored output
2. Update `docs/demo.tape` to call the script
3. Run `vhs docs/demo.tape` to generate `docs/demo.gif`
4. Verify GIF looks good and file size is reasonable

## Style constraints

- No em-dashes anywhere
- No emojis
- Keep tone technical but accessible
