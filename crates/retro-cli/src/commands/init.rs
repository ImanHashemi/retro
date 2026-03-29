use anyhow::{Context, Result};
use colored::Colorize;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::git;
use retro_core::git::HookInstallResult;
use retro_core::util::shorten_path_buf;

use super::git_root_or_cwd;

pub fn run(uninstall: bool, purge: bool, verbose: bool) -> Result<()> {
    if uninstall {
        return run_uninstall(purge, verbose);
    }

    let dir = retro_dir();

    // Create ~/.retro/ directory structure
    std::fs::create_dir_all(&dir).context("creating ~/.retro/")?;
    std::fs::create_dir_all(dir.join("backups")).context("creating ~/.retro/backups/")?;

    // Truncate hook stderr log on fresh init
    let hook_stderr_path = dir.join("hook-stderr.log");
    if hook_stderr_path.exists() {
        let _ = std::fs::write(&hook_stderr_path, "");
    }

    // Create config.toml if it doesn't exist
    let config_path = dir.join("config.toml");
    if !config_path.exists() {
        let config = Config::default();
        config.save(&config_path)?;
        println!("  {} {}", "Created".green(), shorten_path_buf(&config_path));
    } else {
        println!("  {} {}", "Exists".yellow(), shorten_path_buf(&config_path));
    }

    // Initialize database with WAL mode
    let db_path = dir.join("retro.db");
    let db_existed = db_path.exists();
    let conn = db::open_db(&db_path)?;

    if verbose {
        eprintln!("[verbose] retro dir: {}", dir.display());
    }

    let is_wal = db::verify_wal_mode(&conn)?;
    let label = if db_existed { "Exists" } else { "Created" };
    let color_label = if db_existed {
        label.yellow()
    } else {
        label.green()
    };
    if is_wal {
        println!("  {} {} (WAL mode)", color_label, shorten_path_buf(&db_path));
    } else {
        println!(
            "  {} {} (warning: WAL mode not enabled)",
            label.yellow(),
            shorten_path_buf(&db_path)
        );
    }

    // Install git hooks if in a repo
    if git::is_in_git_repo() {
        let repo_root = git_root_or_cwd()?;
        match git::install_hooks(&repo_root) {
            Ok(results) => {
                for (hook, result) in &results {
                    match result {
                        HookInstallResult::Installed => {
                            println!("  {} git hook: {} (ingest → analyze → apply)", "Installed".green(), hook);
                        }
                        HookInstallResult::Updated => {
                            println!("  {} git hook: {} (ingest → analyze → apply)", "Updated".green(), hook);
                        }
                        HookInstallResult::UpToDate => {
                            println!("  {} git hook: {} (already up to date)", "Exists".yellow(), hook);
                        }
                    }
                }
            }
            Err(e) => {
                println!(
                    "  {} could not install git hooks: {e}",
                    "Warning".yellow()
                );
            }
        }
    } else {
        println!("  {} not in a git repository, skipping hooks", "Note".dimmed());
    }

    // Create briefings directory
    let briefings_dir = dir.join("briefings");
    if !briefings_dir.exists() {
        std::fs::create_dir_all(&briefings_dir).context("creating briefings dir")?;
        println!("  {} {}", "Created".green(), shorten_path_buf(&briefings_dir));
    }

    // Install launchd scheduled runner (macOS only)
    if cfg!(target_os = "macos") {
        let config = Config::load(&config_path)?;
        match crate::launchd::install_and_load(&config) {
            Ok(()) => {
                println!("  {} scheduled runner (every {}s)", "Started".green(), config.runner.interval_seconds);
            }
            Err(e) => {
                println!("  {} could not start scheduled runner: {e}", "Warning".yellow());
            }
        }
    }

    // Register project and install briefing hook
    if git::is_in_git_repo() {
        let repo_root = git_root_or_cwd()?;

        // Register project in the DB so the watcher knows about it
        let project_id = db::generate_unique_project_slug(&conn, &repo_root)?;
        let project = retro_core::models::KnowledgeProject {
            id: project_id.clone(),
            path: repo_root.clone(),
            remote_url: git::remote_url(),
            agent_type: "claude_code".to_string(),
            last_seen: chrono::Utc::now(),
        };
        db::upsert_project(&conn, &project)?;
        println!("  {} project: {} ({})", "Registered".green(), project_id, repo_root);

        install_briefing_hook(&repo_root)?;
    }

    println!();
    println!("{}", "retro initialized successfully".green().bold());
    println!("  Run {} to open the dashboard", "retro dash".cyan());

    Ok(())
}

/// Install the SessionStart hook that delivers briefing content at session start.
/// Creates .claude/hooks/retro-briefing.sh and adds the hook to .claude/settings.local.json.
fn install_briefing_hook(project_root: &str) -> Result<()> {
    let root = std::path::Path::new(project_root);
    let hooks_dir = root.join(".claude").join("hooks");
    let hook_path = hooks_dir.join("retro-briefing.sh");
    let settings_path = root.join(".claude").join("settings.local.json");

    // Write hook script
    std::fs::create_dir_all(&hooks_dir).context("creating .claude/hooks/")?;

    let hook_script = r#"#!/bin/bash
# Retro session briefing hook — installed by `retro init`
# Reads the project briefing file and injects it as context at session start.
PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$(pwd)}"
PROJECT_SLUG=$(basename "$PROJECT_DIR" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9-]/-/g')
BRIEFING_FILE="$HOME/.retro/briefings/${PROJECT_SLUG}.md"
if [ -f "$BRIEFING_FILE" ]; then
    cat "$BRIEFING_FILE"
fi
exit 0
"#;

    if hook_path.exists() {
        println!("  {} briefing hook: {}", "Exists".yellow(), hook_path.display());
    } else {
        std::fs::write(&hook_path, hook_script).context("writing briefing hook")?;
        // Make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&hook_path, perms).context("setting hook permissions")?;
        }
        println!("  {} briefing hook: {}", "Created".green(), hook_path.display());
    }

    // Add hook to settings.local.json if not already present
    let hook_entry = r#""SessionStart""#;
    let settings_content = if settings_path.exists() {
        std::fs::read_to_string(&settings_path).unwrap_or_default()
    } else {
        String::new()
    };

    if settings_content.contains(hook_entry) {
        // Already has a SessionStart hook configured
    } else {
        // Read existing JSON or create new
        let mut json: serde_json::Value = if settings_content.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(&settings_content).unwrap_or(serde_json::json!({}))
        };

        // Add hooks.SessionStart
        let obj = json
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("settings.local.json is not a JSON object"))?;
        let hooks = obj
            .entry("hooks")
            .or_insert(serde_json::json!({}));
        let hooks_obj = hooks
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("settings.local.json 'hooks' field is not a JSON object"))?;
        hooks_obj.insert(
            "SessionStart".to_string(),
            serde_json::json!([{
                "matcher": "",
                "hooks": [{
                    "type": "command",
                    "command": ".claude/hooks/retro-briefing.sh"
                }]
            }]),
        );

        let pretty = serde_json::to_string_pretty(&json)?;
        std::fs::write(&settings_path, &pretty).context("writing settings.local.json")?;
        println!("  {} SessionStart hook in settings", "Installed".green());
    }

    Ok(())
}

fn run_uninstall(purge: bool, _verbose: bool) -> Result<()> {
    println!("{}", "Uninstalling retro...".cyan());

    // Unload and remove launchd plist (macOS only)
    if cfg!(target_os = "macos") {
        let _ = crate::launchd::unload();
        let plist = crate::launchd::plist_path();
        if plist.exists() {
            let _ = std::fs::remove_file(&plist);
            println!("  {} launchd plist", "Removed".green());
        }
    }

    // Remove git hooks from current repo
    if git::is_in_git_repo() {
        let repo_root = git_root_or_cwd()?;
        match git::remove_hooks(&repo_root) {
            Ok(modified) => {
                if modified.is_empty() {
                    println!("  {} no retro hooks found", "Note".dimmed());
                } else {
                    for hook in &modified {
                        println!("  {} hook: {}", "Removed".green(), hook);
                    }
                }
            }
            Err(e) => {
                println!("  {} removing hooks: {e}", "Warning".yellow());
            }
        }
    }

    if purge {
        let dir = retro_dir();
        if dir.exists() {
            std::fs::remove_dir_all(&dir).context("removing ~/.retro/")?;
            println!("  {} {}", "Deleted".green(), shorten_path_buf(&dir));
        }
        println!();
        println!(
            "{}",
            "retro fully uninstalled (hooks removed, data purged)."
                .green()
                .bold()
        );
    } else {
        println!();
        println!(
            "{}",
            "retro hooks removed. Data preserved in ~/.retro/."
                .green()
                .bold()
        );
        println!(
            "  Use {} to also delete all retro data.",
            "retro init --uninstall --purge".cyan()
        );
    }

    Ok(())
}
