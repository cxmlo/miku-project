use serde::{Deserialize, Serialize};

use crate::ProtocolError;

// ============================================================================
// 1. Standard JSON-RPC 2.0 Framing Envelope
// ============================================================================

pub const JSONRPC_VERSION_2_0: &str = "2.0";

/// Standard JSON-RPC 2.0 and Gatekeeper-specific error codes.
pub const JSONRPC_PARSE_ERROR: i32 = -32700;
pub const JSONRPC_INVALID_REQUEST: i32 = -32600;
pub const JSONRPC_METHOD_NOT_FOUND: i32 = -32601;
pub const JSONRPC_INVALID_PARAMS: i32 = -32602;
pub const JSONRPC_GATEKEEPER_DENIED: i32 = -32001;
pub const JSONRPC_KILL_SWITCH_ACTIVE: i32 = -32002;
pub const JSONRPC_TIMEOUT_EXPIRED: i32 = -32003;

/// Maximum fail-open timeout in milliseconds when Miku daemon is unreachable.
pub const CLAUDE_HOOK_FAIL_OPEN_TIMEOUT_MS: u64 = 15;

/// JSON-RPC 2.0 request identifier (integer or string; omitted for notifications).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcId {
    Number(i64),
    String(String),
}

/// Incoming JSON-RPC 2.0 request or notification frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRpcRequest<T> {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<JsonRpcId>,
    pub method: String,
    pub params: T,
}

impl<T: Serialize + for<'de> Deserialize<'de>> JsonRpcRequest<T> {
    pub fn new(id: Option<JsonRpcId>, method: impl Into<String>, params: T) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION_2_0.to_string(),
            id,
            method: method.into(),
            params,
        }
    }

    pub fn decode_line(raw_line: &str) -> Result<Self, ProtocolError> {
        let trimmed = raw_line.trim();
        if trimmed.len() > crate::MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge {
                size_bytes: trimmed.len(),
                max_bytes: crate::MAX_FRAME_BYTES,
            });
        }
        let req: Self = serde_json::from_str(trimmed)
            .map_err(|e| ProtocolError::MalformedJson(e.to_string()))?;
        if req.jsonrpc != JSONRPC_VERSION_2_0 {
            return Err(ProtocolError::ContractValidation(format!(
                "invalid jsonrpc version '{}'; expected '2.0'",
                req.jsonrpc
            )));
        }
        if req.method.trim().is_empty() {
            return Err(ProtocolError::ContractValidation(
                "jsonrpc method cannot be empty".to_string(),
            ));
        }
        Ok(req)
    }

    pub fn encode_line(&self) -> Result<String, ProtocolError> {
        let mut s = serde_json::to_string(self)
            .map_err(|e| ProtocolError::MalformedJson(e.to_string()))?;
        s.push('\n');
        Ok(s)
    }
}

/// Outgoing JSON-RPC 2.0 response frame toward Claude Code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcResponse<T> {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<JsonRpcId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl<T: Serialize + for<'de> Deserialize<'de>> JsonRpcResponse<T> {
    pub fn ok(id: Option<JsonRpcId>, result: T) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION_2_0.to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Option<JsonRpcId>, code: i32, message: impl Into<String>, data: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION_2_0.to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data,
            }),
        }
    }

    pub fn decode_line(raw_line: &str) -> Result<Self, ProtocolError> {
        let trimmed = raw_line.trim();
        let resp: Self = serde_json::from_str(trimmed)
            .map_err(|e| ProtocolError::MalformedJson(e.to_string()))?;
        if resp.jsonrpc != JSONRPC_VERSION_2_0 {
            return Err(ProtocolError::ContractValidation(format!(
                "invalid jsonrpc response version '{}'",
                resp.jsonrpc
            )));
        }
        Ok(resp)
    }

    pub fn encode_line(&self) -> Result<String, ProtocolError> {
        let mut s = serde_json::to_string(self)
            .map_err(|e| ProtocolError::MalformedJson(e.to_string()))?;
        s.push('\n');
        Ok(s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// ============================================================================
// 2. Claude Code Hook Events & Strongly-Typed Tool Payloads
// ============================================================================

/// Lifecycle events emitted by Claude Code CLI / hooks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "hook_event", rename_all = "snake_case")]
pub enum ClaudeHookEvent {
    PreToolUse(ClaudePreToolUseParams),
    PostToolUse(ClaudePostToolUseParams),
    Notification(ClaudeNotificationParams),
}

/// Parameters emitted before executing a tool (`PreToolUse`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudePreToolUseParams {
    pub session_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub timestamp_ms: u64,
}

/// Strongly-typed input payload for the `Bash` tool in Claude Code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeBashInput {
    pub command: String,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
}

/// Strongly-typed input payload for file system tools (`Edit`, `Write`, `Read`, `Glob`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeFileToolInput {
    pub file_path: String,
    #[serde(default)]
    pub content: Option<String>,
}

/// Parameters emitted after completing a tool (`PostToolUse`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudePostToolUseParams {
    pub session_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub tool_output: serde_json::Value,
    pub duration_ms: u64,
    pub is_error: bool,
}

/// Reactive progress or cognitive state telemetry from Claude Code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeNotificationParams {
    pub session_id: String,
    pub phase: ClaudeSessionPhase,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeSessionPhase {
    Thinking,
    Working,
    WaitingPermission,
    Idle,
}

// ============================================================================
// 3. Hook Verdicts Returned to Claude Code CLI (stdout / IPC)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeHookDecisionKind {
    Allow,
    Deny,
    FailOpen,
}

/// Direct response emitted by `miku hook` to authorize or veto a tool invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeHookResult {
    pub decision: ClaudeHookDecisionKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit_hash: Option<String>,
}

impl ClaudeHookResult {
    pub fn allow(audit_seq: u64, audit_hash: String) -> Self {
        Self {
            decision: ClaudeHookDecisionKind::Allow,
            reason: None,
            audit_seq: Some(audit_seq),
            audit_hash: Some(audit_hash),
        }
    }

    pub fn deny(reason: impl Into<String>, audit_seq: Option<u64>, audit_hash: Option<String>) -> Self {
        Self {
            decision: ClaudeHookDecisionKind::Deny,
            reason: Some(reason.into()),
            audit_seq,
            audit_hash,
        }
    }

    pub fn fail_open(reason: impl Into<String>) -> Self {
        Self {
            decision: ClaudeHookDecisionKind::FailOpen,
            reason: Some(reason.into()),
            audit_seq: None,
            audit_hash: None,
        }
    }
}
