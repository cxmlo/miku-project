use miku_gatekeeper::Capability;
use serde::{Deserialize, Serialize};

use crate::catalog::{AgentCard, AgentManifest};

/// Real-time operational status of an agent bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentBridgeStatus {
    Active,
    Idle,
    Thinking,
    Offline,
    Unsupported { reason: String },
    Unavailable { reason: String },
    Error { code: String, message: String },
}

impl AgentBridgeStatus {
    pub fn mood_indicator(&self) -> &'static str {
        match self {
            AgentBridgeStatus::Active => "working",
            AgentBridgeStatus::Idle => "idle",
            AgentBridgeStatus::Thinking => "thinking",
            AgentBridgeStatus::Offline => "offline",
            AgentBridgeStatus::Unsupported { .. } => "unsupported",
            AgentBridgeStatus::Unavailable { .. } => "offline",
            AgentBridgeStatus::Error { .. } => "error",
        }
    }
}

/// Inbound event dispatched to an agent bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentBridgeEvent {
    Heartbeat,
    ToolCall {
        tool_name: String,
        capability: Capability,
        target: String,
        payload_json: String,
    },
    StatusSync {
        etag: Option<String>,
        payload: Option<String>,
    },
    Disconnect,
}

/// Outbound response from an agent bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum AgentBridgeResponse {
    Acknowledged { status: AgentBridgeStatus },
    ToolExecuted { output: String },
    NotModified { etag: String },
    Unsupported { reason: String },
}

/// Typed errors produced during agent bridging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentBridgeError {
    Unsupported(String),
    Unavailable(String),
    GatekeeperDenied(String),
    ProtocolError(String),
    InvalidPayload(String),
}

impl std::fmt::Display for AgentBridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentBridgeError::Unsupported(msg) => write!(f, "bridge unsupported: {}", msg),
            AgentBridgeError::Unavailable(msg) => write!(f, "bridge unavailable: {}", msg),
            AgentBridgeError::GatekeeperDenied(msg) => {
                write!(f, "action blocked by Gatekeeper: {}", msg)
            }
            AgentBridgeError::ProtocolError(msg) => write!(f, "protocol error: {}", msg),
            AgentBridgeError::InvalidPayload(msg) => write!(f, "invalid payload: {}", msg),
        }
    }
}

impl std::error::Error for AgentBridgeError {}

/// Polymorphic contract implemented by all agent adapters (Claude Code, Cursor, Antigravity, GitHub).
/// Decouples agent orchestration so desktop UI, CLI, and mobile Miku-Link treat every agent identically.
pub trait AgentBridge: Send + Sync {
    /// Returns the canonical agent ID (e.g. "claude-code", "cursor", "antigravity", "github").
    fn agent_id(&self) -> &str;

    /// Returns the declared manifest from the declarative catalog.
    fn manifest(&self) -> &AgentManifest;

    /// Returns the live operational status.
    fn status(&self) -> AgentBridgeStatus;

    /// Renders a standardized `AgentCard` for Island / mobile UI presentation.
    fn render_card(&self) -> AgentCard;

    /// Dispatches an event to the adapter, evaluating Gatekeeper rules and transport contracts.
    fn handle_event(
        &mut self,
        event: &AgentBridgeEvent,
    ) -> Result<AgentBridgeResponse, AgentBridgeError>;
}
