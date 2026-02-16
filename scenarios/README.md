# Scenario Tests

End-to-end test scenarios for the `retro` CLI. Each `.md` file describes one test scenario that the `/run-scenarios` skill executes.

## Format

```markdown
# Scenario: Short Title

## Description
What this scenario tests and why.

## Setup
Commands to ensure preconditions (run before test steps).

## Steps
1. Run `command here`
2. Run `another command`

## Expected
- Output contains "some text"
- Command exits successfully

## Not Expected
- No "error" or panic in output
```

## Sections

| Section | Required | Purpose |
|---------|----------|---------|
| Description | Yes | Context for the test |
| Setup | No | Idempotent preconditions |
| Steps | Yes | Commands to execute |
| Expected | Yes | Conditions that must be true |
| Not Expected | No | Conditions that must NOT be true |

## Adding a scenario

1. Create a new `.md` file in this directory
2. Follow the format above
3. Run `/run-scenarios scenarios/your-file.md` to test it
4. Run `/run-scenarios` to run all scenarios

## Tips

- Scenarios that use `--dry-run` don't make AI calls and are fast/free
- Setup should be idempotent (safe to run multiple times)
- Expected/Not Expected are evaluated by the AI agent using natural language judgment
- For idempotency tests, the Steps section should run the command twice
