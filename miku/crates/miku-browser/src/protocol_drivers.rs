use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::types_and_guards::{
    sanitize_browser_text, validate_download_target, BrowserError, BrowserPageSnapshot,
    BrowserProtocolKind, DownloadJailPolicy, DownloadReceipt, MAX_DOM_TEXT_EXCERPT_BYTES,
};

pub trait BrowserBackend: Send + Sync {
    fn navigate_and_read_dom<'a>(
        &'a self,
        url: &'a str,
        host: &'a str,
        isolated_profile_used: bool,
        abort_flag: &'a AtomicBool,
    ) -> Pin<Box<dyn Future<Output = Result<BrowserPageSnapshot, BrowserError>> + Send + 'a>>;

    fn download_artifact<'a>(
        &'a self,
        url: &'a str,
        suggested_filename: &'a str,
        jail: &'a DownloadJailPolicy,
        abort_flag: &'a AtomicBool,
    ) -> Pin<Box<dyn Future<Output = Result<DownloadReceipt, BrowserError>> + Send + 'a>>;

    fn active_processes(&self) -> usize;
}

// Minimal RFC 6455 masked text frame exchange over raw Tokio TcpStream (zero new crates).
async fn ws_connect_and_exchange(
    ws_addr: &str,
    ws_path: &str,
    payloads: &[Value],
    abort_flag: &AtomicBool,
) -> Result<Vec<Value>, BrowserError> {
    if abort_flag.load(Ordering::SeqCst) {
        return Err(BrowserError::KillSwitchAborted);
    }
    let mut stream = TcpStream::connect(ws_addr)
        .await
        .map_err(|e| BrowserError::ProtocolError(format!("ws connect failed: {e}")))?;

    let req = format!(
        "GET {ws_path} HTTP/1.1\r\n\
         Host: {ws_addr}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| BrowserError::ProtocolError(format!("ws handshake write failed: {e}")))?;

    let mut header_buf = Vec::with_capacity(512);
    let mut tmp = [0u8; 256];
    loop {
        if abort_flag.load(Ordering::SeqCst) {
            return Err(BrowserError::KillSwitchAborted);
        }
        let n = stream
            .read(&mut tmp)
            .await
            .map_err(|e| BrowserError::ProtocolError(format!("ws handshake read failed: {e}")))?;
        if n == 0 {
            return Err(BrowserError::ProtocolError(
                "EOF during WebSocket handshake".to_string(),
            ));
        }
        header_buf.extend_from_slice(&tmp[..n]);
        if header_buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if header_buf.len() > 8192 {
            return Err(BrowserError::ProtocolError(
                "oversized WebSocket handshake".to_string(),
            ));
        }
    }
    let header_str = String::from_utf8_lossy(&header_buf);
    if !header_str.contains("101 Switching Protocols") {
        return Err(BrowserError::ProtocolError(format!(
            "unexpected ws handshake response: {header_str}"
        )));
    }

    let mut responses = Vec::with_capacity(payloads.len());
    for msg in payloads {
        if abort_flag.load(Ordering::SeqCst) {
            return Err(BrowserError::KillSwitchAborted);
        }
        let text = serde_json::to_vec(msg)
            .map_err(|e| BrowserError::ProtocolError(format!("json encode failed: {e}")))?;
        send_ws_masked_frame(&mut stream, 0x1, &text).await?;
        let reply_bytes = read_ws_frame(&mut stream, abort_flag).await?;
        let parsed: Value = serde_json::from_slice(&reply_bytes)
            .map_err(|e| BrowserError::ProtocolError(format!("invalid json ws reply: {e}")))?;
        responses.push(parsed);
    }

    // Best-effort close frame
    let _ = send_ws_masked_frame(&mut stream, 0x8, &[]).await;
    Ok(responses)
}

async fn send_ws_masked_frame(
    stream: &mut TcpStream,
    opcode: u8,
    payload: &[u8],
) -> Result<(), BrowserError> {
    let mut frame = Vec::with_capacity(payload.len() + 14);
    frame.push(0x80 | (opcode & 0x0f));
    let len = payload.len();
    if len < 126 {
        frame.push(0x80 | (len as u8));
    } else if len <= 65_535 {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        frame.push(0x80 | 127);
        frame.extend_from_slice(&(len as u64).to_be_bytes());
    }
    let mask = [0x4d, 0x69, 0x6b, 0x75]; // "Miku"
    frame.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        frame.push(b ^ mask[i % 4]);
    }
    stream
        .write_all(&frame)
        .await
        .map_err(|e| BrowserError::ProtocolError(format!("ws write frame failed: {e}")))
}

async fn read_ws_frame(stream: &mut TcpStream, abort_flag: &AtomicBool) -> Result<Vec<u8>, BrowserError> {
    let mut hdr = [0u8; 2];
    stream
        .read_exact(&mut hdr)
        .await
        .map_err(|e| BrowserError::ProtocolError(format!("ws read hdr failed: {e}")))?;
    if abort_flag.load(Ordering::SeqCst) {
        return Err(BrowserError::KillSwitchAborted);
    }
    let masked = (hdr[1] & 0x80) != 0;
    let mut len = (hdr[1] & 0x7f) as usize;
    if len == 126 {
        let mut ext = [0u8; 2];
        stream
            .read_exact(&mut ext)
            .await
            .map_err(|e| BrowserError::ProtocolError(format!("ws read len16 failed: {e}")))?;
        len = u16::from_be_bytes(ext) as usize;
    } else if len == 127 {
        let mut ext = [0u8; 8];
        stream
            .read_exact(&mut ext)
            .await
            .map_err(|e| BrowserError::ProtocolError(format!("ws read len64 failed: {e}")))?;
        len = u64::from_be_bytes(ext) as usize;
    }
    if len > 2 * 1024 * 1024 {
        return Err(BrowserError::ProtocolError(
            "ws response frame exceeds 2 MB cap".to_string(),
        ));
    }
    let mut mask_key = [0u8; 4];
    if masked {
        stream
            .read_exact(&mut mask_key)
            .await
            .map_err(|e| BrowserError::ProtocolError(format!("ws read mask failed: {e}")))?;
    }
    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(|e| BrowserError::ProtocolError(format!("ws read payload failed: {e}")))?;
    if masked {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask_key[i % 4];
        }
    }
    Ok(payload)
}

pub struct CdpWebSocketClient {
    pub ws_addr: String,
    pub ws_path: String,
}

impl CdpWebSocketClient {
    pub async fn navigate_and_extract_dom(
        &self,
        url: &str,
        host: &str,
        isolated_profile_used: bool,
        abort_flag: &AtomicBool,
    ) -> Result<BrowserPageSnapshot, BrowserError> {
        let cmds = vec![
            json!({"id": 1, "method": "Page.navigate", "params": {"url": url}}),
            json!({"id": 2, "method": "Runtime.evaluate", "params": {"expression": "document.title + '\\n' + document.body.innerText"}}),
            json!({"id": 3, "method": "Browser.close", "params": {}}),
        ];
        let replies = ws_connect_and_exchange(&self.ws_addr, &self.ws_path, &cmds, abort_flag).await?;
        let eval = replies.get(1).and_then(|v| v.get("result")).cloned().unwrap_or(Value::Null);
        let title = eval
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled Page");
        let text = eval
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(BrowserPageSnapshot {
            url: url.to_string(),
            host: host.to_string(),
            title: sanitize_browser_text(title, 256),
            dom_text: sanitize_browser_text(text, MAX_DOM_TEXT_EXCERPT_BYTES),
            protocol_used: BrowserProtocolKind::ChromiumCdp,
            isolated_profile_used,
        })
    }
}

pub struct GeckoBiDiClient {
    pub ws_addr: String,
    pub ws_path: String,
}

impl GeckoBiDiClient {
    pub async fn navigate_and_extract_dom(
        &self,
        url: &str,
        host: &str,
        isolated_profile_used: bool,
        abort_flag: &AtomicBool,
    ) -> Result<BrowserPageSnapshot, BrowserError> {
        let cmds = vec![
            json!({"id": 1, "method": "session.new", "params": {"capabilities": {}}}),
            json!({"id": 2, "method": "browsingContext.navigate", "params": {"context": "tab-1", "url": url, "wait": "complete"}}),
            json!({"id": 3, "method": "script.evaluate", "params": {"expression": "({title: document.title, text: document.body.innerText})"}}),
            json!({"id": 4, "method": "session.end", "params": {}}),
        ];
        let replies = ws_connect_and_exchange(&self.ws_addr, &self.ws_path, &cmds, abort_flag).await?;
        let eval = replies.get(2).and_then(|v| v.get("result")).cloned().unwrap_or(Value::Null);
        let title = eval
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled Zen Page");
        let text = eval
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(BrowserPageSnapshot {
            url: url.to_string(),
            host: host.to_string(),
            title: sanitize_browser_text(title, 256),
            dom_text: sanitize_browser_text(text, MAX_DOM_TEXT_EXCERPT_BYTES),
            protocol_used: BrowserProtocolKind::GeckoBiDi,
            isolated_profile_used,
        })
    }
}

pub struct LiveLoopbackDualProtocolBackend {
    protocol: BrowserProtocolKind,
    ws_addr: String,
    ws_path: String,
    download_http_addr: Option<String>,
    active_sessions: Arc<AtomicUsize>,
}

struct ActiveSessionGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ActiveSessionGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

impl LiveLoopbackDualProtocolBackend {
    pub fn new(
        protocol: BrowserProtocolKind,
        ws_addr: impl Into<String>,
        ws_path: impl Into<String>,
        download_http_addr: Option<String>,
    ) -> Self {
        Self {
            protocol,
            ws_addr: ws_addr.into(),
            ws_path: ws_path.into(),
            download_http_addr,
            active_sessions: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl BrowserBackend for LiveLoopbackDualProtocolBackend {
    fn navigate_and_read_dom<'a>(
        &'a self,
        url: &'a str,
        host: &'a str,
        isolated_profile_used: bool,
        abort_flag: &'a AtomicBool,
    ) -> Pin<Box<dyn Future<Output = Result<BrowserPageSnapshot, BrowserError>> + Send + 'a>> {
        Box::pin(async move {
            self.active_sessions.fetch_add(1, Ordering::SeqCst);
            let _guard = ActiveSessionGuard {
                counter: Arc::clone(&self.active_sessions),
            };
            match self.protocol {
                BrowserProtocolKind::ChromiumCdp => {
                    let cdp = CdpWebSocketClient {
                        ws_addr: self.ws_addr.clone(),
                        ws_path: self.ws_path.clone(),
                    };
                    cdp.navigate_and_extract_dom(url, host, isolated_profile_used, abort_flag)
                        .await
                }
                BrowserProtocolKind::GeckoBiDi => {
                    let bidi = GeckoBiDiClient {
                        ws_addr: self.ws_addr.clone(),
                        ws_path: self.ws_path.clone(),
                    };
                    bidi.navigate_and_extract_dom(url, host, isolated_profile_used, abort_flag)
                        .await
                }
            }
        })
    }

    fn download_artifact<'a>(
        &'a self,
        url: &'a str,
        suggested_filename: &'a str,
        jail: &'a DownloadJailPolicy,
        abort_flag: &'a AtomicBool,
    ) -> Pin<Box<dyn Future<Output = Result<DownloadReceipt, BrowserError>> + Send + 'a>> {
        Box::pin(async move {
            if abort_flag.load(Ordering::SeqCst) {
                return Err(BrowserError::KillSwitchAborted);
            }
            self.active_sessions.fetch_add(1, Ordering::SeqCst);
            let _guard = ActiveSessionGuard {
                counter: Arc::clone(&self.active_sessions),
            };
            // Pre-check target name & extension BEFORE fetching bytes
            let _ = validate_download_target(jail, suggested_filename, 0)?;

            let body = if let Some(ref addr) = self.download_http_addr {
                fetch_http_loopback(addr, suggested_filename, abort_flag).await?
            } else {
                return Err(BrowserError::ProtocolError(
                    "no download endpoint configured".to_string(),
                ));
            };

            let target = validate_download_target(jail, suggested_filename, body.len())?;
            std::fs::write(&target, &body)
                .map_err(|e| BrowserError::Io(format!("write download failed: {e}")))?;
            let canonical = std::fs::canonicalize(&target)
                .map_err(|e| BrowserError::Io(format!("canonicalize downloaded file failed: {e}")))?;

            Ok(DownloadReceipt {
                file_name: target
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(suggested_filename)
                    .to_string(),
                canonical_path: canonical.to_string_lossy().to_string(),
                bytes_written: body.len(),
                source_url: url.to_string(),
            })
        })
    }

    fn active_processes(&self) -> usize {
        self.active_sessions.load(Ordering::SeqCst)
    }
}

async fn fetch_http_loopback(
    addr: &str,
    artifact_name: &str,
    abort_flag: &AtomicBool,
) -> Result<Vec<u8>, BrowserError> {
    if abort_flag.load(Ordering::SeqCst) {
        return Err(BrowserError::KillSwitchAborted);
    }
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|e| BrowserError::ProtocolError(format!("http download connect failed: {e}")))?;
    let req = format!(
        "GET /downloads/{artifact_name} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| BrowserError::ProtocolError(format!("http download write failed: {e}")))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .map_err(|e| BrowserError::ProtocolError(format!("http download read failed: {e}")))?;
    if abort_flag.load(Ordering::SeqCst) {
        return Err(BrowserError::KillSwitchAborted);
    }
    let pos = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| BrowserError::ProtocolError("malformed HTTP response".to_string()))?;
    Ok(raw[pos + 4..].to_vec())
}
