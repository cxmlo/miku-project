use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use miku_gatekeeper::GatekeeperEngine;
use tokio::sync::Mutex;
use tokio::time::sleep;

use crate::harness::json_repair::repair_tool_call_json;
use crate::harness::providers::{
    format_provider_request, parse_provider_response, sanitize_text_with_secrets,
    send_provider_http_post, ProviderResponse,
};
use crate::harness::types::{HarnessConfig, HarnessError, ToolCallRequest, TurnStepOutcome};
use crate::secrets::manager::SecretManager;
use crate::session_manager::AgentSessionManager;
use crate::{
    CooperativeCancelHandle, ToolDispatchOutcome, ToolError, ToolInvocationRequest,
    ToolRegistry,
};

/// Production Agent Execution Harness (F-06, F-08, F-09).
/// Enforces bounded iteration limits (max_steps), automatic key rotation on 429/5xx,
/// strict tool JSON repair with fail-closed semantics, and zero secret leakage.
pub struct AgentHarness {
    pub config: HarnessConfig,
    pub secret_manager: Arc<Mutex<SecretManager>>,
    pub session_manager: AgentSessionManager,
    pub session_id: String,
    pub session_token: String,
    pub current_key_index: usize,
    pub current_step: usize,
    pub conversation_history: Vec<(String, String)>,
    pub needs_confirmation: bool,
    pub pending_tool_calls: VecDeque<ToolCallRequest>,
    pub simulated_tool_handler:
        Option<Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>>,
}

impl AgentHarness {
    pub fn new(
        config: HarnessConfig,
        secret_manager: Arc<Mutex<SecretManager>>,
        tool_registry: ToolRegistry,
        gatekeeper: GatekeeperEngine,
        session_id: &str,
        session_token: &str,
    ) -> Self {
        let session_manager =
            AgentSessionManager::new(tool_registry, gatekeeper, session_token.to_string());
        Self {
            config,
            secret_manager,
            session_manager,
            session_id: session_id.to_string(),
            session_token: session_token.to_string(),
            current_key_index: 0,
            current_step: 0,
            conversation_history: Vec::new(),
            needs_confirmation: false,
            pending_tool_calls: VecDeque::new(),
            simulated_tool_handler: None,
        }
    }

    pub fn set_simulated_tool_handler<F>(&mut self, handler: F)
    where
        F: Fn(&str, &str) -> Result<String, String> + Send + Sync + 'static,
    {
        self.simulated_tool_handler = Some(Arc::new(handler));
    }

    pub fn tool_registry(&self) -> &ToolRegistry {
        self.session_manager.registry()
    }

    pub fn gatekeeper(&self) -> &GatekeeperEngine {
        self.session_manager.gatekeeper()
    }

    pub fn gatekeeper_mut(&mut self) -> &mut GatekeeperEngine {
        self.session_manager.gatekeeper_mut()
    }

    /// Sanitizes any error message against active secret keys to guarantee zero secret leakage.
    fn sanitize_error(&self, err: HarnessError, active_secret: &[u8]) -> HarnessError {
        let secrets = [active_secret];
        match err {
            HarnessError::NetworkError(msg) => {
                HarnessError::NetworkError(sanitize_text_with_secrets(&msg, &secrets))
            }
            HarnessError::ToolExecutionFailed { tool_name, detail } => {
                HarnessError::ToolExecutionFailed {
                    tool_name,
                    detail: sanitize_text_with_secrets(&detail, &secrets),
                }
            }
            HarnessError::GatekeeperDenied(msg) => {
                HarnessError::GatekeeperDenied(sanitize_text_with_secrets(&msg, &secrets))
            }
            HarnessError::SecretStoreError(msg) => {
                HarnessError::SecretStoreError(sanitize_text_with_secrets(&msg, &secrets))
            }
            HarnessError::InsecureTransport(msg) => {
                HarnessError::InsecureTransport(sanitize_text_with_secrets(&msg, &secrets))
            }
            HarnessError::TlsCertificateValidationFailed(msg) => {
                HarnessError::TlsCertificateValidationFailed(sanitize_text_with_secrets(
                    &msg, &secrets,
                ))
            }
            other => other,
        }
    }

    /// Dispatches an LLM request with automatic key failover on HTTP 429 or 5xx.
    /// Bounded backoff is applied on retry, and secrets are zeroized immediately.
    async fn call_model_with_key_rotation(
        &mut self,
        payload: &[u8],
        cancel_handle: Option<&CooperativeCancelHandle>,
    ) -> Result<ProviderResponse, HarnessError> {
        let total_keys = self.config.key_aliases.len();
        if total_keys == 0 {
            // Ollama local or unauthenticated provider
            let (status, body) = send_provider_http_post(
                &self.config.endpoint_url,
                self.config.provider,
                None,
                payload,
                5_000,
            )
            .await?;
            if (200..300).contains(&status) {
                return parse_provider_response(self.config.provider, &body);
            } else {
                let body_sanitized =
                    sanitize_text_with_secrets(&String::from_utf8_lossy(&body), &[]);
                return Err(HarnessError::NetworkError(format!(
                    "provider returned HTTP status {}: {}",
                    status, body_sanitized
                )));
            }
        }

        let mut attempted_aliases = Vec::new();
        let mut last_status = 0u16;
        let mut backoff_ms = self.config.initial_backoff_ms;
        let start_key_index = self.current_key_index;

        for attempt in 0..total_keys {
            if let Some(cancel) = cancel_handle {
                if cancel.is_cancelled() {
                    return Err(HarnessError::Cancelled);
                }
            }

            let alias_idx = (start_key_index + attempt) % total_keys;
            let alias = self.config.key_aliases[alias_idx].clone();
            attempted_aliases.push(alias.clone());

            // 1. Fetch authorized secret from SecretManager (Gatekeeper-evaluated)
            let secret_buf = {
                let mut sm = self.secret_manager.lock().await;
                sm.get_secret_authorized(&alias, &self.session_token)
                    .map_err(|e| HarnessError::SecretStoreError(e.to_string()))?
            };

            // 2. Dispatch HTTP request
            let res = send_provider_http_post(
                &self.config.endpoint_url,
                self.config.provider,
                Some(secret_buf.as_slice()),
                payload,
                5_000,
            )
            .await;

            match res {
                Ok((status, body)) => {
                    last_status = status;
                    if (200..300).contains(&status) {
                        // Success! Update current_key_index to the successful one
                        self.current_key_index = alias_idx;
                        return parse_provider_response(self.config.provider, &body).map_err(
                            |e| self.sanitize_error(e, secret_buf.as_slice()),
                        );
                    } else if status == 429 || (500..=599).contains(&status) {
                        // Rate limited (429) or transient server error (5xx): rotate key with bounded backoff
                        if attempt + 1 < total_keys {
                            if backoff_ms > 0 {
                                sleep(Duration::from_millis(backoff_ms)).await;
                                backoff_ms = (backoff_ms * 2).min(self.config.max_backoff_ms);
                            }
                            self.current_key_index = (alias_idx + 1) % total_keys;
                            continue;
                        }
                    } else {
                        // 4xx client errors (e.g. 400 Bad Request, 401 Unauthorized, etc.)
                        let body_sanitized = sanitize_text_with_secrets(
                            &String::from_utf8_lossy(&body),
                            &[secret_buf.as_slice()],
                        );
                        return Err(HarnessError::NetworkError(format!(
                            "provider returned HTTP status {}: {}",
                            status, body_sanitized
                        )));
                    }
                }
                Err(e) => {
                    let sanitized = self.sanitize_error(e, secret_buf.as_slice());
                    if attempt + 1 >= total_keys {
                        return Err(sanitized);
                    }
                    if backoff_ms > 0 {
                        sleep(Duration::from_millis(backoff_ms)).await;
                        backoff_ms = (backoff_ms * 2).min(self.config.max_backoff_ms);
                    }
                    self.current_key_index = (alias_idx + 1) % total_keys;
                }
            }
        }

        Err(HarnessError::AllKeysExhausted {
            provider: self.config.provider.as_str().to_string(),
            attempted_aliases,
            last_status,
        })
    }

    /// Executes an agent turn with bounded step iteration.
    /// Halts and requests user confirmation if `current_step >= max_steps`.
    pub async fn execute_turn(
        &mut self,
        user_prompt: &str,
        cancel_handle: Option<&CooperativeCancelHandle>,
    ) -> Result<TurnStepOutcome, HarnessError> {
        self.conversation_history
            .push(("user".to_string(), user_prompt.to_string()));

        self.run_turn_loop(cancel_handle).await
    }

    /// Resumes execution after the user approves additional steps beyond the original max_steps.
    /// Invariant: Uses `saturating_add` to prevent arithmetic overflow on boundary updates.
    pub async fn confirm_and_resume(
        &mut self,
        additional_steps: usize,
        cancel_handle: Option<&CooperativeCancelHandle>,
    ) -> Result<TurnStepOutcome, HarnessError> {
        self.config.max_steps = self.config.max_steps.saturating_add(additional_steps);
        self.needs_confirmation = false;
        self.run_turn_loop(cancel_handle).await
    }

    async fn run_turn_loop(
        &mut self,
        cancel_handle: Option<&CooperativeCancelHandle>,
    ) -> Result<TurnStepOutcome, HarnessError> {
        loop {
            if let Some(cancel) = cancel_handle {
                if cancel.is_cancelled() {
                    return Err(HarnessError::Cancelled);
                }
            }

            // 1. Process any queued pending tool calls first
            if let Some(tool_call) = self.pending_tool_calls.pop_front() {
                if self.current_step >= self.config.max_steps {
                    self.pending_tool_calls.push_front(tool_call.clone());
                    self.needs_confirmation = true;
                    return Ok(TurnStepOutcome::NeedsConfirmation {
                        step: self.current_step,
                        max_steps: self.config.max_steps,
                        pending_tool: Some(tool_call.tool_name),
                    });
                }

                let tool_result = self.dispatch_tool_call(&tool_call).await?;
                self.current_step += 1;
                self.conversation_history.push((
                    "assistant".to_string(),
                    format!("tool_call:{} -> {}", tool_call.tool_name, tool_result),
                ));
                continue;
            }

            // 2. If no pending tools, check hard max_steps boundary before model query
            if self.current_step >= self.config.max_steps {
                self.needs_confirmation = true;
                return Ok(TurnStepOutcome::NeedsConfirmation {
                    step: self.current_step,
                    max_steps: self.config.max_steps,
                    pending_tool: None,
                });
            }

            let payload = format_provider_request(
                self.config.provider,
                &self.config.model,
                &self.conversation_history,
            );

            let model_response = self
                .call_model_with_key_rotation(&payload, cancel_handle)
                .await?;

            match model_response {
                ProviderResponse::Text(reply) => {
                    self.current_step += 1;
                    self.conversation_history
                        .push(("assistant".to_string(), reply.clone()));
                    return Ok(TurnStepOutcome::Completed {
                        reply,
                        total_steps: self.current_step,
                    });
                }
                ProviderResponse::ToolCalls(tools) => {
                    for tc in tools {
                        self.pending_tool_calls.push_back(tc);
                    }
                }
            }
        }
    }

    /// Dispatches a tool invocation through the real AgentSessionManager.
    /// Preserves Gatekeeper evaluation, token verification, SHA-256 audit log chaining,
    /// and fail-closed error reporting if tool is unregistered or denied.
    pub async fn dispatch_tool_call(
        &mut self,
        tool_call: &ToolCallRequest,
    ) -> Result<String, HarnessError> {
        // 1. Repair and validate JSON arguments fail-closed
        let repaired_val = repair_tool_call_json(&tool_call.raw_args).map_err(|e| {
            HarnessError::IrreparableToolJson {
                tool_name: tool_call.tool_name.clone(),
                detail: e.to_string(),
            }
        })?;

        let canonical_args = repaired_val.to_string();

        // 2. Extract target if present, defaulting to "."
        let target = repaired_val
            .get("target")
            .or_else(|| repaired_val.get("path"))
            .and_then(|t| t.as_str())
            .unwrap_or(".")
            .to_string();

        // 3. Handle simulated tool handler if configured for testing
        if let Some(ref handler) = self.simulated_tool_handler {
            let _ = self
                .session_manager
                .registry()
                .get(&tool_call.tool_name)
                .map_err(|e| HarnessError::ToolExecutionFailed {
                    tool_name: tool_call.tool_name.clone(),
                    detail: e.to_string(),
                })?;
            return handler(&tool_call.tool_name, &canonical_args).map_err(|e| {
                HarnessError::ToolExecutionFailed {
                    tool_name: tool_call.tool_name.clone(),
                    detail: e,
                }
            });
        }

        // 4. Dispatch through real AgentSessionManager
        let task_id = format!("task-{}-step-{}", self.session_id, self.current_step);
        let request = ToolInvocationRequest {
            task_id,
            session_id: self.session_id.clone(),
            session_token: self.session_token.clone(),
            agent: "agent-harness".to_string(),
            tool_name: tool_call.tool_name.clone(),
            target,
            args: canonical_args,
            simulated_work_ms: 0,
            timestamp_ms: 0,
        };

        let dispatch_result = self.session_manager.dispatch_immediate(request).await;
        match dispatch_result {
            Ok(ToolDispatchOutcome::Completed(output)) => {
                match tool_call.tool_name.as_str() {
                    "fs_read" => {
                        let path = &output.canonical_target;
                        match std::fs::read_to_string(path) {
                            Ok(content) => Ok(content),
                            Err(e) => Err(HarnessError::ToolExecutionFailed {
                                tool_name: tool_call.tool_name.clone(),
                                detail: format!("failed to read file '{}': {}", path, e),
                            }),
                        }
                    }
                    _ => Err(HarnessError::ToolExecutionFailed {
                        tool_name: tool_call.tool_name.clone(),
                        detail: format!(
                            "tool executor for '{}' is not implemented",
                            tool_call.tool_name
                        ),
                    }),
                }
            }
            Ok(ToolDispatchOutcome::Denied {
                reason,
                detail,
                audit_seq,
                audit_hash,
                ..
            }) => Err(HarnessError::GatekeeperDenied(format!(
                "{}: {} (audit_seq={}, audit_hash={})",
                reason.as_code(),
                detail,
                audit_seq,
                audit_hash
            ))),
            Ok(ToolDispatchOutcome::SuspendedAsk {
                ticket,
                audit_seq,
                audit_hash,
                ..
            }) => Err(HarnessError::GatekeeperDenied(format!(
                "confirmation_pending: req_hash={} (audit_seq={}, audit_hash={})",
                ticket.req_hash, audit_seq, audit_hash
            ))),
            Ok(ToolDispatchOutcome::Cancelled { .. }) => Err(HarnessError::Cancelled),
            Ok(ToolDispatchOutcome::Unauthorized { .. }) => Err(HarnessError::GatekeeperDenied(
                "unauthorized: session token mismatch".to_string(),
            )),
            Err(ToolError::ToolNotFound(name)) => Err(HarnessError::ToolExecutionFailed {
                tool_name: tool_call.tool_name.clone(),
                detail: format!("tool not registered: {}", name),
            }),
            Err(other_err) => Err(HarnessError::ToolExecutionFailed {
                tool_name: tool_call.tool_name.clone(),
                detail: other_err.to_string(),
            }),
        }
    }
}
