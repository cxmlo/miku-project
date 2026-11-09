use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use miku_protocol::{
    IslandOrderCommand, IslandOrderResponse, SessionOrderRecord, TaskLifecycleState,
};

use crate::{
    AgentSessionManager, CooperativeCancelHandle, ToolDescriptor, ToolDispatchOutcome, ToolError,
    ToolInvocationRequest,
};

/// Budget constant: queue overhead per message must be < 10 microseconds.
pub const QUEUE_OVERHEAD_BUDGET_MICROS: f64 = 10.0;

/// Outcome of preparing a task for execution. Allows ConcurrentSessionQueue
/// to perform execution and cooperative cancellation outside the manager lock.
#[derive(Debug)]
pub enum PreparedTaskDispatch {
    ImmediateOutcome(Result<ToolDispatchOutcome, ToolError>),
    ReadyToExecute {
        task_id: String,
        descriptor: ToolDescriptor,
        canonical_target: String,
        args: String,
        simulated_work_ms: u64,
        audit_seq: u64,
        audit_hash: String,
        cancel_handle: CooperativeCancelHandle,
    },
}

pub(crate) async fn wait_or_cancel_work(
    cancel_handle: &CooperativeCancelHandle,
    simulated_work_ms: u64,
) -> bool {
    if cancel_handle.is_cancelled() {
        return true;
    }
    if simulated_work_ms > 0 {
        let mut cancel_rx = cancel_handle.receiver();
        let sleep_fut = tokio::time::sleep(Duration::from_millis(simulated_work_ms));
        tokio::pin!(sleep_fut);

        tokio::select! {
            biased;
            _ = cancel_rx.changed() => *cancel_rx.borrow(),
            _ = &mut sleep_fut => cancel_handle.is_cancelled(),
        }
    } else {
        false
    }
}

/// Thread-safe concurrent coordinator wrapping `AgentSessionManager`.
/// Enforces strict FIFO serialization per session while allowing independent,
/// non-blocking progress across different sessions.
#[derive(Clone)]
pub struct ConcurrentSessionQueue {
    pub(crate) inner: Arc<Mutex<AgentSessionManager>>,
    pub(crate) session_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl ConcurrentSessionQueue {
    pub fn new(manager: AgentSessionManager) -> Self {
        Self {
            inner: Arc::new(Mutex::new(manager)),
            session_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn inner(&self) -> &Arc<Mutex<AgentSessionManager>> {
        &self.inner
    }

    async fn get_session_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.session_locks.lock().await;
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn enqueue_task(
        &self,
        request: ToolInvocationRequest,
    ) -> Result<CooperativeCancelHandle, ToolError> {
        let mut inner = self.inner.lock().await;
        inner.enqueue_task(request)
    }

    pub async fn dispatch_next_in_session(
        &self,
        session_id: &str,
    ) -> Option<Result<ToolDispatchOutcome, ToolError>> {
        let session_lock = self.get_session_lock(session_id).await;
        let _session_guard = session_lock.lock().await;

        let mut inner = self.inner.lock().await;
        let prepared = inner.pop_and_prepare_next_in_session(session_id)?;

        match prepared {
            PreparedTaskDispatch::ImmediateOutcome(res) => Some(res),
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
                if simulated_work_ms == 0 {
                    let cancelled = cancel_handle.is_cancelled();
                    let outcome = inner.finalize_task_execution(
                        &task_id,
                        &descriptor,
                        &canonical_target,
                        &args,
                        audit_seq,
                        audit_hash,
                        cancelled,
                    );
                    Some(Ok(outcome))
                } else {
                    drop(inner);
                    let cancelled = wait_or_cancel_work(&cancel_handle, simulated_work_ms).await;
                    let mut inner = self.inner.lock().await;
                    let outcome = inner.finalize_task_execution(
                        &task_id,
                        &descriptor,
                        &canonical_target,
                        &args,
                        audit_seq,
                        audit_hash,
                        cancelled,
                    );
                    Some(Ok(outcome))
                }
            }
        }
    }

    pub async fn cancel_task(&self, task_id: &str) -> Result<TaskLifecycleState, ToolError> {
        let mut inner = self.inner.lock().await;
        inner.cancel_task(task_id)
    }

    pub async fn cancel_session_queue(&self, session_id: &str) -> usize {
        let mut inner = self.inner.lock().await;
        inner.cancel_session_queue(session_id)
    }

    pub async fn trigger_global_kill_switch(&self, now_ms: u64) -> usize {
        let mut inner = self.inner.lock().await;
        inner.trigger_global_kill_switch(now_ms)
    }

    pub async fn handle_island_order_command(
        &self,
        cmd: IslandOrderCommand,
    ) -> IslandOrderResponse {
        match cmd {
            IslandOrderCommand::ListOrders { session_id } => {
                let orders = match session_id {
                    Some(sid) => self.list_orders_for_session(&sid).await,
                    None => self.list_all_orders().await,
                };
                IslandOrderResponse::OrdersList { orders }
            }
            IslandOrderCommand::CancelOrder { task_id } => match self.cancel_task(&task_id).await {
                Ok(state) => IslandOrderResponse::OrderCancelled { task_id, state },
                Err(err) => IslandOrderResponse::Error {
                    message: err.to_string(),
                },
            },
            IslandOrderCommand::CancelSession { session_id } => {
                let cancelled_count = self.cancel_session_queue(&session_id).await;
                IslandOrderResponse::SessionCancelled {
                    session_id,
                    cancelled_count,
                }
            }
        }
    }

    pub async fn list_orders_for_session(&self, session_id: &str) -> Vec<SessionOrderRecord> {
        let inner = self.inner.lock().await;
        let mut list: Vec<SessionOrderRecord> = inner
            .tasks()
            .values()
            .filter(|t| t.request.session_id == session_id)
            .map(|t| SessionOrderRecord {
                task_id: t.request.task_id.clone(),
                session_id: t.request.session_id.clone(),
                agent: t.request.agent.clone(),
                tool_name: t.request.tool_name.clone(),
                target: t.request.target.clone(),
                state: t.state,
                timestamp_ms: t.request.timestamp_ms,
            })
            .collect();
        list.sort_by(|a, b| a.timestamp_ms.cmp(&b.timestamp_ms).then(a.task_id.cmp(&b.task_id)));
        list
    }

    pub async fn list_all_orders(&self) -> Vec<SessionOrderRecord> {
        let inner = self.inner.lock().await;
        let mut list: Vec<SessionOrderRecord> = inner
            .tasks()
            .values()
            .map(|t| SessionOrderRecord {
                task_id: t.request.task_id.clone(),
                session_id: t.request.session_id.clone(),
                agent: t.request.agent.clone(),
                tool_name: t.request.tool_name.clone(),
                target: t.request.target.clone(),
                state: t.state,
                timestamp_ms: t.request.timestamp_ms,
            })
            .collect();
        list.sort_by(|a, b| a.timestamp_ms.cmp(&b.timestamp_ms).then(a.task_id.cmp(&b.task_id)));
        list
    }

    pub async fn session_queue_len(&self, session_id: &str) -> usize {
        let inner = self.inner.lock().await;
        inner.session_queue_len(session_id)
    }
}
