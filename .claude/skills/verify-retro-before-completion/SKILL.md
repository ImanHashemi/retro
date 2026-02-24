---
name: verify-retro-before-completion
description: Use when marking work complete, creating releases, before merging branches, or when the user asks to verify the retro codebase works end-to-end.
---

Before marking work complete or creating releases, verify the retro codebase:

1. Run the scenario test suite: `cargo test --test scenarios`
2. Do a clean install test:
   - Remove existing retro installation: `cargo uninstall retro-cli` (if installed)
   - Clean build artifacts: `cargo clean`
   - Fresh build: `cargo build --release`
   - Install: `cargo install --path crates/retro-cli`
   - Verify: `retro --version`
3. Test basic end-to-end workflow:
   - `retro init` in a test repository
   - `retro ingest`
   - `retro status`
4. Only proceed with release/merge if all checks pass