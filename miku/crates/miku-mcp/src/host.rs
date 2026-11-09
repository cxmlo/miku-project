use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use miku_agent::ToolRegistry;
use miku_gatekeeper::{
    DenyReason, GatekeeperEngine, GatekeeperVerdict, ProjectProfile, TamperEvidentAuditLog,
};
use miku_protocol::{
    constant_time_token_eq, JsonRpcId, JsonRpcResponse, JSONRPC_GATEKEEPER_DENIED,
    JSONRPC_INVALID_PARAMS, JSONRPC_INVALID_REQUEST, JSONRPC_KILL_SWITCH_ACTIVE,
    JSONRPC_METHOD_NOT_FOUND, JSONRPC_PARSE_ERROR, JSONRPC_VERSION_2_0, MAX_FRAME_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    McpContentBlock, McpHostError, McpInitializeParams, McpInitializeResult,
    McpServerCapabilities, McpServerInfo, McpToolCallParams, McpToolCallResult,
    McpToolDefinition, McpToolsCapability, McpToolsListResult, DEFAULT_MCP_LOOPBACK_PORT,
    MCP_PROTOCOL_VERSION_2024_11_05, SUPPORTED_MCP_PROTOCOL_VERSIONS,
};

pub type McpToolHandlerFn =
    dyn Fn(&str, &str, &str) -> Result<String, String> + Send + Sync + 'static;

#[derive(Debug, Clone)]
pub struct McpHostConfig {
    pub bind_addr: SocketAddr,
    pub client_agent_id: String,
    pub required_session_token: Option<String>,
}

impl McpHostConfig {
    pub fn loopback(port: u16) -> Self {
        Self {
            bind_addr: SocketAddr::from(([127, 0, 0, 1], port)),
            client_agent_id: "mcp_client".to_string(),
            required_session_token: None,
        }
    }

    pub fn default_loopback() -> Self {
        Self::loopback(DEFAULT_MCP_LOOPBACK_PORT)
    }

    pub fn with_session_token(mut self, token: impl Into<String>) -> Self {
        self.required_session_token = Some(token.into());
        self
    }

    pub fn with_agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.client_agent_id = agent_id.into();
        self
    }

    /// Enforces that the MCP Host network listener binds exclusively to loopback (`127.0.0.1` or `::1`).
    pub fn validate_loopback_bind(&self) -> Result<(), McpHostError> {
        if !self.bind_addr.ip().is_loopback() {
            return Err(McpHostError::NonLoopbackBindForbidden(
                self.bind_addr.to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpHostMetricsSnapshot {
    pub total_requests: u64,
    pub initialize_count: u64,
    pub tools_list_count: u64,
    pub tools_call_allowed: u64,
    pub tools_call_denied: u64,
    pub handler_executions: u64,
    pub invalid_requests: u64,
    pub audit_entries_count: usize,
}

pub struct McpHostEngine {
    config: McpHostConfig,
    registry: ToolRegistry,
    gatekeeper: GatekeeperEngine,
    handler: Arc<McpToolHandlerFn>,
    handler_executions: Arc<AtomicU64>,
    total_requests: u64,
    initialize_count: u64,
    tools_list_count: u64,
    tools_call_allowed: u64,
    tools_call_denied: u64,
    invalid_requests: u64,
}

impl McpHostEngine {
    pub fn new(
        config: McpHostConfig,
        profile: ProjectProfile,
        registry: ToolRegistry,
    ) -> Result<Self, McpHostError> {
        let gatekeeper = GatekeeperEngine::new(profile)
            .map_err(|e| McpHostError::ProtocolError(e.to_string()))?;
        let handler_executions = Arc::new(AtomicU64::new(0));
        let default_handler: Arc<McpToolHandlerFn> =
            Arc::new(|tool_name, canonical_target, args| {
                Ok(format!(
                    "MCP_TOOL_OK|tool={}|target={}|args={}",
                    tool_name, canonical_target, args
                ))
            });
        Ok(Self {
            config,
            registry,
            gatekeeper,
            handler: default_handler,
            handler_executions,
            total_requests: 0,
            initialize_count: 0,
            tools_list_count: 0,
            tools_call_allowed: 0,
            tools_call_denied: 0,
            invalid_requests: 0,
        })
    }

    pub fn with_custom_handler(mut self, handler: Arc<McpToolHandlerFn>) -> Self {
        self.handler = handler;
        self
    }

    pub fn handler_executions_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.handler_executions)
    }

    pub fn handler_executions(&self) -> u64 {
        self.handler_executions.load(Ordering::SeqCst)
    }

    pub fn config(&self) -> &McpHostConfig {
        &self.config
    }

    pub fn gatekeeper(&self) -> &GatekeeperEngine {
        &self.gatekeeper
    }

    pub fn gatekeeper_mut(&mut self) -> &mut GatekeeperEngine {
        &mut self.gatekeeper
    }

    pub fn audit_log(&self) -> &TamperEvidentAuditLog {
        self.gatekeeper.audit_log()
    }

    pub fn metrics(&self) -> McpHostMetricsSnapshot {
        McpHostMetricsSnapshot {
            total_requests: self.total_requests,
            initialize_count: self.initialize_count,
            tools_list_count: self.tools_list_count,
            tools_call_allowed: self.tools_call_allowed,
            tools_call_denied: self.tools_call_denied,
            handler_executions: self.handler_executions(),
            invalid_requests: self.invalid_requests,
            audit_entries_count: self.gatekeeper.audit_log().len(),
        }
    }

    fn verify_session_token(&self, provided: Option<&str>) -> bool {
        match &self.config.required_session_token {
            Some(expected) => match provided {
                Some(tok) => constant_time_token_eq(expected, tok),
                None => false,
            },
            None => true,
        }
    }

    /// Processes a single JSON-RPC 2.0 request line and returns a newline-delimited JSON-RPC 2.0 response.
    pub fn handle_jsonrpc_line(&mut self, raw_line: &str) -> String {
        self.total_requests += 1;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.len() > MAX_FRAME_BYTES {
            self.invalid_requests += 1;
            let resp = JsonRpcResponse::<serde_json::Value>::err(
                None,
                JSONRPC_INVALID_REQUEST,
                "MCP frame is empty or exceeds 64 KB limit",
                None,
            );
            return resp.encode_line().unwrap_or_default();
        }

        let val: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => {
                self.invalid_requests += 1;
                let resp = JsonRpcResponse::<serde_json::Value>::err(
                    None,
                    JSONRPC_PARSE_ERROR,
                    "Malformed JSON-RPC 2.0 frame",
                    None,
                );
                return resp.encode_line().unwrap_or_default();
            }
        };

        let id: Option<JsonRpcId> = val
            .get("id")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        let jsonrpc = val.get("jsonrpc").and_then(|v| v.as_str()).unwrap_or("");
        let method = val
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        if jsonrpc != JSONRPC_VERSION_2_0 || method.is_empty() {
            self.invalid_requests += 1;
            let resp = JsonRpcResponse::<serde_json::Value>::err(
                id,
                JSONRPC_INVALID_REQUEST,
                "Invalid JSON-RPC 2.0 request envelope",
                None,
            );
            return resp.encode_line().unwrap_or_default();
        }

        let params_val = val.get("params").cloned().unwrap_or_else(|| json!({}));

        match method {
            "initialize" => self.handle_initialize(id, params_val),
            "notifications/initialized" | "ping" => {
                let resp = JsonRpcResponse::ok(id, json!({}));
                resp.encode_line().unwrap_or_default()
            }
            "tools/list" => self.handle_tools_list(id),
            "tools/call" => self.handle_tools_call(id, params_val),
            unknown => {
                self.invalid_requests += 1;
                let resp = JsonRpcResponse::<serde_json::Value>::err(
                    id,
                    JSONRPC_METHOD_NOT_FOUND,
                    format!("Unknown MCP method '{}'", unknown),
                    None,
                );
                resp.encode_line().unwrap_or_default()
            }
        }
    }

    fn handle_initialize(
        &mut self,
        id: Option<JsonRpcId>,
        params_val: serde_json::Value,
    ) -> String {
        let params: McpInitializeParams = match serde_json::from_value(params_val) {
            Ok(p) => p,
            Err(_) => {
                self.invalid_requests += 1;
                return JsonRpcResponse::<serde_json::Value>::err(
                    id,
                    JSONRPC_INVALID_PARAMS,
                    "Invalid MCP initialize parameters",
                    None,
                )
                .encode_line()
                .unwrap_or_default();
            }
        };

        if !SUPPORTED_MCP_PROTOCOL_VERSIONS.contains(&params.protocol_version.as_str()) {
            self.invalid_requests += 1;
            return JsonRpcResponse::<serde_json::Value>::err(
                id,
                JSONRPC_INVALID_PARAMS,
                format!("Unsupported MCP protocolVersion '{}'", params.protocol_version),
                Some(json!({ "supported": SUPPORTED_MCP_PROTOCOL_VERSIONS })),
            )
            .encode_line()
            .unwrap_or_default();
        }

        if !self.verify_session_token(params.session_token.as_deref()) {
            self.tools_call_denied += 1;
            return JsonRpcResponse::<serde_json::Value>::err(
                id,
                JSONRPC_GATEKEEPER_DENIED,
                "Unauthorized MCP initialize session token",
                Some(json!({ "reason": "unauthorized_session_token" })),
            )
            .encode_line()
            .unwrap_or_default();
        }

        self.initialize_count += 1;
        let result = McpInitializeResult {
            protocol_version: MCP_PROTOCOL_VERSION_2024_11_05.to_string(),
            capabilities: McpServerCapabilities {
                tools: McpToolsCapability {
                    list_changed: false,
                },
            },
            server_info: McpServerInfo {
                name: "miku-mcp-host".to_string(),
                version: "0.1.0".to_string(),
            },
        };
        JsonRpcResponse::ok(id, result)
            .encode_line()
            .unwrap_or_default()
    }

    fn handle_tools_list(&mut self, id: Option<JsonRpcId>) -> String {
        self.tools_list_count += 1;
        let tools = self
            .registry
            .list_deterministic()
            .into_iter()
            .map(|desc| McpToolDefinition {
                name: desc.name.clone(),
                description: desc.description.clone(),
                capability: desc.required_capability.as_str().to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "target": {
                            "type": "string",
                            "description": "Canonical resource path, command, or URL target"
                        },
                        "args": {
                            "type": "string",
                            "description": "Optional arguments for the tool execution"
                        }
                    },
                    "required": ["target"],
                    "additionalProperties": false
                }),
            })
            .collect();

        let result = McpToolsListResult { tools };
        JsonRpcResponse::ok(id, result)
            .encode_line()
            .unwrap_or_default()
    }

    fn handle_tools_call(
        &mut self,
        id: Option<JsonRpcId>,
        params_val: serde_json::Value,
    ) -> String {
        let call_params: McpToolCallParams = match serde_json::from_value(params_val) {
            Ok(p) => p,
            Err(_) => {
                self.invalid_requests += 1;
                return JsonRpcResponse::<serde_json::Value>::err(
                    id,
                    JSONRPC_INVALID_PARAMS,
                    "Invalid MCP tools/call parameters",
                    None,
                )
                .encode_line()
                .unwrap_or_default();
            }
        };

        let ts_ms = call_params.arguments.timestamp_ms.unwrap_or(1_000);

        // 1. Verify session authentication token in constant time (never leak expected token)
        if !self.verify_session_token(call_params.arguments.session_token.as_deref()) {
            self.tools_call_denied += 1;
            let (audit_seq, audit_hash) = self.gatekeeper.audit_log_mut_for_tamper_test().append(
                ts_ms,
                &self.config.client_agent_id,
                "mcp_auth",
                &call_params.name,
                "deny",
                "unauthorized_session_token",
            );
            return JsonRpcResponse::<serde_json::Value>::err(
                id,
                JSONRPC_GATEKEEPER_DENIED,
                "Unauthorized MCP session token rejected by Gatekeeper",
                Some(json!({
                    "reason": "unauthorized_session_token",
                    "audit_seq": audit_seq,
                    "audit_hash": audit_hash
                })),
            )
            .encode_line()
            .unwrap_or_default();
        }

        // 2. Validate tool registration and non-empty target BEFORE Gatekeeper / handler
        let desc = match self.registry.validate_invocation(
            &call_params.name,
            &call_params.arguments.target,
            &call_params.arguments.args,
        ) {
            Ok(d) => d.clone(),
            Err(tool_err) => {
                self.invalid_requests += 1;
                return JsonRpcResponse::<serde_json::Value>::err(
                    id,
                    JSONRPC_INVALID_PARAMS,
                    tool_err.to_string(),
                    None,
                )
                .encode_line()
                .unwrap_or_default();
            }
        };

        // 3. Evaluate in GatekeeperEngine BEFORE invoking the tool handler
        let verdict = self.gatekeeper.evaluate_request(
            &self.config.client_agent_id,
            desc.required_capability,
            &call_params.arguments.target,
            &call_params.arguments.args,
            ts_ms,
        );

        match verdict {
            GatekeeperVerdict::Allow {
                canonical_target,
                audit_seq,
                audit_hash,
            } => {
                self.handler_executions.fetch_add(1, Ordering::SeqCst);
                self.tools_call_allowed += 1;
                match (self.handler)(
                    &desc.name,
                    &canonical_target,
                    &call_params.arguments.args,
                ) {
                    Ok(output_text) => {
                        let res = McpToolCallResult {
                            content: vec![McpContentBlock {
                                kind: "text".to_string(),
                                text: output_text,
                            }],
                            is_error: false,
                            canonical_target,
                            audit_seq,
                            audit_hash,
                        };
                        JsonRpcResponse::ok(id, res)
                            .encode_line()
                            .unwrap_or_default()
                    }
                    Err(handler_err) => {
                        let res = McpToolCallResult {
                            content: vec![McpContentBlock {
                                kind: "text".to_string(),
                                text: handler_err,
                            }],
                            is_error: true,
                            canonical_target,
                            audit_seq,
                            audit_hash,
                        };
                        JsonRpcResponse::ok(id, res)
                            .encode_line()
                            .unwrap_or_default()
                    }
                }
            }
            GatekeeperVerdict::AskPending {
                ticket,
                audit_seq,
                audit_hash,
            } => {
                self.tools_call_denied += 1;
                JsonRpcResponse::<serde_json::Value>::err(
                    id,
                    JSONRPC_GATEKEEPER_DENIED,
                    "MCP tool invocation requires interactive Gatekeeper approval",
                    Some(json!({
                        "reason": "ask_pending_requires_approval",
                        "req_hash": ticket.req_hash,
                        "audit_seq": audit_seq,
                        "audit_hash": audit_hash
                    })),
                )
                .encode_line()
                .unwrap_or_default()
            }
            GatekeeperVerdict::Deny {
                reason,
                detail: _,
                audit_seq,
                audit_hash,
            } => {
                self.tools_call_denied += 1;
                let code = if reason == DenyReason::KillSwitchActive {
                    JSONRPC_KILL_SWITCH_ACTIVE
                } else {
                    JSONRPC_GATEKEEPER_DENIED
                };
                JsonRpcResponse::<serde_json::Value>::err(
                    id,
                    code,
                    format!("Blocked by Gatekeeper ({})", reason.as_code()),
                    Some(json!({
                        "reason": reason.as_code(),
                        "audit_seq": audit_seq,
                        "audit_hash": audit_hash
                    })),
                )
                .encode_line()
                .unwrap_or_default()
            }
        }
    }
}
