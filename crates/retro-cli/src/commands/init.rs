use anyhow::{Context, Result};
use colored::Colorize;
use retro_core::config::{retro_dir, Config};
use retro_core::db;
use retro_core::git;
use retro_core::git::HookInstallResult;
use retro_core::util::shorten_path_buf;

use super::git_root_or_cwd;

pub fn run(uninstall: bool, purge: bool, verbose: bool, v3: bool, from: Option<String>) -> Result<()> {
    if v3 || from.is_some() {
        return init_v3(from.as_deref());
    }

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
        let project_id = db::ensure_project_registered(&conn, &repo_root)?;
        println!("  {} project: {} ({})", "Registered".green(), project_id, repo_root);

        install_briefing_hook(&repo_root)?;

        // Reconcile existing CLAUDE.md rules into the DB
        let project_claude_md = format!("{}/CLAUDE.md", repo_root);
        let home = std::env::var("HOME").unwrap_or_default();
        let global_claude_md = format!("{}/.claude/CLAUDE.md", home);

        let mut total_imported = 0usize;

        // Project-scoped rules
        if let Ok(result) = retro_core::reconcile::reconcile_claude_md(
            &conn,
            &project_claude_md,
            &retro_core::models::NodeScope::Project,
            Some(&project_id),
        ) {
            total_imported += result.imported;
        }

        // Global rules
        if let Ok(result) = retro_core::reconcile::reconcile_claude_md(
            &conn,
            &global_claude_md,
            &retro_core::models::NodeScope::Global,
            None,
        ) {
            total_imported += result.imported;
        }

        if total_imported > 0 {
            println!("  {} {} rules from CLAUDE.md", "Reconciled".green(), total_imported);
        }

        // Hint about superpowers plugin for skill generation
        if !retro_core::projection::skill::is_superpowers_installed() {
            println!(
                "  {} install the superpowers plugin for automatic skill generation",
                "Note:".dimmed()
            );
        }
    }

    println!();
    println!("{}", "retro initialized successfully".green().bold());
    println!("  Run {} to open the dashboard", "retro dash".cyan());

    Ok(())
}

/// v3 initialization. Ordering matters: layout (writes .gitignore) MUST come
/// before repo init (whose first commit stages everything) — otherwise
/// derived files get permanently committed into the knowledge repo.
fn init_v3(from: Option<&str>) -> Result<()> {
    use retro_core::store::{git as store_git, index, Store};

    let dir = retro_dir();

    if let Some(remote) = from {
        // Clone path: target must not already be a store.
        if dir.join(".git").exists() || dir.join("knowledge").exists() {
            anyhow::bail!(
                "{} already contains a store — remove it or run plain `retro init --v3`",
                dir.display()
            );
        }
        // ...nor anything else (a live v2 install, say): git clone needs an empty target.
        let is_non_empty = std::fs::read_dir(&dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        if is_non_empty {
            anyhow::bail!(
                "{} is not empty (an existing retro install?) — git clone needs an empty target. Move it aside or run plain `retro init --v3` to adopt it in place",
                dir.display()
            );
        }
        std::fs::create_dir_all(dir.parent().unwrap_or(&dir))?;
        let status = std::process::Command::new("git")
            .args(["clone", remote, &dir.display().to_string()])
            .status()?;
        anyhow::ensure!(status.success(), "git clone failed");
        // Clone bypasses ensure_repo's create branch: apply local config explicitly.
        store_git::apply_local_config(&dir)?;
        println!("Cloned knowledge store from {remote}");
    }

    // Re-running init over a live install (hooks already firing) must not
    // interleave store writes with a runner pass — same lock discipline as
    // migrate and the dashboard write handlers. Fresh machines have no
    // retro dir yet, and the lock file needs a parent to live in.
    std::fs::create_dir_all(&dir)?;
    let Some(_lock) = retro_core::lock::LockFile::try_acquire(&dir.join("run.lock")) else {
        anyhow::bail!("a retro run is in progress — retry shortly");
    };
    let store = Store::open(&dir);
    store.ensure_layout()?; // BEFORE ensure_repo — see doc comment
    let created = store_git::ensure_repo(&dir)?;
    if created {
        println!("Initialized knowledge store repo at {}", dir.display());
    }
    // Unconditionally (ensure_repo early-returns for existing repos): covers
    // pre-existing store dirs created before machine-local excludes existed.
    store_git::apply_local_config(&dir)?;
    let stats = index::build(&store)?;
    println!("Indexed {} node(s)", stats.nodes);

    // Global hooks in ~/.claude/settings.json (absolute binary path — hooks
    // run outside any shell profile, PATH is not guaranteed).
    let config_path = dir.join("config.toml");
    // Load-modify-save: Config captures all known sections; hand-added unknown keys/comments are dropped (acceptable — config is retro-owned).
    let mut config = Config::load(&config_path)?;

    // Safety-import: rescue any managed-block rules from the user's global
    // CLAUDE.md that aren't in the store yet, before anything can project
    // over that file. Guards against the first-projection-wipes-pre-v3-rules
    // failure (2026-07-13 dogfood incident).
    let rescued = retro_core::migrate::safety_import(
        &store,
        &config.claude_dir().join("CLAUDE.md"),
        &retro_core::store::Scope::Global,
        &[],
        false,
    )?;
    if rescued > 0 {
        println!("  Imported {rescued} existing rule(s) from your CLAUDE.md managed section");
    }
    let exe = std::env::current_exe()?.display().to_string();
    let settings_path = config.claude_dir().join("settings.json");
    if settings_path.exists() {
        retro_core::util::backup_file(&settings_path.display().to_string(), &dir.join("backups"))?;
    }
    let existing: serde_json::Value = match std::fs::read_to_string(&settings_path) {
        Ok(content) => serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("cannot parse {}: {e}", settings_path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => anyhow::bail!("cannot read {}: {e}", settings_path.display()),
    };
    let with_end =
        retro_core::claude_settings::ensure_hook(existing, "SessionEnd", &format!("{exe} observe"))?;
    let with_both =
        retro_core::claude_settings::ensure_hook(with_end, "SessionStart", &format!("{exe} brief"))?;
    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&settings_path, serde_json::to_string_pretty(&with_both)?)?;
    println!("Installed v3 hooks in {}", settings_path.display());

    // Enable the gate.
    config.v3.enabled = true;
    config.save(&config_path)?;
    println!("v3 pipeline enabled");

    // Backup remote (skip when cloning — a remote already exists).
    if from.is_none() && !store_git::has_remote(&dir) {
        print!("Back up your knowledge to a private GitHub repo? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim().eq_ignore_ascii_case("y") {
            match std::process::Command::new("gh")
                .args(["repo", "create", "retro-knowledge", "--private"])
                .status()
            {
                Err(_) => println!(
                    "gh CLI not found — skipping backup setup; add a remote later with: git -C {} remote add origin <url>",
                    dir.display()
                ),
                Ok(status) if status.success() => {
                    let user_out = std::process::Command::new("gh")
                        .args(["api", "user", "-q", ".login"])
                        .output()?;
                    let user = String::from_utf8_lossy(&user_out.stdout).trim().to_string();
                    anyhow::ensure!(!user.is_empty(), "could not determine GitHub username");
                    let url = format!("git@github.com:{user}/retro-knowledge.git");
                    let st = std::process::Command::new("git")
                        .args(["-C", &dir.display().to_string(), "remote", "add", "origin", &url])
                        .status()?;
                    anyhow::ensure!(st.success(), "git remote add failed");
                    match store_git::push_best_effort(&dir) {
                        store_git::PushOutcome::Pushed => {
                            println!("Backed up to {url}")
                        }
                        outcome => println!("Remote added ({url}); first push pending: {outcome:?}"),
                    }
                }
                Ok(_) => println!(
                    "gh repo create failed — you can add a remote later with git -C {} remote add origin <url>",
                    dir.display()
                ),
            }
        }
    }
    println!("\nNote: smoke-test with one session before relying on it — see the rollout section of the plan.");
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
