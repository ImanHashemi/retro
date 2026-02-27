use anyhow::{bail, Context, Result};
use colored::Colorize;
use retro_core::analysis::claude_cli::ClaudeCliBackend;
use retro_core::analysis::prompts;
use retro_core::audit_log;
use retro_core::config::{self, Config};
use retro_core::db;
use retro_core::git;
use retro_core::util;

use chrono::Utc;
use std::io::{self, BufRead, Write};

use super::git_root_or_cwd;

pub fn run(dry_run: bool, verbose: bool) -> Result<()> {
    let dir = config::retro_dir();
    let config_path = dir.join("config.toml");
    let db_path = dir.join("retro.db");
    let audit_path = dir.join("audit.jsonl");

    if !db_path.exists() {
        bail!("retro not initialized. Run `retro init` first.");
    }

    let config = Config::load(&config_path)?;

    // Gate: full_management must be enabled
    if !config.claude_md.full_management {
        bail!(
            "retro curate requires full_management mode.\n\
             Enable it in ~/.retro/config.toml:\n\n  \
             [claude_md]\n  \
             full_management = true"
        );
    }

    let project_root = git_root_or_cwd()?;
    let claude_md_path = format!("{project_root}/CLAUDE.md");
    let conn = db::open_db(&db_path)?;

    // Dissolve managed section delimiters if present (full_management mode)
    let backup_dir = dir.join("backups");
    std::fs::create_dir_all(&backup_dir)?;
    if retro_core::projection::dissolve_if_needed(&claude_md_path, &backup_dir)? {
        println!("Dissolved managed section delimiters (full_management mode).");
    }

    // 1. Read current CLAUDE.md
    let claude_md_content = if std::path::Path::new(&claude_md_path).exists() {
        std::fs::read_to_string(&claude_md_path)
            .context("reading CLAUDE.md")?
    } else {
        String::new()
    };
    let claude_md_lines = claude_md_content.lines().count();

    // 2. Load qualifying patterns (confidence >= threshold)
    let threshold = config.analysis.confidence_threshold;
    let patterns = db::get_patterns(&conn, &["discovered", "active"], Some(&project_root))?;
    let qualifying: Vec<_> = patterns
        .into_iter()
        .filter(|p| p.confidence >= threshold)
        .collect();

    // 3. Load MEMORY.md if available
    let memory_md_path = config.claude_dir().join("MEMORY.md");
    let memory_md = if memory_md_path.exists() {
        std::fs::read_to_string(&memory_md_path).ok()
    } else {
        None
    };

    // 4. Generate project tree
    let project_tree = generate_project_tree(&project_root);

    // 5. Show context summary
    println!();
    println!("{}", "retro curate — agentic CLAUDE.md rewrite".cyan().bold());
    println!();
    println!("  {} {} lines", "CLAUDE.md:".white(), claude_md_lines.to_string().cyan());
    println!(
        "  {} {} patterns (confidence >= {:.1})",
        "Patterns:".white(),
        qualifying.len().to_string().cyan(),
        threshold
    );
    if memory_md.is_some() {
        println!("  {} available", "MEMORY.md:".white());
    }
    println!(
        "  {} {} entries",
        "File tree:".white(),
        project_tree.lines().count().to_string().cyan()
    );

    // 6. Dry run: show summary and exit
    if dry_run {
        println!();
        println!(
            "{}",
            "Dry run — no AI calls made. Run `retro curate` to proceed."
                .yellow()
                .bold()
        );
        return Ok(());
    }

    // 7. Auth check
    if !ClaudeCliBackend::is_available() {
        bail!("claude CLI not found on PATH. Install Claude Code CLI to use curate.");
    }
    ClaudeCliBackend::check_auth()?;

    // 8. Build prompt
    let prompt = prompts::build_curate_prompt(
        &claude_md_content,
        &qualifying,
        memory_md.as_deref(),
        &project_tree,
    );

    if verbose {
        eprintln!("[verbose] curate prompt: {} chars", prompt.len());
    }

    // 9. Execute agentic call
    println!();
    println!(
        "{}",
        "Running agentic CLAUDE.md rewrite (AI will explore codebase)...".cyan()
    );
    println!(
        "  {}",
        "This may take several minutes...".dimmed()
    );

    let backend = ClaudeCliBackend::new(&config.ai);
    let response = backend.execute_agentic(&prompt, Some(&project_root))?;

    if verbose {
        eprintln!(
            "[verbose] agentic response: {} chars, {} input tokens, {} output tokens",
            response.text.len(),
            response.input_tokens,
            response.output_tokens
        );
    }

    // 10. Strip code fences from response
    let new_content = util::strip_code_fences(&response.text);

    if new_content.trim().is_empty() {
        bail!("AI returned empty content. Try again.");
    }

    // 11. Show unified diff
    println!();
    show_diff(&claude_md_content, &new_content)?;

    let new_lines = new_content.lines().count();
    println!();
    println!(
        "  {} {} -> {} lines",
        "CLAUDE.md:".white(),
        claude_md_lines.to_string().dimmed(),
        new_lines.to_string().green()
    );

    // 12. Ask for confirmation
    println!();
    println!(
        "{}",
        "Create a PR with this rewrite? You can review and edit the PR on GitHub"
            .white()
    );
    print!(
        "{}",
        "before merging if you want to tweak anything. [y/N] ".white()
    );
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();

    if answer != "y" && answer != "yes" {
        println!("{}", "Discarded.".dimmed());
        // Audit log: curate_rejected
        audit_log::append(
            &audit_path,
            "curate_rejected",
            serde_json::json!({
                "project": &project_root,
                "claude_md_lines_before": claude_md_lines,
                "claude_md_lines_after": new_lines,
                "input_tokens": response.input_tokens,
                "output_tokens": response.output_tokens,
            }),
        )?;
        return Ok(());
    }

    // 13. Create PR: backup, branch, write, commit, push, PR
    let backup_dir = dir.join("backups");
    std::fs::create_dir_all(&backup_dir)?;
    util::backup_file(&claude_md_path, &backup_dir)?;

    let original_branch = git::current_branch()?;
    let default_branch = git::default_branch()
        .context("detecting default branch (is `gh` installed and authenticated?)")?;

    if let Err(e) = git::fetch_branch(&default_branch) {
        eprintln!("  {} fetching {}: {e}", "Warning".yellow(), default_branch);
    }

    let did_stash = git::stash_push().unwrap_or(false);

    let date = Utc::now().format("%Y%m%d-%H%M%S");
    let branch_name = format!("retro/curate-{date}");
    let start_point = format!("origin/{default_branch}");

    if let Err(e) = git::create_branch(&branch_name, Some(&start_point)) {
        if did_stash {
            let _ = git::stash_pop();
        }
        bail!("failed to create branch: {e}");
    }

    // Write new CLAUDE.md
    std::fs::write(&claude_md_path, &new_content)
        .context("writing CLAUDE.md")?;

    let commit_msg = "retro curate: rewrite CLAUDE.md\n\nAgentic rewrite generated by retro curate.";
    if let Err(e) = git::commit_files(&["CLAUDE.md"], commit_msg) {
        // Restore: switch back, pop stash
        let _ = git::checkout_branch(&original_branch);
        if did_stash {
            let _ = git::stash_pop();
        }
        bail!("failed to commit: {e}");
    }

    let pr_url = if git::is_gh_available() {
        if let Err(e) = git::push_current_branch() {
            eprintln!("  {} pushing branch: {e}", "Warning".yellow());
            let _ = git::checkout_branch(&original_branch);
            if did_stash {
                let _ = git::stash_pop();
            }
            bail!("failed to push branch: {e}");
        }

        let title = "retro curate: rewrite CLAUDE.md";
        let body = format!(
            "## Retro Curate — CLAUDE.md Rewrite\n\n\
             Agentic rewrite of CLAUDE.md based on:\n\
             - {} discovered patterns (confidence >= {:.1})\n\
             - Codebase exploration by AI\n\
             {}\n\
             **Lines:** {} -> {}\n\n\
             ---\nGenerated by `retro curate`.",
            qualifying.len(),
            threshold,
            if memory_md.is_some() { "- MEMORY.md context\n" } else { "" },
            claude_md_lines,
            new_lines,
        );

        match git::create_pr(title, &body, &default_branch) {
            Ok(url) => Some(url),
            Err(e) => {
                eprintln!("  {} creating PR: {e}", "Warning".yellow());
                println!(
                    "  {}",
                    format!("Changes committed to branch `{branch_name}`. Create PR manually.")
                        .dimmed()
                );
                None
            }
        }
    } else {
        println!(
            "  {}",
            format!("Changes committed to branch `{branch_name}`.").dimmed()
        );
        println!(
            "  {}",
            "Install `gh` CLI to auto-create PRs, or create one manually.".dimmed()
        );
        None
    };

    // Switch back to original branch
    let _ = git::checkout_branch(&original_branch);
    if did_stash {
        let _ = git::stash_pop();
    }

    // Show result
    if let Some(ref url) = pr_url {
        println!();
        println!("  {} {}", "PR created:".green().bold(), url.cyan().underline());
    }

    // 14. Audit log: curate_applied
    audit_log::append(
        &audit_path,
        "curate_applied",
        serde_json::json!({
            "project": &project_root,
            "pr_url": pr_url,
            "claude_md_lines_before": claude_md_lines,
            "claude_md_lines_after": new_lines,
            "input_tokens": response.input_tokens,
            "output_tokens": response.output_tokens,
            "patterns_used": qualifying.len(),
        }),
    )?;

    Ok(())
}

/// Generate a project file tree using `find`, filtering out common noise directories.
fn generate_project_tree(project_root: &str) -> String {
    let output = std::process::Command::new("find")
        .arg(project_root)
        .args([
            "-not", "-path", "*/.git/*",
            "-not", "-path", "*/target/*",
            "-not", "-path", "*/node_modules/*",
            "-not", "-path", "*/__pycache__/*",
            "-not", "-path", "*/.venv/*",
            "-not", "-path", "*/dist/*",
            "-not", "-path", "*/.next/*",
            "-not", "-name", "*.lock",
            "-not", "-name", "*.pyc",
            "-not", "-path", "*/.git",
            "-type", "f",
        ])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout);
            // Make paths relative to project root
            let prefix = format!("{}/", project_root.trim_end_matches('/'));
            raw.lines()
                .map(|line| line.strip_prefix(&prefix).unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => "(file tree unavailable)".to_string(),
    }
}

/// Show a unified diff between old and new content using the `diff` command.
fn show_diff(old_content: &str, new_content: &str) -> Result<()> {
    let old_path = "/tmp/retro-curate-old.md";
    let new_path = "/tmp/retro-curate-new.md";

    std::fs::write(old_path, old_content).context("writing temp old file")?;
    std::fs::write(new_path, new_content).context("writing temp new file")?;

    let output = std::process::Command::new("diff")
        .args(["-u", old_path, new_path])
        .output()
        .context("running diff")?;

    // diff returns exit code 1 when files differ (not an error)
    let diff_output = String::from_utf8_lossy(&output.stdout);

    if diff_output.trim().is_empty() {
        println!("{}", "(no changes)".dimmed());
    } else {
        // Print custom headers, skip first 2 lines (temp file paths)
        println!("{}", "--- CLAUDE.md (current)".red());
        println!("{}", "+++ CLAUDE.md (proposed)".green());
        for line in diff_output.lines().skip(2) {
            if line.starts_with('+') {
                println!("{}", line.green());
            } else if line.starts_with('-') {
                println!("{}", line.red());
            } else if line.starts_with("@@") {
                println!("{}", line.cyan());
            } else {
                println!("{}", line);
            }
        }
    }

    // Clean up temp files
    let _ = std::fs::remove_file(old_path);
    let _ = std::fs::remove_file(new_path);

    Ok(())
}
