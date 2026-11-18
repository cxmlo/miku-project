pub mod security;
pub mod types;
pub mod ui_static;
pub mod ws_ui;

#[cfg(test)]
mod tests_6_2;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use miku_core::OrchestratorHandle;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::watch;

use crate::{
    compute_websocket_accept_key, handle_authenticated_ws_stream, GatewayError, SessionRegistry,
    MAX_HTTP_UPGRADE_HEADER_BYTES, MIKU_LINK_WS_PATH,
};

pub use security::{
    extract_session_token, generate_ephemeral_session_token, has_valid_session_token,
    is_allowed_lan_host, is_allowed_loopback_host, is_allowed_origin, sanitize_relative_ui_path,
};
pub use types::{
    DefaultGatewayApiHandler, HttpRequestHead, HttpResponse, LoopbackApiHandler,
    LoopbackServerConfig,
};
pub use ui_static::{
    mime_type_for_path, resolve_default_ui_dir, resolve_user_ui_override_dir, serve_ui_asset,
    LOOPBACK_UI_CSP,
};

const MAX_HTTP_BODY_BYTES: usize = 2 * 1024 * 1024;
pub const UI_WS_PATH: &str = "/ws";

pub struct LoopbackContext {
    pub session_token: String,
    pub user_override_dir: Option<PathBuf>,
    pub default_ui_dir: PathBuf,
    pub handler: Arc<dyn LoopbackApiHandler>,
    pub allow_lan_pairing: bool,
}

impl LoopbackContext {
    pub fn from_config(cfg: LoopbackServerConfig, orchestrator: OrchestratorHandle) -> Self {
        let session_token = cfg
            .session_token
            .unwrap_or_else(generate_ephemeral_session_token);
        let user_override_dir = cfg.user_override_dir.or_else(resolve_user_ui_override_dir);
        let default_ui_dir = cfg.default_ui_dir.unwrap_or_else(resolve_default_ui_dir);
        let handler = cfg
            .api_handler
            .unwrap_or_else(|| Arc::new(DefaultGatewayApiHandler::new(orchestrator)));
        Self {
            session_token,
            user_override_dir,
            default_ui_dir,
            handler,
            allow_lan_pairing: cfg.allow_lan_pairing,
        }
    }
}

fn status_reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Internal Server Error",
    }
}

async fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequestHead, GatewayError> {
    let mut buf = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];

    while !buf.ends_with(b"\r\n\r\n") {
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        if n == 0 {
            return Err(GatewayError::ConnectionClosed);
        }
        buf.push(byte[0]);
        if buf.len() > MAX_HTTP_UPGRADE_HEADER_BYTES {
            return Err(GatewayError::InvalidHttpUpgrade(
                "HTTP header exceeded 4096 bytes".into(),
            ));
        }
    }

    let head_str = String::from_utf8(buf)
        .map_err(|_| GatewayError::InvalidHttpUpgrade("non-UTF8 HTTP header".into()))?;
    let mut lines = head_str.lines();
    let req_line = lines
        .next()
        .ok_or_else(|| GatewayError::InvalidHttpUpgrade("empty request".into()))?;
    let parts: Vec<&str> = req_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(GatewayError::InvalidHttpUpgrade("malformed request line".into()));
    }

    let method = parts[0].to_ascii_uppercase();
    let raw_target = parts[1];
    let (path, query) = match raw_target.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (raw_target.to_string(), None),
    };

    let mut headers = Vec::new();
    let mut content_len: usize = 0;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().to_string();
            if key == "content-length" {
                content_len = val.parse::<usize>().unwrap_or(0);
            }
            headers.push((key, val));
        }
    }

    if content_len > MAX_HTTP_BODY_BYTES {
        return Err(GatewayError::FrameTooLarge {
            declared_bytes: content_len,
            max_bytes: MAX_HTTP_BODY_BYTES,
        });
    }

    let mut body = vec![0u8; content_len];
    if content_len > 0 {
        stream
            .read_exact(&mut body)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
    }

    Ok(HttpRequestHead {
        method,
        path,
        query,
        headers,
        body,
    })
}

async fn write_response(stream: &mut TcpStream, resp: &HttpResponse) -> Result<(), GatewayError> {
    stream
        .write_all(&resp.to_bytes())
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))?;
    stream
        .flush()
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))
}

pub async fn dispatch_connection(
    mut stream: TcpStream,
    orchestrator: OrchestratorHandle,
    registry: Arc<SessionRegistry>,
    handshake_timeout: Duration,
    shutdown_rx: watch::Receiver<bool>,
    ctx: Arc<LoopbackContext>,
) -> Result<(), GatewayError> {
    let req = match tokio::time::timeout(handshake_timeout, read_http_request(&mut stream)).await {
        Ok(Ok(r)) => r,
        Ok(Err(GatewayError::FrameTooLarge { .. })) => {
            let resp = HttpResponse::json(
                413,
                "Payload Too Large",
                json!({ "error": "payload_too_large" }),
            );
            let _ = write_response(&mut stream, &resp).await;
            return Ok(());
        }
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(GatewayError::HandshakeTimeout),
    };

    // Route 1: Legacy & mobile Miku-Link WebSocket (/link)
    if req.method == "GET" && req.path == MIKU_LINK_WS_PATH && req.is_websocket_upgrade() {
        let key = req.header("sec-websocket-key").unwrap_or("");
        let accept = compute_websocket_accept_key(key);
        let upgrade = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept
        );
        stream
            .write_all(upgrade.as_bytes())
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        stream
            .flush()
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        return handle_authenticated_ws_stream(
            stream,
            orchestrator,
            registry,
            handshake_timeout,
            shutdown_rx,
        )
        .await;
    }

    // Strict Host/Origin check: loopback always allowed; LAN private IPs allowed only when allow_lan_pairing=true
    let is_loopback_host = is_allowed_loopback_host(req.header("host"));
    let is_lan_host = !is_loopback_host
        && ctx.allow_lan_pairing
        && is_allowed_lan_host(req.header("host"));
    if (!is_loopback_host && !is_lan_host) || !is_allowed_origin(req.header("origin")) {
        let resp = HttpResponse::json(
            403,
            "Forbidden",
            json!({ "error": "forbidden_host_or_origin" }),
        );
        let _ = write_response(&mut stream, &resp).await;
        return Ok(());
    }

    // Route 2: Live Dynamic Island & Phone Link UI WebSocket (/ws)
    if req.method == "GET" && req.path == UI_WS_PATH && req.is_websocket_upgrade() {
        let desktop_ws = is_loopback_host
            && has_valid_session_token(&req.headers, req.query.as_deref(), &ctx.session_token);
        let mobile_ws = (is_loopback_host || is_lan_host)
            && ctx.handler.authorize_mobile_request(
                "WS",
                UI_WS_PATH,
                &req.headers,
                req.query.as_deref(),
            );
        if !desktop_ws && !mobile_ws {
            let resp = HttpResponse::json(
                401,
                "Unauthorized",
                json!({ "error": "missing_or_invalid_session_token" }),
            );
            let _ = write_response(&mut stream, &resp).await;
            return Ok(());
        }
        let ws_key = req.header("sec-websocket-key").unwrap_or("").to_string();
        return ws_ui::handle_ui_ws_connection(stream, &ws_key, ctx, shutdown_rx).await;
    }

    // Route 3: Loopback & Mobile Phone Link API (/api/*)
    if req.path.starts_with("/api/") {
        let desktop_api = is_loopback_host
            && has_valid_session_token(&req.headers, req.query.as_deref(), &ctx.session_token);
        let mobile_public =
            (is_loopback_host || is_lan_host) && ctx.handler.is_lan_public_endpoint(&req.method, &req.path);
        let mobile_api = !desktop_api
            && !mobile_public
            && (is_loopback_host || is_lan_host)
            && ctx.handler.authorize_mobile_request(
                &req.method,
                &req.path,
                &req.headers,
                req.query.as_deref(),
            );
        if !desktop_api && !mobile_public && !mobile_api {
            let resp = HttpResponse::json(
                401,
                "Unauthorized",
                json!({ "error": "missing_or_invalid_session_token" }),
            );
            let _ = write_response(&mut stream, &resp).await;
            return Ok(());
        }
        let handler = Arc::clone(&ctx.handler);
        let method = req.method;
        let path = req.path;
        let query = req.query;
        let body = req.body;
        let (code, val) = tokio::task::spawn_blocking(move || {
            handler.handle_api(&method, &path, query.as_deref(), &body)
        })
        .await
        .unwrap_or_else(|e| (500, json!({ "error": format!("api task panic: {}", e) })));
        let resp = HttpResponse::json(code, status_reason(code), val);
        let _ = write_response(&mut stream, &resp).await;
        return Ok(());
    }

    // Route 4: Modular UI static files (loopback desktop only)
    if !is_loopback_host {
        let resp = HttpResponse::json(403, "Forbidden", json!({ "error": "ui_loopback_only" }));
        let _ = write_response(&mut stream, &resp).await;
        return Ok(());
    }
    if req.method != "GET" && req.method != "HEAD" {
        let resp = HttpResponse::text(405, "Method Not Allowed", "Method not allowed");
        let _ = write_response(&mut stream, &resp).await;
        return Ok(());
    }

    let resp = serve_ui_asset(
        &req.path,
        ctx.user_override_dir.as_deref(),
        &ctx.default_ui_dir,
        &ctx.session_token,
    );
    let _ = write_response(&mut stream, &resp).await;
    Ok(())
}
