use anyhow::Result;

use super::apply::{run_apply, DisplayMode};

/// `retro diff` — alias for `retro apply --dry-run` with diff-style output.
/// Deprecated: use `retro run --dry-run` instead.
pub fn run(global: bool, verbose: bool) -> Result<()> {
    super::warn_command_deprecated("diff", "retro run --dry-run");
    run_apply(global, true, false, DisplayMode::Diff, verbose)
}
