use anyhow::Result;
use colored::Colorize;
use retro_core::git;

use super::git_root_or_cwd;

pub fn run_remove() -> Result<()> {
    if !git::is_in_git_repo() {
        anyhow::bail!("not inside a git repository");
    }

    let repo_root = git_root_or_cwd()?;

    println!("{}", "Removing retro git hooks...".cyan());

    let modified = git::remove_hooks(&repo_root)?;

    if modified.is_empty() {
        println!("{}", "No retro hooks found.".yellow());
    } else {
        for hook in &modified {
            println!("  {} {}", "Removed".green(), hook);
        }
        println!();
        println!("{}", "Hooks removed successfully.".green().bold());
    }

    Ok(())
}
