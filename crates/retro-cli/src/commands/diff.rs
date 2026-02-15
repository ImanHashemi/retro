use anyhow::Result;

use super::apply::{run_apply, DisplayMode};

/// `retro diff` — alias for `retro apply --dry-run` with diff-style output.
pub fn run(global: bool, verbose: bool) -> Result<()> {
    run_apply(global, true, DisplayMode::Diff, verbose)
}
