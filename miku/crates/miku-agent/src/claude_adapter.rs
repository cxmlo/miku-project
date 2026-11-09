use miku_gatekeeper::{
    Capability, DenyReason, GatekeeperEngine, GatekeeperVerdict, PermissionTicket,
};
use miku_protocol::{
    ClaudeBashInput, ClaudeHookResult, ClaudePreToolUseParams, JsonRpcId, JsonRpcRequest,
    JsonRpcResponse, MikuLinkEnvelopeV1, PermissionDecisionKind, JSONRPC_GATEKEEPER_DENIED,
    JSONRPC_INVALID_PARAMS, JSONRPC_KILL_SWITCH_ACTIVE, JSONRPC_TIMEOUT_EXPIRED,
};

pub use crate::claude_tracker_and_telemetry::*;

#[derive(Debug, Clone)]
pub struct ClaudeInterceptionOutcome {
    pub rpc_response: JsonRpcResponse<ClaudeHookResult>,
    pub pending_ticket: Option<PermissionTicket>,
    pub island_envelope: MikuLinkEnvelopeV1,
    pub avatar_mood: AvatarMoodState,
}

pub struct ClaudeCodeAdapter {
    pub tracker: ClaudeCliSessionTracker,
    pub gatekeeper: GatekeeperEngine,
    pub avatar_dispatcher: AvatarActivityDispatcher,
}

impl ClaudeCodeAdapter {
    pub fn new(gatekeeper: GatekeeperEngine) -> Self {
        Self {
            tracker: ClaudeCliSessionTracker::new(),
            gatekeeper,
            avatar_dispatcher: AvatarActivityDispatcher::new(),
        }
    }

    pub fn map_claude_tool_to_capability(
        params: &ClaudePreToolUseParams,
    ) -> Result<(Capability, String, String), String> {
        let name_lower = params.tool_name.trim().to_ascii_lowercase();
        match name_lower.as_str() {
            "bash" | "shell" | "shell_exec" => {
                let input: ClaudeBashInput =
                    serde_json::from_value(params.tool_input.clone())
                        .map_err(|e| format!("invalid Bash tool_input: {}", e))?;
                let cmd = input.command.trim().to_string();
                if cmd.is_empty() {
                    return Err("Bash command cannot be empty".to_string());
                }
                let desc = input.description.unwrap_or_default();
                Ok((Capability::Shell, cmd, desc))
            }
            "edit" | "fileedit" | "write" | "filewrite" | "notebookedit" | "fs_write" => {
                let path = params
                    .tool_input
                    .get("file_path")
                    .or_else(|| params.tool_input.get("path"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing file_path in file write tool_input".to_string())?
                    .to_string();
                let content = params
                    .tool_input
                    .get("content")
                    .or_else(|| params.tool_input.get("new_string"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok((Capability::FsWrite, path, content))
            }
            "read" | "fileread" | "glob" | "grep" | "ls" | "fs_read" => {
                let path = params
                    .tool_input
                    .get("file_path")
                    .or_else(|| params.tool_input.get("path"))
                    .or_else(|| params.tool_input.get("pattern"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("src")
                    .to_string();
                Ok((Capability::FsRead, path, String::new()))
            }
            "browser" | "webfetch" | "web_fetch" => {
                let url = params
                    .tool_input
                    .get("url")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing url in browser tool_input".to_string())?
                    .to_string();
                Ok((Capability::Browser, url, String::new()))
            }
            _ => Ok((
                Capability::Shell,
                params.tool_name.clone(),
                params.tool_input.to_string(),
            )),
        }
    }

    pub fn handle_pre_tool_use(
        &mut self,
        request: &JsonRpcRequest<ClaudePreToolUseParams>,
    ) -> ClaudeInterceptionOutcome {
        let params = &request.params;
        let now_ms = params.timestamp_ms;
        let req_id_str = match &request.id {
            Some(JsonRpcId::Number(n)) => n.to_string(),
            Some(JsonRpcId::String(s)) => s.clone(),
            None => "notif".to_string(),
        };

        let (capability, target, args) = match Self::map_claude_tool_to_capability(params) {
            Ok(mapped) => mapped,
            Err(err_msg) => {
                let env = self.avatar_dispatcher.transition_mood(
                    &params.session_id,
                    AvatarMoodState::Alert,
                    &err_msg,
                    now_ms,
                );
                return ClaudeInterceptionOutcome {
                    rpc_response: JsonRpcResponse::err(
                        request.id.clone(),
                        JSONRPC_INVALID_PARAMS,
                        err_msg,
                        None,
                    ),
                    pending_ticket: None,
                    island_envelope: env,
                    avatar_mood: AvatarMoodState::Alert,
                };
            }
        };

        let (verdict, gk_envelope) = self.gatekeeper.evaluate_into_protocol_envelope(
            &req_id_str,
            "claude_code",
            capability,
            &target,
            &args,
            now_ms,
        );

        match verdict {
            GatekeeperVerdict::Allow {
                canonical_target,
                audit_seq,
                audit_hash,
            } => {
                let _ = self.avatar_dispatcher.transition_mood(
                    &params.session_id,
                    AvatarMoodState::Working,
                    format!("{}:{}", params.tool_name, canonical_target),
                    now_ms,
                );
                ClaudeInterceptionOutcome {
                    rpc_response: JsonRpcResponse::ok(
                        request.id.clone(),
                        ClaudeHookResult::allow(audit_seq, audit_hash),
                    ),
                    pending_ticket: None,
                    island_envelope: gk_envelope,
                    avatar_mood: AvatarMoodState::Working,
                }
            }
            GatekeeperVerdict::AskPending {
                ticket,
                audit_seq,
                audit_hash,
            } => {
                let _ = self.avatar_dispatcher.transition_mood(
                    &params.session_id,
                    AvatarMoodState::Alert,
                    format!("AskPending:{}:{}", params.tool_name, ticket.canonical_target),
                    now_ms,
                );
                ClaudeInterceptionOutcome {
                    rpc_response: JsonRpcResponse::err(
                        request.id.clone(),
                        JSONRPC_TIMEOUT_EXPIRED,
                        "awaiting user Y/N decision on Miku Island",
                        Some(serde_json::json!({
                            "req_hash": ticket.req_hash,
                            "audit_seq": audit_seq,
                            "audit_hash": audit_hash,
                        })),
                    ),
                    pending_ticket: Some(ticket),
                    island_envelope: gk_envelope,
                    avatar_mood: AvatarMoodState::Alert,
                }
            }
            GatekeeperVerdict::Deny {
                reason,
                detail,
                audit_seq,
                audit_hash,
            } => {
                let _ = self.avatar_dispatcher.transition_mood(
                    &params.session_id,
                    AvatarMoodState::Alert,
                    format!("Denied:{}:{}", reason.as_code(), target),
                    now_ms,
                );
                let rpc_code = if reason == DenyReason::KillSwitchActive {
                    JSONRPC_KILL_SWITCH_ACTIVE
                } else {
                    JSONRPC_GATEKEEPER_DENIED
                };
                ClaudeInterceptionOutcome {
                    rpc_response: JsonRpcResponse::err(
                        request.id.clone(),
                        rpc_code,
                        format!("{}: {}", reason.as_code(), detail),
                        Some(serde_json::json!({
                            "reason": reason.as_code(),
                            "audit_seq": audit_seq,
                            "audit_hash": audit_hash,
                        })),
                    ),
                    pending_ticket: None,
                    island_envelope: gk_envelope,
                    avatar_mood: AvatarMoodState::Alert,
                }
            }
        }
    }

    pub fn resolve_pending_ticket(
        &mut self,
        id: Option<JsonRpcId>,
        session_id: &str,
        req_hash: &str,
        decision: PermissionDecisionKind,
        from_mobile: bool,
        biometric_verified: bool,
        capability: Capability,
        target: &str,
        args: &str,
        now_ms: u64,
    ) -> ClaudeInterceptionOutcome {
        let verdict = self.gatekeeper.resolve_permission_decision(
            req_hash,
            decision,
            from_mobile,
            biometric_verified,
            "claude_code",
            capability,
            target,
            args,
            now_ms,
        );

        match verdict {
            GatekeeperVerdict::Allow {
                canonical_target,
                audit_seq,
                audit_hash,
            } => {
                let env = self.avatar_dispatcher.transition_mood(
                    session_id,
                    AvatarMoodState::Working,
                    format!("Approved:{}", canonical_target),
                    now_ms,
                );
                ClaudeInterceptionOutcome {
                    rpc_response: JsonRpcResponse::ok(
                        id,
                        ClaudeHookResult::allow(audit_seq, audit_hash),
                    ),
                    pending_ticket: None,
                    island_envelope: env,
                    avatar_mood: AvatarMoodState::Working,
                }
            }
            GatekeeperVerdict::Deny {
                reason,
                detail,
                audit_seq,
                audit_hash,
            } => {
                let env = self.avatar_dispatcher.transition_mood(
                    session_id,
                    AvatarMoodState::Alert,
                    format!("Rejected:{}:{}", reason.as_code(), detail),
                    now_ms,
                );
                ClaudeInterceptionOutcome {
                    rpc_response: JsonRpcResponse::err(
                        id,
                        JSONRPC_GATEKEEPER_DENIED,
                        format!("{}: {}", reason.as_code(), detail),
                        Some(serde_json::json!({
                            "audit_seq": audit_seq,
                            "audit_hash": audit_hash,
                        })),
                    ),
                    pending_ticket: None,
                    island_envelope: env,
                    avatar_mood: AvatarMoodState::Alert,
                }
            }
            GatekeeperVerdict::AskPending { ticket, .. } => {
                let env = self.avatar_dispatcher.transition_mood(
                    session_id,
                    AvatarMoodState::Alert,
                    "Still pending",
                    now_ms,
                );
                ClaudeInterceptionOutcome {
                    rpc_response: JsonRpcResponse::err(
                        id,
                        JSONRPC_TIMEOUT_EXPIRED,
                        "still awaiting decision",
                        None,
                    ),
                    pending_ticket: Some(ticket),
                    island_envelope: env,
                    avatar_mood: AvatarMoodState::Alert,
                }
            }
        }
    }
}
