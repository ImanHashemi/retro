use anyhow::Result;
use retro_core::config::{Config, retro_dir};

/// Atomic write: tmp sibling + rename, matching the projector's discipline
/// for these same files (a crash mid-write must not truncate settings.json
/// or a CLAUDE.md).
fn write_atomic(path: &std::path::Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("retro-tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Remove retro from the machine: Claude Code hooks, projected content,
/// v1/v2 remnants. The store survives unless --purge (typed confirmation).
pub fn run(purge: bool) -> Result<()> {
    let dir = retro_dir();
    // A corrupt config must not block uninstall — defaults suffice here.
    let config = Config::load(&dir.join("config.toml")).unwrap_or_default();
    let claude_dir = config.claude_dir();
    // Projection files race the runner — same lock discipline as migrate.
    // The dir may be gone after a prior --purge; recreate so reruns stay
    // idempotent (and --purge below re-removes it).
    std::fs::create_dir_all(&dir)?;
    let Some(_lock) = retro_core::lock::LockFile::try_acquire(&dir.join("run.lock")) else {
        anyhow::bail!("a retro run is in progress — retry shortly");
    };
    let backups = dir.join("backups");

    // 1. Stop everything that could regenerate content mid-uninstall:
    //    Claude Code hooks out of settings.json (backup first), then the
    //    legacy v2 launchd runner.
    let settings_path = claude_dir.join("settings.json");
    if settings_path.exists() {
        retro_core::util::backup_file(&settings_path.display().to_string(), &backups)?;
        let raw = std::fs::read_to_string(&settings_path)?;
        let mut settings: serde_json::Value = serde_json::from_str(&raw)?;
        let a =
            retro_core::claude_settings::remove_retro_hook(&mut settings, "SessionEnd", "observe");
        let b =
            retro_core::claude_settings::remove_retro_hook(&mut settings, "SessionStart", "brief");
        if a || b {
            write_atomic(&settings_path, &serde_json::to_string_pretty(&settings)?)?;
            println!(
                "  removed SessionEnd/SessionStart hooks from {}",
                settings_path.display()
            );
        }
    }
    if cfg!(target_os = "macos") && retro_core::migrate::remove_v2_launchd() {
        println!("  removed v2 launchd runner");
    }

    // 2. Projected content. Managed blocks are stripped, never whole files:
    //    both the global CLAUDE.md and per-project CLAUDE.local.md can hold
    //    user-authored content outside the block. A CLAUDE.local.md that is
    //    empty after stripping was retro's alone and gets removed.
    let global_md = claude_dir.join("CLAUDE.md");
    if global_md.exists() {
        retro_core::util::backup_file(&global_md.display().to_string(), &backups)?;
        let content = std::fs::read_to_string(&global_md)?;
        let stripped = retro_core::projection::claude_md::strip_managed_section(&content);
        if stripped != content {
            write_atomic(&global_md, &stripped)?;
            println!("  removed managed section from {}", global_md.display());
        }
    }
    if let Ok(map) = retro_core::store::projects::PathMap::load(&dir) {
        for (slug, path) in &map.paths {
            let local = std::path::Path::new(path).join("CLAUDE.local.md");
            if !local.exists() {
                continue;
            }
            retro_core::util::backup_file(&local.display().to_string(), &backups)?;
            let content = std::fs::read_to_string(&local)?;
            let stripped = retro_core::projection::claude_md::strip_managed_section(&content);
            if stripped.trim().is_empty() {
                if std::fs::remove_file(&local).is_ok() {
                    println!("  removed {} ({slug})", local.display());
                }
            } else if stripped != content {
                write_atomic(&local, &stripped)?;
                println!(
                    "  removed managed section from {} ({slug}, user content kept)",
                    local.display()
                );
            }
            // Drop the CLAUDE.local.md ignore line retro added to the repo's
            // info/exclude; failure is non-fatal (read-only repo, etc.).
            let _ = retro_core::projection::local_md::remove_git_exclude(std::path::Path::new(
                path,
            ));
        }
    }

    // 3. v1/v2 remnants across every known project (idempotent, tolerate
    //    absence): v1 git hooks and the v2 per-project briefing hook.
    for p in retro_core::migrate::all_known_project_paths(&dir) {
        for h in retro_core::migrate::remove_v1_hooks(&p) {
            println!("  removed v1 {h} hook in {p}");
        }
        let project = std::path::Path::new(&p);
        let briefing = project.join(".claude/hooks/retro-briefing.sh");
        if briefing.exists() && std::fs::remove_file(&briefing).is_ok() {
            println!("  removed v2 briefing hook in {p}");
        }
        let local_settings = project.join(".claude/settings.local.json");
        if let Ok(raw) = std::fs::read_to_string(&local_settings) {
            if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if retro_core::claude_settings::remove_hooks_containing(
                    &mut v,
                    "SessionStart",
                    "retro-briefing.sh",
                ) {
                    write_atomic(&local_settings, &serde_json::to_string_pretty(&v)?)?;
                    println!(
                        "  removed v2 briefing hook entry from {}",
                        local_settings.display()
                    );
                }
            }
        }
    }

    // 4. The store itself — only with --purge, only with explicit consent.
    if purge {
        // This run's backups live INSIDE the store — move them out first so
        // the purge doesn't destroy its own safety net.
        let rescue = dirs_home().join(format!(
            ".retro-uninstall-backups-{}",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        ));
        println!(
            "\n  --purge deletes {} INCLUDING its git history (unpushed knowledge is unrecoverable).",
            dir.display()
        );
        println!(
            "  Backups taken by this run will be preserved at {}",
            rescue.display()
        );
        print!("  Type 'yes' to confirm: ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if answer.trim() == "yes" {
            if backups.exists() {
                if let Err(e) = std::fs::rename(&backups, &rescue) {
                    println!("  warning: could not move backups out ({e}) — purging anyway");
                }
            }
            std::fs::remove_dir_all(&dir)?;
            println!("  removed {}", dir.display());
        } else {
            println!("  purge cancelled");
        }
    } else {
        println!(
            "\n  store kept at {} (use --purge to delete it)",
            dir.display()
        );
    }
    println!("\nretro uninstalled.");
    Ok(())
}

/// $HOME, falling back to the temp dir (the rescue location must exist
/// somewhere OUTSIDE the store being purged).
fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}
