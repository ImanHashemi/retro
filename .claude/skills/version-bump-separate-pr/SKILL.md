---
name: version-bump-separate-pr
description: Use when preparing to release a new version, when the user mentions "publish", "cargo publish", "version bump", or when changes to Cargo.toml version fields are needed.
---

When preparing to publish a new version to crates.io:

1. **Create a separate branch for the version bump:**
   ```bash
   git checkout -b release/v<VERSION>
   ```

2. **Update version in all three Cargo.toml files:**
   - `Cargo.toml` (workspace root)
   - `crates/retro-cli/Cargo.toml`
   - `crates/retro-core/Cargo.toml`
   
   Ensure versions match across all three files.

3. **Commit and create PR:**
   ```bash
   git add Cargo.toml crates/*/Cargo.toml
   git commit -m "chore: bump version to <VERSION>"
   git push -u origin release/v<VERSION>
   gh pr create --title "chore: bump version to <VERSION>" --body "Version bump for release"
   ```

4. **Wait for explicit merge confirmation from user** before proceeding.

5. **Only after PR is merged**, provide the cargo publish commands:
   ```bash
   # Publish retro-core first (dependency)
   cd crates/retro-core && cargo publish
   
   # Wait for crates.io to process, then publish retro-cli
   cd ../retro-cli && cargo publish
   ```

Never run `cargo publish` or suggest it until the version bump PR has been explicitly confirmed as merged by the user.