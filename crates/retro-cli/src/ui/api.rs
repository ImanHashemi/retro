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
        _ => request.respond(json_response(&json!({"error": "not found"}), 404))?,
    }
    Ok(())
}
