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

use std::collections::BTreeMap;

use miku_gatekeeper::{Capability, DenyReason, PermissionTicket};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

pub mod adapters;
pub mod bridge;
pub mod catalog;
pub mod claude_adapter;
pub mod claude_tracker_and_telemetry;
pub mod harness;
pub mod secrets;
pub mod session_manager;
pub mod session_queue;
pub mod token_saver;

pub use adapters::*;
pub use bridge::*;
pub use catalog::*;
pub use claude_adapter::*;
pub use harness::{
    repair_tool_call_json, AgentHarness, HarnessConfig, HarnessError, LlmProvider,
    ToolCallRequest, ToolJsonRepairError, TurnStepOutcome,
};
pub use secrets::*;
pub use session_manager::*;
pub use session_queue::*;
pub use token_saver::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InternalToolFamily {
    Fs,
    Shell,
    Browser,
    Memory,
}

impl InternalToolFamily {
    pub fn as_str(&self) -> &'static str {
        match self {
            InternalToolFamily::Fs => "fs",
            InternalToolFamily::Shell => "shell",
            InternalToolFamily::Browser => "browser",
            InternalToolFamily::Memory => "memory",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub family: InternalToolFamily,
    pub required_capability: Capability,
    pub requires_non_empty_target: bool,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolError {
    DuplicateTool(String),
    ToolNotFound(String),
    InvalidArguments(String),
    UnauthorizedSessionToken,
    GatekeeperDenied {
        reason: DenyReason,
        detail: String,
        audit_seq: u64,
        audit_hash: String,
    },
    TaskNotFound(String),
    TaskCancelled(String),
    ExecutionFailed(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::DuplicateTool(name) => write!(f, "duplicate tool registration '{}'", name),
            ToolError::ToolNotFound(name) => write!(f, "tool '{}' is not registered", name),
            ToolError::InvalidArguments(msg) => write!(f, "invalid tool arguments: {}", msg),
            ToolError::UnauthorizedSessionToken => {
                write!(f, "unauthorized agent/session token rejected")
            }
            ToolError::GatekeeperDenied { reason, detail, .. } => {
                write!(f, "blocked by Gatekeeper ({}): {}", reason.as_code(), detail)
            }
            ToolError::TaskNotFound(id) => write!(f, "task '{}' not found", id),
            ToolError::TaskCancelled(id) => write!(f, "task '{}' was cooperatively cancelled", id),
            ToolError::ExecutionFailed(err) => write!(f, "tool execution failed: {}", err),
        }
    }
}

impl std::error::Error for ToolError {}

/// Deterministic registry of internal tools (`FS`, `Shell`, `Browser`, `Memory`)
/// mapped immutably to Gatekeeper `Capability` policies.
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, ToolDescriptor>,
}

impl ToolRegistry {
    pub fn new_empty() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn with_default_internal_tools() -> Self {
        let mut reg = Self::new_empty();
        let defaults = [
            ToolDescriptor {
                name: "fs_read".to_string(),
                family: InternalToolFamily::Fs,
                required_capability: Capability::FsRead,
                requires_non_empty_target: true,
                description: "Read file confined within canonical project jail".to_string(),
            },
            ToolDescriptor {
                name: "fs_write".to_string(),
                family: InternalToolFamily::Fs,
                required_capability: Capability::FsWrite,
                requires_non_empty_target: true,
                description: "Write file confined within canonical project jail".to_string(),
            },
            ToolDescriptor {
                name: "shell_exec".to_string(),
                family: InternalToolFamily::Shell,
                required_capability: Capability::Shell,
                requires_non_empty_target: true,
                description: "Execute deterministic command under Gatekeeper profile".to_string(),
            },
            ToolDescriptor {
                name: "browser_action".to_string(),
                family: InternalToolFamily::Browser,
                required_capability: Capability::Browser,
                requires_non_empty_target: true,
                description: "Execute isolated CDP/BiDi browser action under domain policy"
                    .to_string(),
            },
            ToolDescriptor {
                name: "memory_query".to_string(),
                family: InternalToolFamily::Memory,
                required_capability: Capability::FsRead,
                requires_non_empty_target: true,
                description: "Query local Markdown/FTS5 project memory under Gatekeeper"
                    .to_string(),
            },
        ];
        for desc in defaults {
            let _ = reg.register(desc);
        }
        reg
    }

    pub fn register(&mut self, descriptor: ToolDescriptor) -> Result<(), ToolError> {
        let key = descriptor.name.trim().to_ascii_lowercase();
        if key.is_empty() {
            return Err(ToolError::InvalidArguments(
                "tool name cannot be empty".to_string(),
            ));
        }
        if self.tools.contains_key(&key) {
            return Err(ToolError::DuplicateTool(key));
        }
        let mut normalized = descriptor;
        normalized.name = key.clone();
        self.tools.insert(key, normalized);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<&ToolDescriptor, ToolError> {
        let key = name.trim().to_ascii_lowercase();
        self.tools
            .get(&key)
            .ok_or(ToolError::ToolNotFound(key))
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn list_deterministic(&self) -> Vec<&ToolDescriptor> {
        self.tools.values().collect()
    }

    pub fn validate_invocation(
        &self,
        tool_name: &str,
        raw_target: &str,
        raw_args: &str,
    ) -> Result<&ToolDescriptor, ToolError> {
        let desc = self.get(tool_name)?;
        if desc.requires_non_empty_target && raw_target.trim().is_empty() {
            return Err(ToolError::InvalidArguments(format!(
                "tool '{}' requires a non-empty target",
                desc.name
            )));
        }
        if raw_target.bytes().any(|b| b == 0 || b < 0x20)
            || raw_args.bytes().any(|b| b == 0)
        {
            return Err(ToolError::InvalidArguments(
                "target or args contain null/control byte".to_string(),
            ));
        }
        Ok(desc)
    }
}

pub use miku_protocol::{
    IslandOrderCommand, IslandOrderResponse, SessionOrderRecord, TaskLifecycleState,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInvocationRequest {
    pub task_id: String,
    pub session_id: String,
    pub agent: String,
    pub session_token: String,
    pub tool_name: String,
    pub target: String,
    pub args: String,
    pub simulated_work_ms: u64,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecutionOutput {
    pub task_id: String,
    pub tool_name: String,
    pub family: InternalToolFamily,
    pub canonical_target: String,
    pub result_summary: String,
    pub audit_seq: u64,
    pub audit_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolDispatchOutcome {
    Completed(ToolExecutionOutput),
    SuspendedAsk {
        task_id: String,
        ticket: PermissionTicket,
        audit_seq: u64,
        audit_hash: String,
    },
    Denied {
        task_id: String,
        reason: DenyReason,
        detail: String,
        audit_seq: u64,
        audit_hash: String,
    },
    Unauthorized {
        task_id: String,
    },
    Cancelled {
        task_id: String,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct CooperativeCancelHandle {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl CooperativeCancelHandle {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self { tx, rx }
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.rx.borrow()
    }

    pub fn receiver(&self) -> watch::Receiver<bool> {
        self.rx.clone()
    }
}

impl Default for CooperativeCancelHandle {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ManagedTaskRecord {
    pub request: ToolInvocationRequest,
    pub state: TaskLifecycleState,
    pub pending_ticket: Option<PermissionTicket>,
    pub output: Option<ToolExecutionOutput>,
    pub cancel_handle: CooperativeCancelHandle,
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod claude_tests;

#[cfg(test)]
mod session_queue_tests;

#[cfg(test)]
mod catalog_tests;

