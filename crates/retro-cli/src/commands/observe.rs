use std::io::Read;

use anyhow::Result;
use retro_core::config::{Config, retro_dir};
use retro_core::health;
use retro_core::hook_event::HookEvent;
use retro_core::store::state::RunnerState;
use retro_core::store::{Store, projects, queue};

/// What observe_event did with the session (drives the health record).
enum ObserveOutcome {
    Enqueued,
    Excluded,
}

/// SessionEnd hook entry. Contract: NEVER fail the hook — errors are recorded
/// in health and swallowed; stdout stays clean; exit code is always 0.
pub fn run() -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml")).unwrap_or_default();
    if !config.v3.enabled {
        return Ok(());
    }
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Some(event) = HookEvent::parse(&input) else {
        let _ = health::record(&dir, "observe", false, "unparseable hook event");
        return Ok(());
    };
    match observe_event(&dir, &config, &event) {
        Err(e) => {
            let _ = health::record(&dir, "observe", false, &e.to_string());
        }
        Ok(ObserveOutcome::Excluded) => {
            let _ = health::record(
                &dir,
                "observe",
                true,
                &format!("excluded {}", event.session_id),
            );
        }
        Ok(ObserveOutcome::Enqueued) => {
            let _ = health::record(
                &dir,
                "observe",
                true,
                &format!("enqueued {}", event.session_id),
            );
            spawn_worker();
        }
    }
    Ok(())
}

fn observe_event(
    dir: &std::path::Path,
    config: &Config,
    event: &HookEvent,
) -> Result<ObserveOutcome, retro_core::errors::CoreError> {
    if projects::is_excluded(&event.cwd, &config.privacy.exclude_projects)
        || projects::is_store_dir(dir, &event.cwd)
    {
        return Ok(ObserveOutcome::Excluded);
    }
    let store = Store::open(dir);
    store.ensure_layout()?;

    queue::enqueue(
        dir,
        &queue::QueueEntry {
            session_id: event.session_id.clone(),
            transcript_path: event.transcript_path.clone(),
            cwd: Some(event.cwd.clone()),
            enqueued_at: chrono::Utc::now().to_rfc3339(),
        },
    )?;

    let mut state = RunnerState::load(dir)?;
    if !event.cwd.is_empty() {
        let reg = projects::register(&store, &event.cwd)?;
        if reg.newly_registered {
            state.notifications.push(format!(
                "retro is now watching `{}` — exclude via privacy.exclude_projects in ~/.retro/config.toml",
                reg.slug
            ));
        }
    }
    // Advance the observe watermark to the transcript's mtime so the
    // SessionStart catch-up scan doesn't re-enqueue this session.
    if let Ok(meta) = std::fs::metadata(&event.transcript_path) {
        if let Ok(mtime) = meta.modified() {
            if let Ok(secs) = mtime.duration_since(std::time::UNIX_EPOCH) {
                state.last_observed_unix = state.last_observed_unix.max(secs.as_secs());
            }
        }
    }
    state.save(dir)?;
    Ok(ObserveOutcome::Enqueued)
}

/// Detached background worker; inherits this process's environment (auth works
/// here — the whole point of hook-time capture). Errors ignored: the next
/// observe/brief will spawn again.
fn spawn_worker() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .args(["run", "--background"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}
