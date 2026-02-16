---
name: run-scenarios
description: >
  Use when the user wants to run scenario tests, validate retro CLI behavior,
  check for regressions, or verify fixes. Also use when the user says
  "run scenarios", "test scenarios", or "run the tests".
argument-hint: "[scenario-file.md]"
---

# Run Scenarios

Execute end-to-end scenario tests for the `retro` CLI by dispatching sub-agents.

## Step 1: Build

Run `source ~/.cargo/env 2>/dev/null; cargo build` via Bash. If it fails, stop.

## Step 2: Discover scenarios

- If `$ARGUMENTS` names a specific file, use only that file.
- Otherwise, use Glob to find all `*.md` files in `scenarios/` excluding `README.md`.

## Step 3: Dispatch sub-agents

For each scenario file, use the **Task tool** to launch a `general-purpose` sub-agent. If scenarios are independent (no shared state between them), launch them **in parallel** using multiple Task tool calls in a single message.

The prompt for each sub-agent must include:

1. The full contents of the scenario file (read it first, then paste into the prompt)
2. These execution instructions:

```
You are a scenario test runner. Execute this scenario and return ONLY a result line.

SCENARIO FILE: <paste full scenario content here>

INSTRUCTIONS:
- Extract shell commands from backtick-quoted strings in Setup and Steps sections.
- If Setup contains only prose (e.g., "None needed"), skip it.
- Run Setup commands first via Bash. If any fail, return: ERROR: <scenario title> — setup failed: <error>
- Run each Steps command via Bash. Append `| cat` to retro commands to disable ANSI colors.
- Binary path: ./target/debug/retro
- For retro commands that invoke Claude CLI internally (analyze/apply/audit WITHOUT --dry-run), prefix with `unset CLAUDECODE &&`
- Commands with --dry-run do NOT need unset CLAUDECODE.
- Run from the repo root: /home/claude/repositories/retro
- After running all steps, evaluate Expected and Not Expected conditions against captured output.
- Expected conditions MUST all be true. Not Expected conditions must NOT appear.

RETURN FORMAT — return exactly one of these as your final answer:
  PASS: <scenario title>
  FAIL: <scenario title> — <bullet list of failures>
  ERROR: <scenario title> — <error description>
  SKIP: <scenario title> — <reason>

Use SKIP when a scenario needs a real AI call and Claude CLI is unavailable.
```

## Step 4: Collect results and report

After all sub-agents return, collect their result lines and print a report:

```
[PASS] Scenario: Init is idempotent
[FAIL] Scenario: Token counts not dollars
  - FAIL: Found "cost" in analyze.rs
[PASS] Scenario: Audit dry-run skips AI

========================================
Scorecard: 2/3 scenarios passed
========================================
```

Map each sub-agent's result: PASS → `[PASS]`, FAIL → `[FAIL]` with details, ERROR → `[ERROR]`, SKIP → `[SKIP]`.
