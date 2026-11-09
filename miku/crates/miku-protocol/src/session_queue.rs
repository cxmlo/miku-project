use serde::{Deserialize, Serialize};

/// Lifecycle state of a managed task/order in an agent session.
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

impl TaskLifecycleState {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskLifecycleState::Pending => "pending",
            TaskLifecycleState::SuspendedAsk => "suspended_ask",
            TaskLifecycleState::Executing => "executing",
            TaskLifecycleState::Completed => "completed",
            TaskLifecycleState::Cancelled => "cancelled",
            TaskLifecycleState::Denied => "denied",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskLifecycleState::Completed
                | TaskLifecycleState::Cancelled
                | TaskLifecycleState::Denied
        )
    }
}

/// Minimal, safe and serializable summary of an order for Island inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOrderRecord {
    pub task_id: String,
    pub session_id: String,
    pub agent: String,
    pub tool_name: String,
    pub target: String,
    pub state: TaskLifecycleState,
    pub timestamp_ms: u64,
}

/// Safe, deny-by-default commands dispatched from the Island to query or cancel queues.
/// Note: Under no circumstance can an IslandOrderCommand execute a tool or bypass Gatekeeper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum IslandOrderCommand {
    ListOrders {
        #[serde(default)]
        session_id: Option<String>,
    },
    CancelOrder {
        task_id: String,
    },
    CancelSession {
        session_id: String,
    },
}

/// Deterministic response emitted back to the Island.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IslandOrderResponse {
    OrdersList {
        orders: Vec<SessionOrderRecord>,
    },
    OrderCancelled {
        task_id: String,
        state: TaskLifecycleState,
    },
    SessionCancelled {
        session_id: String,
        cancelled_count: usize,
    },
    Error {
        message: String,
    },
}
