#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks
)]

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Duration;

use miku_gatekeeper::{
    Capability, DenyReason, GatekeeperEngine, GatekeeperVerdict, PermissionTicket,
};
use miku_protocol::{
    constant_time_token_eq, MikuLinkEnvelopeV1, MikuLinkMessageV1, PermissionDecisionKind,
    MIKU_PROTOCOL_VERSION_V1,
};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLifecycleState {
    Pending,
    SuspendedAsk,
    Executing,
    Completed,
    Cancelled,
    Denied,
}

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

/// Manages single-threaded FIFO task queues per agent session, pre-execution Gatekeeper
/// interception by token, and cooperative cancellation in `< 5.0 ms`.
pub struct AgentSessionManager {
    registry: ToolRegistry,
    gatekeeper: GatekeeperEngine,
    expected_session_token: String,
    session_queues: HashMap<String, VecDeque<String>>,
    tasks: HashMap<String, ManagedTaskRecord>,
    handler_execution_count: u64,
}

impl AgentSessionManager {
    pub fn new(
        registry: ToolRegistry,
        gatekeeper: GatekeeperEngine,
        expected_session_token: String,
    ) -> Self {
        Self {
            registry,
            gatekeeper,
            expected_session_token,
            session_queues: HashMap::new(),
            tasks: HashMap::new(),
            handler_execution_count: 0,
        }
    }

    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut ToolRegistry {
        &mut self.registry
    }

    pub fn gatekeeper(&self) -> &GatekeeperEngine {
        &self.gatekeeper
    }

    pub fn gatekeeper_mut(&mut self) -> &mut GatekeeperEngine {
        &mut self.gatekeeper
    }

    pub fn handler_execution_count(&self) -> u64 {
        self.handler_execution_count
    }

    pub fn task_state(&self, task_id: &str) -> Option<TaskLifecycleState> {
        self.tasks.get(task_id).map(|t| t.state)
    }

    pub fn cancel_handle_for(&self, task_id: &str) -> Option<CooperativeCancelHandle> {
        self.tasks.get(task_id).map(|t| t.cancel_handle.clone())
    }

    pub fn enqueue_task(&mut self, request: ToolInvocationRequest) -> Result<CooperativeCancelHandle, ToolError> {
        let _ = self.registry.validate_invocation(
            &request.tool_name,
            &request.target,
            &request.args,
        )?;
        let cancel_handle = CooperativeCancelHandle::new();
        let task_id = request.task_id.clone();
        let session_id = request.session_id.clone();

        self.tasks.insert(
            task_id.clone(),
            ManagedTaskRecord {
                request,
                state: TaskLifecycleState::Pending,
                pending_ticket: None,
                output: None,
                cancel_handle: cancel_handle.clone(),
            },
        );
        self.session_queues
            .entry(session_id)
            .or_default()
            .push_back(task_id);

        Ok(cancel_handle)
    }

    /// Cooperatively cancels a specific task (`Pending`, `SuspendedAsk`, or `Executing`).
    pub fn cancel_task(&mut self, task_id: &str) -> Result<TaskLifecycleState, ToolError> {
        let Some(record) = self.tasks.get_mut(task_id) else {
            return Err(ToolError::TaskNotFound(task_id.to_string()));
        };
        record.cancel_handle.cancel();
        if !matches!(
            record.state,
            TaskLifecycleState::Completed | TaskLifecycleState::Denied
        ) {
            record.state = TaskLifecycleState::Cancelled;
            record.pending_ticket = None;
        }
        Ok(record.state)
    }

    /// Triggers the global Kill Switch in `GatekeeperEngine` and cooperatively cancels
    /// all pending, suspended, or executing tasks across all sessions.
    pub fn trigger_global_kill_switch(&mut self, now_ms: u64) -> usize {
        let _ = self.gatekeeper.trigger_kill_switch(now_ms);
        let mut cancelled_tasks = 0usize;
        for record in self.tasks.values_mut() {
            if matches!(
                record.state,
                TaskLifecycleState::Pending
                    | TaskLifecycleState::SuspendedAsk
                    | TaskLifecycleState::Executing
            ) {
                record.cancel_handle.cancel();
                record.state = TaskLifecycleState::Cancelled;
                record.pending_ticket = None;
                cancelled_tasks += 1;
            }
        }
        self.session_queues.clear();
        cancelled_tasks
    }

    /// Drains and executes the next queued task in FIFO order for `session_id`.
    pub async fn dispatch_next_in_session(
        &mut self,
        session_id: &str,
    ) -> Option<Result<ToolDispatchOutcome, ToolError>> {
        let task_id = self.session_queues.get_mut(session_id)?.pop_front()?;
        Some(self.execute_registered_task(&task_id).await)
    }

    /// Enqueues and immediately dispatches a single tool invocation request.
    pub async fn dispatch_immediate(
        &mut self,
        request: ToolInvocationRequest,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let task_id = request.task_id.clone();
        let session_id = request.session_id.clone();
        let _ = self.enqueue_task(request)?;
        if let Some(q) = self.session_queues.get_mut(&session_id) {
            q.retain(|id| id != &task_id);
        }
        self.execute_registered_task(&task_id).await
    }

    async fn execute_registered_task(
        &mut self,
        task_id: &str,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let (request, cancel_handle, initial_state) = {
            let Some(rec) = self.tasks.get(task_id) else {
                return Err(ToolError::TaskNotFound(task_id.to_string()));
            };
            (
                rec.request.clone(),
                rec.cancel_handle.clone(),
                rec.state,
            )
        };

        // 0. Pre-flight cooperative cancellation or Kill Switch check
        if initial_state == TaskLifecycleState::Cancelled
            || cancel_handle.is_cancelled()
            || self.gatekeeper.is_kill_switch_active()
        {
            if let Some(rec) = self.tasks.get_mut(task_id) {
                rec.state = TaskLifecycleState::Cancelled;
            }
            return Ok(ToolDispatchOutcome::Cancelled {
                task_id: task_id.to_string(),
                reason: "cancelled_before_execution".to_string(),
            });
        }

        // 1. Constant-time session token validation BEFORE Gatekeeper or handler
        if !constant_time_token_eq(&request.session_token, &self.expected_session_token) {
            if let Some(rec) = self.tasks.get_mut(task_id) {
                rec.state = TaskLifecycleState::Denied;
            }
            return Ok(ToolDispatchOutcome::Unauthorized {
                task_id: task_id.to_string(),
            });
        }

        // 2. Deterministic tool lookup and schema validation
        let descriptor = self
            .registry
            .validate_invocation(&request.tool_name, &request.target, &request.args)?
            .clone();

        // 3. Pre-execution Gatekeeper evaluation (Deny > Ask > Allow)
        let verdict = self.gatekeeper.evaluate_request(
            &request.agent,
            descriptor.required_capability,
            &request.target,
            &request.args,
            request.timestamp_ms,
        );

        match verdict {
            GatekeeperVerdict::Deny {
                reason,
                detail,
                audit_seq,
                audit_hash,
            } => {
                if let Some(rec) = self.tasks.get_mut(task_id) {
                    rec.state = TaskLifecycleState::Denied;
                }
                Ok(ToolDispatchOutcome::Denied {
                    task_id: task_id.to_string(),
                    reason,
                    detail,
                    audit_seq,
                    audit_hash,
                })
            }
            GatekeeperVerdict::AskPending {
                ticket,
                audit_seq,
                audit_hash,
            } => {
                if let Some(rec) = self.tasks.get_mut(task_id) {
                    rec.state = TaskLifecycleState::SuspendedAsk;
                    rec.pending_ticket = Some(ticket.clone());
                }
                Ok(ToolDispatchOutcome::SuspendedAsk {
                    task_id: task_id.to_string(),
                    ticket,
                    audit_seq,
                    audit_hash,
                })
            }
            GatekeeperVerdict::Allow {
                canonical_target,
                audit_seq,
                audit_hash,
            } => {
                self.run_authorized_handler(
                    task_id,
                    &descriptor,
                    &canonical_target,
                    &request.args,
                    request.simulated_work_ms,
                    audit_seq,
                    audit_hash,
                    cancel_handle,
                )
                .await
            }
        }
    }

    /// Resumes a `SuspendedAsk` task after receiving a `PermissionDecision` from the user,
    /// enforcing anti-TOCTOU binding in `GatekeeperEngine`.
    pub async fn resume_suspended_ask_task(
        &mut self,
        task_id: &str,
        req_hash: &str,
        decision: PermissionDecisionKind,
        from_mobile_client: bool,
        biometric_verified: bool,
        execution_target: &str,
        execution_args: &str,
        now_ms: u64,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let (request, cancel_handle, current_state) = {
            let Some(rec) = self.tasks.get(task_id) else {
                return Err(ToolError::TaskNotFound(task_id.to_string()));
            };
            (rec.request.clone(), rec.cancel_handle.clone(), rec.state)
        };

        if current_state == TaskLifecycleState::Cancelled || cancel_handle.is_cancelled() {
            return Ok(ToolDispatchOutcome::Cancelled {
                task_id: task_id.to_string(),
                reason: "cancelled_while_suspended".to_string(),
            });
        }

        let descriptor = self.registry.get(&request.tool_name)?.clone();

        let verdict = self.gatekeeper.resolve_permission_decision(
            req_hash,
            decision,
            from_mobile_client,
            biometric_verified,
            &request.agent,
            descriptor.required_capability,
            execution_target,
            execution_args,
            now_ms,
        );

        match verdict {
            GatekeeperVerdict::Allow {
                canonical_target,
                audit_seq,
                audit_hash,
            } => {
                self.run_authorized_handler(
                    task_id,
                    &descriptor,
                    &canonical_target,
                    execution_args,
                    request.simulated_work_ms,
                    audit_seq,
                    audit_hash,
                    cancel_handle,
                )
                .await
            }
            GatekeeperVerdict::Deny {
                reason,
                detail,
                audit_seq,
                audit_hash,
            } => {
                if let Some(rec) = self.tasks.get_mut(task_id) {
                    rec.state = TaskLifecycleState::Denied;
                    rec.pending_ticket = None;
                }
                Ok(ToolDispatchOutcome::Denied {
                    task_id: task_id.to_string(),
                    reason,
                    detail,
                    audit_seq,
                    audit_hash,
                })
            }
            GatekeeperVerdict::AskPending {
                ticket,
                audit_seq,
                audit_hash,
            } => Ok(ToolDispatchOutcome::SuspendedAsk {
                task_id: task_id.to_string(),
                ticket,
                audit_seq,
                audit_hash,
            }),
        }
    }

    async fn run_authorized_handler(
        &mut self,
        task_id: &str,
        descriptor: &ToolDescriptor,
        canonical_target: &str,
        args: &str,
        simulated_work_ms: u64,
        audit_seq: u64,
        audit_hash: String,
        cancel_handle: CooperativeCancelHandle,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if let Some(rec) = self.tasks.get_mut(task_id) {
            rec.state = TaskLifecycleState::Executing;
            rec.pending_ticket = None;
        }

        if cancel_handle.is_cancelled() || self.gatekeeper.is_kill_switch_active() {
            if let Some(rec) = self.tasks.get_mut(task_id) {
                rec.state = TaskLifecycleState::Cancelled;
            }
            return Ok(ToolDispatchOutcome::Cancelled {
                task_id: task_id.to_string(),
                reason: "cancelled_pre_handler".to_string(),
            });
        }

        if simulated_work_ms > 0 {
            let mut cancel_rx = cancel_handle.receiver();
            let sleep_fut = tokio::time::sleep(Duration::from_millis(simulated_work_ms));
            tokio::pin!(sleep_fut);

            let cancelled = tokio::select! {
                biased;
                _ = cancel_rx.changed() => {
                    *cancel_rx.borrow()
                }
                _ = &mut sleep_fut => {
                    cancel_handle.is_cancelled()
                }
            };

            if cancelled || self.gatekeeper.is_kill_switch_active() {
                if let Some(rec) = self.tasks.get_mut(task_id) {
                    rec.state = TaskLifecycleState::Cancelled;
                }
                return Ok(ToolDispatchOutcome::Cancelled {
                    task_id: task_id.to_string(),
                    reason: "cancelled_cooperatively_in_flight".to_string(),
                });
            }
        }

        // Execute deterministic internal tool handler
        self.handler_execution_count += 1;
        let result_summary = match descriptor.family {
            InternalToolFamily::Fs => {
                format!("fs_ok:{}:len={}", canonical_target, args.len())
            }
            InternalToolFamily::Shell => {
                format!("shell_ok:cmd={}:args={}", canonical_target, args.trim())
            }
            InternalToolFamily::Browser => {
                format!("browser_ok:url={}:action={}", canonical_target, args.trim())
            }
            InternalToolFamily::Memory => {
                format!("memory_ok:scope={}:query={}", canonical_target, args.trim())
            }
        };

        let output = ToolExecutionOutput {
            task_id: task_id.to_string(),
            tool_name: descriptor.name.clone(),
            family: descriptor.family,
            canonical_target: canonical_target.to_string(),
            result_summary,
            audit_seq,
            audit_hash,
        };

        if let Some(rec) = self.tasks.get_mut(task_id) {
            rec.state = TaskLifecycleState::Completed;
            rec.output = Some(output.clone());
        }

        Ok(ToolDispatchOutcome::Completed(output))
    }

    /// Translates a `ToolDispatchOutcome` into a `MikuLinkEnvelopeV1` correlated by `reply_to`.
    pub fn outcome_to_protocol_envelope(
        &self,
        request_envelope_id: &str,
        agent: &str,
        outcome: &ToolDispatchOutcome,
    ) -> MikuLinkEnvelopeV1 {
        match outcome {
            ToolDispatchOutcome::Completed(out) => MikuLinkEnvelopeV1 {
                v: MIKU_PROTOCOL_VERSION_V1,
                id: format!("agent-done-{}", out.task_id),
                reply_to: Some(request_envelope_id.to_string()),
                msg: MikuLinkMessageV1::AgentEvent {
                    agent: agent.to_string(),
                    kind: format!("tool_completed:{}", out.tool_name),
                    title: out.result_summary.clone(),
                },
            },
            ToolDispatchOutcome::SuspendedAsk {
                task_id, ticket, ..
            } => MikuLinkEnvelopeV1 {
                v: MIKU_PROTOCOL_VERSION_V1,
                id: format!("agent-ask-{}", task_id),
                reply_to: Some(request_envelope_id.to_string()),
                msg: MikuLinkMessageV1::PermissionRequest {
                    req_hash: ticket.req_hash.clone(),
                    agent: ticket.agent.clone(),
                    capability: ticket.capability.as_str().to_string(),
                    target: ticket.canonical_target.clone(),
                    ttl_sec: 60,
                    pc_only: ticket.pc_only,
                },
            },
            ToolDispatchOutcome::Denied {
                task_id,
                reason,
                detail,
                ..
            } => MikuLinkEnvelopeV1 {
                v: MIKU_PROTOCOL_VERSION_V1,
                id: format!("agent-deny-{}", task_id),
                reply_to: Some(request_envelope_id.to_string()),
                msg: MikuLinkMessageV1::Error {
                    code: "gatekeeper_denied".to_string(),
                    supported: None,
                    detail: Some(format!("{}: {}", reason.as_code(), detail)),
                },
            },
            ToolDispatchOutcome::Unauthorized { task_id } => MikuLinkEnvelopeV1 {
                v: MIKU_PROTOCOL_VERSION_V1,
                id: format!("agent-unauth-{}", task_id),
                reply_to: Some(request_envelope_id.to_string()),
                msg: MikuLinkMessageV1::Error {
                    code: "unauthorized".to_string(),
                    supported: None,
                    detail: Some("invalid session token for tool dispatch".to_string()),
                },
            },
            ToolDispatchOutcome::Cancelled { task_id, reason } => MikuLinkEnvelopeV1 {
                v: MIKU_PROTOCOL_VERSION_V1,
                id: format!("agent-cancel-{}", task_id),
                reply_to: Some(request_envelope_id.to_string()),
                msg: MikuLinkMessageV1::AgentEvent {
                    agent: agent.to_string(),
                    kind: "tool_cancelled".to_string(),
                    title: format!("{}:{}", task_id, reason),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use miku_core::{build_current_thread_runtime, spawn_orchestrator};
    use miku_gatekeeper::{PolicyEffect, ProjectProfile};
    use miku_gateway::{start_gateway_server, GatewayConfig, LoopbackWsClient};
    use miku_protocol::BaseContractBundle;

    fn sample_agent_profile() -> ProjectProfile {
        ProjectProfile::new("miku-agent-test", "C:/Users/cmiranda/dev/miku", 60_000)
            .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
            .with_rule(Capability::FsRead, "memory/**", PolicyEffect::Allow, false)
            .with_rule(
                Capability::FsRead,
                "src/secrets/**",
                PolicyEffect::Deny,
                true,
            )
            .with_rule(Capability::FsWrite, "src/**", PolicyEffect::Ask, false)
            .with_rule(Capability::Shell, "cargo test*", PolicyEffect::Allow, false)
            .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true)
            .with_rule(
                Capability::Browser,
                "https://docs.rs/*",
                PolicyEffect::Allow,
                false,
            )
    }

    // LEDGER-1.4-1
    #[test]
    fn test_deterministic_tool_registry_internal_tools_and_duplicate_rejection() {
        let mut reg = ToolRegistry::with_default_internal_tools();
        assert_eq!(reg.len(), 5);

        // Verify deterministic BTreeMap ordering
        let names: Vec<String> = reg
            .list_deterministic()
            .into_iter()
            .map(|d| d.name.clone())
            .collect();
        assert_eq!(
            names,
            vec![
                "browser_action",
                "fs_read",
                "fs_write",
                "memory_query",
                "shell_exec"
            ]
        );

        // Verify exact family and capability mappings for FS, Shell, Browser, Memory
        assert_eq!(
            reg.get("fs_read").unwrap().required_capability,
            Capability::FsRead
        );
        assert_eq!(
            reg.get("fs_write").unwrap().required_capability,
            Capability::FsWrite
        );
        assert_eq!(
            reg.get("shell_exec").unwrap().required_capability,
            Capability::Shell
        );
        assert_eq!(
            reg.get("browser_action").unwrap().required_capability,
            Capability::Browser
        );
        assert_eq!(
            reg.get("memory_query").unwrap().family,
            InternalToolFamily::Memory
        );

        // Duplicate registration must fail deterministically
        let dup_err = reg
            .register(ToolDescriptor {
                name: "FS_READ".to_string(),
                family: InternalToolFamily::Fs,
                required_capability: Capability::FsRead,
                requires_non_empty_target: true,
                description: "duplicate".to_string(),
            })
            .expect_err("duplicate tool must be rejected");
        assert!(matches!(dup_err, ToolError::DuplicateTool(ref n) if n == "fs_read"));

        // Unknown tool must fail with ToolNotFound
        assert!(matches!(
            reg.get("unknown_tool"),
            Err(ToolError::ToolNotFound(_))
        ));

        // Empty target or null bytes must fail schema validation
        assert!(matches!(
            reg.validate_invocation("fs_read", "   ", ""),
            Err(ToolError::InvalidArguments(_))
        ));
        assert!(matches!(
            reg.validate_invocation("fs_read", "src/lib.rs\0", ""),
            Err(ToolError::InvalidArguments(_))
        ));
    }

    // LEDGER-1.4-2
    #[test]
    fn test_pre_execution_gatekeeper_interception_and_capability_token_validation() {
        let rt = build_current_thread_runtime().expect("current_thread runtime");
        rt.block_on(async {
            let gk = GatekeeperEngine::new(sample_agent_profile()).expect("init gk");
            let mut mgr = AgentSessionManager::new(
                ToolRegistry::with_default_internal_tools(),
                gk,
                "miku_session_secret_01".to_string(),
            );

            // 1. Invalid session token -> Unauthorized (0 handler executions)
            let out_unauth = mgr
                .dispatch_immediate(ToolInvocationRequest {
                    task_id: "t-unauth".to_string(),
                    session_id: "s1".to_string(),
                    agent: "claude_code".to_string(),
                    session_token: "wrong_token".to_string(),
                    tool_name: "fs_read".to_string(),
                    target: "src/lib.rs".to_string(),
                    args: "".to_string(),
                    simulated_work_ms: 0,
                    timestamp_ms: 1_000,
                })
                .await
                .expect("dispatch unauth");
            assert!(matches!(out_unauth, ToolDispatchOutcome::Unauthorized { .. }));
            assert_eq!(mgr.handler_execution_count(), 0);
            assert_eq!(mgr.task_state("t-unauth"), Some(TaskLifecycleState::Denied));

            // 2. Gatekeeper ExplicitDenyRule ("src/secrets/**") -> Denied (0 handler executions)
            let out_deny = mgr
                .dispatch_immediate(ToolInvocationRequest {
                    task_id: "t-deny".to_string(),
                    session_id: "s1".to_string(),
                    agent: "claude_code".to_string(),
                    session_token: "miku_session_secret_01".to_string(),
                    tool_name: "fs_read".to_string(),
                    target: "src/secrets/key.pem".to_string(),
                    args: "".to_string(),
                    simulated_work_ms: 0,
                    timestamp_ms: 1_001,
                })
                .await
                .expect("dispatch deny");
            assert!(matches!(
                out_deny,
                ToolDispatchOutcome::Denied {
                    reason: DenyReason::ExplicitDenyRule,
                    ..
                }
            ));
            assert_eq!(mgr.handler_execution_count(), 0);

            // 3. Gatekeeper PathJailViolation ("..\..\Windows\System32\cmd.exe") -> Denied
            let out_jail = mgr
                .dispatch_immediate(ToolInvocationRequest {
                    task_id: "t-jail".to_string(),
                    session_id: "s1".to_string(),
                    agent: "cursor".to_string(),
                    session_token: "miku_session_secret_01".to_string(),
                    tool_name: "fs_write".to_string(),
                    target: r"..\..\Windows\System32\cmd.exe".to_string(),
                    args: "pwn".to_string(),
                    simulated_work_ms: 0,
                    timestamp_ms: 1_002,
                })
                .await
                .expect("dispatch jail");
            assert!(matches!(
                out_jail,
                ToolDispatchOutcome::Denied {
                    reason: DenyReason::PathJailViolation,
                    ..
                }
            ));
            assert_eq!(mgr.handler_execution_count(), 0);

            // 4. Gatekeeper AskPending ("fs_write" on "src/main.rs") -> Suspends execution until approved
            let out_ask = mgr
                .dispatch_immediate(ToolInvocationRequest {
                    task_id: "t-ask".to_string(),
                    session_id: "s1".to_string(),
                    agent: "claude_code".to_string(),
                    session_token: "miku_session_secret_01".to_string(),
                    tool_name: "fs_write".to_string(),
                    target: "src/main.rs".to_string(),
                    args: "fn verified() {}".to_string(),
                    simulated_work_ms: 0,
                    timestamp_ms: 1_003,
                })
                .await
                .expect("dispatch ask");
            let ticket = match out_ask {
                ToolDispatchOutcome::SuspendedAsk { ticket, .. } => ticket,
                other => panic!("expected SuspendedAsk, got {:?}", other),
            };
            assert_eq!(
                mgr.task_state("t-ask"),
                Some(TaskLifecycleState::SuspendedAsk)
            );
            assert_eq!(mgr.handler_execution_count(), 0);

            // Resume with matching args -> Completed (handler_execution_count becomes 1)
            let out_resumed = mgr
                .resume_suspended_ask_task(
                    "t-ask",
                    &ticket.req_hash,
                    PermissionDecisionKind::Allow,
                    false,
                    false,
                    "src/main.rs",
                    "fn verified() {}",
                    1_004,
                )
                .await
                .expect("resume ask");
            assert!(matches!(out_resumed, ToolDispatchOutcome::Completed(_)));
            assert_eq!(mgr.handler_execution_count(), 1);
            assert_eq!(
                mgr.task_state("t-ask"),
                Some(TaskLifecycleState::Completed)
            );
        });
    }

    // LEDGER-1.4-3
    #[test]
    fn test_agent_session_serial_queue_and_cooperative_cancellation_lifecycle() {
        let rt = build_current_thread_runtime().expect("current_thread runtime");
        rt.block_on(async {
            let gk = GatekeeperEngine::new(sample_agent_profile()).expect("init gk");
            let mut mgr = AgentSessionManager::new(
                ToolRegistry::with_default_internal_tools(),
                gk,
                "miku_session_secret_01".to_string(),
            );

            // Enqueue 3 tasks in session "sess_fifo"
            let _h1 = mgr
                .enqueue_task(ToolInvocationRequest {
                    task_id: "fifo-1".to_string(),
                    session_id: "sess_fifo".to_string(),
                    agent: "claude_code".to_string(),
                    session_token: "miku_session_secret_01".to_string(),
                    tool_name: "fs_read".to_string(),
                    target: "src/one.rs".to_string(),
                    args: "".to_string(),
                    simulated_work_ms: 0,
                    timestamp_ms: 2_000,
                })
                .expect("enqueue 1");

            let h2 = mgr
                .enqueue_task(ToolInvocationRequest {
                    task_id: "fifo-2".to_string(),
                    session_id: "sess_fifo".to_string(),
                    agent: "claude_code".to_string(),
                    session_token: "miku_session_secret_01".to_string(),
                    tool_name: "shell_exec".to_string(),
                    target: "cargo test --lib".to_string(),
                    args: "--nocapture".to_string(),
                    simulated_work_ms: 250,
                    timestamp_ms: 2_001,
                })
                .expect("enqueue 2");

            let _h3 = mgr
                .enqueue_task(ToolInvocationRequest {
                    task_id: "fifo-3".to_string(),
                    session_id: "sess_fifo".to_string(),
                    agent: "claude_code".to_string(),
                    session_token: "miku_session_secret_01".to_string(),
                    tool_name: "memory_query".to_string(),
                    target: "memory/architecture.md".to_string(),
                    args: "ADR-007".to_string(),
                    simulated_work_ms: 0,
                    timestamp_ms: 2_002,
                })
                .expect("enqueue 3");

            // 1st task completes in FIFO order
            let r1 = mgr
                .dispatch_next_in_session("sess_fifo")
                .await
                .unwrap()
                .expect("run 1");
            assert!(matches!(r1, ToolDispatchOutcome::Completed(ref o) if o.task_id == "fifo-1"));

            // Trigger cooperative cancellation on 2nd task while in flight (250 ms simulated work, cancelled after 1 ms)
            let h2_clone = h2.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(1)).await;
                h2_clone.cancel();
            });

            let t_cancel = Instant::now();
            let r2 = mgr
                .dispatch_next_in_session("sess_fifo")
                .await
                .unwrap()
                .expect("run 2");
            let cancel_elapsed_ms = t_cancel.elapsed().as_secs_f64() * 1000.0;
            assert!(matches!(r2, ToolDispatchOutcome::Cancelled { ref task_id, .. } if task_id == "fifo-2"));
            assert!(
                cancel_elapsed_ms < 50.0,
                "cooperative cancellation must abort well before 250 ms work finishes"
            );
            assert_eq!(
                mgr.task_state("fifo-2"),
                Some(TaskLifecycleState::Cancelled)
            );

            // Trigger Global Kill Switch -> cancels remaining queued task "fifo-3"
            let cancelled_count = mgr.trigger_global_kill_switch(2_050);
            assert_eq!(cancelled_count, 1);
            assert_eq!(
                mgr.task_state("fifo-3"),
                Some(TaskLifecycleState::Cancelled)
            );
            assert!(mgr.dispatch_next_in_session("sess_fifo").await.is_none());
        });
    }

    // LEDGER-1.4-4
    #[test]
    fn test_end_to_end_tool_dispatch_over_gateway_and_core_event_bus() {
        let rt = build_current_thread_runtime().expect("current_thread runtime");
        rt.block_on(async {
            let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
            let (orch_handle, orch_join) =
                spawn_orchestrator(bundle, "miku_agent_ws_tok".to_string(), 64);
            let (gw_handle, gw_join) =
                start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                    .await
                    .expect("start gateway");

            let gk = GatekeeperEngine::new(sample_agent_profile()).expect("init gk");
            let mut mgr = AgentSessionManager::new(
                ToolRegistry::with_default_internal_tools(),
                gk,
                "miku_agent_ws_tok".to_string(),
            );

            let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
                .await
                .expect("connect WS");
            let hello = r##"{"v":1,"id":"h-agent","msg":{"type":"hello","data":{"client":"miku_island","token":"miku_agent_ws_tok","capabilities":["command","agent_events"]}}}"##;
            client.send_text(hello).await.expect("send hello");
            let _ = client.recv_envelope().await.expect("recv hello_ack");

            // Send command over Gateway WS and execute all 4 internal tool families (FS, Shell, Browser, Memory)
            let cmd = r##"{"v":1,"id":"cmd-1","msg":{"type":"command","data":{"session_id":"sess_01","action":"dispatch_tool","text":"browser_action https://docs.rs/tokio"}}}"##;
            client.send_text(cmd).await.expect("send command");
            let gw_ack = client.recv_envelope().await.expect("recv gw ack");
            assert_eq!(gw_ack.reply_to.as_deref(), Some("cmd-1"));

            for (idx, (tool, target, args)) in [
                ("fs_read", "src/lib.rs", ""),
                ("shell_exec", "cargo test --workspace", ""),
                ("browser_action", "https://docs.rs/tokio", "navigate"),
                ("memory_query", "memory/notes.md", "tokio current_thread"),
            ]
            .iter()
            .enumerate()
            {
                let req_id = format!("req-{}", idx + 1);
                let outcome = mgr
                    .dispatch_immediate(ToolInvocationRequest {
                        task_id: format!("task-{}", idx + 1),
                        session_id: "sess_01".to_string(),
                        agent: "claude_code".to_string(),
                        session_token: "miku_agent_ws_tok".to_string(),
                        tool_name: (*tool).to_string(),
                        target: (*target).to_string(),
                        args: (*args).to_string(),
                        simulated_work_ms: 0,
                        timestamp_ms: 3_000 + (idx as u64),
                    })
                    .await
                    .expect("dispatch internal tool");
                assert!(matches!(outcome, ToolDispatchOutcome::Completed(_)));

                let env = mgr.outcome_to_protocol_envelope(&req_id, "claude_code", &outcome);
                assert_eq!(env.reply_to.as_deref(), Some(req_id.as_str()));
            }

            assert_eq!(mgr.handler_execution_count(), 4);
            assert!(mgr.gatekeeper().audit_log().verify_chain().is_ok());

            client.close_cleanly().await.expect("close WS");
            gw_handle.trigger_shutdown();
            gw_join.await.expect("gw join");
            orch_handle.trigger_system_shutdown_signal();
            orch_join.await.expect("orch join");
        });
    }
}
