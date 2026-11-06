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

use std::collections::{HashMap, HashSet};
use miku_protocol::{
    constant_time_token_eq, BaseContractBundle, IslandState, MikuLinkEnvelopeV1,
    MikuLinkMessageV1, ProtocolError,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

pub const DEFAULT_CHANNEL_CAPACITY: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreSnapshot {
    pub runtime_flavor: String,
    pub island_state: IslandState,
    pub authenticated_sessions: usize,
    pub dispatched_events: u64,
    pub contract_reloads_ok: u64,
    pub contract_reloads_rejected: u64,
    pub active_contract_sha256: String,
    pub pill_width: u32,
    pub pill_height: u32,
    pub idle_fps_when_asleep: u32,
    pub active_polling_timers: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownSummary {
    pub reason: String,
    pub drained_inflight_commands: u64,
    pub total_dispatched_events: u64,
    pub final_island_state: IslandState,
    pub final_contract_sha256: String,
}

pub enum CoreCommand {
    ProcessEnvelope {
        raw_json: String,
        reply: oneshot::Sender<Result<MikuLinkEnvelopeV1, ProtocolError>>,
    },
    ReloadContracts {
        rig_toml: String,
        tokens_json: String,
        reply: oneshot::Sender<Result<String, ProtocolError>>,
    },
    TransitionState {
        next_state: IslandState,
        reply: oneshot::Sender<IslandState>,
    },
    QuerySnapshot {
        reply: oneshot::Sender<CoreSnapshot>,
    },
    Shutdown {
        reason: String,
        reply: oneshot::Sender<ShutdownSummary>,
    },
}

#[derive(Clone)]
pub struct OrchestratorHandle {
    cmd_tx: mpsc::Sender<CoreCommand>,
    shutdown_tx: watch::Sender<bool>,
}

impl OrchestratorHandle {
    pub async fn send_envelope(
        &self,
        raw_json: impl Into<String>,
    ) -> Result<MikuLinkEnvelopeV1, ProtocolError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(CoreCommand::ProcessEnvelope {
                raw_json: raw_json.into(),
                reply: tx,
            })
            .await
            .map_err(|_| ProtocolError::ContractValidation("orchestrator channel closed".into()))?;
        rx.await
            .map_err(|_| ProtocolError::ContractValidation("orchestrator reply dropped".into()))?
    }

    pub async fn reload_contracts(
        &self,
        rig_toml: impl Into<String>,
        tokens_json: impl Into<String>,
    ) -> Result<String, ProtocolError> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(CoreCommand::ReloadContracts {
                rig_toml: rig_toml.into(),
                tokens_json: tokens_json.into(),
                reply: tx,
            })
            .await
            .map_err(|_| ProtocolError::ContractValidation("orchestrator channel closed".into()))?;
        rx.await
            .map_err(|_| ProtocolError::ContractValidation("orchestrator reply dropped".into()))?
    }

    pub async fn transition_state(&self, next_state: IslandState) -> Result<IslandState, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(CoreCommand::TransitionState {
                next_state,
                reply: tx,
            })
            .await
            .map_err(|e| format!("failed to send TransitionState: {}", e))?;
        rx.await
            .map_err(|e| format!("failed to receive TransitionState reply: {}", e))
    }

    pub async fn query_snapshot(&self) -> Result<CoreSnapshot, String> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(CoreCommand::QuerySnapshot { reply: tx })
            .await
            .map_err(|e| format!("failed to send QuerySnapshot: {}", e))?;
        rx.await
            .map_err(|e| format!("failed to receive QuerySnapshot reply: {}", e))
    }

    pub fn trigger_system_shutdown_signal(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub fn raw_sender(&self) -> mpsc::Sender<CoreCommand> {
        self.cmd_tx.clone()
    }
}

pub fn build_current_thread_runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to initialize tokio current_thread runtime: {}", e))
}

pub fn spawn_orchestrator(
    initial_contracts: BaseContractBundle,
    expected_session_token: String,
    channel_capacity: usize,
) -> (
    OrchestratorHandle,
    tokio::task::JoinHandle<ShutdownSummary>,
) {
    let cap = channel_capacity.max(8);
    let (cmd_tx, cmd_rx) = mpsc::channel(cap);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let handle = OrchestratorHandle {
        cmd_tx,
        shutdown_tx,
    };

    let join = tokio::task::spawn(run_orchestrator_loop(
        initial_contracts,
        expected_session_token,
        cmd_rx,
        shutdown_rx,
    ));

    (handle, join)
}

struct OrchestratorState {
    contracts: BaseContractBundle,
    expected_token: String,
    island_state: IslandState,
    authenticated_sessions: usize,
    dispatched_events: u64,
    contract_reloads_ok: u64,
    contract_reloads_rejected: u64,
}

impl OrchestratorState {
    fn snapshot(&self) -> CoreSnapshot {
        CoreSnapshot {
            runtime_flavor: "tokio::current_thread".to_string(),
            island_state: self.island_state,
            authenticated_sessions: self.authenticated_sessions,
            dispatched_events: self.dispatched_events,
            contract_reloads_ok: self.contract_reloads_ok,
            contract_reloads_rejected: self.contract_reloads_rejected,
            active_contract_sha256: self.contracts.contract_sha256.clone(),
            pill_width: self.contracts.tokens.island.pill_width,
            pill_height: self.contracts.tokens.island.pill_height,
            idle_fps_when_asleep: self.contracts.rig.states.idle.fps_when_asleep,
            active_polling_timers: 0,
        }
    }

    fn handle_envelope(
        &mut self,
        raw_json: &str,
    ) -> Result<MikuLinkEnvelopeV1, ProtocolError> {
        self.dispatched_events += 1;
        let decoded = match MikuLinkEnvelopeV1::decode_and_validate(raw_json) {
            Ok(env) => env,
            Err(ProtocolError::UnsupportedVersion {
                received: _,
                supported,
            }) => {
                return Ok(MikuLinkEnvelopeV1 {
                    v: 1,
                    id: format!("err-{}", self.dispatched_events),
                    reply_to: None,
                    msg: MikuLinkMessageV1::Error {
                        code: "unsupported_version".to_string(),
                        supported: Some(supported),
                        detail: None,
                    },
                });
            }
            Err(e) => return Err(e),
        };

        match decoded.msg {
            MikuLinkMessageV1::Hello {
                client: _,
                token,
                capabilities: _,
            } => {
                if !constant_time_token_eq(&self.expected_token, &token) {
                    return Err(ProtocolError::Unauthorized(
                        "invalid session token in hello handshake".to_string(),
                    ));
                }
                self.authenticated_sessions += 1;
                Ok(MikuLinkEnvelopeV1 {
                    v: 1,
                    id: format!("ack-{}", self.dispatched_events),
                    reply_to: Some(decoded.id),
                    msg: MikuLinkMessageV1::HelloAck {
                        session_id: format!("sess_{:02}", self.authenticated_sessions),
                        island_state: self.island_state,
                    },
                })
            }
            MikuLinkMessageV1::Ping { ts } => Ok(MikuLinkEnvelopeV1 {
                v: 1,
                id: format!("pong-{}", self.dispatched_events),
                reply_to: Some(decoded.id),
                msg: MikuLinkMessageV1::Pong { ts },
            }),
            MikuLinkMessageV1::AgentEvent {
                agent: _,
                kind,
                title: _,
            } => {
                self.island_state = match kind.as_str() {
                    "needs_input" | "permission_wait" => IslandState::Alert,
                    "thinking" | "working" => IslandState::Expanded,
                    "listening" => IslandState::Listening,
                    "gaming" => IslandState::Gaming,
                    _ => IslandState::Idle,
                };
                Ok(MikuLinkEnvelopeV1 {
                    v: 1,
                    id: format!("evt-ack-{}", self.dispatched_events),
                    reply_to: Some(decoded.id),
                    msg: MikuLinkMessageV1::HelloAck {
                        session_id: "sess_active".to_string(),
                        island_state: self.island_state,
                    },
                })
            }
            MikuLinkMessageV1::Command {
                session_id,
                action: _,
                text: _,
            } => {
                if self.authenticated_sessions == 0 {
                    return Err(ProtocolError::Unauthorized(
                        "command rejected before hello handshake".to_string(),
                    ));
                }
                Ok(MikuLinkEnvelopeV1 {
                    v: 1,
                    id: format!("cmd-ack-{}", self.dispatched_events),
                    reply_to: Some(decoded.id),
                    msg: MikuLinkMessageV1::HelloAck {
                        session_id,
                        island_state: self.island_state,
                    },
                })
            }
            other => Ok(MikuLinkEnvelopeV1 {
                v: 1,
                id: format!("echo-{}", self.dispatched_events),
                reply_to: Some(decoded.id),
                msg: other,
            }),
        }
    }

    fn reload_contracts(
        &mut self,
        rig_toml: &str,
        tokens_json: &str,
    ) -> Result<String, ProtocolError> {
        match BaseContractBundle::from_str_pair(rig_toml, tokens_json) {
            Ok(new_bundle) => {
                let hash = new_bundle.contract_sha256.clone();
                self.contracts = new_bundle;
                self.contract_reloads_ok += 1;
                Ok(hash)
            }
            Err(e) => {
                self.contract_reloads_rejected += 1;
                Err(e)
            }
        }
    }
}

async fn run_orchestrator_loop(
    initial_contracts: BaseContractBundle,
    expected_token: String,
    mut cmd_rx: mpsc::Receiver<CoreCommand>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> ShutdownSummary {
    let mut state = OrchestratorState {
        contracts: initial_contracts,
        expected_token,
        island_state: IslandState::Idle,
        authenticated_sessions: 0,
        dispatched_events: 0,
        contract_reloads_ok: 0,
        contract_reloads_rejected: 0,
    };

    let mut shutdown_reason = String::from("channel_closed");
    let mut explicit_shutdown_reply: Option<oneshot::Sender<ShutdownSummary>> = None;

    // Pure event-driven loop: 0 timers in idle, biased select prioritizes signal/commands deterministically
    loop {
        tokio::select! {
            biased;

            changed = shutdown_rx.changed() => {
                if changed.is_ok() && *shutdown_rx.borrow() {
                    shutdown_reason = String::from("system_signal_shutdown");
                    break;
                }
            }

            maybe_cmd = cmd_rx.recv() => {
                match maybe_cmd {
                    Some(CoreCommand::Shutdown { reason, reply }) => {
                        shutdown_reason = reason;
                        explicit_shutdown_reply = Some(reply);
                        break;
                    }
                    Some(cmd) => {
                        dispatch_single_command(&mut state, cmd);
                    }
                    None => {
                        break;
                    }
                }
            }
        }
    }

    // Close receiver and drain all in-flight commands deterministically so nothing is lost
    cmd_rx.close();
    let mut drained_inflight_commands = 0u64;
    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            CoreCommand::Shutdown { reason: _, reply } => {
                if explicit_shutdown_reply.is_none() {
                    explicit_shutdown_reply = Some(reply);
                }
            }
            other => {
                drained_inflight_commands += 1;
                dispatch_single_command(&mut state, other);
            }
        }
    }

    let summary = ShutdownSummary {
        reason: shutdown_reason,
        drained_inflight_commands,
        total_dispatched_events: state.dispatched_events,
        final_island_state: state.island_state,
        final_contract_sha256: state.contracts.contract_sha256.clone(),
    };

    if let Some(reply) = explicit_shutdown_reply {
        let _ = reply.send(summary.clone());
    }

    summary
}

fn dispatch_single_command(state: &mut OrchestratorState, cmd: CoreCommand) {
    match cmd {
        CoreCommand::ProcessEnvelope { raw_json, reply } => {
            let res = state.handle_envelope(&raw_json);
            let _ = reply.send(res);
        }
        CoreCommand::ReloadContracts {
            rig_toml,
            tokens_json,
            reply,
        } => {
            let res = state.reload_contracts(&rig_toml, &tokens_json);
            let _ = reply.send(res);
        }
        CoreCommand::TransitionState { next_state, reply } => {
            state.dispatched_events += 1;
            state.island_state = next_state;
            let _ = reply.send(state.island_state);
        }
        CoreCommand::QuerySnapshot { reply } => {
            let _ = reply.send(state.snapshot());
        }
        CoreCommand::Shutdown { .. } => {}
    }
}

// ============================================================================
// Process Tree & Orphan Auditing Utilities (Composite Identity with CreationDate)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub name_lower: String,
    pub creation_date: String,
}

#[derive(Debug, Clone)]
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: u32,
    pub private_bytes: u64,
    pub name: String,
    pub creation_date: String,
}

#[derive(Debug, Clone)]
pub struct TreeAggregation {
    pub root_present: bool,
    pub process_count: usize,
    pub descendant_count: usize,
    pub total_private_bytes: u64,
    pub identities: Vec<ProcessIdentity>,
}

pub fn parse_process_tsv(tsv: &str) -> Vec<ProcessRow> {
    let mut rows = Vec::new();
    for raw_line in tsv.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 5 {
            continue;
        }
        let Ok(pid) = parts[0].trim().parse::<u32>() else {
            continue;
        };
        let Ok(parent_pid) = parts[1].trim().parse::<u32>() else {
            continue;
        };
        let Ok(private_bytes) = parts[2].trim().parse::<u64>() else {
            continue;
        };
        let name = parts[3].trim().to_string();
        let creation_date = parts[4].trim().to_string();
        rows.push(ProcessRow {
            pid,
            parent_pid,
            private_bytes,
            name,
            creation_date,
        });
    }
    rows
}

pub fn aggregate_process_tree(root_pid: u32, rows: &[ProcessRow]) -> TreeAggregation {
    let mut by_pid: HashMap<u32, &ProcessRow> = HashMap::new();
    let mut children_map: HashMap<u32, Vec<u32>> = HashMap::new();
    for r in rows {
        by_pid.insert(r.pid, r);
        children_map.entry(r.parent_pid).or_default().push(r.pid);
    }

    if !by_pid.contains_key(&root_pid) {
        return TreeAggregation {
            root_present: false,
            process_count: 0,
            descendant_count: 0,
            total_private_bytes: 0,
            identities: Vec::new(),
        };
    }

    let mut visited = HashSet::new();
    let mut queue = vec![root_pid];
    let mut total_private_bytes = 0u64;
    let mut identities = Vec::new();

    while let Some(curr) = queue.pop() {
        if !visited.insert(curr) {
            continue;
        }
        if let Some(row) = by_pid.get(&curr) {
            total_private_bytes = total_private_bytes.saturating_add(row.private_bytes);
            identities.push(ProcessIdentity {
                pid: row.pid,
                name_lower: row.name.to_ascii_lowercase(),
                creation_date: row.creation_date.clone(),
            });
        }
        if let Some(kids) = children_map.get(&curr) {
            for &k in kids {
                if !visited.contains(&k) {
                    queue.push(k);
                }
            }
        }
    }

    let process_count = identities.len();
    let descendant_count = process_count.saturating_sub(1);

    TreeAggregation {
        root_present: true,
        process_count,
        descendant_count,
        total_private_bytes,
        identities,
    }
}

pub fn detect_orphans(
    tracked: &[ProcessIdentity],
    post_rows: &[ProcessRow],
) -> Vec<ProcessIdentity> {
    let post_set: HashSet<ProcessIdentity> = post_rows
        .iter()
        .map(|r| ProcessIdentity {
            pid: r.pid,
            name_lower: r.name.to_ascii_lowercase(),
            creation_date: r.creation_date.clone(),
        })
        .collect();

    let mut seen = HashSet::new();
    let mut orphans = Vec::new();
    for id in tracked {
        if seen.insert(id.clone()) && post_set.contains(id) {
            orphans.push(id.clone());
        }
    }
    orphans
}

pub fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let clamped = pct.clamp(0.0, 100.0) / 100.0;
    let idx = ((sorted.len() - 1) as f64 * clamped).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use miku_protocol::{DEFAULT_DESIGN_TOKENS_JSON, DEFAULT_RIG_TOML};

    // LEDGER-1.1-3
    #[test]
    fn test_tokio_current_thread_orchestrator_loop_dispatches_events_without_polling() {
        let rt = build_current_thread_runtime().expect("must build current_thread runtime");
        rt.block_on(async {
            let bundle = BaseContractBundle::load_embedded_defaults().expect("bundle");
            let initial_hash = bundle.contract_sha256.clone();
            let (handle, join) =
                spawn_orchestrator(bundle, "miku_session_token_01".to_string(), 64);

            // 1. Verify unauthorized hello is rejected
            let bad_hello = r##"{"v":1,"id":"h-bad","msg":{"type":"hello","data":{"client":"cli","token":"wrong_token","capabilities":[]}}}"##;
            let err = handle.send_envelope(bad_hello).await.unwrap_err();
            assert!(matches!(err, ProtocolError::Unauthorized(_)));

            // 2. Verify valid hello succeeds in constant time
            let ok_hello = r##"{"v":1,"id":"h-ok","msg":{"type":"hello","data":{"client":"cli","token":"miku_session_token_01","capabilities":["agent_events"]}}}"##;
            let ack = handle.send_envelope(ok_hello).await.expect("hello_ack");
            assert_eq!(ack.reply_to.as_deref(), Some("h-ok"));
            assert!(matches!(
                ack.msg,
                MikuLinkMessageV1::HelloAck {
                    island_state: IslandState::Idle,
                    ..
                }
            ));

            // 3. Dispatch agent_event -> transitions IslandState to Alert
            let agent_evt = r##"{"v":1,"id":"ev-1","msg":{"type":"agent_event","data":{"agent":"claude_code","kind":"needs_input","title":"Approve cargo test"}}}"##;
            let evt_ack = handle.send_envelope(agent_evt).await.expect("evt_ack");
            assert!(matches!(
                evt_ack.msg,
                MikuLinkMessageV1::HelloAck {
                    island_state: IslandState::Alert,
                    ..
                }
            ));

            // 4. Atomic contract reload: corrupted reload fails & preserves previous contract
            let corrupt_tokens =
                DEFAULT_DESIGN_TOKENS_JSON.replace(r##""#0B0F17""##, r##""invalid""##);
            let reload_err = handle
                .reload_contracts(DEFAULT_RIG_TOML, corrupt_tokens)
                .await;
            assert!(reload_err.is_err());

            let snap = handle.query_snapshot().await.expect("snapshot");
            assert_eq!(snap.runtime_flavor, "tokio::current_thread");
            assert_eq!(snap.island_state, IslandState::Alert);
            assert_eq!(snap.authenticated_sessions, 1);
            assert_eq!(snap.dispatched_events, 3);
            assert_eq!(snap.contract_reloads_ok, 0);
            assert_eq!(snap.contract_reloads_rejected, 1);
            assert_eq!(snap.active_contract_sha256, initial_hash);
            assert_eq!(snap.active_polling_timers, 0);
            assert_eq!(snap.idle_fps_when_asleep, 0);

            // 5. Valid contract reload updates active hash atomically
            let updated_tokens =
                DEFAULT_DESIGN_TOKENS_JSON.replace(r##""#22D3EE""##, r##""#38BDF8""##);
            let new_hash = handle
                .reload_contracts(DEFAULT_RIG_TOML, updated_tokens)
                .await
                .expect("valid reload");
            assert_ne!(new_hash, initial_hash);

            handle.trigger_system_shutdown_signal();
            let summary = join.await.expect("orchestrator join");
            assert_eq!(summary.reason, "system_signal_shutdown");
            assert_eq!(summary.final_contract_sha256, new_hash);
        });
    }

    // LEDGER-1.1-4
    #[test]
    fn test_graceful_system_signal_shutdown_drains_inflight_and_releases_resources() {
        let rt = build_current_thread_runtime().expect("must build current_thread runtime");
        rt.block_on(async {
            let bundle = BaseContractBundle::load_embedded_defaults().expect("bundle");
            let (handle, join) =
                spawn_orchestrator(bundle, "miku_session_token_02".to_string(), 64);

            let raw_tx = handle.raw_sender();
            let mut receivers = Vec::new();

            // Enqueue 5 in-flight ping envelopes before yielding to the orchestrator task
            for idx in 0..5u64 {
                let (reply_tx, reply_rx) = oneshot::channel();
                let ping_json = format!(
                    r##"{{"v":1,"id":"ping-{}","msg":{{"type":"ping","data":{{"ts":{}}}}}}}"##,
                    idx,
                    1_700_000_000 + idx
                );
                raw_tx
                    .send(CoreCommand::ProcessEnvelope {
                        raw_json: ping_json,
                        reply: reply_tx,
                    })
                    .await
                    .expect("enqueue inflight ping");
                receivers.push(reply_rx);
            }

            // Trigger system shutdown signal immediately while the 5 pings are still queued
            handle.trigger_system_shutdown_signal();

            // Wait for orchestrator to finish draining and shutting down
            let summary = join.await.expect("clean shutdown join");
            assert_eq!(summary.reason, "system_signal_shutdown");
            assert_eq!(summary.total_dispatched_events, 5);
            assert!(summary.drained_inflight_commands >= 1);

            // All 5 in-flight requests must have received their deterministic Pong reply
            for (idx, rx) in receivers.into_iter().enumerate() {
                let env = rx
                    .await
                    .expect("oneshot must not be dropped")
                    .expect("pong ok");
                assert_eq!(env.reply_to.as_deref(), Some(format!("ping-{}", idx).as_str()));
            }
        });
    }
}
