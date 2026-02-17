# Contributing to Retro

Thanks for your interest in contributing!

## Setup

Requires the [Rust toolchain](https://rustup.rs/) and a C compiler (`build-essential` on Ubuntu) for bundled SQLite.

```sh
git clone https://github.com/ImanHashemi/retro
cd retro
cargo build
cargo test
```

## Project Structure

Cargo workspace with two crates:

- `crates/retro-core/` — library crate with all logic (ingestion, analysis, projection, DB, etc.)
- `crates/retro-cli/` — binary crate (`retro`) with clap commands

See [CLAUDE.md](CLAUDE.md) for architecture details, key design decisions, and coding conventions.

## Running Tests

```sh
cargo test
```

All tests are unit tests using fixtures. No AI calls or network access required.

## Code Style

- `thiserror` for errors in retro-core, `anyhow` in retro-cli
- No async, no tokio. Sync `std::process::Command` for subprocesses.
- Shell out to `git` and `gh` directly (no git2 crate)
- All domain types live in `retro-core/src/models.rs`
- All DB operations live in `retro-core/src/db.rs`

## Submitting Changes

1. Fork the repo and create a branch
2. Make your changes
3. Run `cargo test` and `cargo clippy`
4. Open a PR with a clear description of what you changed and why

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
