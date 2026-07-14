//! v3 dashboard: sync tiny_http server, localhost-only, single embedded page.
//! Read APIs serve store/index/health/state; write APIs go through the store
//! (file edit -> commit -> reindex -> reproject).

pub mod api;

use anyhow::Result;
use retro_core::config::Config;
use std::path::PathBuf;

const INDEX_HTML: &str = include_str!("assets/index.html");

/// Serve until the process is killed (Ctrl+C). Binds 127.0.0.1 only.
pub fn serve(store_root: PathBuf, config: Config) -> Result<()> {
    let addr = format!("127.0.0.1:{}", config.ui.port);
    let server =
        tiny_http::Server::http(&addr).map_err(|e| anyhow::anyhow!("cannot bind {addr}: {e}"))?;
    println!("retro dashboard: http://{addr}  (Ctrl+C to stop)");

    for request in server.incoming_requests() {
        let url = request.url().to_string();
        let method = request.method().clone();
        let response = api::route(&store_root, &config, &method, &url, request);
        if let Err(e) = response {
            eprintln!("ui: request error: {e}");
        }
    }
    Ok(())
}

pub(crate) fn html_response(body: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let mut resp = tiny_http::Response::from_string(body);
    resp.add_header(
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
            .unwrap(),
    );
    resp
}

pub(crate) fn json_response(
    value: &serde_json::Value,
    status: u16,
) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let mut resp = tiny_http::Response::from_string(value.to_string());
    resp.add_header(
        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap(),
    );
    resp.with_status_code(status)
}

pub(crate) fn index_html() -> &'static str {
    INDEX_HTML
}
