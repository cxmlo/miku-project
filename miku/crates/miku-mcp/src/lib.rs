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

//! `miku-mcp`: Local Model Context Protocol (MCP `2024-11-05`) Host for Miku Desktop Orchestrator (`F-07` / `ADR-004`).
//!
//! Exposes Miku's authorized tools (`ToolRegistry`) to local MCP clients (Cursor, Claude Code, etc.)
//! over loopback (`127.0.0.1` / `::1`) and stdio JSON-RPC 2.0, enforcing pre-execution evaluation
//! in `miku_gatekeeper::GatekeeperEngine` and SHA-256 tamper-evident audit logging on every call.

use serde::{Deserialize, Serialize};

pub mod host;
pub mod transport;
pub mod client;


#[cfg(test)]
mod tests;
#[cfg(test)]
mod client_tests;

pub use host::*;
pub use transport::*;
pub use client::*;

pub const MCP_PROTOCOL_VERSION_2024_11_05: &str = "2024-11-05";
pub const SUPPORTED_MCP_PROTOCOL_VERSIONS: &[&str] = &[MCP_PROTOCOL_VERSION_2024_11_05];
pub const DEFAULT_MCP_LOOPBACK_PORT: u16 = 47410;
pub const MAX_MCP_DISPATCH_LATENCY_MS: f64 = 5.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpHostError {
    NonLoopbackBindForbidden(String),
    IoError(String),
    ProtocolError(String),
    UnauthorizedSession,
    ConnectionClosed,
}

impl std::fmt::Display for McpHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpHostError::NonLoopbackBindForbidden(addr) => write!(
                f,
                "MCP Host bind to non-loopback address '{}' is strictly forbidden",
                addr
            ),
            McpHostError::IoError(msg) => write!(f, "MCP Host I/O error: {}", msg),
            McpHostError::ProtocolError(msg) => write!(f, "MCP protocol error: {}", msg),
            McpHostError::UnauthorizedSession => write!(f, "unauthorized MCP session"),
            McpHostError::ConnectionClosed => write!(f, "MCP connection closed"),
        }
    }
}

impl std::error::Error for McpHostError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInitializeParams {
    pub protocol_version: String,
    #[serde(default)]
    pub client_info: Option<McpClientInfo>,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolsCapability {
    pub list_changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerCapabilities {
    pub tools: McpToolsCapability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInitializeResult {
    pub protocol_version: String,
    pub capabilities: McpServerCapabilities,
    pub server_info: McpServerInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDefinition {
    pub name: String,
    pub description: String,
    pub capability: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolsListResult {
    pub tools: Vec<McpToolDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolCallArguments {
    pub target: String,
    #[serde(default)]
    pub args: String,
    #[serde(default)]
    pub session_token: Option<String>,
    #[serde(default)]
    pub timestamp_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolCallParams {
    pub name: String,
    pub arguments: McpToolCallArguments,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallResult {
    pub content: Vec<McpContentBlock>,
    pub is_error: bool,
    pub canonical_target: String,
    pub audit_seq: u64,
    pub audit_hash: String,
}


