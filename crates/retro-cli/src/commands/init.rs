use anyhow::Result;
use retro_core::config::{retro_dir, Config};

/// Initialize the v3 personal store: git-backed ~/.retro, global Claude Code
/// hooks (SessionEnd observe, SessionStart brief).
///
/// Ordering matters: layout (writes .gitignore) MUST come before repo init
/// (whose first commit stages everything) — otherwise derived files get
/// permanently committed into the knowledge repo.
pub fn run(from: Option<String>) -> Result<()> {
    use retro_core::store::{git as store_git, index, Store};

    let dir = retro_dir();

    if let Some(remote) = from.as_deref() {
        // Clone path: target must not already be a store.
        if dir.join(".git").exists() || dir.join("knowledge").exists() {
            anyhow::bail!(
                "{} already contains a store — remove it or run plain `retro init`",
                dir.display()
            );
        }
        // ...nor anything else (a live install, say): git clone needs an empty target.
        let is_non_empty = std::fs::read_dir(&dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        if is_non_empty {
            anyhow::bail!(
                "{} is not empty (an existing retro install?) — git clone needs an empty target. Move it aside or run plain `retro init` to adopt it in place",
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

    let config_path = dir.join("config.toml");
    let config = Config::load(&config_path)?;

    // Safety-import: rescue any managed-block rules from the user's global
    // CLAUDE.md that aren't in the store yet, before anything can project
    // over that file. Guards against the first-projection-wipes-pre-v3-rules
    // failure (2026-07-13 dogfood incident). MUST run before index::build
    // (rescued nodes would otherwise leave the index stale — doctor fails
    // right after a fresh init) and gets its own commit (otherwise the
    // rescue lands mislabeled as "user: edit knowledge" on the next run).
    let rescued = retro_core::migrate::safety_import(
        &store,
        &config.claude_dir().join("CLAUDE.md"),
        &retro_core::store::Scope::Global,
        &[],
        false,
    )?;
    if rescued > 0 {
        println!("  Imported {rescued} existing rule(s) from your CLAUDE.md managed section");
        store_git::commit_all(&dir, "retro: import existing rules (init)")?;
    }

    let stats = index::build(&store)?;
    println!("Indexed {} node(s)", stats.nodes);

    // After a rescue the managed block no longer matches the store (the
    // import dedups near-identical bullets) — reproject immediately so a
    // fresh init ends with doctor green, not "projection out of date".
    // Safe precisely because the rescue ran first.
    if rescued > 0 {
        let projected = retro_core::projection::local_md::project_global_md(
            &store,
            &config.claude_dir().join("CLAUDE.md"),
            config.knowledge.confidence_threshold,
            Some(&dir.join("backups")),
        )?;
        println!("  Projected {projected} rule(s) back to the managed section");
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
    println!(
        "\nretro is watching: it learns from your sessions automatically from here on. Run `retro doctor` anytime to verify the setup, and `retro ui` to see what it knows."
    );
    Ok(())
}
