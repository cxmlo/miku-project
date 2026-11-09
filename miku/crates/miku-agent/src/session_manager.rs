use std::collections::{HashMap, VecDeque};

use miku_gatekeeper::{GatekeeperEngine, GatekeeperVerdict};
use miku_protocol::{
    constant_time_token_eq, MikuLinkEnvelopeV1, MikuLinkMessageV1, PermissionDecisionKind,
    MIKU_PROTOCOL_VERSION_V1,
};

use crate::session_queue::{wait_or_cancel_work, PreparedTaskDispatch};
use crate::{
    CooperativeCancelHandle, InternalToolFamily, ManagedTaskRecord, TaskLifecycleState,
    ToolDescriptor, ToolDispatchOutcome, ToolError, ToolExecutionOutput, ToolInvocationRequest,
    ToolRegistry,
};

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

    pub fn tasks(&self) -> &HashMap<String, ManagedTaskRecord> {
        &self.tasks
    }

    pub fn enqueue_task(
        &mut self,
        request: ToolInvocationRequest,
    ) -> Result<CooperativeCancelHandle, ToolError> {
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
    /// If pending, it is immediately purged from the active session queue.
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
        let session_id = record.request.session_id.clone();
        if let Some(q) = self.session_queues.get_mut(&session_id) {
            q.retain(|id| id != task_id);
        }
        Ok(record.state)
    }

    /// Cooperatively cancels all pending, suspended, and executing tasks in `session_id`.
    pub fn cancel_session_queue(&mut self, session_id: &str) -> usize {
        let mut cancelled_count = 0usize;
        if let Some(queue) = self.session_queues.remove(session_id) {
            for task_id in queue {
                if let Some(record) = self.tasks.get_mut(&task_id) {
                    record.cancel_handle.cancel();
                    if !matches!(
                        record.state,
                        TaskLifecycleState::Completed | TaskLifecycleState::Denied
                    ) {
                        record.state = TaskLifecycleState::Cancelled;
                        record.pending_ticket = None;
                        cancelled_count += 1;
                    }
                }
            }
        }
        for record in self.tasks.values_mut() {
            if record.request.session_id == session_id
                && matches!(
                    record.state,
                    TaskLifecycleState::Executing | TaskLifecycleState::SuspendedAsk
                )
            {
                record.cancel_handle.cancel();
                record.state = TaskLifecycleState::Cancelled;
                record.pending_ticket = None;
                cancelled_count += 1;
            }
        }
        cancelled_count
    }

    /// Returns the active pending queue length for a given session.
    pub fn session_queue_len(&self, session_id: &str) -> usize {
        self.session_queues.get(session_id).map(|q| q.len()).unwrap_or(0)
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

    pub fn pop_and_prepare_next_in_session(
        &mut self,
        session_id: &str,
    ) -> Option<PreparedTaskDispatch> {
        let task_id = self.session_queues.get_mut(session_id)?.pop_front()?;
        Some(self.prepare_task_execution(&task_id))
    }

    pub fn prepare_task_execution(&mut self, task_id: &str) -> PreparedTaskDispatch {
        let (request, cancel_handle, initial_state) = {
            let Some(rec) = self.tasks.get(task_id) else {
                return PreparedTaskDispatch::ImmediateOutcome(Err(ToolError::TaskNotFound(
                    task_id.to_string(),
                )));
            };
            (rec.request.clone(), rec.cancel_handle.clone(), rec.state)
        };

        if initial_state == TaskLifecycleState::Cancelled
            || cancel_handle.is_cancelled()
            || self.gatekeeper.is_kill_switch_active()
        {
            if let Some(rec) = self.tasks.get_mut(task_id) {
                rec.state = TaskLifecycleState::Cancelled;
            }
            return PreparedTaskDispatch::ImmediateOutcome(Ok(ToolDispatchOutcome::Cancelled {
                task_id: task_id.to_string(),
                reason: "cancelled_before_execution".to_string(),
            }));
        }

        if !constant_time_token_eq(&request.session_token, &self.expected_session_token) {
            if let Some(rec) = self.tasks.get_mut(task_id) {
                rec.state = TaskLifecycleState::Denied;
            }
            return PreparedTaskDispatch::ImmediateOutcome(Ok(ToolDispatchOutcome::Unauthorized {
                task_id: task_id.to_string(),
            }));
        }

        let descriptor = match self.registry.validate_invocation(
            &request.tool_name,
            &request.target,
            &request.args,
        ) {
            Ok(d) => d.clone(),
            Err(e) => return PreparedTaskDispatch::ImmediateOutcome(Err(e)),
        };

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
                PreparedTaskDispatch::ImmediateOutcome(Ok(ToolDispatchOutcome::Denied {
                    task_id: task_id.to_string(),
                    reason,
                    detail,
                    audit_seq,
                    audit_hash,
                }))
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
                PreparedTaskDispatch::ImmediateOutcome(Ok(ToolDispatchOutcome::SuspendedAsk {
                    task_id: task_id.to_string(),
                    ticket,
                    audit_seq,
                    audit_hash,
                }))
            }
            GatekeeperVerdict::Allow {
                canonical_target,
                audit_seq,
                audit_hash,
            } => {
                if let Some(rec) = self.tasks.get_mut(task_id) {
                    rec.state = TaskLifecycleState::Executing;
                    rec.pending_ticket = None;
                }
                PreparedTaskDispatch::ReadyToExecute {
                    task_id: task_id.to_string(),
                    descriptor,
                    canonical_target,
                    args: request.args,
                    simulated_work_ms: request.simulated_work_ms,
                    audit_seq,
                    audit_hash,
                    cancel_handle,
                }
            }
        }
    }

    pub fn finalize_task_execution(
        &mut self,
        task_id: &str,
        descriptor: &ToolDescriptor,
        canonical_target: &str,
        args: &str,
        audit_seq: u64,
        audit_hash: String,
        cancelled: bool,
    ) -> ToolDispatchOutcome {
        if cancelled || self.gatekeeper.is_kill_switch_active() {
            if let Some(rec) = self.tasks.get_mut(task_id) {
                rec.state = TaskLifecycleState::Cancelled;
            }
            return ToolDispatchOutcome::Cancelled {
                task_id: task_id.to_string(),
                reason: "cancelled_cooperatively_in_flight".to_string(),
            };
        }

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

        ToolDispatchOutcome::Completed(output)
    }

    async fn execute_registered_task(
        &mut self,
        task_id: &str,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let prepared = self.prepare_task_execution(task_id);
        match prepared {
            PreparedTaskDispatch::ImmediateOutcome(res) => res,
            PreparedTaskDispatch::ReadyToExecute {
                task_id,
                descriptor,
                canonical_target,
                args,
                simulated_work_ms,
                audit_seq,
                audit_hash,
                cancel_handle,
            } => {
                let cancelled = cancel_handle.is_cancelled()
                    || self.gatekeeper.is_kill_switch_active()
                    || wait_or_cancel_work(&cancel_handle, simulated_work_ms).await;
                Ok(self.finalize_task_execution(
                    &task_id,
                    &descriptor,
                    &canonical_target,
                    &args,
                    audit_seq,
                    audit_hash,
                    cancelled,
                ))
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
                if let Some(rec) = self.tasks.get_mut(task_id) {
                    rec.state = TaskLifecycleState::Executing;
                    rec.pending_ticket = None;
                }
                let cancelled = cancel_handle.is_cancelled()
                    || self.gatekeeper.is_kill_switch_active()
                    || wait_or_cancel_work(&cancel_handle, request.simulated_work_ms).await;
                Ok(self.finalize_task_execution(
                    task_id,
                    &descriptor,
                    &canonical_target,
                    execution_args,
                    audit_seq,
                    audit_hash,
                    cancelled,
                ))
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
