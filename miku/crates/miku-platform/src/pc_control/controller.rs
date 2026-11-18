use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use miku_gatekeeper::{Capability, GatekeeperEngine, GatekeeperVerdict, PermissionTicket};
use miku_protocol::PermissionDecisionKind;
use serde::{Deserialize, Serialize};

use crate::pc_control::backends::{InputStep, PcControlBackend};
use crate::pc_control::types_and_guards::{
    matches_immutable_shell_denylist, PcActionExecutionResult, PcControlAction,
    MAX_KILL_SWITCH_ABORT_BUDGET_MS,
};
use crate::PlatformError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillSwitchAbortReport {
    pub elapsed_ms: f64,
    pub within_20ms_budget: bool,
    pub revoked_gatekeeper_tickets: usize,
    pub aborted_pending_local_actions: usize,
    pub abort_flag_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PcControlOutcome {
    Executed(PcActionExecutionResult),
    AwaitingApproval {
        ticket: PermissionTicket,
    },
    Denied {
        reason_code: String,
        detail: String,
    },
}

#[derive(Debug, Clone)]
struct PendingPcAction {
    agent: String,
    action: PcControlAction,
    raw_target: String,
    raw_args: String,
    req_hash: String,
}

pub struct PcControlController<B: PcControlBackend> {
    backend: B,
    abort_flag: Arc<AtomicBool>,
    resident_threads: Arc<AtomicUsize>,
    pending_actions: HashMap<String, PendingPcAction>,
    executed_count: u64,
    denied_count: u64,
}

impl<B: PcControlBackend> PcControlController<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            abort_flag: Arc::new(AtomicBool::new(false)),
            resident_threads: Arc::new(AtomicUsize::new(0)),
            pending_actions: HashMap::new(),
            executed_count: 0,
            denied_count: 0,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn active_resident_threads(&self) -> usize {
        self.resident_threads.load(Ordering::SeqCst)
    }

    pub fn abort_flag_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.abort_flag)
    }

    pub fn reset_abort_flag_after_recovery(&self) {
        self.abort_flag.store(false, Ordering::SeqCst);
    }

    pub fn executed_count(&self) -> u64 {
        self.executed_count
    }

    pub fn denied_count(&self) -> u64 {
        self.denied_count
    }

    pub fn execute_action(
        &mut self,
        agent: &str,
        action: PcControlAction,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
    ) -> Result<PcControlOutcome, PlatformError> {
        action.validate()?;
        if self.abort_flag.load(Ordering::SeqCst) {
            self.denied_count += 1;
            return Ok(PcControlOutcome::Denied {
                reason_code: "deny_kill_switch".to_string(),
                detail: "PC control abort flag is active from Kill Switch".to_string(),
            });
        }

        // Enforce immutable global destructive shell denylist BEFORE consulting profile rules.
        if let PcControlAction::ShellCommand { program, args, .. } = &action {
            if let Some(matched) = matches_immutable_shell_denylist(program, args) {
                self.denied_count += 1;
                return Ok(PcControlOutcome::Denied {
                    reason_code: "deny_immutable_shell_guard".to_string(),
                    detail: format!(
                        "command matched immutable global destructive shell pattern '{}'",
                        matched
                    ),
                });
            }
        }

        let (raw_target, raw_args) = action.raw_target_and_args();
        let capability = action.capability();
        let verdict =
            gatekeeper.evaluate_request(agent, capability, &raw_target, &raw_args, now_ms);

        match verdict {
            GatekeeperVerdict::Allow {
                canonical_target, ..
            } => self.run_authorized_action(&action, &canonical_target),
            GatekeeperVerdict::AskPending { ticket, .. } => {
                self.pending_actions.insert(
                    ticket.req_hash.clone(),
                    PendingPcAction {
                        agent: agent.to_string(),
                        action,
                        raw_target,
                        raw_args,
                        req_hash: ticket.req_hash.clone(),
                    },
                );
                Ok(PcControlOutcome::AwaitingApproval { ticket })
            }
            GatekeeperVerdict::Deny { reason, detail, .. } => {
                self.denied_count += 1;
                Ok(PcControlOutcome::Denied {
                    reason_code: reason.as_code().to_string(),
                    detail,
                })
            }
        }
    }

    pub fn resolve_pending_action(
        &mut self,
        pending_req_hash: &str,
        decision: PermissionDecisionKind,
        expected_req_hash: &str,
        from_mobile_session: bool,
        biometric_verified: bool,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
    ) -> Result<PcControlOutcome, PlatformError> {
        let pending = self.pending_actions.remove(pending_req_hash).ok_or_else(|| {
            PlatformError::RegistryBackendError(format!(
                "unknown or already resolved PC control ticket '{}'",
                pending_req_hash
            ))
        })?;

        if pending.req_hash != expected_req_hash {
            self.denied_count += 1;
            return Ok(PcControlOutcome::Denied {
                reason_code: "deny_toctou".to_string(),
                detail: "pending PC control req_hash mismatch".to_string(),
            });
        }

        let verdict = gatekeeper.resolve_permission_decision(
            expected_req_hash,
            decision,
            from_mobile_session,
            biometric_verified,
            &pending.agent,
            pending.action.capability(),
            &pending.raw_target,
            &pending.raw_args,
            now_ms,
        );

        match verdict {
            GatekeeperVerdict::Allow {
                canonical_target, ..
            } => self.run_authorized_action(&pending.action, &canonical_target),
            GatekeeperVerdict::Deny { reason, detail, .. } => {
                self.denied_count += 1;
                Ok(PcControlOutcome::Denied {
                    reason_code: reason.as_code().to_string(),
                    detail,
                })
            }
            GatekeeperVerdict::AskPending { .. } => {
                self.denied_count += 1;
                Err(PlatformError::RegistryBackendError(
                    "unexpected AskPending after resolve_permission_decision".to_string(),
                ))
            }
        }
    }

    pub fn execute_authorized_input_sequence(
        &mut self,
        agent: &str,
        steps: &[InputStep],
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
    ) -> Result<PcControlOutcome, PlatformError> {
        let summary_target = format!("input:sequence:{}steps", steps.len());
        let summary_args = format!("sequence_len={}", steps.len());
        let verdict = gatekeeper.evaluate_request(
            agent,
            Capability::Input,
            &summary_target,
            &summary_args,
            now_ms,
        );
        match verdict {
            GatekeeperVerdict::Allow {
                canonical_target, ..
            } => {
                let mut completed_steps = 0usize;
                for step in steps {
                    if self.abort_flag.load(Ordering::SeqCst) {
                        self.denied_count += 1;
                        return Ok(PcControlOutcome::Denied {
                            reason_code: "deny_kill_switch".to_string(),
                            detail: format!(
                                "input sequence aborted by Kill Switch after {}/{} steps",
                                completed_steps,
                                steps.len()
                            ),
                        });
                    }
                    self.backend.execute_input(step, &self.abort_flag)?;
                    completed_steps += 1;
                }
                self.executed_count += 1;
                Ok(PcControlOutcome::Executed(PcActionExecutionResult {
                    capability: Capability::Input.as_str().to_string(),
                    used_ui_automation_tree: false,
                    canonical_target,
                    stdout_or_value: format!("completed {}/{} input steps", completed_steps, steps.len()),
                    stderr: String::new(),
                    exit_code: Some(0),
                    bytes_written: 0,
                }))
            }
            GatekeeperVerdict::AskPending { ticket, .. } => {
                Ok(PcControlOutcome::AwaitingApproval { ticket })
            }
            GatekeeperVerdict::Deny { reason, detail, .. } => {
                self.denied_count += 1;
                Ok(PcControlOutcome::Denied {
                    reason_code: reason.as_code().to_string(),
                    detail,
                })
            }
        }
    }

    pub fn trip_kill_switch(
        &mut self,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
    ) -> KillSwitchAbortReport {
        let t0 = Instant::now();
        self.abort_flag.store(true, Ordering::SeqCst);
        let revoked_gatekeeper_tickets = gatekeeper.trigger_kill_switch(now_ms);
        let aborted_pending_local_actions = self.pending_actions.len();
        self.pending_actions.clear();
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        KillSwitchAbortReport {
            elapsed_ms,
            within_20ms_budget: elapsed_ms < MAX_KILL_SWITCH_ABORT_BUDGET_MS,
            revoked_gatekeeper_tickets,
            aborted_pending_local_actions,
            abort_flag_active: self.abort_flag.load(Ordering::SeqCst),
        }
    }

    fn run_authorized_action(
        &mut self,
        action: &PcControlAction,
        canonical_target: &str,
    ) -> Result<PcControlOutcome, PlatformError> {
        if self.abort_flag.load(Ordering::SeqCst) {
            self.denied_count += 1;
            return Ok(PcControlOutcome::Denied {
                reason_code: "deny_kill_switch".to_string(),
                detail: "PC control aborted before backend execution".to_string(),
            });
        }
        let capability = action.capability().as_str().to_string();
        let result = match action {
            PcControlAction::FsRead { .. } => {
                let value = self.backend.fs_read(canonical_target)?;
                PcActionExecutionResult {
                    capability,
                    used_ui_automation_tree: false,
                    canonical_target: canonical_target.to_string(),
                    stdout_or_value: value,
                    stderr: String::new(),
                    exit_code: Some(0),
                    bytes_written: 0,
                }
            }
            PcControlAction::FsWrite {
                content,
                create_dirs,
                ..
            } => {
                let bytes = self
                    .backend
                    .fs_write(canonical_target, content, *create_dirs)?;
                PcActionExecutionResult {
                    capability,
                    used_ui_automation_tree: false,
                    canonical_target: canonical_target.to_string(),
                    stdout_or_value: format!("wrote {} bytes", bytes),
                    stderr: String::new(),
                    exit_code: Some(0),
                    bytes_written: bytes,
                }
            }
            PcControlAction::ShellCommand {
                program,
                args,
                cwd,
                timeout_ms,
            } => {
                let out = self.backend.run_shell(
                    program,
                    args,
                    cwd.as_deref(),
                    *timeout_ms,
                    &self.abort_flag,
                )?;
                if out.aborted_by_kill_switch {
                    self.denied_count += 1;
                    return Ok(PcControlOutcome::Denied {
                        reason_code: "deny_kill_switch".to_string(),
                        detail: out.stderr,
                    });
                }
                PcActionExecutionResult {
                    capability,
                    used_ui_automation_tree: false,
                    canonical_target: canonical_target.to_string(),
                    stdout_or_value: out.stdout,
                    stderr: out.stderr,
                    exit_code: out.exit_code,
                    bytes_written: 0,
                }
            }
            PcControlAction::ClipboardRead => {
                let val = self.backend.clipboard_read()?;
                PcActionExecutionResult {
                    capability,
                    used_ui_automation_tree: false,
                    canonical_target: canonical_target.to_string(),
                    stdout_or_value: val,
                    stderr: String::new(),
                    exit_code: Some(0),
                    bytes_written: 0,
                }
            }
            PcControlAction::ClipboardWrite { text } => {
                self.backend.clipboard_write(text)?;
                PcActionExecutionResult {
                    capability,
                    used_ui_automation_tree: false,
                    canonical_target: canonical_target.to_string(),
                    stdout_or_value: "clipboard updated".to_string(),
                    stderr: String::new(),
                    exit_code: Some(0),
                    bytes_written: text.len(),
                }
            }
            PcControlAction::UiAutomationInvoke {
                window_title_pattern,
                automation_id_or_name,
                control_type,
            } => {
                let receipt = self.backend.ui_automation_invoke(
                    window_title_pattern,
                    automation_id_or_name,
                    control_type,
                )?;
                PcActionExecutionResult {
                    capability,
                    used_ui_automation_tree: receipt.used_ui_automation_tree,
                    canonical_target: canonical_target.to_string(),
                    stdout_or_value: receipt.target_summary,
                    stderr: String::new(),
                    exit_code: Some(0),
                    bytes_written: 0,
                }
            }
            PcControlAction::UiAutomationFallbackPixelClick {
                window_title_pattern,
                x,
                y,
                ui_tree_unavailable_reason,
            } => {
                let receipt = self.backend.ui_automation_pixel_fallback(
                    window_title_pattern,
                    *x,
                    *y,
                    ui_tree_unavailable_reason,
                )?;
                PcActionExecutionResult {
                    capability,
                    used_ui_automation_tree: receipt.used_ui_automation_tree,
                    canonical_target: canonical_target.to_string(),
                    stdout_or_value: receipt.target_summary,
                    stderr: String::new(),
                    exit_code: Some(0),
                    bytes_written: 0,
                }
            }
            PcControlAction::InputKeyCombo { keys } => {
                let desc = self
                    .backend
                    .execute_input(&InputStep::KeyCombo(keys.clone()), &self.abort_flag)?;
                PcActionExecutionResult {
                    capability,
                    used_ui_automation_tree: false,
                    canonical_target: canonical_target.to_string(),
                    stdout_or_value: desc,
                    stderr: String::new(),
                    exit_code: Some(0),
                    bytes_written: 0,
                }
            }
            PcControlAction::InputTypeText { text } => {
                let desc = self
                    .backend
                    .execute_input(&InputStep::TypeText(text.clone()), &self.abort_flag)?;
                PcActionExecutionResult {
                    capability,
                    used_ui_automation_tree: false,
                    canonical_target: canonical_target.to_string(),
                    stdout_or_value: desc,
                    stderr: String::new(),
                    exit_code: Some(0),
                    bytes_written: text.len(),
                }
            }
            PcControlAction::InputMouseClick { x, y } => {
                let desc = self
                    .backend
                    .execute_input(&InputStep::MouseClick { x: *x, y: *y }, &self.abort_flag)?;
                PcActionExecutionResult {
                    capability,
                    used_ui_automation_tree: false,
                    canonical_target: canonical_target.to_string(),
                    stdout_or_value: desc,
                    stderr: String::new(),
                    exit_code: Some(0),
                    bytes_written: 0,
                }
            }
        };
        self.executed_count += 1;
        Ok(PcControlOutcome::Executed(result))
    }
}
