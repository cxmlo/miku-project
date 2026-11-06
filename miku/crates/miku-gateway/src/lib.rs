#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity,
    clippy::invisible_characters
)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use miku_core::OrchestratorHandle;
use miku_protocol::{
    MikuLinkEnvelopeV1, MikuLinkMessageV1, ProtocolError, MAX_AUTH_FRAME_BYTES, MAX_FRAME_BYTES,
    MIKU_PROTOCOL_VERSION_V1, SUPPORTED_PROTOCOL_VERSIONS,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

pub const DEFAULT_LOOPBACK_PORT: u16 = 47400;
pub const MIKU_LINK_WS_PATH: &str = "/link";
pub const HANDSHAKE_TIMEOUT_MS: u64 = 2_000;
pub const MAX_HTTP_UPGRADE_HEADER_BYTES: usize = 4_096;
const WS_GUID_RFC6455: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatewayError {
    NonLoopbackBindForbidden(String),
    IoError(String),
    InvalidHttpUpgrade(String),
    WebSocketProtocol(String),
    FrameTooLarge {
        declared_bytes: usize,
        max_bytes: usize,
    },
    HandshakeTimeout,
    ConnectionClosed,
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatewayError::NonLoopbackBindForbidden(addr) => write!(
                f,
                "bind to non-loopback address '{}' is forbidden without explicit LAN pairing",
                addr
            ),
            GatewayError::IoError(msg) => write!(f, "I/O error: {}", msg),
            GatewayError::InvalidHttpUpgrade(msg) => write!(f, "invalid HTTP upgrade: {}", msg),
            GatewayError::WebSocketProtocol(msg) => write!(f, "WebSocket protocol error: {}", msg),
            GatewayError::FrameTooLarge {
                declared_bytes,
                max_bytes,
            } => write!(
                f,
                "declared WebSocket frame {} bytes exceeds cap {} bytes",
                declared_bytes, max_bytes
            ),
            GatewayError::HandshakeTimeout => write!(f, "WebSocket hello handshake timed out"),
            GatewayError::ConnectionClosed => write!(f, "WebSocket connection closed"),
        }
    }
}

impl std::error::Error for GatewayError {}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub bind_addr: SocketAddr,
    pub allow_lan_pairing: bool,
    pub handshake_timeout: Duration,
}

impl GatewayConfig {
    pub fn loopback(port: u16) -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], port)),
            allow_lan_pairing: false,
            handshake_timeout: Duration::from_millis(HANDSHAKE_TIMEOUT_MS),
        }
    }

    pub fn validate_bind_policy(&self) -> Result<(), GatewayError> {
        if !self.allow_lan_pairing && !self.bind_addr.ip().is_loopback() {
            return Err(GatewayError::NonLoopbackBindForbidden(
                self.bind_addr.to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct SessionRegistry {
    active_connections: AtomicUsize,
    active_authenticated_sessions: AtomicUsize,
    total_accepted_connections: AtomicU64,
    total_authenticated_handshakes: AtomicU64,
    total_rejected_unauth: AtomicU64,
    total_rejected_version: AtomicU64,
    total_rejected_oversized: AtomicU64,
    total_messages_routed: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayMetricsSnapshot {
    pub active_connections: usize,
    pub active_authenticated_sessions: usize,
    pub total_accepted_connections: u64,
    pub total_authenticated_handshakes: u64,
    pub total_rejected_unauth: u64,
    pub total_rejected_version: u64,
    pub total_rejected_oversized: u64,
    pub total_messages_routed: u64,
}

impl SessionRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn snapshot(&self) -> GatewayMetricsSnapshot {
        GatewayMetricsSnapshot {
            active_connections: self.active_connections.load(Ordering::SeqCst),
            active_authenticated_sessions: self
                .active_authenticated_sessions
                .load(Ordering::SeqCst),
            total_accepted_connections: self.total_accepted_connections.load(Ordering::SeqCst),
            total_authenticated_handshakes: self
                .total_authenticated_handshakes
                .load(Ordering::SeqCst),
            total_rejected_unauth: self.total_rejected_unauth.load(Ordering::SeqCst),
            total_rejected_version: self.total_rejected_version.load(Ordering::SeqCst),
            total_rejected_oversized: self.total_rejected_oversized.load(Ordering::SeqCst),
            total_messages_routed: self.total_messages_routed.load(Ordering::SeqCst),
        }
    }
}

/// RAII guard that guarantees active_connections and active_authenticated_sessions
/// are decremented deterministically when a socket task exits for any reason.
struct SessionGuard {
    registry: Arc<SessionRegistry>,
    is_authenticated: bool,
}

impl SessionGuard {
    fn new(registry: Arc<SessionRegistry>) -> Self {
        registry.active_connections.fetch_add(1, Ordering::SeqCst);
        registry
            .total_accepted_connections
            .fetch_add(1, Ordering::SeqCst);
        Self {
            registry,
            is_authenticated: false,
        }
    }

    fn mark_authenticated(&mut self) {
        if !self.is_authenticated {
            self.is_authenticated = true;
            self.registry
                .active_authenticated_sessions
                .fetch_add(1, Ordering::SeqCst);
            self.registry
                .total_authenticated_handshakes
                .fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.registry
            .active_connections
            .fetch_sub(1, Ordering::SeqCst);
        if self.is_authenticated {
            self.registry
                .active_authenticated_sessions
                .fetch_sub(1, Ordering::SeqCst);
        }
    }
}

pub struct GatewayServerHandle {
    pub local_addr: SocketAddr,
    pub registry: Arc<SessionRegistry>,
    shutdown_tx: watch::Sender<bool>,
}

impl GatewayServerHandle {
    pub fn metrics(&self) -> GatewayMetricsSnapshot {
        self.registry.snapshot()
    }

    pub fn trigger_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

pub async fn start_gateway_server(
    config: GatewayConfig,
    orchestrator: OrchestratorHandle,
) -> Result<(GatewayServerHandle, tokio::task::JoinHandle<()>), GatewayError> {
    config.validate_bind_policy()?;

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| GatewayError::IoError(e.to_string()))?;

    let registry = SessionRegistry::new();
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let server_registry = Arc::clone(&registry);
    let handshake_timeout = config.handshake_timeout;

    let join = tokio::task::spawn(async move {
        let mut client_tasks = Vec::new();
        loop {
            tokio::select! {
                biased;

                changed = shutdown_rx.changed() => {
                    if changed.is_ok() && *shutdown_rx.borrow() {
                        break;
                    }
                }

                accept_res = listener.accept() => {
                    match accept_res {
                        Ok((stream, _peer)) => {
                            let _ = stream.set_nodelay(true);
                            let reg = Arc::clone(&server_registry);
                            let orch = orchestrator.clone();
                            let conn_shutdown = shutdown_rx.clone();
                            let task = tokio::task::spawn(async move {
                                let _ = handle_ws_connection(
                                    stream,
                                    orch,
                                    reg,
                                    handshake_timeout,
                                    conn_shutdown,
                                )
                                .await;
                            });
                            client_tasks.push(task);
                        }
                        Err(_) => break,
                    }
                }
            }
        }

        for t in client_tasks {
            let _ = t.await;
        }
    });

    Ok((
        GatewayServerHandle {
            local_addr,
            registry,
            shutdown_tx,
        },
        join,
    ))
}

async fn handle_ws_connection(
    mut stream: TcpStream,
    orchestrator: OrchestratorHandle,
    registry: Arc<SessionRegistry>,
    handshake_timeout: Duration,
    mut shutdown_rx: watch::Receiver<bool>,
) -> Result<(), GatewayError> {
    let mut guard = SessionGuard::new(Arc::clone(&registry));

    // 1. Perform HTTP/1.1 -> WebSocket RFC 6455 Upgrade within handshake timeout
    tokio::time::timeout(handshake_timeout, perform_server_http_upgrade(&mut stream))
        .await
        .map_err(|_| GatewayError::HandshakeTimeout)??;

    // 2. First WebSocket frame MUST be a valid `hello` envelope within handshake timeout
    let first_frame = tokio::select! {
        biased;
        _ = shutdown_rx.changed() => return Ok(()),
        res = tokio::time::timeout(
            handshake_timeout,
            read_ws_frame_bounded(&mut stream, MAX_AUTH_FRAME_BYTES, true),
        ) => {
            match res {
                Ok(inner) => inner,
                Err(_) => {
                    registry.total_rejected_unauth.fetch_add(1, Ordering::SeqCst);
                    let _ = send_error_and_close(
                        &mut stream,
                        "handshake_timeout",
                        None,
                        Some("hello handshake timed out".to_string()),
                        1008,
                    )
                    .await;
                    return Err(GatewayError::HandshakeTimeout);
                }
            }
        }
    };

    let first_payload = match first_frame {
        Ok(WsFrame {
            opcode: 0x1,
            payload,
        }) => payload,
        Ok(WsFrame { opcode: 0x8, .. }) => {
            let _ = write_ws_frame(&mut stream, 0x8, &[], false).await;
            return Ok(());
        }
        Ok(_) => {
            registry.total_rejected_unauth.fetch_add(1, Ordering::SeqCst);
            let _ = send_error_and_close(
                &mut stream,
                "unauthorized",
                None,
                Some("first frame must be a text JSON hello message".to_string()),
                1008,
            )
            .await;
            return Err(GatewayError::WebSocketProtocol(
                "expected text frame for hello".into(),
            ));
        }
        Err(GatewayError::FrameTooLarge {
            declared_bytes,
            max_bytes,
        }) => {
            registry
                .total_rejected_oversized
                .fetch_add(1, Ordering::SeqCst);
            let _ = send_error_and_close(
                &mut stream,
                "frame_too_large",
                None,
                Some(format!(
                    "declared {} bytes exceeds auth cap {}",
                    declared_bytes, max_bytes
                )),
                1009,
            )
            .await;
            return Err(GatewayError::FrameTooLarge {
                declared_bytes,
                max_bytes,
            });
        }
        Err(e) => return Err(e),
    };

    let raw_hello = String::from_utf8(first_payload)
        .map_err(|e| GatewayError::WebSocketProtocol(e.to_string()))?;

    // Inspect protocol version and message variant before authenticating
    match MikuLinkEnvelopeV1::decode_and_validate(&raw_hello) {
        Ok(env) => match env.msg {
            MikuLinkMessageV1::Hello { .. } => {
                match orchestrator.send_envelope(raw_hello).await {
                    Ok(ack_env) => {
                        guard.mark_authenticated();
                        registry
                            .total_messages_routed
                            .fetch_add(1, Ordering::SeqCst);
                        let encoded = ack_env
                            .encode_json()
                            .map_err(|e| GatewayError::WebSocketProtocol(e.to_string()))?;
                        write_ws_frame(&mut stream, 0x1, encoded.as_bytes(), false).await?;
                    }
                    Err(ProtocolError::Unauthorized(detail)) => {
                        registry.total_rejected_unauth.fetch_add(1, Ordering::SeqCst);
                        send_error_and_close(&mut stream, "unauthorized", None, Some(detail), 1008)
                            .await?;
                        return Ok(());
                    }
                    Err(other_err) => {
                        registry.total_rejected_unauth.fetch_add(1, Ordering::SeqCst);
                        send_error_and_close(
                            &mut stream,
                            "protocol_error",
                            None,
                            Some(other_err.to_string()),
                            1002,
                        )
                        .await?;
                        return Ok(());
                    }
                }
            }
            _ => {
                registry.total_rejected_unauth.fetch_add(1, Ordering::SeqCst);
                send_error_and_close(
                    &mut stream,
                    "unauthorized",
                    None,
                    Some("connection must complete hello handshake before sending commands".into()),
                    1008,
                )
                .await?;
                return Ok(());
            }
        },
        Err(ProtocolError::UnsupportedVersion {
            received: _,
            supported,
        }) => {
            registry
                .total_rejected_version
                .fetch_add(1, Ordering::SeqCst);
            send_error_and_close(
                &mut stream,
                "unsupported_version",
                Some(supported),
                None,
                1002,
            )
            .await?;
            return Ok(());
        }
        Err(ProtocolError::FrameTooLarge {
            size_bytes,
            max_bytes,
        }) => {
            registry
                .total_rejected_oversized
                .fetch_add(1, Ordering::SeqCst);
            send_error_and_close(
                &mut stream,
                "frame_too_large",
                None,
                Some(format!("{} > {}", size_bytes, max_bytes)),
                1009,
            )
            .await?;
            return Ok(());
        }
        Err(e) => {
            registry.total_rejected_unauth.fetch_add(1, Ordering::SeqCst);
            send_error_and_close(
                &mut stream,
                "malformed_envelope",
                None,
                Some(e.to_string()),
                1002,
            )
            .await?;
            return Ok(());
        }
    }

    // 3. Authenticated session loop (64 KB frame cap enforced in header before allocation)
    loop {
        let frame_res = tokio::select! {
            biased;

            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    let _ = write_ws_close_frame(&mut stream, 1001, false).await;
                    break;
                }
                continue;
            }

            read_res = read_ws_frame_bounded(&mut stream, MAX_FRAME_BYTES, true) => read_res,
        };

        let frame = match frame_res {
            Ok(f) => f,
            Err(GatewayError::ConnectionClosed) => break,
            Err(GatewayError::FrameTooLarge {
                declared_bytes,
                max_bytes,
            }) => {
                registry
                    .total_rejected_oversized
                    .fetch_add(1, Ordering::SeqCst);
                let _ = send_error_and_close(
                    &mut stream,
                    "frame_too_large",
                    None,
                    Some(format!(
                        "declared {} bytes exceeds 64KB cap ({})",
                        declared_bytes, max_bytes
                    )),
                    1009,
                )
                .await;
                break;
            }
            Err(_) => break,
        };

        match frame.opcode {
            0x1 => {
                let Ok(text) = String::from_utf8(frame.payload) else {
                    let _ = send_error_and_close(
                        &mut stream,
                        "invalid_utf8",
                        None,
                        None,
                        1007,
                    )
                    .await;
                    break;
                };

                match MikuLinkEnvelopeV1::decode_and_validate(&text) {
                    Ok(_) => match orchestrator.send_envelope(text).await {
                        Ok(reply_env) => {
                            registry
                                .total_messages_routed
                                .fetch_add(1, Ordering::SeqCst);
                            if let Ok(json_out) = reply_env.encode_json() {
                                if write_ws_frame(&mut stream, 0x1, json_out.as_bytes(), false)
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                        Err(ProtocolError::Unauthorized(msg)) => {
                            registry.total_rejected_unauth.fetch_add(1, Ordering::SeqCst);
                            let _ =
                                send_error_and_close(&mut stream, "unauthorized", None, Some(msg), 1008)
                                    .await;
                            break;
                        }
                        Err(err) => {
                            let _ = send_error_and_close(
                                &mut stream,
                                "protocol_error",
                                None,
                                Some(err.to_string()),
                                1002,
                            )
                            .await;
                            break;
                        }
                    },
                    Err(ProtocolError::UnsupportedVersion { supported, .. }) => {
                        registry
                            .total_rejected_version
                            .fetch_add(1, Ordering::SeqCst);
                        let _ = send_error_and_close(
                            &mut stream,
                            "unsupported_version",
                            Some(supported),
                            None,
                            1002,
                        )
                        .await;
                        break;
                    }
                    Err(ProtocolError::FrameTooLarge {
                        size_bytes,
                        max_bytes,
                    }) => {
                        registry
                            .total_rejected_oversized
                            .fetch_add(1, Ordering::SeqCst);
                        let _ = send_error_and_close(
                            &mut stream,
                            "frame_too_large",
                            None,
                            Some(format!("{} > {}", size_bytes, max_bytes)),
                            1009,
                        )
                        .await;
                        break;
                    }
                    Err(err) => {
                        let _ = send_error_and_close(
                            &mut stream,
                            "malformed_envelope",
                            None,
                            Some(err.to_string()),
                            1002,
                        )
                        .await;
                        break;
                    }
                }
            }
            0x9 => {
                // WS Ping -> WS Pong
                if write_ws_frame(&mut stream, 0xA, &frame.payload, false)
                    .await
                    .is_err()
                {
                    break;
                }
            }
            0xA => {
                // WS Pong -> ignore
            }
            0x8 => {
                // Clean WS Close handshake
                let _ = write_ws_close_frame(&mut stream, 1000, false).await;
                break;
            }
            _ => {
                let _ = write_ws_close_frame(&mut stream, 1003, false).await;
                break;
            }
        }
    }

    Ok(())
}

async fn send_error_and_close(
    stream: &mut TcpStream,
    code: &str,
    supported: Option<Vec<u8>>,
    detail: Option<String>,
    close_code: u16,
) -> Result<(), GatewayError> {
    let err_env = MikuLinkEnvelopeV1 {
        v: MIKU_PROTOCOL_VERSION_V1,
        id: "gw-err-01".to_string(),
        reply_to: None,
        msg: MikuLinkMessageV1::Error {
            code: code.to_string(),
            supported: supported.or_else(|| {
                if code == "unsupported_version" {
                    Some(SUPPORTED_PROTOCOL_VERSIONS.to_vec())
                } else {
                    None
                }
            }),
            detail,
        },
    };
    if let Ok(json_str) = err_env.encode_json() {
        let _ = write_ws_frame(stream, 0x1, json_str.as_bytes(), false).await;
    }
    let _ = write_ws_close_frame(stream, close_code, false).await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
    Ok(())
}

// ============================================================================
// RFC 6455 HTTP/1.1 Upgrade + Bounded Framing + Client Helper
// ============================================================================

#[derive(Debug, Clone)]
pub struct WsFrame {
    pub opcode: u8,
    pub payload: Vec<u8>,
}

async fn perform_server_http_upgrade(stream: &mut TcpStream) -> Result<(), GatewayError> {
    let mut buf = Vec::with_capacity(512);
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
                "HTTP upgrade header exceeded 4096 bytes".into(),
            ));
        }
    }

    let req_str = String::from_utf8(buf)
        .map_err(|_| GatewayError::InvalidHttpUpgrade("non-UTF8 HTTP header".into()))?;
    let mut lines = req_str.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| GatewayError::InvalidHttpUpgrade("empty request".into()))?;

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "GET" || parts[1] != MIKU_LINK_WS_PATH {
        let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(resp.as_bytes()).await;
        return Err(GatewayError::InvalidHttpUpgrade(format!(
            "invalid request path in '{}'",
            request_line
        )));
    }

    let mut ws_key: Option<String> = None;
    let mut has_upgrade_ws = false;

    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let k_lower = k.trim().to_ascii_lowercase();
            let v_trim = v.trim();
            if k_lower == "sec-websocket-key" {
                ws_key = Some(v_trim.to_string());
            } else if k_lower == "upgrade" && v_trim.to_ascii_lowercase().contains("websocket") {
                has_upgrade_ws = true;
            }
        }
    }

    let key = ws_key.ok_or_else(|| {
        GatewayError::InvalidHttpUpgrade("missing Sec-WebSocket-Key header".into())
    })?;
    if !has_upgrade_ws {
        return Err(GatewayError::InvalidHttpUpgrade(
            "missing Upgrade: websocket header".into(),
        ));
    }

    let accept_val = compute_websocket_accept_key(&key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept_val
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))?;
    stream
        .flush()
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))?;
    Ok(())
}

/// Inspects payload length in the WebSocket header BEFORE allocating the payload buffer.
pub async fn read_ws_frame_bounded(
    stream: &mut TcpStream,
    max_payload_bytes: usize,
    require_masked: bool,
) -> Result<WsFrame, GatewayError> {
    let mut hdr = [0u8; 2];
    match stream.read_exact(&mut hdr).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            return Err(GatewayError::ConnectionClosed);
        }
        Err(e) => return Err(GatewayError::IoError(e.to_string())),
    }

    let opcode = hdr[0] & 0x0F;
    let masked = (hdr[1] & 0x80) != 0;
    if require_masked && !masked {
        return Err(GatewayError::WebSocketProtocol(
            "RFC 6455 requires client-to-server frames to be masked".into(),
        ));
    }

    let len_code = (hdr[1] & 0x7F) as usize;
    let payload_len: usize = if len_code <= 125 {
        len_code
    } else if len_code == 126 {
        let mut ext = [0u8; 2];
        stream
            .read_exact(&mut ext)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        u16::from_be_bytes(ext) as usize
    } else {
        let mut ext = [0u8; 8];
        stream
            .read_exact(&mut ext)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        let raw_u64 = u64::from_be_bytes(ext);
        usize::try_from(raw_u64).unwrap_or(usize::MAX)
    };

    let mut mask_key = [0u8; 4];
    if masked {
        stream
            .read_exact(&mut mask_key)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
    }

    // CRITICAL SECURITY CHECK: Cut immediately BEFORE allocating payload buffer if over cap!
    if payload_len > max_payload_bytes {
        return Err(GatewayError::FrameTooLarge {
            declared_bytes: payload_len,
            max_bytes: max_payload_bytes,
        });
    }

    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        stream
            .read_exact(&mut payload)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        if masked {
            for i in 0..payload_len {
                payload[i] ^= mask_key[i & 3];
            }
        }
    }

    Ok(WsFrame { opcode, payload })
}

pub async fn write_ws_frame(
    stream: &mut TcpStream,
    opcode: u8,
    payload: &[u8],
    masked: bool,
) -> Result<(), GatewayError> {
    let mut header = Vec::with_capacity(14);
    header.push(0x80 | (opcode & 0x0F));

    let mask_bit: u8 = if masked { 0x80 } else { 0x00 };
    let len = payload.len();
    if len <= 125 {
        header.push(mask_bit | (len as u8));
    } else if len <= 65_535 {
        header.push(mask_bit | 126);
        header.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        header.push(mask_bit | 127);
        header.extend_from_slice(&(len as u64).to_be_bytes());
    }

    if masked {
        let mask_key: [u8; 4] = [0x39, 0xC5, 0xBB, 0x22];
        header.extend_from_slice(&mask_key);
        stream
            .write_all(&header)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        let mut masked_payload = Vec::with_capacity(len);
        for i in 0..len {
            masked_payload.push(payload[i] ^ mask_key[i & 3]);
        }
        stream
            .write_all(&masked_payload)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
    } else {
        stream
            .write_all(&header)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        if !payload.is_empty() {
            stream
                .write_all(payload)
                .await
                .map_err(|e| GatewayError::IoError(e.to_string()))?;
        }
    }

    stream
        .flush()
        .await
        .map_err(|e| GatewayError::IoError(e.to_string()))?;
    Ok(())
}

pub async fn write_ws_close_frame(
    stream: &mut TcpStream,
    status_code: u16,
    masked: bool,
) -> Result<(), GatewayError> {
    let bytes = status_code.to_be_bytes();
    write_ws_frame(stream, 0x8, &bytes, masked).await
}

/// Deterministic loopback WebSocket client for integration tests and benchmarks.
pub struct LoopbackWsClient {
    stream: TcpStream,
}

impl LoopbackWsClient {
    pub async fn connect(addr: SocketAddr) -> Result<Self, GatewayError> {
        let mut stream = TcpStream::connect(addr)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        let _ = stream.set_nodelay(true);

        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let expected_accept = compute_websocket_accept_key(key);
        let req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            MIKU_LINK_WS_PATH, addr, key
        );
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        stream
            .flush()
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;

        let mut resp_buf = Vec::with_capacity(256);
        let mut b = [0u8; 1];
        while !resp_buf.ends_with(b"\r\n\r\n") {
            let n = stream
                .read(&mut b)
                .await
                .map_err(|e| GatewayError::IoError(e.to_string()))?;
            if n == 0 {
                return Err(GatewayError::ConnectionClosed);
            }
            resp_buf.push(b[0]);
        }

        let resp_str = String::from_utf8_lossy(&resp_buf);
        if !resp_str.starts_with("HTTP/1.1 101") || !resp_str.contains(&expected_accept) {
            return Err(GatewayError::InvalidHttpUpgrade(resp_str.into_owned()));
        }

        Ok(Self { stream })
    }

    pub async fn send_text(&mut self, text: &str) -> Result<(), GatewayError> {
        write_ws_frame(&mut self.stream, 0x1, text.as_bytes(), true).await
    }

    /// Sends only the RFC 6455 frame header announcing an oversized payload (`declared_len` bytes)
    /// without allocating or transmitting the multi-megabyte body, testing pre-allocation cut.
    pub async fn send_oversized_header_only(
        &mut self,
        declared_len: u64,
    ) -> Result<(), GatewayError> {
        let mut hdr = Vec::with_capacity(14);
        hdr.push(0x81); // FIN + Text opcode
        hdr.push(0x80 | 127); // Masked + 64-bit extended length
        hdr.extend_from_slice(&declared_len.to_be_bytes());
        hdr.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]); // Masking key
        self.stream
            .write_all(&hdr)
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        self.stream
            .flush()
            .await
            .map_err(|e| GatewayError::IoError(e.to_string()))?;
        Ok(())
    }

    pub async fn recv_envelope(&mut self) -> Result<MikuLinkEnvelopeV1, GatewayError> {
        let frame = read_ws_frame_bounded(&mut self.stream, MAX_FRAME_BYTES, false).await?;
        if frame.opcode == 0x8 {
            return Err(GatewayError::ConnectionClosed);
        }
        let text = String::from_utf8(frame.payload)
            .map_err(|e| GatewayError::WebSocketProtocol(e.to_string()))?;
        MikuLinkEnvelopeV1::decode_and_validate(&text)
            .map_err(|e| GatewayError::WebSocketProtocol(e.to_string()))
    }

    pub async fn close_cleanly(mut self) -> Result<(), GatewayError> {
        write_ws_close_frame(&mut self.stream, 1000, true).await?;
        let _ = read_ws_frame_bounded(&mut self.stream, 128, false).await;
        Ok(())
    }
}

// ============================================================================
// Self-Contained SHA-1 + Base64 for RFC 6455 Sec-WebSocket-Accept
// ============================================================================

pub fn compute_websocket_accept_key(sec_websocket_key: &str) -> String {
    let mut input = Vec::with_capacity(sec_websocket_key.len() + WS_GUID_RFC6455.len());
    input.extend_from_slice(sec_websocket_key.trim().as_bytes());
    input.extend_from_slice(WS_GUID_RFC6455.as_bytes());
    let digest = sha1_digest(&input);
    base64_encode(&digest)
}

fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xEFCDAB89;
    let mut h2: u32 = 0x98BADCFE;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xC3D2E1F0;

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            let j = i * 4;
            w[i] = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for i in 0..80 {
            let (f, k) = if i < 20 {
                ((b & c) | ((!b) & d), 0x5A827999u32)
            } else if i < 40 {
                (b ^ c ^ d, 0x6ED9EBA1u32)
            } else if i < 60 {
                ((b & c) | (b & d) | (c & d), 0x8F1BBCDCu32)
            } else {
                (b ^ c ^ d, 0xCA62C1D6u32)
            };

            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use miku_core::{build_current_thread_runtime, spawn_orchestrator};
    use miku_protocol::{BaseContractBundle, IslandState};

    // LEDGER-1.2-1
    #[test]
    fn test_loopback_bind_policy_rejects_non_loopback_without_explicit_pairing() {
        let ok_v4 = GatewayConfig::loopback(47400);
        assert!(ok_v4.validate_bind_policy().is_ok());

        let ok_v6 = GatewayConfig {
            bind_addr: "[::1]:47400".parse().unwrap(),
            allow_lan_pairing: false,
            handshake_timeout: Duration::from_millis(2000),
        };
        assert!(ok_v6.validate_bind_policy().is_ok());

        // 0.0.0.0 and LAN IPs must be rejected when allow_lan_pairing == false
        for forbidden in ["0.0.0.0:47400", "192.168.1.25:47400", "[::]:47400"] {
            let cfg = GatewayConfig {
                bind_addr: forbidden.parse().unwrap(),
                allow_lan_pairing: false,
                handshake_timeout: Duration::from_millis(2000),
            };
            assert!(matches!(
                cfg.validate_bind_policy(),
                Err(GatewayError::NonLoopbackBindForbidden(_))
            ));
        }

        // Verify official RFC 6455 Sec-WebSocket-Accept test vector
        assert_eq!(
            compute_websocket_accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    // LEDGER-1.2-2
    #[test]
    fn test_ws_handshake_hello_ack_and_ping_pong_with_constant_time_token() {
        let rt = build_current_thread_runtime().expect("current_thread runtime");
        rt.block_on(async {
            let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
            let (orch_handle, orch_join) =
                spawn_orchestrator(bundle, "miku_ws_secret_token_v1".to_string(), 64);

            let (gw_handle, gw_join) =
                start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                    .await
                    .expect("start gateway");

            let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
                .await
                .expect("connect WS client");

            // 1. Send valid hello
            let hello = r##"{"v":1,"id":"c-hello-1","msg":{"type":"hello","data":{"client":"miku_android","token":"miku_ws_secret_token_v1","capabilities":["agent_events","permission_decision"]}}}"##;
            client.send_text(hello).await.expect("send hello");
            let ack = client.recv_envelope().await.expect("recv hello_ack");
            assert_eq!(ack.reply_to.as_deref(), Some("c-hello-1"));
            assert!(matches!(
                ack.msg,
                MikuLinkMessageV1::HelloAck {
                    island_state: IslandState::Idle,
                    ..
                }
            ));

            // 2. Send ping -> pong
            let ping = r##"{"v":1,"id":"c-ping-1","msg":{"type":"ping","data":{"ts":1762315641}}}"##;
            client.send_text(ping).await.expect("send ping");
            let pong = client.recv_envelope().await.expect("recv pong");
            assert_eq!(pong.reply_to.as_deref(), Some("c-ping-1"));
            assert!(matches!(
                pong.msg,
                MikuLinkMessageV1::Pong { ts: 1762315641 }
            ));

            // 3. Send agent_event -> transitions miku-core IslandState to Alert
            let evt = r##"{"v":1,"id":"c-evt-1","msg":{"type":"agent_event","data":{"agent":"claude_code","kind":"needs_input","title":"Approve file edit"}}}"##;
            client.send_text(evt).await.expect("send agent_event");
            let evt_ack = client.recv_envelope().await.expect("recv evt_ack");
            assert!(matches!(
                evt_ack.msg,
                MikuLinkMessageV1::HelloAck {
                    island_state: IslandState::Alert,
                    ..
                }
            ));

            client.close_cleanly().await.expect("clean close");
            gw_handle.trigger_shutdown();
            gw_join.await.expect("gateway join");

            orch_handle.trigger_system_shutdown_signal();
            orch_join.await.expect("orchestrator join");
        });
    }

    // LEDGER-1.2-3
    #[test]
    fn test_ws_rejects_unauthenticated_commands_invalid_tokens_and_unsupported_versions() {
        let rt = build_current_thread_runtime().expect("current_thread runtime");
        rt.block_on(async {
            let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
            let (orch_handle, orch_join) =
                spawn_orchestrator(bundle, "miku_valid_token_99".to_string(), 64);

            let (gw_handle, gw_join) =
                start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                    .await
                    .expect("start gateway");

            // Case A: Client sends command BEFORE hello -> rejected with unauthorized
            {
                let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
                    .await
                    .expect("connect A");
                let cmd = r##"{"v":1,"id":"unauth-cmd","msg":{"type":"command","data":{"session_id":"sess_01","action":"enqueue_prompt","text":"cargo test"}}}"##;
                client.send_text(cmd).await.expect("send unauth cmd");
                let resp = client.recv_envelope().await.expect("recv error");
                assert!(matches!(
                    resp.msg,
                    MikuLinkMessageV1::Error { ref code, .. } if code == "unauthorized"
                ));
            }

            // Case B: Client sends hello with wrong token -> rejected with unauthorized
            {
                let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
                    .await
                    .expect("connect B");
                let bad_hello = r##"{"v":1,"id":"bad-tok","msg":{"type":"hello","data":{"client":"intruder","token":"wrong_token_00","capabilities":[]}}}"##;
                client.send_text(bad_hello).await.expect("send bad hello");
                let resp = client.recv_envelope().await.expect("recv error");
                assert!(matches!(
                    resp.msg,
                    MikuLinkMessageV1::Error { ref code, .. } if code == "unauthorized"
                ));
            }

            // Case C: Client sends unsupported protocol version v=2 -> rejected with unsupported_version
            {
                let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
                    .await
                    .expect("connect C");
                let v2_msg = r##"{"v":2,"id":"v2-msg","msg":{"type":"hello","data":{"client":"future","token":"miku_valid_token_99","capabilities":[]}}}"##;
                client.send_text(v2_msg).await.expect("send v2");
                let resp = client.recv_envelope().await.expect("recv error");
                assert!(matches!(
                    resp.msg,
                    MikuLinkMessageV1::Error {
                        ref code,
                        supported: Some(ref sup),
                        ..
                    } if code == "unsupported_version" && sup == &[1u8]
                ));
            }

            let metrics = gw_handle.metrics();
            assert_eq!(metrics.total_rejected_unauth, 2);
            assert_eq!(metrics.total_rejected_version, 1);

            gw_handle.trigger_shutdown();
            gw_join.await.expect("gateway join");
            orch_handle.trigger_system_shutdown_signal();
            orch_join.await.expect("orchestrator join");
        });
    }

    // LEDGER-1.2-4
    #[test]
    fn test_ws_frame_size_cap_64kb_and_clean_session_disconnect_without_leaks() {
        let rt = build_current_thread_runtime().expect("current_thread runtime");
        rt.block_on(async {
            let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
            let (orch_handle, orch_join) =
                spawn_orchestrator(bundle, "miku_cap_token_77".to_string(), 64);

            let (gw_handle, gw_join) =
                start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                    .await
                    .expect("start gateway");

            // 1. Authenticate a client, then send an oversized header (> 64 KB = 70_000 bytes)
            {
                let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
                    .await
                    .expect("connect");
                let hello = r##"{"v":1,"id":"h-1","msg":{"type":"hello","data":{"client":"cli","token":"miku_cap_token_77","capabilities":[]}}}"##;
                client.send_text(hello).await.expect("send hello");
                let _ = client.recv_envelope().await.expect("hello_ack");

                // Declare 70,000 bytes (> 65,536 cap) in the WS header
                client
                    .send_oversized_header_only(70_000)
                    .await
                    .expect("send oversized header");
                let err_env = client.recv_envelope().await.expect("recv frame_too_large");
                assert!(matches!(
                    err_env.msg,
                    MikuLinkMessageV1::Error { ref code, .. } if code == "frame_too_large"
                ));
            }

            // 2. Connect & disconnect 10 clients (5 clean closes + 5 abrupt drops)
            for i in 0..10 {
                let mut c = LoopbackWsClient::connect(gw_handle.local_addr)
                    .await
                    .expect("connect loop");
                let hello = format!(
                    r##"{{"v":1,"id":"h-loop-{}","msg":{{"type":"hello","data":{{"client":"cli","token":"miku_cap_token_77","capabilities":[]}}}}}}"##,
                    i
                );
                c.send_text(&hello).await.expect("send hello");
                let _ = c.recv_envelope().await.expect("ack");
                if i % 2 == 0 {
                    c.close_cleanly().await.expect("clean close");
                } else {
                    drop(c); // Abrupt TCP disconnect
                }
            }

            // Yield briefly on current_thread so all SessionGuard drops complete
            tokio::time::sleep(Duration::from_millis(15)).await;

            let m = gw_handle.metrics();
            assert_eq!(
                m.active_connections, 0,
                "active_connections must return to 0 after disconnects"
            );
            assert_eq!(
                m.active_authenticated_sessions, 0,
                "active_authenticated_sessions must return to 0 after disconnects"
            );
            assert_eq!(m.total_rejected_oversized, 1);

            gw_handle.trigger_shutdown();
            gw_join.await.expect("gateway join");
            orch_handle.trigger_system_shutdown_signal();
            orch_join.await.expect("orchestrator join");
        });
    }
}
