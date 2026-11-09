use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use miku_gatekeeper::{Capability, GatekeeperEngine, GatekeeperVerdict};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::{McpContentBlock, McpToolCallResult, McpToolDefinition};

pub const DEFAULT_CLIENT_TIMEOUT_MS: u64 = 5_000;
pub const DEFAULT_CLIENT_IDLE_TTL_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpClientError {
    Timeout { server: String, timeout_ms: u64 },
    GatekeeperDenied { tool: String, reason: String, detail: String, audit_seq: u64, audit_hash: String },
    ServerNotFound(String),
    ServerSpawnFailed { server: String, error: String },
    TransportError(String),
    ProtocolError(String),
    KillSwitchActive,
    IoError(String),
}

impl fmt::Display for McpClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            McpClientError::Timeout { server, timeout_ms } => {
                write!(f, "MCP client timeout on server '{}' after {} ms", server, timeout_ms)
            }
            McpClientError::GatekeeperDenied { tool, reason, detail, audit_seq, .. } => {
                write!(f, "Gatekeeper denied MCP tool '{}' (reason={}, seq={}): {}", tool, reason, audit_seq, detail)
            }
            McpClientError::ServerNotFound(name) => write!(f, "MCP server '{}' not configured", name),
            McpClientError::ServerSpawnFailed { server, error } => write!(f, "failed to spawn MCP server '{}': {}", server, error),
            McpClientError::TransportError(err) => write!(f, "MCP transport error: {}", err),
            McpClientError::ProtocolError(err) => write!(f, "MCP protocol error: {}", err),
            McpClientError::KillSwitchActive => write!(f, "MCP operations blocked: Kill Switch active"),
            McpClientError::IoError(err) => write!(f, "MCP client I/O error: {}", err),
        }
    }
}

impl std::error::Error for McpClientError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalMcpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "default_client_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_client_idle_ttl_ms")]
    pub idle_ttl_ms: u64,
    #[serde(default)]
    pub auto_start: bool,
}

fn default_client_timeout_ms() -> u64 { DEFAULT_CLIENT_TIMEOUT_MS }
fn default_client_idle_ttl_ms() -> u64 { DEFAULT_CLIENT_IDLE_TTL_MS }

/// Parses `[mcp_servers.<name>]` declarations from a raw `.miku.toml` document.
pub fn parse_mcp_servers_from_toml(raw_toml: &str) -> Result<HashMap<String, ExternalMcpServerConfig>, McpClientError> {
    #[derive(Deserialize)]
    struct TomlRoot {
        #[serde(default)]
        mcp_servers: HashMap<String, TomlMcpEntry>,
    }

    #[derive(Deserialize)]
    struct TomlMcpEntry {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default = "default_client_timeout_ms")]
        timeout_ms: u64,
        #[serde(default = "default_client_idle_ttl_ms")]
        idle_ttl_ms: u64,
        #[serde(default)]
        auto_start: bool,
    }

    let parsed: TomlRoot = toml::from_str(raw_toml)
        .map_err(|e| McpClientError::ProtocolError(format!("failed to parse .miku.toml mcp_servers: {}", e)))?;

    let mut out = HashMap::new();
    for (name, entry) in parsed.mcp_servers {
        out.insert(name.clone(), ExternalMcpServerConfig {
            name,
            command: entry.command,
            args: entry.args,
            env: entry.env,
            timeout_ms: entry.timeout_ms,
            idle_ttl_ms: entry.idle_ttl_ms,
            auto_start: entry.auto_start,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Default)]
pub struct MockServerBehavior {
    pub response_delay_ms: u64,
    pub return_corrupt_json: bool,
    pub disconnect_on_call: bool,
    pub tools: Vec<McpToolDefinition>,
}

enum ServerBackend {
    Process {
        child: Child,
        stdin: ChildStdin,
        stdout: tokio::io::Lines<BufReader<ChildStdout>>,
        pid: u32,
    },
    Mock {
        behavior: MockServerBehavior,
        simulated_pid: u32,
    },
}

struct ExternalServerInstance {
    config: ExternalMcpServerConfig,
    backend: Option<ServerBackend>,
    mock_behavior: Option<MockServerBehavior>,
    active_sessions: HashSet<String>,
    last_activity_ms: u64,
    known_tools: Vec<McpToolDefinition>,
}

impl ExternalServerInstance {
    fn new(config: ExternalMcpServerConfig) -> Self {
        Self {
            config,
            backend: None,
            mock_behavior: None,
            active_sessions: HashSet::new(),
            last_activity_ms: 0,
            known_tools: Vec::new(),
        }
    }

    fn new_mock(config: ExternalMcpServerConfig, behavior: MockServerBehavior, _sim_pid: u32) -> Self {
        Self {
            config,
            backend: None,
            mock_behavior: Some(behavior),
            active_sessions: HashSet::new(),
            last_activity_ms: 0,
            known_tools: Vec::new(),
        }
    }

    fn is_running(&self) -> bool { self.backend.is_some() }

    fn current_pid(&self) -> Option<u32> {
        match &self.backend {
            Some(ServerBackend::Process { pid, .. }) => Some(*pid),
            Some(ServerBackend::Mock { simulated_pid, .. }) => Some(*simulated_pid),
            None => None,
        }
    }

    async fn ensure_started(&mut self, now_ms: u64) -> Result<(), McpClientError> {
        if self.backend.is_some() { return Ok(()); }
        if let Some(behavior) = &self.mock_behavior {
            self.backend = Some(ServerBackend::Mock {
                behavior: behavior.clone(),
                simulated_pid: 77000 + (now_ms as u32 % 1000),
            });
            self.known_tools = behavior.tools.clone();
            return Ok(());
        }

        let mut cmd = Command::new(&self.config.command);
        cmd.args(&self.config.args);
        for (k, v) in &self.config.env { cmd.env(k, v); }
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());

        #[cfg(windows)]
        { cmd.creation_flags(0x08000000); } // CREATE_NO_WINDOW

        let mut child = cmd.spawn().map_err(|e| McpClientError::ServerSpawnFailed {
            server: self.config.name.clone(),
            error: e.to_string(),
        })?;

        let pid = child.id().unwrap_or(0);
        let stdin = child.stdin.take().ok_or_else(|| McpClientError::ServerSpawnFailed {
            server: self.config.name.clone(),
            error: "child process stdin pipe unavailable".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| McpClientError::ServerSpawnFailed {
            server: self.config.name.clone(),
            error: "child process stdout pipe unavailable".to_string(),
        })?;

        let lines = BufReader::new(stdout).lines();
        self.backend = Some(ServerBackend::Process { child, stdin, stdout: lines, pid });
        Ok(())
    }

    async fn terminate(&mut self) -> Result<(), McpClientError> {
        if let Some(ServerBackend::Process { mut child, .. }) = self.backend.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        self.active_sessions.clear();
        Ok(())
    }
}

pub struct ExternalMcpClientManager {
    gatekeeper: GatekeeperEngine,
    kill_switch: Arc<AtomicBool>,
    servers: HashMap<String, ExternalServerInstance>,
}

impl ExternalMcpClientManager {
    pub fn new(gatekeeper: GatekeeperEngine) -> Self {
        Self {
            gatekeeper,
            kill_switch: Arc::new(AtomicBool::new(false)),
            servers: HashMap::new(),
        }
    }

    pub fn gatekeeper(&self) -> &GatekeeperEngine { &self.gatekeeper }
    pub fn gatekeeper_mut(&mut self) -> &mut GatekeeperEngine { &mut self.gatekeeper }

    pub fn register_server(&mut self, config: ExternalMcpServerConfig) -> Result<(), McpClientError> {
        let name = config.name.clone();
        self.servers.insert(name, ExternalServerInstance::new(config));
        Ok(())
    }

    pub fn register_mock_server(&mut self, config: ExternalMcpServerConfig, behavior: MockServerBehavior) -> Result<(), McpClientError> {
        let name = config.name.clone();
        let sim_pid = 88000 + (self.servers.len() as u32);
        self.servers.insert(name, ExternalServerInstance::new_mock(config, behavior, sim_pid));
        Ok(())
    }

    pub async fn acquire_session(&mut self, server_name: &str, session_id: &str, now_ms: u64) -> Result<(), McpClientError> {
        if self.is_kill_switch_active() { return Err(McpClientError::KillSwitchActive); }
        let srv = self.servers.get_mut(server_name).ok_or_else(|| McpClientError::ServerNotFound(server_name.to_string()))?;
        srv.ensure_started(now_ms).await?;
        srv.active_sessions.insert(session_id.to_string());
        srv.last_activity_ms = now_ms;
        Ok(())
    }

    pub async fn release_session(&mut self, session_id: &str, now_ms: u64) {
        for srv in self.servers.values_mut() {
            if srv.active_sessions.remove(session_id) { srv.last_activity_ms = now_ms; }
        }
    }

    pub async fn reap_idle_servers(&mut self, now_ms: u64) -> usize {
        let mut reaped = 0;
        for srv in self.servers.values_mut() {
            if srv.is_running() && srv.active_sessions.is_empty() {
                let elapsed = now_ms.saturating_sub(srv.last_activity_ms);
                if elapsed >= srv.config.idle_ttl_ms {
                    let _ = srv.terminate().await;
                    reaped += 1;
                }
            }
        }
        reaped
    }

    pub fn is_server_running(&self, server_name: &str) -> bool {
        self.servers.get(server_name).map(|s| s.is_running()).unwrap_or(false)
    }

    pub fn active_process_count(&self) -> usize {
        self.servers.values().filter(|s| s.is_running()).count()
    }

    pub fn active_process_pids(&self) -> Vec<u32> {
        self.servers.values().filter_map(|s| s.current_pid()).collect()
    }

    pub async fn list_tools(&mut self, server_name: &str, session_id: &str, now_ms: u64) -> Result<Vec<McpToolDefinition>, McpClientError> {
        self.acquire_session(server_name, session_id, now_ms).await?;
        let srv = self.servers.get(server_name).ok_or_else(|| McpClientError::ServerNotFound(server_name.to_string()))?;
        Ok(srv.known_tools.clone())
    }

    pub async fn call_tool(
        &mut self,
        server_name: &str,
        tool_name: &str,
        target: &str,
        args: &str,
        session_id: &str,
        now_ms: u64,
    ) -> Result<McpToolCallResult, McpClientError> {
        if self.is_kill_switch_active() { return Err(McpClientError::KillSwitchActive); }

        let srv = self.servers.get(server_name).ok_or_else(|| McpClientError::ServerNotFound(server_name.to_string()))?;
        let timeout_ms = srv.config.timeout_ms;

        let capability = srv.known_tools
            .iter()
            .find(|t| t.name == tool_name)
            .and_then(|t| Capability::parse(&t.capability))
            .unwrap_or_else(|| {
                if tool_name.contains("secret") || tool_name.contains("write") {
                    Capability::FsWrite
                } else if tool_name.contains("read") {
                    Capability::FsRead
                } else {
                    Capability::Shell
                }
            });

        let verdict = self.gatekeeper.evaluate_request(session_id, capability, target, args, now_ms);

        match verdict {
            GatekeeperVerdict::Deny { reason, detail, audit_seq, audit_hash } => {
                Err(McpClientError::GatekeeperDenied {
                    tool: tool_name.to_string(),
                    reason: reason.as_code().to_string(),
                    detail,
                    audit_seq,
                    audit_hash,
                })
            }
            GatekeeperVerdict::AskPending { ticket, .. } => {
                let (audit_seq, audit_hash) = self.gatekeeper.audit_log_mut_for_tamper_test().append(
                    now_ms, session_id, capability.as_str(), target, "ask_pending", "requires_interactive_consent",
                );
                Err(McpClientError::GatekeeperDenied {
                    tool: tool_name.to_string(),
                    reason: "ask_pending".to_string(),
                    detail: format!("ticket {} pending user consent", ticket.req_hash),
                    audit_seq,
                    audit_hash,
                })
            }
            GatekeeperVerdict::Allow { canonical_target, .. } => {
                self.acquire_session(server_name, session_id, now_ms).await?;
                let srv_mut = self.servers.get_mut(server_name).ok_or_else(|| McpClientError::ServerNotFound(server_name.to_string()))?;

                let dispatch_res = tokio::time::timeout(
                    Duration::from_millis(timeout_ms),
                    Self::execute_backend_call(srv_mut, tool_name, &canonical_target, args),
                ).await;

                match dispatch_res {
                    Ok(Ok(output_text)) => {
                        let (final_seq, final_hash) = self.gatekeeper.audit_log_mut_for_tamper_test().append(
                            now_ms, session_id, capability.as_str(), &canonical_target, "allow", "mcp_dispatch_completed",
                        );
                        Ok(McpToolCallResult {
                            content: vec![McpContentBlock { kind: "text".to_string(), text: output_text }],
                            is_error: false,
                            canonical_target,
                            audit_seq: final_seq,
                            audit_hash: final_hash,
                        })
                    }
                    Ok(Err(client_err)) => Err(client_err),
                    Err(_elapsed) => Err(McpClientError::Timeout { server: server_name.to_string(), timeout_ms }),
                }
            }
        }
    }

    async fn execute_backend_call(
        srv: &mut ExternalServerInstance,
        tool: &str,
        target: &str,
        args: &str,
    ) -> Result<String, McpClientError> {
        let backend = srv.backend.as_mut().ok_or_else(|| {
            McpClientError::TransportError("server backend is not active".to_string())
        })?;

        match backend {
            ServerBackend::Mock { behavior, .. } => {
                if behavior.response_delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(behavior.response_delay_ms)).await;
                }
                if behavior.disconnect_on_call {
                    srv.backend = None;
                    return Err(McpClientError::TransportError("external server disconnected during invocation".to_string()));
                }
                if behavior.return_corrupt_json {
                    return Err(McpClientError::ProtocolError("invalid JSON-RPC payload received from external server (-32700)".to_string()));
                }
                Ok(format!("ok: {} -> target='{}' args='{}'", tool, target, args))
            }
            ServerBackend::Process { stdin, stdout, .. } => {
                let req = json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": { "name": tool, "arguments": { "target": target, "args": args } }
                });
                let mut line = serde_json::to_string(&req).map_err(|e| McpClientError::ProtocolError(format!("json encode error: {}", e)))?;
                line.push('\n');

                stdin.write_all(line.as_bytes()).await.map_err(|e| McpClientError::TransportError(format!("failed writing to child stdin: {}", e)))?;
                stdin.flush().await.map_err(|e| McpClientError::TransportError(format!("failed flushing child stdin: {}", e)))?;

                let resp_line = stdout.next_line().await.map_err(|e| McpClientError::TransportError(format!("failed reading child stdout: {}", e)))?;

                match resp_line {
                    Some(l) => {
                        let parsed: serde_json::Value = serde_json::from_str(&l).map_err(|e| McpClientError::ProtocolError(format!("corrupt json response: {}", e)))?;
                        if let Some(err) = parsed.get("error") {
                            return Err(McpClientError::ProtocolError(format!("jsonrpc error: {}", err)));
                        }
                        let text = parsed.get("result")
                            .and_then(|r| r.get("content"))
                            .and_then(|c| c.get(0))
                            .and_then(|b| b.get("text"))
                            .and_then(|t| t.as_str())
                            .unwrap_or("ok")
                            .to_string();
                        Ok(text)
                    }
                    None => {
                        srv.backend = None;
                        Err(McpClientError::TransportError("child process stdout closed EOF".to_string()))
                    }
                }
            }
        }
    }

    pub async fn trip_kill_switch(&mut self) -> Result<(), McpClientError> {
        self.kill_switch.store(true, Ordering::SeqCst);
        self.gatekeeper.trigger_kill_switch(1);
        for srv in self.servers.values_mut() {
            let _ = srv.terminate().await;
        }
        Ok(())
    }

    pub async fn shutdown(&mut self) -> Result<(), McpClientError> {
        for srv in self.servers.values_mut() {
            let _ = srv.terminate().await;
        }
        Ok(())
    }

    fn is_kill_switch_active(&self) -> bool {
        self.kill_switch.load(Ordering::SeqCst) || self.gatekeeper.is_kill_switch_active()
    }
}
