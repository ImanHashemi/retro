use anyhow::Result;
use retro_core::config::{Config, retro_dir};
use retro_core::store::{queue, state::RunnerState};
use retro_core::{briefing, health, observer};

/// SessionStart hook entry: catch-up scan + briefing to stdout.
/// Same never-fail contract as observe.
pub fn run() -> Result<()> {
    let dir = retro_dir();
    let config = Config::load(&dir.join("config.toml")).unwrap_or_default();
    if !config.v3.enabled {
        return Ok(());
    }
    let mut state = RunnerState::load(&dir).unwrap_or_default();

    // Catch-up: enqueue sessions modified since the watermark (crashed
    // sessions, other machines, missed hooks). cwd is unknown here; the
    // pipeline recovers it from the transcript's metadata.
    // Watermark safety margin: subtract 60 seconds so a crashed parallel
    // session whose last write predates the watermark isn't lost forever.
    // Queue enqueue is idempotent by session id, so overlap is harmless.
    const WATERMARK_SAFETY_SECS: u64 = 60;
    let since = if state.last_observed_unix > WATERMARK_SAFETY_SECS {
        Some(
            std::time::UNIX_EPOCH
                + std::time::Duration::from_secs(state.last_observed_unix - WATERMARK_SAFETY_SECS),
        )
    } else {
        None
    };
    let modified = observer::find_modified_sessions(&config.claude_dir(), since, &[]);
    let mut enqueued = 0usize;
    let mut max_seen = state.last_observed_unix;
    for m in &modified {
        let Some(stem) = m.path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let entry = queue::QueueEntry {
            session_id: stem.to_string(),
            transcript_path: m.path.display().to_string(),
            cwd: None,
            enqueued_at: chrono::Utc::now().to_rfc3339(),
        };
        // Exclusion is enforced at drain time (cwd unknown here).
        if queue::enqueue(&dir, &entry).is_ok() {
            enqueued += 1;
        }
        if let Ok(secs) = m.mtime.duration_since(std::time::UNIX_EPOCH) {
            max_seen = max_seen.max(secs.as_secs());
        }
    }
    state.last_observed_unix = max_seen;

    // Briefing: drained notifications + current health warnings.
    let notifications = state.drain_notifications();
    let warnings = health::Health::load(&dir)
        .map(|h| h.warnings())
        .unwrap_or_default();
    let text = briefing::build_v3_briefing(&notifications, &warnings);
    if !text.is_empty() {
        print!("{text}");
    }
    let _ = state.save(&dir);
    let _ = health::record(
        &dir,
        "brief",
        true,
        &format!("caught up {enqueued} session(s)"),
    );

    if enqueued > 0 {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe)
                .args(["run", "--background"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
    }
    Ok(())
}
