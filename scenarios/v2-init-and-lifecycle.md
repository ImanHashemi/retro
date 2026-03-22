# Scenario: v2 Init and Full Lifecycle

## Description

Tests the complete Retro 2.0 lifecycle from fresh install to uninstall. Verifies that `retro init` creates all v2 artifacts (database, config, launchd plist, briefing skill, briefings directory), that `retro start`/`retro stop` manage the scheduled runner, and that `retro init --uninstall --purge` cleans up everything.

This is the primary happy-path test for Retro 2.0.

## Setup

```bash
# Clean slate — remove any existing retro installation
retro init --uninstall --purge 2>/dev/null || true
cargo build
```

## Steps

1. Run `cargo run -- init` and capture output
2. Check that `~/.retro/retro.db` exists: `test -f ~/.retro/retro.db && echo "DB exists"`
3. Check that `~/.retro/config.toml` exists: `test -f ~/.retro/config.toml && echo "Config exists"`
4. Check that `~/.retro/briefings/` directory exists: `test -d ~/.retro/briefings && echo "Briefings dir exists"`
5. Check that launchd plist was created: `test -f ~/Library/LaunchAgents/com.retro.runner.plist && echo "Plist exists"`
6. Check that briefing skill was created: `test -f .claude/skills/retro-briefing.md && echo "Briefing skill exists"`
7. Read the briefing skill content: `cat .claude/skills/retro-briefing.md`
8. Run `cargo run -- status` and capture output
9. Run `cargo run -- stop` to unload the launchd job
10. Run `cargo run -- start` to reload the launchd job
11. Run `cargo run -- stop` to clean up
12. Run `cargo run -- init --uninstall --purge` to fully remove
13. Check that `~/.retro/` is gone: `test -d ~/.retro && echo "Still exists" || echo "Cleaned up"`
14. Check that plist is gone: `test -f ~/Library/LaunchAgents/com.retro.runner.plist && echo "Still exists" || echo "Cleaned up"`

## Expected

- `retro init` output contains "retro initialized successfully" or similar success message
- `retro init` output mentions "Started" for the scheduled runner
- `retro init` output mentions "Created" for the briefing skill
- DB, config, briefings directory, plist, and briefing skill all exist after init
- The briefing skill file contains "retro-briefing" and "briefings" in its content
- `retro status` runs without error and shows database information
- `retro stop` output contains "Stopped"
- `retro start` output contains "Started" and mentions an interval
- `retro init --uninstall --purge` removes `~/.retro/` and the plist file
- Final checks show "Cleaned up" for both paths

## Not Expected

- No panics, crashes, or "error:" messages in any step
- No "not initialized" errors after init
