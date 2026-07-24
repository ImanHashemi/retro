//! Dashboard JSON API. All handlers are synchronous and read the same
//! retro-core modules the CLI uses.

use std::io::Read;
use std::path::Path;

use anyhow::Result;
use retro_core::config::Config;
use serde_json::json;
use tiny_http::{Method, Request};

use super::{html_response, index_html, json_response};

/// Request bodies are capped well below anything a legitimate edit needs
/// (rule bodies are a few paragraphs at most) — guards against a client
/// streaming an unbounded body at a localhost-only server.
const MAX_BODY_BYTES: u64 = 64 * 1024;

/// `ai.model` values the pipeline accepts — must match what
/// `ClaudeCliBackend` passes to the `claude` CLI's `--model` flag.
const ALLOWED_MODELS: [&str; 3] = ["sonnet", "haiku", "opus"];

pub fn route(
    store_root: &Path,
    config: &Config,
    method: &Method,
    url: &str,
    mut request: Request,
) -> Result<()> {
    let path = url.split('?').next().unwrap_or(url);
    match (method, path) {
        (Method::Get, "/") => request.respond(html_response(index_html()))?,
        (Method::Get, "/api/ping") => request.respond(json_response(&json!({"ok": true}), 200))?,
        (Method::Get, "/api/xray") => {
            let (body, status) = api_xray(store_root, config);
            request.respond(json_response(&body, status))?
        }
        (Method::Get, "/api/nodes") => {
            let (body, status) = api_nodes(store_root, url);
            request.respond(json_response(&body, status))?
        }
        (Method::Get, "/api/node") => {
            let (body, status) = api_node(store_root, url);
            request.respond(json_response(&body, status))?
        }
        (Method::Get, "/api/health") => {
            let (body, status) = api_health(store_root, config);
            request.respond(json_response(&body, status))?
        }
        (Method::Get, "/api/history") => {
            let (body, status) = api_history(store_root, url);
            request.respond(json_response(&body, status))?
        }
        (Method::Get, "/api/doctor") => {
            let (body, status) = api_doctor(store_root, config);
            request.respond(json_response(&body, status))?
        }
        (Method::Get, "/api/config") => {
            let (body, status) = api_config_get(config);
            request.respond(json_response(&body, status))?
        }
        (Method::Post, "/api/config") => {
            let (body, status) = match read_json_body(&mut request) {
                Ok(json_body) => api_config_post(store_root, &json_body),
                Err(err_response) => err_response,
            };
            request.respond(json_response(&body, status))?
        }
        (Method::Post, "/api/node/invalidate") => {
            let (body, status) = match read_json_body(&mut request) {
                Ok(json_body) => api_node_invalidate(store_root, config, &json_body),
                Err(err_response) => err_response,
            };
            request.respond(json_response(&body, status))?
        }
        (Method::Post, "/api/node/update") => {
            let (body, status) = match read_json_body(&mut request) {
                Ok(json_body) => api_node_update(store_root, config, &json_body),
                Err(err_response) => err_response,
            };
            request.respond(json_response(&body, status))?
        }
        (Method::Post, "/api/project/exclude") => {
            let (body, status) = match read_json_body(&mut request) {
                Ok(json_body) => api_project_exclude(store_root, config, &json_body),
                Err(err_response) => err_response,
            };
            request.respond(json_response(&body, status))?
        }
        _ => request.respond(json_response(&json!({"error": "not found"}), 404))?,
    }
    Ok(())
}

/// Read the request body (tiny_http consumes the reader on first read, so
/// this must happen before any response is sent) and parse it as JSON.
fn read_json_body(request: &mut Request) -> Result<serde_json::Value, (serde_json::Value, u16)> {
    parse_json_body(request.as_reader())
}

/// Testable core of [`read_json_body`], decoupled from `tiny_http::Request`.
/// Any failure — oversized body, I/O error, malformed JSON — is a client
/// error, returned as a ready-to-send 400 response. Reads raw bytes so an
/// oversized body reports "too large" even when the cap splits a multibyte
/// character (serde validates UTF-8 during parsing).
fn parse_json_body(reader: &mut dyn Read) -> Result<serde_json::Value, (serde_json::Value, u16)> {
    let mut buf = Vec::new();
    let mut limited = reader.take(MAX_BODY_BYTES + 1);
    if let Err(e) = limited.read_to_end(&mut buf) {
        return Err((
            json!({"error": format!("failed to read request body: {e}")}),
            400,
        ));
    }
    if buf.len() as u64 > MAX_BODY_BYTES {
        return Err((json!({"error": "request body too large"}), 400));
    }
    serde_json::from_slice(&buf)
        .map_err(|e| (json!({"error": format!("malformed JSON body: {e}")}), 400))
}

/// Write handlers must not interleave with a runner pass (git index-lock
/// contention, mislabeled commits from `add -A` sweeping in-flight writes).
/// Returns the held lock, or a ready-to-send 503 if a run is in progress.
fn acquire_write_lock(
    store_root: &Path,
) -> Result<retro_core::lock::LockFile, (serde_json::Value, u16)> {
    retro_core::lock::LockFile::try_acquire(&store_root.join("run.lock")).ok_or((
        json!({"error": "a retro run is in progress — retry shortly"}),
        503,
    ))
}

/// Shared post-write pipeline: commit the store, rebuild the index (index
/// failures are recorded to health and otherwise swallowed — the write
/// itself already succeeded), then reproject the affected scope's managed
/// file. Mirrors the discipline `runner_v3` uses after every mutation.
fn after_write(
    store_root: &Path,
    config: &Config,
    scope: &retro_core::store::Scope,
    message: &str,
) -> Result<(), retro_core::errors::CoreError> {
    use retro_core::store::{Store, git as store_git, index, projects::PathMap};
    let store = Store::open(store_root);
    store_git::commit_all(store_root, message).map(|_| ())?;
    if let Err(e) = index::build(&store) {
        retro_core::health::record(store_root, "index", false, &e.to_string())?;
    }
    let threshold = config.knowledge.confidence_threshold;
    match scope {
        retro_core::store::Scope::Global => {
            let path = config.claude_dir().join("CLAUDE.md");
            retro_core::projection::local_md::project_global_md(
                &store,
                &path,
                threshold,
                Some(&store_root.join("backups")),
            )?;
        }
        retro_core::store::Scope::Project(slug) => {
            let map = PathMap::load(store_root)?;
            if let Some(p) = map.paths.get(slug) {
                retro_core::projection::local_md::project_local_md(
                    &store,
                    slug,
                    Path::new(p),
                    threshold,
                )?;
            }
        }
    }
    Ok(())
}

/// `POST /api/node/invalidate` — body `{"scope","id"}`. Marks the node
/// inactive (never deletes) and reprojects the affected scope.
fn api_node_invalidate(
    store_root: &Path,
    config: &Config,
    body: &serde_json::Value,
) -> (serde_json::Value, u16) {
    let Some(scope_param) = body.get("scope").and_then(|v| v.as_str()) else {
        return (json!({"error": "missing scope"}), 400);
    };
    let Some(id) = body.get("id").and_then(|v| v.as_str()) else {
        return (json!({"error": "missing id"}), 400);
    };
    let scope = match retro_core::store::Scope::parse(scope_param) {
        Ok(s) => s,
        Err(e) => return (json!({"error": e.to_string()}), 400),
    };
    // Store::get returns Ok(None) for an invalid slug rather than erroring,
    // so untrusted ids must be validated here, before any store call.
    if !retro_core::store::is_valid_slug(id) {
        return (json!({"error": "invalid id"}), 400);
    }
    let _lock = match acquire_write_lock(store_root) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let store = retro_core::store::Store::open(store_root);
    // Already-inactive nodes keep their provenance: Store::invalidate would
    // overwrite invalidated_by, which analysis supersession uses to record
    // the superseding node's id.
    match store.get(&scope, id) {
        Ok(Some(n)) if !n.is_active() => return (json!({"ok": true}), 200),
        Ok(Some(_)) => {}
        Ok(None) => return (json!({"error": "node not found"}), 404),
        Err(e) => return (json!({"error": e.to_string()}), 500),
    }
    match store.invalidate(&scope, id, "user") {
        Ok(true) => {}
        Ok(false) => return (json!({"error": "node not found"}), 404),
        // I/O failure or malformed frontmatter on an existing node file is a
        // server-state problem, not a bad request.
        Err(e) => return (json!({"error": e.to_string()}), 500),
    }
    let message = format!("user: invalidate {id} (dashboard)");
    if let Err(e) = after_write(store_root, config, &scope, &message) {
        return (
            json!({"error": format!("change saved, but post-write processing failed: {e}")}),
            500,
        );
    }
    (json!({"ok": true}), 200)
}

/// `POST /api/node/update` — body `{"scope","id","body"?,"confidence"?}`.
/// At least one of `body`/`confidence` must be present; confidence is
/// clamped into `[0.0, 1.0]` rather than rejected.
fn api_node_update(
    store_root: &Path,
    config: &Config,
    body: &serde_json::Value,
) -> (serde_json::Value, u16) {
    let Some(scope_param) = body.get("scope").and_then(|v| v.as_str()) else {
        return (json!({"error": "missing scope"}), 400);
    };
    let Some(id) = body.get("id").and_then(|v| v.as_str()) else {
        return (json!({"error": "missing id"}), 400);
    };
    let scope = match retro_core::store::Scope::parse(scope_param) {
        Ok(s) => s,
        Err(e) => return (json!({"error": e.to_string()}), 400),
    };
    if !retro_core::store::is_valid_slug(id) {
        return (json!({"error": "invalid id"}), 400);
    }

    let new_body = match body.get("body") {
        None => None,
        Some(v) => match v.as_str() {
            Some(s) => Some(s),
            None => return (json!({"error": "body must be a string"}), 400),
        },
    };
    if let Some(b) = new_body {
        if b.trim().is_empty() {
            return (json!({"error": "body must not be empty"}), 400);
        }
    }
    let new_confidence = match body.get("confidence") {
        None => None,
        Some(v) => match v.as_f64() {
            Some(c) if c.is_finite() => Some(c),
            _ => return (json!({"error": "confidence must be a finite number"}), 400),
        },
    };
    if new_body.is_none() && new_confidence.is_none() {
        return (
            json!({"error": "must provide body and/or confidence"}),
            400,
        );
    }

    let _lock = match acquire_write_lock(store_root) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let store = retro_core::store::Store::open(store_root);
    let mut node = match store.get(&scope, id) {
        Ok(Some(n)) => n,
        Ok(None) => return (json!({"error": "node not found"}), 404),
        Err(e) => return (json!({"error": e.to_string()}), 500),
    };
    if let Some(b) = new_body {
        node.body = b.to_string();
    }
    if let Some(c) = new_confidence {
        node.confidence = c.clamp(0.0, 1.0);
    }
    node.updated = chrono::Utc::now().date_naive();
    if let Err(e) = store.write_node(&node) {
        return (json!({"error": e.to_string()}), 500);
    }
    let message = format!("user: edit {id} (dashboard)");
    if let Err(e) = after_write(store_root, config, &scope, &message) {
        return (
            json!({"error": format!("change saved, but post-write processing failed: {e}")}),
            500,
        );
    }
    (json!({"ok": true}), 200)
}

/// `POST /api/project/exclude` — body `{"slug"}`. Records the project's
/// path in `privacy.exclude_projects` (so it is never re-registered), then
/// deletes its knowledge subtree and `CLAUDE.local.md`. No reprojection is
/// needed — cleanup removes the file outright rather than regenerating it.
fn api_project_exclude(
    store_root: &Path,
    _config: &Config,
    body: &serde_json::Value,
) -> (serde_json::Value, u16) {
    let Some(slug) = body.get("slug").and_then(|v| v.as_str()) else {
        return (json!({"error": "missing slug"}), 400);
    };
    if !retro_core::store::is_valid_slug(slug) {
        return (json!({"error": "invalid slug"}), 400);
    }

    let _lock = match acquire_write_lock(store_root) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let map = match retro_core::store::projects::PathMap::load(store_root) {
        Ok(m) => m,
        Err(e) => return (json!({"error": e.to_string()}), 500),
    };
    let Some(path) = map.paths.get(slug).cloned() else {
        return (json!({"error": "project not found"}), 404);
    };

    // Reload config from disk rather than mutating the server's startup-time
    // snapshot — saving a stale snapshot would silently revert any edits the
    // user made to config.toml while the server was running.
    let config_path = store_root.join("config.toml");
    let mut updated_config = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => return (json!({"error": e.to_string()}), 500),
    };
    if !updated_config
        .privacy
        .exclude_projects
        .iter()
        .any(|p| p == &path)
    {
        updated_config.privacy.exclude_projects.push(path.clone());
    }
    if let Err(e) = updated_config.save(&config_path) {
        return (json!({"error": e.to_string()}), 500);
    }

    let store = retro_core::store::Store::open(store_root);
    if let Err(e) = retro_core::store::projects::cleanup_excluded(&store, slug, Some(&path)) {
        return (json!({"error": e.to_string()}), 500);
    }

    let message = format!("retro: exclude {slug}");
    if let Err(e) = retro_core::store::git::commit_all(store_root, &message) {
        return (json!({"error": e.to_string()}), 500);
    }
    if let Err(e) = retro_core::store::index::build(&store) {
        if let Err(e2) = retro_core::health::record(store_root, "index", false, &e.to_string()) {
            return (json!({"error": e2.to_string()}), 500);
        }
    }

    (json!({"ok": true}), 200)
}

/// Minimal query-string parser: `key=value` pairs, last `?` onward.
/// Values are fully percent-decoded (the frontend builds URLs with
/// `URLSearchParams`, which encodes all non-ASCII and most specials).
fn query_param(url: &str, key: &str) -> Option<String> {
    url.split('?').nth(1)?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k == key {
            Some(percent_decode(v))
        } else {
            None
        }
    })
}

/// Decode `+` as space and `%XX` hex escapes; invalid escapes pass through
/// literally. Decoded bytes are interpreted as UTF-8 (lossy).
fn percent_decode(v: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        (b as char).to_digit(16).map(|d| d as u8)
    }
    let bytes = v.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push(hi << 4 | lo);
                    i += 3;
                }
                _ => {
                    out.push(b'%');
                    i += 1;
                }
            },
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn api_nodes(store_root: &Path, url: &str) -> (serde_json::Value, u16) {
    let conn = match retro_core::store::index::open(store_root) {
        Ok(c) => c,
        // 409: the index just hasn't been built yet (actionable by the user);
        // anything else (e.g. a corrupt index.db) is a server-side failure.
        Err(e @ retro_core::errors::CoreError::NotInitialized(_)) => {
            return (json!({"error": e.to_string()}), 409);
        }
        Err(e) => return (json!({"error": e.to_string()}), 500),
    };
    let filter = retro_core::store::index::NodeFilter {
        scope: query_param(url, "scope"),
        node_type: query_param(url, "type"),
        active_only: query_param(url, "active").as_deref() == Some("true"),
        text: query_param(url, "q"),
    };
    match retro_core::store::index::query(&conn, &filter) {
        Ok(rows) => (
            json!(
                rows.iter()
                    .map(|r| json!({
                        "id": r.id, "scope": r.scope, "type": r.node_type,
                        "confidence": r.confidence, "active": r.active,
                        "updated": r.updated,
                        // token estimate from the FULL body (the `body` field
                        // is truncated for transport) so the rule table's
                        // TOKENS column is honest, not capped at the preview.
                        "tokens_est": r.body.len() / 4,
                        "body": retro_core::util::truncate_str(&r.body, 200),
                        "sources": r.sources,
                    }))
                    .collect::<Vec<_>>()
            ),
            200,
        ),
        Err(e) => (json!({"error": e.to_string()}), 500),
    }
}

fn api_xray(store_root: &Path, config: &Config) -> (serde_json::Value, u16) {
    use retro_core::store::{Store, projects::PathMap};
    let est = |bytes: u64| bytes / 4; // rough tokens
    let file_info = |p: &Path| -> serde_json::Value {
        match std::fs::metadata(p) {
            Ok(m) => json!({"present": true, "bytes": m.len(), "tokens_est": est(m.len())}),
            Err(_) => json!({"present": false}),
        }
    };
    let store = Store::open(store_root);
    // A whole-load failure must not render as "0 nodes" with no explanation —
    // surface it through the same warnings channel per-file issues use.
    let loaded = store
        .load_all()
        .unwrap_or_else(|e| retro_core::store::LoadResult {
            nodes: vec![],
            warnings: vec![format!("store load failed: {e}")],
        });
    let map = PathMap::load(store_root).unwrap_or_default();
    let claude_dir = config.claude_dir();

    let mut projects_json = Vec::new();
    for (slug, path) in &map.paths {
        let root = Path::new(path);
        let node_count = loaded
            .nodes
            .iter()
            .filter(|(_, n)| matches!(&n.scope, retro_core::store::Scope::Project(s) if s == slug))
            .filter(|(_, n)| n.is_active())
            .count();
        // Auto-memory lives under claude_dir/projects/<encoded-path>/memory/
        // (encoding matches Claude Code's own directory naming, verified against
        // the shared helper used by observer.rs and ingest for the same purpose).
        let encoded = retro_core::ingest::encode_project_path(path);
        let memory = claude_dir
            .join("projects")
            .join(&encoded)
            .join("memory")
            .join("MEMORY.md");
        projects_json.push(json!({
            "slug": slug, "path": path,
            "claude_md": file_info(&root.join("CLAUDE.md")),
            "claude_local_md": file_info(&root.join("CLAUDE.local.md")),
            "memory_md": file_info(&memory),
            "active_nodes": node_count,
        }));
    }
    let skills_count = std::fs::read_dir(claude_dir.join("skills"))
        .map(|d| d.count())
        .unwrap_or(0);

    // Store-wide live/held/vetoed breakdown, independent of scope: vetoed is
    // any invalidated node; among the rest, confidence vs. the projection
    // gate splits live (projected) from held (below threshold, not yet
    // projected). Mirrors the same threshold `after_write` uses to reproject.
    let threshold = config.knowledge.confidence_threshold;
    let (mut live, mut held, mut vetoed) = (0usize, 0usize, 0usize);
    for (_, n) in &loaded.nodes {
        if !n.is_active() {
            vetoed += 1;
        } else if n.confidence < threshold {
            held += 1;
        } else {
            live += 1;
        }
    }

    (
        json!({
            "global_claude_md": file_info(&claude_dir.join("CLAUDE.md")),
            "global_active_nodes": loaded.nodes.iter()
                .filter(|(_, n)| n.is_active() && n.scope == retro_core::store::Scope::Global).count(),
            "skills_count": skills_count,
            "projects": projects_json,
            "store_warnings": loaded.warnings,
            "total_nodes": loaded.nodes.len(),
            "store": json!({"live": live, "held": held, "vetoed": vetoed}),
        }),
        200,
    )
}

fn api_node(store_root: &Path, url: &str) -> (serde_json::Value, u16) {
    let Some(scope_param) = query_param(url, "scope") else {
        return (json!({"error": "missing scope param"}), 400);
    };
    let Some(id) = query_param(url, "id") else {
        return (json!({"error": "missing id param"}), 400);
    };
    let scope = match retro_core::store::Scope::parse(&scope_param) {
        Ok(s) => s,
        Err(e) => return (json!({"error": e.to_string()}), 400),
    };
    if !retro_core::store::is_valid_slug(&id) {
        return (json!({"error": "invalid id"}), 400);
    }
    let store = retro_core::store::Store::open(store_root);
    match store.get(&scope, &id) {
        Ok(Some(node)) => {
            let path = store.node_path(&scope, &id);
            (
                json!({
                    "id": node.id,
                    "scope": node.scope.to_string(),
                    "type": node.node_type.as_str(),
                    "confidence": node.confidence,
                    "sources": node.sources,
                    "created": node.created.to_string(),
                    "updated": node.updated.to_string(),
                    "invalidated_by": node.invalidated_by,
                    "body": node.body,
                    "path": path.display().to_string(),
                }),
                200,
            )
        }
        Ok(None) => (json!({"error": "node not found"}), 404),
        // I/O failure or malformed frontmatter in an on-disk node file is a
        // server-state problem, not a bad request.
        Err(e) => (json!({"error": e.to_string()}), 500),
    }
}

fn api_health(store_root: &Path, config: &Config) -> (serde_json::Value, u16) {
    let health = retro_core::health::Health::load(store_root).unwrap_or_default();
    let queue_len = retro_core::store::queue::list(store_root)
        .map(|q| q.len())
        .unwrap_or(0);
    let state = retro_core::store::state::RunnerState::load(store_root).unwrap_or_default();
    let today = chrono::Utc::now().date_naive().to_string();
    let budget_remaining = state.budget_remaining(&today, config.runner.max_ai_calls_per_day);
    (
        json!({
            "stages": health.stages,
            "queue_len": queue_len,
            "budget_remaining": budget_remaining,
            "budget_max": config.runner.max_ai_calls_per_day,
            "notifications_pending": state.notifications.len(),
        }),
        200,
    )
}

fn api_history(store_root: &Path, url: &str) -> (serde_json::Value, u16) {
    let limit: usize = query_param(url, "limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(50)
        .min(500);
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(store_root)
        .args([
            "log",
            "--format=%h|%ad|%s",
            "--date=iso",
            "-n",
            &limit.to_string(),
        ])
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let entries: Vec<serde_json::Value> = text
                .lines()
                .filter_map(|line| {
                    let mut parts = line.splitn(3, '|');
                    let hash = parts.next()?;
                    let date = parts.next()?;
                    let subject = parts.next().unwrap_or("");
                    Some(json!({"hash": hash, "date": date, "subject": subject}))
                })
                .collect();
            (json!(entries), 200)
        }
        Ok(out) => (
            json!({"error": String::from_utf8_lossy(&out.stderr).to_string()}),
            500,
        ),
        Err(e) => (json!({"error": e.to_string()}), 500),
    }
}

fn api_doctor(store_root: &Path, config: &Config) -> (serde_json::Value, u16) {
    let report = retro_core::doctor::run_checks(store_root, config, false);
    match serde_json::to_value(&report) {
        Ok(v) => (v, 200),
        Err(e) => (json!({"error": e.to_string()}), 500),
    }
}

/// `GET /api/config` — the whitelisted subset of config fields the pipeline
/// actually reads (not the whole `Config` struct), plus the allowed `model`
/// values so the Config tab can render a picker rather than a free-text field.
fn api_config_get(config: &Config) -> (serde_json::Value, u16) {
    (
        json!({
            "confidence_threshold": config.knowledge.confidence_threshold,
            "max_ai_calls_per_day": config.runner.max_ai_calls_per_day,
            "model": config.ai.model,
            "models": ALLOWED_MODELS,
        }),
        200,
    )
}

/// `POST /api/config` — body is a partial patch over the same whitelist
/// `api_config_get` exposes. Present-but-invalid values 400 (validated
/// up front, before touching the lock or config file, mirroring the other
/// write handlers' untrusted-input discipline); absent keys are left
/// untouched; unknown keys are silently ignored; an empty patch is a 200
/// no-op. Returns the post-save `api_config_get` body.
fn api_config_post(store_root: &Path, body: &serde_json::Value) -> (serde_json::Value, u16) {
    let confidence_threshold = match body.get("confidence_threshold") {
        None => None,
        Some(v) => match v.as_f64() {
            Some(f) if (0.0..=1.0).contains(&f) => Some(f),
            _ => {
                return (
                    json!({"error": "confidence_threshold must be a number in [0.0, 1.0]"}),
                    400,
                );
            }
        },
    };
    let max_ai_calls_per_day = match body.get("max_ai_calls_per_day") {
        None => None,
        Some(v) => match v.as_u64() {
            Some(n) if n <= 1000 => Some(n as u32),
            _ => {
                return (
                    json!({"error": "max_ai_calls_per_day must be an integer in [0, 1000]"}),
                    400,
                );
            }
        },
    };
    let model = match body.get("model") {
        None => None,
        Some(v) => match v.as_str() {
            Some(s) if ALLOWED_MODELS.contains(&s) => Some(s.to_string()),
            _ => {
                return (
                    json!({"error": format!("model must be one of {ALLOWED_MODELS:?}")}),
                    400,
                );
            }
        },
    };

    // Nothing to change → don't take the lock or rewrite config.toml.
    // Config::save reformats the whole file (dropping comments/legacy keys),
    // so a no-op patch must stay a true no-op.
    if confidence_threshold.is_none() && max_ai_calls_per_day.is_none() && model.is_none() {
        return match Config::load(&store_root.join("config.toml")) {
            Ok(c) => api_config_get(&c),
            Err(e) => (json!({"error": e.to_string()}), 500),
        };
    }

    let _lock = match acquire_write_lock(store_root) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    // Reload from disk rather than trusting a startup-time snapshot — same
    // reasoning as api_project_exclude: saving a stale in-memory config
    // would silently revert any concurrent edits to config.toml.
    let config_path = store_root.join("config.toml");
    let mut config = match Config::load(&config_path) {
        Ok(c) => c,
        Err(e) => return (json!({"error": e.to_string()}), 500),
    };
    // The threshold is the projection gate — a change alters what every
    // managed file contains, so note it and reproject below.
    let threshold_changed = confidence_threshold
        .map(|t| (t - config.knowledge.confidence_threshold).abs() > f64::EPSILON)
        .unwrap_or(false);
    if let Some(t) = confidence_threshold {
        config.knowledge.confidence_threshold = t;
    }
    if let Some(n) = max_ai_calls_per_day {
        config.runner.max_ai_calls_per_day = n;
    }
    if let Some(m) = model {
        config.ai.model = m;
    }
    if let Err(e) = config.save(&config_path) {
        return (json!({"error": e.to_string()}), 500);
    }
    // config.toml is git-tracked in the store; commit it as its own labeled
    // mutation so the next commit_all (another handler, or the runner's
    // opening sweep) can't fold it into an unrelated, pushed commit.
    if let Err(e) = retro_core::store::git::commit_all(store_root, "user: config update (dashboard)") {
        return (json!({"error": e.to_string()}), 500);
    }
    // Reproject every managed file so a threshold change is reflected
    // immediately, not only after the next runner pass. (Budget/model
    // changes affect future runs only — no reprojection needed.)
    if threshold_changed {
        if let Err(e) = reproject_all(store_root, &config) {
            return (
                json!({"error": format!("config saved, but reprojection failed: {e}")}),
                500,
            );
        }
    }
    api_config_get(&config)
}

/// Reproject the global managed block and every registered project's
/// `CLAUDE.local.md` at the current threshold. Used after a threshold change
/// (nodes are unchanged, so no index rebuild). Mirrors the runner's and
/// migrate's projection step.
fn reproject_all(
    store_root: &Path,
    config: &Config,
) -> Result<(), retro_core::errors::CoreError> {
    use retro_core::store::{Store, projects::PathMap};
    let store = Store::open(store_root);
    let threshold = config.knowledge.confidence_threshold;
    retro_core::projection::local_md::project_global_md(
        &store,
        &config.claude_dir().join("CLAUDE.md"),
        threshold,
        Some(&store_root.join("backups")),
    )?;
    let map = PathMap::load(store_root)?;
    for (slug, p) in &map.paths {
        retro_core::projection::local_md::project_local_md(&store, slug, Path::new(p), threshold)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use retro_core::store::{Node, NodeType, Scope, Store, index, projects::PathMap};
    use tempfile::TempDir;

    fn node(id: &str, scope: Scope, t: NodeType, body: &str) -> Node {
        Node {
            id: id.to_string(),
            scope,
            node_type: t,
            confidence: 0.8,
            sources: vec!["session:abc".to_string()],
            created: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            updated: NaiveDate::from_ymd_opt(2026, 7, 1).unwrap(),
            invalidated_by: None,
            body: body.to_string(),
        }
    }

    #[test]
    fn nodes_endpoint_filters_and_409s_without_index() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();

        // no index built yet -> 409 with a helpful error
        let (body, status) = api_nodes(tmp.path(), "/api/nodes");
        assert_eq!(status, 409);
        assert!(body["error"].as_str().unwrap().contains("index not built"));

        store
            .write_node(&node(
                "g-rule",
                Scope::Global,
                NodeType::Rule,
                "always run smoke tests first",
            ))
            .unwrap();
        store
            .write_node(&node(
                "p-pattern",
                Scope::Project("my-proj".to_string()),
                NodeType::Pattern,
                "paired observations",
            ))
            .unwrap();
        index::build(&store).unwrap();

        let (body, status) = api_nodes(tmp.path(), "/api/nodes");
        assert_eq!(status, 200);
        assert_eq!(body.as_array().unwrap().len(), 2);

        let (body, status) = api_nodes(tmp.path(), "/api/nodes?scope=global");
        assert_eq!(status, 200);
        let rows = body.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "g-rule");

        let (body, status) = api_nodes(tmp.path(), "/api/nodes?type=pattern");
        assert_eq!(status, 200);
        assert_eq!(body.as_array().unwrap().len(), 1);
    }

    #[test]
    fn xray_lists_projects_and_globals() {
        let store_tmp = TempDir::new().unwrap();
        let claude_tmp = TempDir::new().unwrap();
        let proj_tmp = TempDir::new().unwrap();

        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();
        store
            .write_node(&node(
                "global-rule",
                Scope::Global,
                NodeType::Rule,
                "global",
            ))
            .unwrap();
        store
            .write_node(&node(
                "proj-rule",
                Scope::Project("my-proj".to_string()),
                NodeType::Rule,
                "proj",
            ))
            .unwrap();

        let mut map = PathMap::default();
        map.paths.insert(
            "my-proj".to_string(),
            proj_tmp.path().to_str().unwrap().to_string(),
        );
        map.save(store_tmp.path()).unwrap();

        std::fs::write(proj_tmp.path().join("CLAUDE.md"), "# hi\n").unwrap();
        std::fs::write(claude_tmp.path().join("CLAUDE.md"), "# global claude\n").unwrap();
        std::fs::create_dir_all(claude_tmp.path().join("skills/one")).unwrap();
        std::fs::create_dir_all(claude_tmp.path().join("skills/two")).unwrap();
        // auto-memory at claude_dir/projects/<encoded-path>/memory/MEMORY.md
        let encoded =
            retro_core::ingest::encode_project_path(proj_tmp.path().to_str().unwrap());
        let memory_dir = claude_tmp.path().join("projects").join(&encoded).join("memory");
        std::fs::create_dir_all(&memory_dir).unwrap();
        std::fs::write(memory_dir.join("MEMORY.md"), "# memory\n").unwrap();

        let mut config = Config::default();
        config.paths.claude_dir = claude_tmp.path().display().to_string();

        let (body, status) = api_xray(store_tmp.path(), &config);
        assert_eq!(status, 200);
        assert_eq!(body["global_active_nodes"], 1);
        assert_eq!(body["skills_count"], 2);
        assert!(body["global_claude_md"]["present"].as_bool().unwrap());
        let projects = body["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["slug"], "my-proj");
        assert_eq!(projects[0]["active_nodes"], 1);
        assert!(projects[0]["claude_md"]["present"].as_bool().unwrap());
        assert!(projects[0]["memory_md"]["present"].as_bool().unwrap());
    }

    #[test]
    fn xray_reports_total_and_store_breakdown() {
        let store_tmp = TempDir::new().unwrap();
        let claude_tmp = TempDir::new().unwrap();
        let store = Store::open(store_tmp.path());
        store.ensure_layout().unwrap();

        let mut live = node("live-rule", Scope::Global, NodeType::Rule, "a live rule");
        live.confidence = 0.9;
        store.write_node(&live).unwrap();

        let mut held = node("held-rule", Scope::Global, NodeType::Rule, "a held rule");
        held.confidence = 0.3;
        store.write_node(&held).unwrap();

        let mut vetoed = node(
            "vetoed-rule",
            Scope::Global,
            NodeType::Rule,
            "a vetoed rule",
        );
        vetoed.invalidated_by = Some("user".to_string());
        store.write_node(&vetoed).unwrap();

        let mut config = Config::default();
        config.paths.claude_dir = claude_tmp.path().display().to_string();
        config.knowledge.confidence_threshold = 0.7;

        let (body, status) = api_xray(store_tmp.path(), &config);
        assert_eq!(status, 200, "{body:?}");
        assert_eq!(body["total_nodes"], 3);
        assert_eq!(body["store"], json!({"live": 1, "held": 1, "vetoed": 1}));
    }

    #[test]
    fn history_parses_git_log() {
        let tmp = TempDir::new().unwrap();
        retro_core::store::git::ensure_repo(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("note.md"), "hello").unwrap();
        retro_core::store::git::commit_all(tmp.path(), "test: add note").unwrap();
        // pipes in the subject must survive the %h|%ad|%s split
        std::fs::write(tmp.path().join("note.md"), "hello again").unwrap();
        retro_core::store::git::commit_all(tmp.path(), "test: piped | subject | here").unwrap();

        let (body, status) = api_history(tmp.path(), "/api/history?limit=2");
        assert_eq!(status, 200);
        let entries = body.as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["subject"], "test: piped | subject | here");
        assert_eq!(entries[1]["subject"], "test: add note");
        assert!(entries[0]["hash"].as_str().unwrap().len() >= 7);
        assert!(!entries[0]["date"].as_str().unwrap().is_empty());
    }

    #[test]
    fn node_endpoint_validates_id_and_decodes_params() {
        let tmp = TempDir::new().unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        store
            .write_node(&node("real-rule", Scope::Global, NodeType::Rule, "a rule"))
            .unwrap();

        // path traversal attempts are rejected before touching the store
        let (body, status) =
            api_node(tmp.path(), "/api/node?scope=global&id=..%2F..%2F..%2Fetc%2Fpasswd");
        assert_eq!(status, 400);
        assert_eq!(body["error"], "invalid id");
        let (_, status) = api_node(tmp.path(), "/api/node?scope=global&id=UPPER");
        assert_eq!(status, 400);

        // missing params -> 400, unknown-but-valid slug -> 404, valid -> 200
        let (_, status) = api_node(tmp.path(), "/api/node?scope=global");
        assert_eq!(status, 400);
        let (_, status) = api_node(tmp.path(), "/api/node?scope=global&id=no-such-node");
        assert_eq!(status, 404);
        let (body, status) = api_node(tmp.path(), "/api/node?scope=global&id=real-rule");
        assert_eq!(status, 200);
        assert_eq!(body["id"], "real-rule");

        // full percent-decoding (URLSearchParams-style encoding)
        assert_eq!(percent_decode("caf%C3%A9+au%20lait"), "café au lait");
        assert_eq!(percent_decode("100%"), "100%"); // truncated escape passes through
    }

    /// `ensure_layout` + an initialized git repo, ready for write-endpoint tests.
    fn store_with_repo(tmp: &TempDir) -> Store {
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        retro_core::store::git::ensure_repo(tmp.path()).unwrap();
        store
    }

    fn commit_count(root: &Path) -> usize {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-list", "--count", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap()
    }

    #[test]
    fn invalidate_flow_marks_inactive_commits_and_reprojects() {
        let store_tmp = TempDir::new().unwrap();
        let claude_tmp = TempDir::new().unwrap();
        let store = store_with_repo(&store_tmp);
        store
            .write_node(&node(
                "old-rule",
                Scope::Global,
                NodeType::Rule,
                "always run smoke tests first",
            ))
            .unwrap();
        index::build(&store).unwrap();

        let mut config = Config::default();
        config.paths.claude_dir = claude_tmp.path().display().to_string();

        // seed an initial projection so we can prove the rule is REMOVED,
        // not merely never written
        let claude_md_path = claude_tmp.path().join("CLAUDE.md");
        retro_core::projection::local_md::project_global_md(
            &store,
            &claude_md_path,
            config.knowledge.confidence_threshold,
            None,
        )
        .unwrap();
        let before = std::fs::read_to_string(&claude_md_path).unwrap();
        assert!(before.contains("always run smoke tests first"), "got: {before}");

        let commits_before = commit_count(store_tmp.path());
        let (body, status) = api_node_invalidate(
            store_tmp.path(),
            &config,
            &json!({"scope": "global", "id": "old-rule"}),
        );
        assert_eq!(status, 200, "body: {body}");
        assert_eq!(body["ok"], true);

        let n = store.get(&Scope::Global, "old-rule").unwrap().unwrap();
        assert!(!n.is_active());
        assert_eq!(n.invalidated_by.as_deref(), Some("user"));

        assert!(commit_count(store_tmp.path()) > commits_before);

        let after = std::fs::read_to_string(&claude_md_path).unwrap();
        assert!(
            !after.contains("always run smoke tests first"),
            "got: {after}"
        );
    }

    #[test]
    fn update_flow_changes_body_clamps_confidence_and_reprojects() {
        let store_tmp = TempDir::new().unwrap();
        let claude_tmp = TempDir::new().unwrap();
        let store = store_with_repo(&store_tmp);
        store
            .write_node(&node(
                "my-rule",
                Scope::Global,
                NodeType::Rule,
                "original body",
            ))
            .unwrap();
        index::build(&store).unwrap();

        let mut config = Config::default();
        config.paths.claude_dir = claude_tmp.path().display().to_string();

        let (body, status) = api_node_update(
            store_tmp.path(),
            &config,
            &json!({
                "scope": "global", "id": "my-rule",
                "body": "new body text", "confidence": 5.0,
            }),
        );
        assert_eq!(status, 200, "body: {body}");
        assert_eq!(body["ok"], true);

        let n = store.get(&Scope::Global, "my-rule").unwrap().unwrap();
        assert_eq!(n.body, "new body text");
        assert_eq!(n.confidence, 1.0, "confidence must clamp to 1.0, not error");

        let claude_md =
            std::fs::read_to_string(claude_tmp.path().join("CLAUDE.md")).unwrap();
        assert!(claude_md.contains("new body text"), "got: {claude_md}");
        assert!(!claude_md.contains("original body"));
    }

    #[test]
    fn update_rejects_empty_body_and_requires_a_field() {
        let store_tmp = TempDir::new().unwrap();
        let store = store_with_repo(&store_tmp);
        store
            .write_node(&node(
                "my-rule",
                Scope::Global,
                NodeType::Rule,
                "original body",
            ))
            .unwrap();
        index::build(&store).unwrap();
        let config = Config::default();

        let (body, status) = api_node_update(
            store_tmp.path(),
            &config,
            &json!({"scope": "global", "id": "my-rule", "body": ""}),
        );
        assert_eq!(status, 400);
        assert_eq!(body["error"], "body must not be empty");

        let (body, status) = api_node_update(
            store_tmp.path(),
            &config,
            &json!({"scope": "global", "id": "my-rule"}),
        );
        assert_eq!(status, 400);
        assert_eq!(body["error"], "must provide body and/or confidence");
    }

    #[test]
    fn exclude_flow_updates_config_and_removes_knowledge_dir() {
        let store_tmp = TempDir::new().unwrap();
        let proj_tmp = TempDir::new().unwrap();
        let store = store_with_repo(&store_tmp);
        store
            .write_node(&node(
                "proj-rule",
                Scope::Project("my-proj".to_string()),
                NodeType::Rule,
                "a project rule",
            ))
            .unwrap();
        index::build(&store).unwrap();

        let mut map = PathMap::default();
        map.paths.insert(
            "my-proj".to_string(),
            proj_tmp.path().to_str().unwrap().to_string(),
        );
        map.save(store_tmp.path()).unwrap();

        let config = Config::default();
        let commits_before = commit_count(store_tmp.path());

        let (body, status) =
            api_project_exclude(store_tmp.path(), &config, &json!({"slug": "my-proj"}));
        assert_eq!(status, 200, "body: {body}");
        assert_eq!(body["ok"], true);

        let saved = Config::load(&store_tmp.path().join("config.toml")).unwrap();
        assert!(
            saved
                .privacy
                .exclude_projects
                .iter()
                .any(|p| p == proj_tmp.path().to_str().unwrap()),
            "got: {:?}",
            saved.privacy.exclude_projects
        );

        assert!(
            !store_tmp
                .path()
                .join("knowledge/projects/my-proj")
                .exists()
        );
        let map = PathMap::load(store_tmp.path()).unwrap();
        assert!(!map.paths.contains_key("my-proj"));

        assert!(commit_count(store_tmp.path()) > commits_before);
    }

    #[test]
    fn write_endpoints_validate_ids_scopes_and_slugs() {
        let store_tmp = TempDir::new().unwrap();
        let store = store_with_repo(&store_tmp);
        index::build(&store).unwrap();
        let config = Config::default();

        // path traversal id -> 400, rejected before any store call
        let (body, status) = api_node_invalidate(
            store_tmp.path(),
            &config,
            &json!({"scope": "global", "id": "../../etc/passwd"}),
        );
        assert_eq!(status, 400);
        assert_eq!(body["error"], "invalid id");

        let (body, status) = api_node_update(
            store_tmp.path(),
            &config,
            &json!({"scope": "global", "id": "../../etc/passwd", "body": "x"}),
        );
        assert_eq!(status, 400);
        assert_eq!(body["error"], "invalid id");

        // unknown-but-valid id -> 404
        let (_, status) = api_node_invalidate(
            store_tmp.path(),
            &config,
            &json!({"scope": "global", "id": "no-such-node"}),
        );
        assert_eq!(status, 404);
        let (_, status) = api_node_update(
            store_tmp.path(),
            &config,
            &json!({"scope": "global", "id": "no-such-node", "body": "x"}),
        );
        assert_eq!(status, 404);

        // invalid scope string -> 400
        let (_, status) = api_node_invalidate(
            store_tmp.path(),
            &config,
            &json!({"scope": "team/x", "id": "whatever"}),
        );
        assert_eq!(status, 400);

        // exclude: invalid slug -> 400, unknown-but-valid slug -> 404
        let (body, status) =
            api_project_exclude(store_tmp.path(), &config, &json!({"slug": "../etc"}));
        assert_eq!(status, 400);
        assert_eq!(body["error"], "invalid slug");
        let (_, status) = api_project_exclude(
            store_tmp.path(),
            &config,
            &json!({"slug": "no-such-project"}),
        );
        assert_eq!(status, 404);
    }

    #[test]
    fn parse_json_body_rejects_malformed_and_oversized() {
        let mut cursor = std::io::Cursor::new(b"{not valid json".to_vec());
        let (body, status) = parse_json_body(&mut cursor).unwrap_err();
        assert_eq!(status, 400);
        assert!(body["error"].as_str().unwrap().contains("malformed JSON"));

        let huge = vec![b'a'; (MAX_BODY_BYTES + 10) as usize];
        let mut cursor = std::io::Cursor::new(huge);
        let (body, status) = parse_json_body(&mut cursor).unwrap_err();
        assert_eq!(status, 400);
        assert!(body["error"].as_str().unwrap().contains("too large"));
    }

    #[test]
    fn config_get_returns_whitelisted_fields() {
        // Pure function over `&Config` — no filesystem/store touched, so
        // unlike the other config tests this needs no TempDir.
        let mut config = Config::default();
        config.knowledge.confidence_threshold = 0.7;
        config.runner.max_ai_calls_per_day = 10;
        config.ai.model = "sonnet".to_string();
        let (body, status) = api_config_get(&config);
        assert_eq!(status, 200);
        assert_eq!(body["confidence_threshold"], 0.7);
        assert_eq!(body["max_ai_calls_per_day"], 10);
        assert_eq!(body["model"], "sonnet");
        assert!(
            body["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m == "sonnet")
        );
    }

    #[test]
    fn config_post_persists_whitelisted_fields_and_preserves_the_rest() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.toml");
        let mut base = Config::default();
        base.ui.port = 9191; // a non-whitelisted field that MUST survive
        // MUST isolate claude_dir: a threshold change reprojects, and the
        // default "~/.claude" would target the developer's REAL global
        // CLAUDE.md during `cargo test` (this exact leak wiped a user's file
        // on 2026-07-23 before the empty-wipe guard caught it).
        base.paths.claude_dir = claude.path().display().to_string();
        base.save(&cfg_path).unwrap();
        retro_core::store::git::ensure_repo(tmp.path()).unwrap();

        // a non-whitelisted key in the body is silently ignored, not applied
        let (body, status) = api_config_post(
            tmp.path(),
            &json!({"confidence_threshold": 0.85, "max_ai_calls_per_day": 20, "model": "haiku", "port": 1}),
        );
        assert_eq!(status, 200, "{body:?}");
        let reloaded = Config::load(&cfg_path).unwrap();
        assert!((reloaded.knowledge.confidence_threshold - 0.85).abs() < 1e-9);
        assert_eq!(reloaded.runner.max_ai_calls_per_day, 20);
        assert_eq!(reloaded.ai.model, "haiku");
        assert_eq!(reloaded.ui.port, 9191, "non-whitelisted field preserved");
        // config.toml is committed (its own labeled mutation), tree clean
        assert!(!retro_core::store::git::has_changes(tmp.path()).unwrap());
    }

    #[test]
    fn config_post_validates_ranges_and_model() {
        let tmp = TempDir::new().unwrap();
        Config::default().save(&tmp.path().join("config.toml")).unwrap();
        // reject cases 400 during up-front validation, before the lock/repo,
        // so no git repo is needed here
        for bad in [
            json!({"confidence_threshold": 1.5}),
            json!({"confidence_threshold": -0.1}),
            json!({"confidence_threshold": "0.7"}), // wrong type
            json!({"max_ai_calls_per_day": 100000}),
            json!({"max_ai_calls_per_day": 3.5}), // float
            json!({"max_ai_calls_per_day": -1}),  // negative
            json!({"model": "gpt-4"}),
        ] {
            assert_eq!(api_config_post(tmp.path(), &bad).1, 400, "{bad:?}");
        }
        // empty patch is a no-op success and does NOT rewrite/commit anything
        assert_eq!(api_config_post(tmp.path(), &json!({})).1, 200);
    }

    #[test]
    fn config_post_rejects_multi_field_patch_without_partial_write() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.toml");
        Config::default().save(&cfg_path).unwrap();
        retro_core::store::git::ensure_repo(tmp.path()).unwrap();
        let before = Config::load(&cfg_path).unwrap().knowledge.confidence_threshold;
        // first field valid, second invalid → whole patch rejected, nothing saved
        let (_, status) = api_config_post(
            tmp.path(),
            &json!({"confidence_threshold": 0.9, "model": "gpt-4"}),
        );
        assert_eq!(status, 400);
        let after = Config::load(&cfg_path).unwrap().knowledge.confidence_threshold;
        assert!((before - after).abs() < 1e-9, "no partial write");
    }

    #[test]
    fn config_post_threshold_change_commits_and_reprojects() {
        let tmp = TempDir::new().unwrap();
        let claude = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.toml");
        let mut base = Config::default();
        base.paths.claude_dir = claude.path().display().to_string();
        base.knowledge.confidence_threshold = 0.9; // starts high
        base.save(&cfg_path).unwrap();
        let store = Store::open(tmp.path());
        store.ensure_layout().unwrap();
        // a node at 0.80 — below the 0.9 start, at/above a 0.7 target
        store
            .write_node(&node("mid", Scope::Global, NodeType::Rule, "a promotable rule"))
            .unwrap();
        retro_core::store::git::ensure_repo(tmp.path()).unwrap();
        index::build(&store).unwrap();

        // lower the threshold so the 0.80 node now projects
        let (_, status) = api_config_post(tmp.path(), &json!({"confidence_threshold": 0.7}));
        assert_eq!(status, 200);
        let projected = std::fs::read_to_string(claude.path().join("CLAUDE.md")).unwrap();
        assert!(projected.contains("a promotable rule"), "reprojected at new threshold");
        // config change is its own labeled commit
        let log = std::process::Command::new("git")
            .args(["-C", tmp.path().to_str().unwrap(), "log", "--format=%s", "-1"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "user: config update (dashboard)");
    }
}
