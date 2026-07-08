//! The v3 pipeline: drain the session queue into the knowledge store.
//! No daemon — invoked by hooks (`retro run --background`) or manually.

use std::path::{Path, PathBuf};

use crate::analysis::backend::AnalysisBackend;
use crate::analysis::v3 as analysis_v3;
use crate::config::Config;
use crate::errors::CoreError;
use crate::health;
use crate::ingest::session::parse_session_file;
use crate::lock::LockFile;
use crate::models::Session;
use crate::projection::local_md;
use crate::scrub;
use crate::store::state::RunnerState;
use crate::store::{git as store_git, index, projects, queue, Store};

#[derive(Debug, Default)]
pub struct RunV3Summary {
    pub sessions_processed: usize,
    pub sessions_pending: usize,
    pub sessions_skipped: usize,
    pub ai_calls: u32,
    pub nodes_created: usize,
    pub nodes_updated: usize,
    pub nodes_merged: usize,
    pub nodes_invalidated: usize,
    /// Operations the analysis stage rejected as invalid/hostile (see health
    /// stage "analyze-skips" for the reasons).
    pub ops_skipped: usize,
    pub rules_projected_global: usize,
    pub pushed: bool,
}

/// Run the v3 pipeline once. Returns Ok(None) if another run holds the lock
/// (normal when hooks race — not an error). `dry_run` reports what WOULD
/// happen: no AI calls, no writes, no commits.
pub fn run_v3(
    store_root: &Path,
    config: &Config,
    backend: &dyn AnalysisBackend,
    dry_run: bool,
) -> Result<Option<RunV3Summary>, CoreError> {
    let Some(_lock) = LockFile::try_acquire(&store_root.join("run.lock")) else {
        return Ok(None);
    };
    let mut summary = RunV3Summary::default();
    let store = Store::open(store_root);
    // Layout creation (knowledge/ dirs, .gitignore) is itself a write — dry_run
    // must touch nothing, so this is deferred to the real-run path alongside
    // ensure_repo(). load_all()/parse_session_file() below tolerate a
    // not-yet-created store (missing dirs are skipped, never an error).
    if !dry_run {
        store.ensure_layout()?;
    }

    // Stage: commit manual edits (files-as-truth: user edits become history).
    if !dry_run {
        store_git::ensure_repo(store_root)?;
        if store_git::commit_all(store_root, "user: edit knowledge")? {
            health::record(store_root, "manual-edits", true, "committed user edits")?;
        }
    }

    // Stage: exclusion sweep — a project excluded AFTER registration gets its
    // knowledge deleted (recoverable via store git history) and its
    // CLAUDE.local.md removed. Spec §5: exclusion = removal.
    if !dry_run {
        let map = projects::PathMap::load(store_root)?;
        for (slug, path) in map.paths.clone() {
            if projects::is_excluded(&path, &config.privacy.exclude_projects) {
                projects::cleanup_excluded(&store, &slug, Some(&path))?;
                let mut st = RunnerState::load(store_root)?;
                st.notifications.push(format!(
                    "retro stopped watching `{slug}` (excluded) and removed its knowledge"
                ));
                st.save(store_root)?;
                health::record(store_root, "exclude", true, &format!("cleaned up {slug}"))?;
            }
        }
    }

    // Stage: prune stale queue entries (deleted transcripts) — visible, not silent.
    let pruned = queue::prune_stale(store_root)?;
    if !pruned.is_empty() {
        health::record(
            store_root,
            "queue",
            true,
            &format!(
                "pruned {} stale entr(ies): {}",
                pruned.len(),
                pruned.join(", ")
            ),
        )?;
    }

    // Stage: load + parse queue into per-project groups.
    let entries = queue::list(store_root)?;
    // (slug, project_path, [(session_id, transcript_mtime_unix, session)])
    let mut groups: Vec<(String, String, Vec<(String, u64, Session)>)> = Vec::new();
    for entry in &entries {
        let path = PathBuf::from(&entry.transcript_path);
        let mtime_unix = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let cwd_hint = entry.cwd.clone().unwrap_or_default();
        let mut session = match parse_session_file(&path, &entry.session_id, &cwd_hint) {
            Ok(s) => s,
            Err(_) => {
                // unparseable transcript: drop from queue, note in health
                if !dry_run {
                    queue::remove(store_root, &entry.session_id)?;
                }
                summary.sessions_skipped += 1;
                continue;
            }
        };
        let cwd = session
            .metadata
            .cwd
            .clone()
            .filter(|c| !c.is_empty())
            .unwrap_or(cwd_hint);
        if cwd.is_empty() {
            if !dry_run {
                queue::remove(store_root, &entry.session_id)?;
            }
            summary.sessions_skipped += 1;
            continue;
        }
        if projects::is_excluded(&cwd, &config.privacy.exclude_projects) {
            if !dry_run {
                queue::remove(store_root, &entry.session_id)?;
            }
            summary.sessions_skipped += 1;
            continue;
        }
        if session.user_messages.len() < 2 {
            // low signal: processed (removed), never analyzed
            if !dry_run {
                queue::remove(store_root, &entry.session_id)?;
            }
            summary.sessions_skipped += 1;
            continue;
        }
        if config.privacy.scrub_secrets {
            scrub::scrub_session(&mut session);
        }
        let slug = if dry_run {
            // dry-run must not write project.toml; use a path-derived label
            crate::store::slugify(
                Path::new(&cwd)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("project"),
            )
        } else {
            let reg = projects::register(&store, &cwd)?;
            if reg.newly_registered {
                let mut state = RunnerState::load(store_root)?;
                state.notifications.push(format!(
                    "retro is now watching `{}` — exclude via privacy.exclude_projects in ~/.retro/config.toml",
                    reg.slug
                ));
                state.save(store_root)?;
            }
            reg.slug
        };
        match groups.iter_mut().find(|(s, _, _)| s == &slug) {
            Some((_, _, sessions)) => {
                sessions.push((entry.session_id.clone(), mtime_unix, session))
            }
            None => groups.push((
                slug,
                cwd,
                vec![(entry.session_id.clone(), mtime_unix, session)],
            )),
        }
    }

    if dry_run {
        summary.sessions_pending = groups.iter().map(|(_, _, s)| s.len()).sum();
        return Ok(Some(summary));
    }

    // Stage: budget-gated analysis, one AI call per project group.
    // State is re-loaded fresh around each mutation — never held across an AI
    // call, so concurrent hook writes (observe/brief) aren't clobbered by a
    // stale save.
    let today = chrono::Utc::now().date_naive().to_string();
    let mut touched: Vec<(String, String)> = Vec::new(); // (slug, path) that got/changed nodes
    let mut learned: Vec<String> = Vec::new();
    for (slug, project_path, group) in &groups {
        let state = RunnerState::load(store_root)?;
        if state.budget_remaining(&today, config.runner.max_ai_calls_per_day) == 0 {
            let waiting: usize =
                groups.iter().map(|(_, _, s)| s.len()).sum::<usize>() - summary.sessions_processed;
            health::record(
                store_root,
                "analyze",
                false,
                &format!("daily AI budget exhausted; {waiting} session(s) remain queued"),
            )?;
            summary.sessions_pending = waiting;
            break;
        }
        let sessions: Vec<Session> = group.iter().map(|(_, _, s)| s.clone()).collect();
        let result = match analysis_v3::analyze_sessions(&store, backend, &sessions, Some(slug)) {
            Ok(r) => r,
            Err(e) => {
                health::record(store_root, "analyze", false, &e.to_string())?;
                // leave this group queued for a future run; keep going with others
                continue;
            }
        };
        summary.ai_calls += 1;
        summary.sessions_processed += result.sessions_analyzed;
        summary.nodes_created += result.nodes_created;
        summary.nodes_updated += result.nodes_updated;
        summary.nodes_merged += result.nodes_merged;
        summary.nodes_invalidated += result.nodes_invalidated;
        summary.ops_skipped += result.ops_skipped;
        learned.extend(result.learned.iter().map(|b| {
            let first_line = b.lines().next().unwrap_or(b);
            format!("Learned: {}", crate::util::truncate_str(first_line, 100))
        }));
        let mut state = RunnerState::load(store_root)?;
        state.record_ai_calls(&today, 1);
        for (session_id, mtime_unix, _) in group {
            queue::remove(store_root, session_id)?;
            state.record_processed(session_id, *mtime_unix);
        }
        state.save(store_root)?;
        touched.push((slug.clone(), project_path.clone()));
        let detail = if result.ops_skipped > 0 {
            format!(
                "{}: +{} nodes, {} op(s) skipped",
                slug, result.nodes_created, result.ops_skipped
            )
        } else {
            format!("{}: +{} nodes", slug, result.nodes_created)
        };
        health::record(store_root, "analyze", true, &detail)?;
        if !result.skipped.is_empty() {
            health::record(
                store_root,
                "analyze-skips",
                true,
                &result.skipped.join("; "),
            )?;
        }
    }

    // Anything still queued (budget exhaustion OR failed groups) is pending —
    // authoritative recount so the summary can't understate it.
    summary.sessions_pending = queue::list(store_root)?.len();

    // Stage: surface store parse warnings (skipped/misplaced files) — visible,
    // never silent (matches the JSONL-skipping convention elsewhere).
    let loaded = store.load_all()?;
    if !loaded.warnings.is_empty() {
        let joined = loaded.warnings.join("; ");
        health::record(
            store_root,
            "store",
            true,
            &format!(
                "{} file(s) skipped: {}",
                loaded.warnings.len(),
                crate::util::truncate_str(&joined, 500)
            ),
        )?;
    }

    // Stage: projection (global always — cheap and idempotent; locals for touched projects).
    let threshold = config.knowledge.confidence_threshold;
    let global_md = config.claude_dir().join("CLAUDE.md");
    let backups = store_root.join("backups");
    match local_md::project_global_md(&store, &global_md, threshold, Some(&backups)) {
        Ok(n) => {
            summary.rules_projected_global = n;
            health::record(store_root, "project", true, &format!("global: {n} rule(s)"))?;
        }
        Err(e) => health::record(store_root, "project", false, &e.to_string())?,
    }
    for (slug, project_path) in &touched {
        if let Err(e) = local_md::project_local_md(&store, slug, Path::new(project_path), threshold)
        {
            health::record(store_root, "project", false, &format!("{slug}: {e}"))?;
        }
    }

    // Stage: notifications for the next briefing.
    if !learned.is_empty() {
        let mut st = RunnerState::load(store_root)?;
        st.notifications.extend(learned);
        st.save(store_root)?;
    }

    // Stage: index, commit, push.
    index::build(&store)?;
    let committed = store_git::commit_all(
        store_root,
        &format!(
            "retro: learn {} node(s), update {}",
            summary.nodes_created,
            summary.nodes_updated + summary.nodes_merged
        ),
    )?;
    if committed {
        match store_git::push_best_effort(store_root) {
            store_git::PushOutcome::Pushed => {
                summary.pushed = true;
                health::record(store_root, "push", true, "pushed")?;
            }
            store_git::PushOutcome::NoRemote => {
                health::record(store_root, "push", true, "no remote configured")?;
            }
            store_git::PushOutcome::Failed(err) => {
                health::record(store_root, "push", false, &err)?;
            }
        }
    }
    health::record(
        store_root,
        "run",
        true,
        &format!("{} session(s)", summary.sessions_processed),
    )?;
    Ok(Some(summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::backend::MockBackend;
    use tempfile::TempDir;

    /// Minimal session JSONL the v2 parser accepts: two user entries with cwd.
    /// `UserEntry::uuid` has no `#[serde(default)]`, so it MUST be present —
    /// the naive shape (no uuid) fails to deserialize. `message.content` is a
    /// plain string, which matches `MessageContent::Text`.
    fn write_fixture_session(dir: &Path, id: &str, cwd: &str) -> PathBuf {
        let path = dir.join(format!("{id}.jsonl"));
        let line = |n: u32, text: &str| {
            format!(
                r#"{{"type":"user","uuid":"{id}-{n}","sessionId":"{id}","cwd":"{cwd}","timestamp":"2026-07-06T10:00:0{n}Z","message":{{"role":"user","content":"{text}"}}}}"#
            )
        };
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                line(0, "first message"),
                line(1, "second message")
            ),
        )
        .unwrap();
        path
    }

    /// CRITICAL: config.paths.claude_dir MUST point at a tempdir — with the
    /// default (~/.claude) the projection stage would write the developer's
    /// real global CLAUDE.md during tests. The returned TempDir keeps it alive.
    fn setup() -> (TempDir, TempDir, Config) {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store_git::ensure_repo(tmp.path()).unwrap();
        let mut config = Config::default();
        config.v3.enabled = true;
        config.paths.claude_dir = claude.path().display().to_string();
        (tmp, claude, config)
    }

    #[test]
    fn empty_queue_is_a_quiet_noop() {
        let (tmp, _claude, config) = setup();
        let backend = MockBackend::with_responses(vec![]);
        let summary = run_v3(tmp.path(), &config, &backend, false).unwrap();
        let summary = summary.expect("lock acquired");
        assert_eq!(summary.sessions_processed, 0);
        assert_eq!(summary.ai_calls, 0);
    }

    #[test]
    fn drains_queue_analyzes_and_projects() {
        let (tmp, _claude, config) = setup();
        let proj = TempDir::new().unwrap();
        std::process::Command::new("git")
            .arg("-C")
            .arg(proj.path())
            .arg("init")
            .output()
            .unwrap();
        let transcript = write_fixture_session(tmp.path(), "sess-1", proj.path().to_str().unwrap());
        queue::enqueue(
            tmp.path(),
            &queue::QueueEntry {
                session_id: "sess-1".to_string(),
                transcript_path: transcript.display().to_string(),
                cwd: Some(proj.path().display().to_string()),
                enqueued_at: "2026-07-06T10:00:00Z".to_string(),
            },
        )
        .unwrap();

        let response = r#"{"reasoning":"found one","operations":[
            {"action":"create_node","node_type":"rule","scope":"project","content":"Project rule from analysis.","confidence":0.9}
        ]}"#;
        let backend = MockBackend::with_responses(vec![response.to_string()]);
        let summary = run_v3(tmp.path(), &config, &backend, false)
            .unwrap()
            .unwrap();

        assert_eq!(summary.sessions_processed, 1);
        assert_eq!(summary.ai_calls, 1);
        assert_eq!(summary.nodes_created, 1);
        // queue drained
        assert!(queue::list(tmp.path()).unwrap().is_empty());
        // node landed in the project scope
        let store = Store::open(tmp.path());
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.nodes.len(), 1);
        // CLAUDE.local.md projected into the project
        let local = std::fs::read_to_string(proj.path().join("CLAUDE.local.md")).unwrap();
        assert!(local.contains("Project rule from analysis."));
        // store committed (clean tree)
        assert!(!store_git::has_changes(tmp.path()).unwrap());
        // health recorded
        let h = health::Health::load(tmp.path()).unwrap();
        assert!(h.stages.contains_key("analyze"));
        // session recorded as processed (watermark for catch-up rescans)
        let final_state = RunnerState::load(tmp.path()).unwrap();
        assert!(
            final_state.processed.contains_key("sess-1"),
            "got: {:?}",
            final_state.processed
        );
    }

    #[test]
    fn dry_run_leaves_unparseable_entries_queued() {
        let (tmp, _claude, config) = setup();
        let bad = tmp.path().join("bad.jsonl");
        std::fs::write(&bad, "not valid jsonl").unwrap();
        queue::enqueue(
            tmp.path(),
            &queue::QueueEntry {
                session_id: "bad-sess".to_string(),
                transcript_path: bad.display().to_string(),
                cwd: Some("/tmp/x".to_string()),
                enqueued_at: "2026-07-06T10:00:00Z".to_string(),
            },
        )
        .unwrap();
        let backend = MockBackend::with_responses(vec![]);
        let summary = run_v3(tmp.path(), &config, &backend, true)
            .unwrap()
            .unwrap();
        assert_eq!(summary.sessions_skipped, 1);
        assert_eq!(
            queue::list(tmp.path()).unwrap().len(),
            1,
            "dry-run must not remove"
        );
    }

    #[test]
    fn failed_analysis_leaves_sessions_queued_and_pending() {
        let (tmp, _claude, config) = setup();
        let proj = TempDir::new().unwrap();
        let transcript =
            write_fixture_session(tmp.path(), "fail-sess", proj.path().to_str().unwrap());
        queue::enqueue(
            tmp.path(),
            &queue::QueueEntry {
                session_id: "fail-sess".to_string(),
                transcript_path: transcript.display().to_string(),
                cwd: Some(proj.path().display().to_string()),
                enqueued_at: "2026-07-06T10:00:00Z".to_string(),
            },
        )
        .unwrap();
        let backend = MockBackend::with_responses(vec![]); // exhausted mock -> analyze_sessions errors
        let summary = run_v3(tmp.path(), &config, &backend, false)
            .unwrap()
            .unwrap();
        assert_eq!(summary.ai_calls, 0);
        assert_eq!(
            summary.sessions_pending, 1,
            "failed group counts as pending"
        );
        assert_eq!(queue::list(tmp.path()).unwrap().len(), 1, "stays queued");
        let h = health::Health::load(tmp.path()).unwrap();
        assert!(!h.stages["analyze"].ok, "failure recorded");
    }

    #[test]
    fn budget_exhaustion_leaves_sessions_queued_with_health_warning() {
        let (tmp, _claude, mut config) = setup();
        config.runner.max_ai_calls_per_day = 0; // no budget at all
        let proj = TempDir::new().unwrap();
        let transcript = write_fixture_session(tmp.path(), "sess-2", proj.path().to_str().unwrap());
        queue::enqueue(
            tmp.path(),
            &queue::QueueEntry {
                session_id: "sess-2".to_string(),
                transcript_path: transcript.display().to_string(),
                cwd: Some(proj.path().display().to_string()),
                enqueued_at: "2026-07-06T10:00:00Z".to_string(),
            },
        )
        .unwrap();
        let backend = MockBackend::with_responses(vec![]); // any call would error
        let summary = run_v3(tmp.path(), &config, &backend, false)
            .unwrap()
            .unwrap();
        assert_eq!(summary.ai_calls, 0);
        assert_eq!(queue::list(tmp.path()).unwrap().len(), 1, "stays queued");
        let h = health::Health::load(tmp.path()).unwrap();
        let w = h.warnings();
        assert!(w.iter().any(|x| x.contains("budget")), "got: {w:?}");
    }

    #[test]
    fn dry_run_makes_no_ai_calls_and_no_writes() {
        let (tmp, _claude, config) = setup();
        let proj = TempDir::new().unwrap();
        let transcript = write_fixture_session(tmp.path(), "sess-3", proj.path().to_str().unwrap());
        queue::enqueue(
            tmp.path(),
            &queue::QueueEntry {
                session_id: "sess-3".to_string(),
                transcript_path: transcript.display().to_string(),
                cwd: Some(proj.path().display().to_string()),
                enqueued_at: "2026-07-06T10:00:00Z".to_string(),
            },
        )
        .unwrap();
        let backend = MockBackend::with_responses(vec![]);
        let summary = run_v3(tmp.path(), &config, &backend, true)
            .unwrap()
            .unwrap();
        assert_eq!(summary.ai_calls, 0);
        assert_eq!(summary.sessions_pending, 1);
        assert_eq!(queue::list(tmp.path()).unwrap().len(), 1, "queue untouched");
        assert!(Store::open(tmp.path()).load_all().unwrap().nodes.is_empty());
    }

    /// Regression: on a store that has NEVER been initialized (no
    /// `ensure_layout`/`ensure_repo` yet — the state `retro init --v3` leaves
    /// things in before first real run), dry-run must not create so much as
    /// a directory or `.gitignore`. Caught by manual CLI verification, not by
    /// `dry_run_makes_no_ai_calls_and_no_writes` above, whose `setup()` helper
    /// already pre-creates the layout.
    #[test]
    fn dry_run_on_a_never_initialized_store_creates_nothing() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let mut config = Config::default();
        config.v3.enabled = true;
        config.paths.claude_dir = claude.path().display().to_string();

        let backend = MockBackend::with_responses(vec![]);
        let summary = run_v3(tmp.path(), &config, &backend, true)
            .unwrap()
            .unwrap();
        assert_eq!(summary.sessions_pending, 0);
        assert_eq!(
            std::fs::read_dir(tmp.path()).unwrap().count(),
            0,
            "dry-run must not create knowledge/, .gitignore, or anything else"
        );
    }
}
