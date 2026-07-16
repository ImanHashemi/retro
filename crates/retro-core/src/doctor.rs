//! End-to-end health verification for the v3 pipeline. Read-only:
//! every check inspects state; none mutate. Consumed by `retro doctor`
//! and the dashboard.

use std::path::Path;

use serde::Serialize;

use crate::config::Config;
use crate::errors::CoreError;

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn all_ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Store, git as store_git, index};
    use tempfile::TempDir;

    fn config_for(claude_dir: &Path) -> Config {
        let mut config = Config::default();
        config.paths.claude_dir = claude_dir.display().to_string();
        config
    }

    #[test]
    fn healthy_store_passes_structural_checks() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store_git::ensure_repo(tmp.path()).unwrap();
        index::build(&store).unwrap();
        // hooks present in settings.json
        std::fs::write(
            claude.path().join("settings.json"),
            r#"{"hooks":{"SessionEnd":[{"matcher":"","hooks":[{"type":"command","command":"/bin/retro observe"}]}],"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"/bin/retro brief"}]}]}}"#,
        )
        .unwrap();

        let report = run_checks_for_tests(tmp.path(), &config_for(claude.path()));
        let by_name = |n: &str| report.checks.iter().find(|c| c.name == n).unwrap();
        assert!(by_name("store-present").ok);
        assert!(by_name("store-repo").ok);
        assert!(by_name("index").ok);
        assert!(by_name("hooks").ok);
        assert!(by_name("queue").ok);
    }

    #[test]
    fn unhealthy_conditions_are_reported() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        // no repo, no index, no hooks, stale index after node write
        let report = run_checks_for_tests(tmp.path(), &config_for(claude.path()));
        let by_name = |n: &str| report.checks.iter().find(|c| c.name == n).unwrap();
        assert!(!by_name("store-repo").ok);
        assert!(!by_name("index").ok);
        assert!(!by_name("hooks").ok);
        assert!(!report.all_ok());
    }

    #[test]
    fn missing_store_short_circuits() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        // no ensure_layout() call — the knowledge dir does not exist yet.
        let config = config_for(claude.path());
        let report = run_checks_for_tests(tmp.path(), &config);
        assert_eq!(report.checks.len(), 1);
        assert!(!report.checks[0].ok);
        assert!(report.checks[0].detail.contains("retro init"));
    }
}

/// Run all checks. `probe_claude` additionally spawns `claude --version`
/// (subprocess, no tokens) — optional because it's slow and env-dependent.
/// `probe_env` additionally checks machine-level state (the v2 launchd plist
/// under $HOME) — off in tests, on in the CLI.
pub fn run_checks(store_root: &Path, config: &Config, probe_claude: bool) -> DoctorReport {
    run_checks_inner(store_root, config, probe_claude, true)
}

pub fn run_checks_for_tests(store_root: &Path, config: &Config) -> DoctorReport {
    run_checks_inner(store_root, config, false, false)
}

fn run_checks_inner(
    store_root: &Path,
    config: &Config,
    probe_claude: bool,
    probe_env: bool,
) -> DoctorReport {
    let mut checks = Vec::new();

    let store = crate::store::Store::open(store_root);
    if !store.knowledge_dir().is_dir() {
        checks.push(Check {
            name: "store-present".to_string(),
            ok: false,
            detail: "no knowledge store found — run `retro init`".to_string(),
        });
        return DoctorReport { checks };
    }
    checks.push(Check {
        name: "store-present".to_string(),
        ok: true,
        detail: "initialized".to_string(),
    });

    // Store repo
    let repo_ok = crate::store::git::is_repo(store_root);
    checks.push(Check {
        name: "store-repo".to_string(),
        ok: repo_ok,
        detail: if repo_ok {
            format!("git repo at {}", store_root.display())
        } else {
            "store is not a git repo — run `retro init`".to_string()
        },
    });

    // Index built + fresh
    let index_check = match crate::store::index::open(store_root) {
        Ok(conn) => match crate::store::index::is_fresh(&store, &conn) {
            Ok(true) => (true, "built and fresh".to_string()),
            Ok(false) => (false, "stale — run `retro reindex`".to_string()),
            Err(e) => (false, format!("freshness check failed: {e}")),
        },
        Err(e) => {
            let msg = e.to_string();
            let detail = if msg.contains("retro reindex") {
                msg
            } else {
                format!("{msg} — run `retro reindex`")
            };
            (false, detail)
        }
    };
    checks.push(Check {
        name: "index".to_string(),
        ok: index_check.0,
        detail: index_check.1,
    });

    // Hooks installed (global settings.json contains retro observe + brief)
    let settings_path = config.claude_dir().join("settings.json");
    let hooks_ok = std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .map(|v| {
            let has = |event: &str, sub: &str| {
                v["hooks"][event]
                    .as_array()
                    .map(|groups| {
                        groups.iter().any(|g| {
                            g["hooks"].as_array().is_some_and(|hs| {
                                hs.iter().any(|h| {
                                    h["command"]
                                        .as_str()
                                        .is_some_and(|c| c.contains(&format!("retro {sub}")))
                                })
                            })
                        })
                    })
                    .unwrap_or(false)
            };
            has("SessionEnd", "observe") && has("SessionStart", "brief")
        })
        .unwrap_or(false);
    checks.push(Check {
        name: "hooks".to_string(),
        ok: hooks_ok,
        detail: if hooks_ok {
            "SessionEnd + SessionStart installed".to_string()
        } else {
            format!(
                "missing in {} — run `retro init`",
                settings_path.display()
            )
        },
    });

    // Queue age
    let queue_check = match crate::store::queue::list(store_root) {
        Ok(entries) if entries.is_empty() => (true, "empty".to_string()),
        Ok(entries) => {
            let oldest_stale = chrono::DateTime::parse_from_rfc3339(&entries[0].enqueued_at)
                .map(|t| chrono::Utc::now().signed_duration_since(t) > chrono::Duration::hours(24))
                .unwrap_or(false);
            if oldest_stale {
                (
                    false,
                    format!(
                        "{} entr(ies), oldest > 24h — pipeline not draining?",
                        entries.len()
                    ),
                )
            } else {
                (true, format!("{} entr(ies), draining", entries.len()))
            }
        }
        Err(e) => (false, format!("unreadable: {e}")),
    };
    checks.push(Check {
        name: "queue".to_string(),
        ok: queue_check.0,
        detail: queue_check.1,
    });

    // Recent stage failures (from health.json)
    if let Ok(health) = crate::health::Health::load(store_root) {
        let warnings = health.warnings();
        checks.push(Check {
            name: "stages".to_string(),
            ok: warnings.is_empty(),
            detail: if warnings.is_empty() {
                "all stages healthy".to_string()
            } else {
                warnings.join("; ")
            },
        });
    }

    // Projections current (spec §9): the global managed block must match a
    // fresh regeneration from the store (cheap string comparison, no writes).
    let projection_check = (|| -> Result<(bool, String), CoreError> {
        let rules = crate::projection::local_md::projectable_rules(
            &store,
            &crate::store::Scope::Global,
            config.knowledge.confidence_threshold,
        )?;
        let path = config.claude_dir().join("CLAUDE.md");
        // Parity with project_global_md's own empty-guard: never treat "no
        // rules and no file yet" as out of date — there is nothing to project.
        if rules.is_empty() && !path.exists() {
            return Ok((true, "nothing to project yet".to_string()));
        }
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        // Reuse the writer's own idempotence contract: regenerating over the
        // current content must be a no-op when projections are current.
        let regenerated = crate::projection::claude_md::update_claude_md_content(&existing, &rules);
        if regenerated == existing {
            Ok((true, format!("{} global rule(s) projected", rules.len())))
        } else {
            Ok((
                false,
                "global CLAUDE.md out of date — run `retro run`".to_string(),
            ))
        }
    })();
    match projection_check {
        Ok((ok, detail)) => checks.push(Check {
            name: "projection".to_string(),
            ok,
            detail,
        }),
        Err(e) => checks.push(Check {
            name: "projection".to_string(),
            ok: false,
            detail: e.to_string(),
        }),
    }

    // v2 runner coexistence (Plan 2 final-review carry-over): both pipelines
    // being live doubles AI spend and double-writes the global managed block.
    let v2_plist = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
        .join("Library/LaunchAgents/com.retro.runner.plist");
    if probe_env && v2_plist.exists() {
        checks.push(Check {
            name: "v2-runner".to_string(),
            ok: false,
            detail: "v2 launchd runner still installed — run `retro migrate` to remove it".to_string(),
        });
    }

    // Backup remote (informational: ok either way, detail differs)
    let has_remote = crate::store::git::has_remote(store_root);
    checks.push(Check {
        name: "backup".to_string(),
        ok: true,
        detail: if has_remote {
            "remote configured".to_string()
        } else {
            "no backup remote (optional) — rerun `retro init` to set one up".to_string()
        },
    });

    // claude CLI probe (optional)
    if probe_claude {
        let probe = std::process::Command::new("claude")
            .arg("--version")
            .output();
        let (ok, detail) = match probe {
            Ok(out) if out.status.success() => (
                true,
                String::from_utf8_lossy(&out.stdout).trim().to_string(),
            ),
            Ok(out) => (false, format!("claude --version exited {}", out.status)),
            Err(e) => (false, format!("claude CLI not runnable: {e}")),
        };
        checks.push(Check {
            name: "claude-cli".to_string(),
            ok,
            detail,
        });
    }

    DoctorReport { checks }
}
