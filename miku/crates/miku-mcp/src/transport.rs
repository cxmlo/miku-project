use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use miku_protocol::{JsonRpcId, JsonRpcRequest, JsonRpcResponse};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Mutex};

use crate::{
    McpClientInfo, McpHostEngine, McpHostError, McpHostMetricsSnapshot, McpInitializeParams,
    McpInitializeResult, McpToolCallArguments, McpToolCallParams, McpToolCallResult,
    McpToolsListResult, MCP_PROTOCOL_VERSION_2024_11_05,
};

struct ConnectionCounterGuard {
    counter: Arc<AtomicUsize>,
}

impl ConnectionCounterGuard {
    fn new(counter: Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for ConnectionCounterGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct McpLoopbackServerHandle {
    pub local_addr: SocketAddr,
    engine: Arc<Mutex<McpHostEngine>>,
    active_connections: Arc<AtomicUsize>,
    shutdown_tx: watch::Sender<bool>,
}

impl McpLoopbackServerHandle {
    pub fn active_connections(&self) -> usize {
        self.active_connections.load(Ordering::SeqCst)
    }

    pub async fn metrics(&self) -> McpHostMetricsSnapshot {
        self.engine.lock().await.metrics()
    }

    pub async fn verify_audit_chain(&self) -> Result<usize, String> {
        let guard = self.engine.lock().await;
        guard
            .audit_log()
            .verify_chain()
            .map_err(|e| e.to_string())?;
        Ok(guard.audit_log().len())
    }

    pub async fn trigger_kill_switch(&self, now_ms: u64) -> usize {
        self.engine
            .lock()
            .await
            .gatekeeper_mut()
            .trigger_kill_switch(now_ms)
    }

    pub fn trigger_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Starts the local MCP Host listener on `127.0.0.1` / `::1`, rejecting any non-loopback address.
pub async fn start_mcp_loopback_host(
    engine: McpHostEngine,
) -> Result<(McpLoopbackServerHandle, tokio::task::JoinHandle<()>), McpHostError> {
    engine.config().validate_loopback_bind()?;
    let bind_addr = engine.config().bind_addr;

    let listener = TcpListener::bind(bind_addr)
        .await
        .map_err(|e| McpHostError::IoError(e.to_string()))?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| McpHostError::IoError(e.to_string()))?;

    let shared_engine = Arc::new(Mutex::new(engine));
    let active_connections = Arc::new(AtomicUsize::new(0));
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

    let loop_engine = Arc::clone(&shared_engine);
    let loop_conns = Arc::clone(&active_connections);

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
                            let eng = Arc::clone(&loop_engine);
                            let conns = Arc::clone(&loop_conns);
                            let conn_shutdown = shutdown_rx.clone();
                            let task = tokio::task::spawn(async move {
                                handle_mcp_stream(stream, eng, conns, conn_shutdown).await;
                            });
                            client_tasks.push(task);
                        }
                        Err(_) => break,
                    }
                }
            }
        }
        drop(listener);
        for task in client_tasks {
            let _ = task.await;
        }
    });

    Ok((
        McpLoopbackServerHandle {
            local_addr,
            engine: shared_engine,
            active_connections,
            shutdown_tx,
        },
        join,
    ))
}

async fn handle_mcp_stream(
    stream: TcpStream,
    engine: Arc<Mutex<McpHostEngine>>,
    active_connections: Arc<AtomicUsize>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let _guard = ConnectionCounterGuard::new(active_connections);
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        let read_res = tokio::select! {
            biased;

            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }

            res = reader.read_line(&mut line) => res,
        };

        match read_res {
            Ok(0) => break,
            Ok(_) => {
                let response_line = {
                    let mut guard = engine.lock().await;
                    guard.handle_jsonrpc_line(&line)
                };
                if write_half.write_all(response_line.as_bytes()).await.is_err() {
                    break;
                }
                if write_half.flush().await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = write_half.shutdown().await;
}

/// Test-only client helper for verifying `miku-mcp` Host endpoints (`initialize`, `tools/list`, `tools/call`).
/// Production external MCP client corresponds to Step 3.4.
pub struct McpTestClient {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    next_id: i64,
}

impl McpTestClient {
    pub async fn connect(addr: SocketAddr) -> Result<Self, McpHostError> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| McpHostError::IoError(e.to_string()))?;
        let _ = stream.set_nodelay(true);
        let (read_half, write_half) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read_half),
            writer: write_half,
            next_id: 1,
        })
    }

    pub async fn send_raw_line(
        &mut self,
        raw_line: &str,
    ) -> Result<JsonRpcResponse<serde_json::Value>, McpHostError> {
        let mut framed = raw_line.trim().to_string();
        framed.push('\n');
        self.writer
            .write_all(framed.as_bytes())
            .await
            .map_err(|e| McpHostError::IoError(e.to_string()))?;
        self.writer
            .flush()
            .await
            .map_err(|e| McpHostError::IoError(e.to_string()))?;

        let mut resp_line = String::new();
        let n = self
            .reader
            .read_line(&mut resp_line)
            .await
            .map_err(|e| McpHostError::IoError(e.to_string()))?;
        if n == 0 {
            return Err(McpHostError::ConnectionClosed);
        }
        JsonRpcResponse::decode_line(&resp_line)
            .map_err(|e| McpHostError::ProtocolError(e.to_string()))
    }

    pub async fn initialize(
        &mut self,
        client_name: &str,
        session_token: Option<&str>,
    ) -> Result<McpInitializeResult, McpHostError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = JsonRpcRequest::new(
            Some(JsonRpcId::Number(id)),
            "initialize",
            McpInitializeParams {
                protocol_version: MCP_PROTOCOL_VERSION_2024_11_05.to_string(),
                client_info: Some(McpClientInfo {
                    name: client_name.to_string(),
                    version: "0.1.0".to_string(),
                }),
                session_token: session_token.map(str::to_string),
            },
        );
        let line = req
            .encode_line()
            .map_err(|e| McpHostError::ProtocolError(e.to_string()))?;
        let resp = self.send_raw_line(&line).await?;
        if let Some(err) = resp.error {
            return Err(McpHostError::ProtocolError(format!(
                "MCP initialize error ({}): {}",
                err.code, err.message
            )));
        }
        let val = resp
            .result
            .ok_or_else(|| McpHostError::ProtocolError("missing initialize result".into()))?;
        serde_json::from_value(val).map_err(|e| McpHostError::ProtocolError(e.to_string()))
    }

    pub async fn list_tools(&mut self) -> Result<McpToolsListResult, McpHostError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = JsonRpcRequest::new(Some(JsonRpcId::Number(id)), "tools/list", json!({}));
        let line = req
            .encode_line()
            .map_err(|e| McpHostError::ProtocolError(e.to_string()))?;
        let resp = self.send_raw_line(&line).await?;
        if let Some(err) = resp.error {
            return Err(McpHostError::ProtocolError(format!(
                "MCP tools/list error ({}): {}",
                err.code, err.message
            )));
        }
        let val = resp
            .result
            .ok_or_else(|| McpHostError::ProtocolError("missing tools/list result".into()))?;
        serde_json::from_value(val).map_err(|e| McpHostError::ProtocolError(e.to_string()))
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: McpToolCallArguments,
    ) -> Result<JsonRpcResponse<McpToolCallResult>, McpHostError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = JsonRpcRequest::new(
            Some(JsonRpcId::Number(id)),
            "tools/call",
            McpToolCallParams {
                name: name.to_string(),
                arguments,
            },
        );
        let mut framed = req
            .encode_line()
            .map_err(|e| McpHostError::ProtocolError(e.to_string()))?;
        if !framed.ends_with('\n') {
            framed.push('\n');
        }
        self.writer
            .write_all(framed.as_bytes())
            .await
            .map_err(|e| McpHostError::IoError(e.to_string()))?;
        self.writer
            .flush()
            .await
            .map_err(|e| McpHostError::IoError(e.to_string()))?;

        let mut resp_line = String::new();
        let n = self
            .reader
            .read_line(&mut resp_line)
            .await
            .map_err(|e| McpHostError::IoError(e.to_string()))?;
        if n == 0 {
            return Err(McpHostError::ConnectionClosed);
        }
        JsonRpcResponse::decode_line(&resp_line)
            .map_err(|e| McpHostError::ProtocolError(e.to_string()))
    }
}
