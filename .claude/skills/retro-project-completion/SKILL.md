---
name: retro-project-completion
description: Use when completing retro feature work, after implementation and tests pass, before creating PRs. Keywords: scenario tests, clean install, retro documentation updates.
---

# Retro Project Completion Workflow

When completing feature work on retro:

## 1. Run All Scenario Tests

```bash
# Run the full scenario test suite
cargo test

# If scenarios exist, also run:
./scenarios/run_tests.sh  # or equivalent scenario runner
```

## 2. Update Documentation

- **CLAUDE.md**: Update architecture notes, new commands, design decisions
- **README.md**: Update command tables, usage examples, feature lists
- **scenarios/README.md**: Document any new test scenarios added

## 3. Clean Install Test

Provide commands for clean testing:

```bash
# Remove existing retro data
rm -rf ~/.retro

# Rebuild and test fresh install
cargo build --release
./target/release/retro init
```

## 4. Create PR

After verification:
- Commit all changes with clear message
- Push branch
- Create PR with description covering: what changed, why, testing performed