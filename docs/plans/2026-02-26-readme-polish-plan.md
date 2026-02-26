# README Polish + Demo GIF Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Polish the README with concrete output examples and replace the broken demo GIF with a scripted VHS recording showing the full analyze/patterns/review flow.

**Architecture:** Three deliverables: (1) `docs/demo-output.sh` shell script that prints hardcoded colored output mimicking retro commands, (2) updated `docs/demo.tape` VHS script that calls the shell script, (3) updated `README.md` with rewritten intro, output examples, and new GIF.

**Tech Stack:** Bash (ANSI escape codes for colors), VHS (Charm), Markdown

---

### Task 1: Create the demo output script

**Files:**
- Create: `docs/demo-output.sh`

The script takes a subcommand argument (`analyze`, `patterns`, `review`) and prints hardcoded colored terminal output that mimics real retro CLI output. Uses ANSI escape codes directly (no dependencies). The output must match retro's actual formatting from `crates/retro-cli/src/commands/analyze.rs`, `patterns.rs`, and `review.rs`.

**Step 1: Write `docs/demo-output.sh`**

```bash
#!/usr/bin/env bash
# Scripted retro output for demo GIF recording.
# Usage: ./docs/demo-output.sh <analyze|patterns|review>

# ANSI color codes matching retro's colored output
RESET='\033[0m'
BOLD='\033[1m'
DIM='\033[2m'
CYAN='\033[36m'
GREEN='\033[32m'
YELLOW='\033[33m'
WHITE='\033[37m'
BOLD_GREEN='\033[1;32m'
BOLD_YELLOW='\033[1;33m'
BOLD_WHITE='\033[1;37m'

case "$1" in
  analyze)
    echo -e "${CYAN}Step 1/3: Ingesting new sessions...${RESET}"
    sleep 0.3
    echo -e "  ${GREEN}4${RESET} new sessions ingested"
    sleep 0.5
    echo -e "${CYAN}Step 2/3: Analyzing sessions (window: 14d)...${RESET}"
    echo -e "  ${DIM}This may take a minute (AI-powered analysis)...${RESET}"
    sleep 2
    echo -e "${CYAN}Step 3/3: Recording audit log...${RESET}"
    sleep 0.3
    echo ""
    echo -e "  Batch 1/1: 12 sessions, 48K chars → 892 tokens out, ${GREEN}3${RESET} new + ${YELLOW}1${RESET} updated"
    echo -e "    ${DIM}Found recurring testing workflow and two explicit directives about package management and commit style.${RESET}"
    echo ""
    echo -e "${BOLD_GREEN}Analysis complete!${RESET}"
    echo -e "  ${WHITE}Sessions analyzed:${RESET} ${CYAN}12${RESET}"
    echo -e "  ${WHITE}New patterns:${RESET}      ${GREEN}3${RESET}"
    echo -e "  ${WHITE}Updated patterns:${RESET}  ${YELLOW}1${RESET}"
    echo -e "  ${WHITE}Total patterns:${RESET}    ${CYAN}4${RESET}"
    echo -e "  ${WHITE}Tokens:${RESET}            ${CYAN}52340${RESET} in / ${CYAN}892${RESET} out"
    echo ""
    echo -e "Run ${CYAN}retro patterns${RESET} to see discovered patterns."
    ;;

  patterns)
    echo -e "Patterns (${GREEN}3 discovered${RESET}, ${CYAN}1 active${RESET}, 0 archived)"
    echo ""
    echo -e "  ${YELLOW}[discovered]${RESET} repetitive_instruction (confidence: ${BOLD}82%${RESET}, seen: 4x)"
    echo -e "    \"User consistently tells the agent to use uv instead of pip for all Python package operations\""
    echo -e "    → ${CYAN}claude_md${RESET}"
    echo ""
    echo -e "  ${YELLOW}[discovered]${RESET} workflow_pattern (confidence: ${BOLD}75%${RESET}, seen: 3x)"
    echo -e "    \"User guides agent through run-tests-then-lint-then-commit workflow before every PR\""
    echo -e "    → ${CYAN}skill${RESET}"
    echo ""
    echo -e "  ${YELLOW}[discovered]${RESET} repetitive_instruction (confidence: ${BOLD}78%${RESET}, seen: 1x)"
    echo -e "    \"Always use conventional commit messages with type prefix (feat:, fix:, docs:)\""
    echo -e "    → ${CYAN}claude_md${RESET}"
    echo ""
    echo -e "  ${CYAN}[active]${RESET} recurring_mistake (confidence: ${BOLD}88%${RESET}, seen: 5x)"
    echo -e "    \"Agent forgets to run database migrations before running integration tests\""
    echo -e "    → ${CYAN}claude_md${RESET}"
    ;;

  review)
    echo -e "${BOLD_WHITE}Pending review (2 items):${RESET}"
    echo ""
    echo -e "  ${CYAN}1.${RESET} [claude_md] Always use uv instead of pip for Python package management"
    echo -e "  ${CYAN}2.${RESET} [skill] Pre-PR checklist: run tests, lint, then commit"
    echo ""
    echo -n -e "Enter actions (e.g. ${DIM}1a 2s 3d${RESET}, ${DIM}all:a${RESET}): "
    # VHS will type "1a 2a" via the tape script
    read -r _input
    echo ""
    echo -e "  ${GREEN}Applied:${RESET} #1 claude_md rule added to CLAUDE.md"
    echo -e "  ${GREEN}Applied:${RESET} #2 skill written to .claude/skills/pre-pr-checklist/SKILL.md"
    echo ""
    echo -e "${BOLD_GREEN}2 items applied.${RESET} Shared changes committed to ${CYAN}retro/updates-20260226-091500${RESET}."
    echo -e "Run ${CYAN}retro sync${RESET} after PR is merged."
    ;;

  *)
    echo "Usage: $0 <analyze|patterns|review>"
    exit 1
    ;;
esac
```

**Step 2: Make it executable and test each subcommand**

Run: `chmod +x docs/demo-output.sh`
Run: `./docs/demo-output.sh analyze`
Run: `./docs/demo-output.sh patterns`
Run: `./docs/demo-output.sh review` (type "1a 2a" when prompted)

Verify: output has colors, formatting matches real retro output style.

**Step 3: Commit**

```bash
git add docs/demo-output.sh
git commit -m "docs: add scripted demo output for GIF recording"
```

---

### Task 2: Update the VHS tape script

**Files:**
- Modify: `docs/demo.tape`

Replace the current tape (which runs real retro commands against an empty DB) with one that calls `docs/demo-output.sh` for each scene.

**Step 1: Write the updated tape**

```tape
# VHS tape script for retro hero GIF
# Install: https://github.com/charmbracelet/vhs
# Run: vhs docs/demo.tape

Output docs/demo.gif

Set Shell "bash"
Set FontSize 15
Set Width 1000
Set Height 600
Set Theme "Catppuccin Mocha"
Set Padding 20
Set TypingSpeed 60ms

# Scene 1: Analyze
Sleep 1s
Type "retro analyze"
Enter
Sleep 500ms
Hide
Type "./docs/demo-output.sh analyze"
Enter
Sleep 4s
Show
Sleep 4s

# Scene 2: Patterns
Type "retro patterns"
Enter
Sleep 500ms
Hide
Type "./docs/demo-output.sh patterns"
Enter
Sleep 1s
Show
Sleep 6s

# Scene 3: Review
Type "retro review"
Enter
Sleep 500ms
Hide
Type "./docs/demo-output.sh review"
Enter
Sleep 2s
Show
Sleep 1s
Type "1a 2a"
Enter
Sleep 4s
```

Note: The Hide/Show trick makes VHS hide the actual command being typed (`./docs/demo-output.sh ...`) and only show the fake prompt text (`retro analyze`). We need to verify this works with VHS. If Hide/Show doesn't work as expected, an alternative is to alias `retro` to the script in the shell init.

**Step 2: Test the tape**

Run: `vhs docs/demo.tape`

Verify:
- GIF shows three scenes with correct output
- Typing animation looks natural
- File size is reasonable (under 2MB)
- The "retro analyze" / "retro patterns" / "retro review" commands appear as typed text
- The scripted output appears after each command

**Step 3: If Hide/Show doesn't work, use alias approach**

Alternative tape strategy: set up a shell alias at the start so `retro` calls the demo script.

```tape
Hide
Type "alias retro='./docs/demo-output.sh'"
Enter
Sleep 200ms
Show
```

Then just `Type "retro analyze"` / `Enter` normally.

**Step 4: Commit**

```bash
git add docs/demo.tape docs/demo.gif
git commit -m "docs: update demo GIF with full analyze/patterns/review flow"
```

---

### Task 3: Update README.md intro

**Files:**
- Modify: `README.md:1-11`

**Step 1: Replace the intro**

Replace lines 1-11 (tagline through GIF reference) with:

```markdown
# retro

**Active context curator for AI coding agents.**

You've told your agent "always use uv, not pip" across a dozen sessions. You've corrected the same testing mistake three times this week. Your agent forgets everything between conversations, and you're the one doing the remembering.

Retro fixes this. It analyzes your Claude Code session history, discovers patterns (repeated instructions, recurring mistakes, workflow conventions, explicit directives) and turns them into skills and CLAUDE.md rules automatically. Your agent gets better after every session, without you maintaining its context by hand.

You stay in control: every change goes through a review queue where you approve, skip, or dismiss. Shared changes are proposed as PRs.

![retro demo](docs/demo.gif)
```

**Step 2: Verify the markdown renders correctly**

Read back the file and check formatting.

**Step 3: Commit**

```bash
git add README.md
git commit -m "docs: rewrite README intro with problem/solution/control framing"
```

---

### Task 4: Update README.md Quick Start section

**Files:**
- Modify: `README.md` (Quick Start section, roughly lines 13-39)

**Step 1: Replace the Quick Start section**

```markdown
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
```

**Step 2: Verify markdown renders correctly**

Read back the file and check formatting.

**Step 3: Commit**

```bash
git add README.md
git commit -m "docs: add example output to Quick Start section"
```

---

### Task 5: Update README.md "What Retro Generates" section

**Files:**
- Modify: `README.md` (What Retro Generates section, roughly lines 72-80)

**Step 1: Add concrete examples**

```markdown
## What Retro Generates

**CLAUDE.md rules** are conventions discovered from your sessions, added to a managed section in your project's CLAUDE.md:

```markdown
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
```

**Step 2: Verify markdown renders correctly**

Read back the file and check formatting.

**Step 3: Commit**

```bash
git add README.md
git commit -m "docs: add concrete examples to What Retro Generates section"
```

---

### Task 6: Update README.md Status section

**Files:**
- Modify: `README.md` (Status section, roughly lines 154-173)

**Step 1: Update the Status section**

```markdown
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
```

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: update Status section with current features"
```

---

### Task 7: Final review and push

**Step 1: Read the full README and check for em-dashes**

Search for `—` (em-dash, U+2014) in README.md. Remove any found.

**Step 2: Verify GIF renders on GitHub**

Check `docs/demo.gif` exists and is referenced correctly.

**Step 3: Push all changes**

```bash
git push origin fix/embedded-code-fence-parsing
```
