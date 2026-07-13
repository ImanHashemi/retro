//! Dashboard JSON API. All handlers are synchronous and read the same
//! retro-core modules the CLI uses.

use std::path::Path;

use anyhow::Result;
use retro_core::config::Config;
use serde_json::json;
use tiny_http::{Method, Request};

use super::{html_response, index_html, json_response};

pub fn route(
    store_root: &Path,
    config: &Config,
    method: &Method,
    url: &str,
    request: Request,
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
        _ => request.respond(json_response(&json!({"error": "not found"}), 404))?,
    }
    Ok(())
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
    let loaded = store.load_all().unwrap_or(retro_core::store::LoadResult {
        nodes: vec![],
        warnings: vec![],
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
    (
        json!({
            "global_claude_md": file_info(&claude_dir.join("CLAUDE.md")),
            "global_active_nodes": loaded.nodes.iter()
                .filter(|(_, n)| n.is_active() && n.scope == retro_core::store::Scope::Global).count(),
            "skills_count": skills_count,
            "projects": projects_json,
            "store_warnings": loaded.warnings,
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
}
