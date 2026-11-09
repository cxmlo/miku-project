use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use miku_gatekeeper::GatekeeperEngine;
use miku_memory::StaticSystemPrefix;
use tokio::sync::Mutex;
use tokio::time::sleep;

use crate::harness::json_repair::repair_tool_call_json;
use crate::harness::providers::{
     parse_provider_response, sanitize_text_with_secrets, send_provider_http_post, ProviderResponse,
};
use crate::harness::types::{HarnessConfig, HarnessError, ToolCallRequest, TurnStepOutcome};
use crate::secrets::manager::SecretManager;
use crate::session_manager::AgentSessionManager;
use crate::token_saver::{
    build_static_system_prefix_from_registry, extract_or_estimate_token_usage,
    format_provider_request_with_stable_prefix, BudgetWarningAlert, DailySpendTracker,
    FallbackAlternative, LocalIntentRouter, LocalSystemSnapshot, PreFlightBudgetDecision,
    RouteDecision,
};
use crate::{
    CooperativeCancelHandle, ToolDispatchOutcome, ToolError, ToolInvocationRequest,
    ToolRegistry,
};

/// Production Agent Execution Harness (F-06, F-08, F-09).
/// Enforces bounded iteration limits (max_steps), automatic key rotation on 429/5xx,
/// strict tool JSON repair with fail-closed semantics, zero secret leakage,
/// 0-token local intent routing, byte-stable KV-cache prefix, and per-project daily budget caps.
pub struct AgentHarness {
    pub config: HarnessConfig,
    pub secret_manager: Arc<Mutex<SecretManager>>,
    pub session_manager: AgentSessionManager,
    pub session_id: String,
    pub session_token: String,
    pub project_id: String,
    pub current_key_index: usize,
    pub current_step: usize,
    pub conversation_history: Vec<(String, String)>,
    pub needs_confirmation: bool,
    pub pending_tool_calls: VecDeque<ToolCallRequest>,
    pub simulated_tool_handler:
        Option<Arc<dyn Fn(&str, &str) -> Result<String, String> + Send + Sync>>,
    pub local_routing_enabled: bool,
    pub local_snapshot: LocalSystemSnapshot,
    pub static_prefix: Option<StaticSystemPrefix>,
    pub prefix_cached_in_session: bool,
    pub last_request_payload: Vec<u8>,
    pub spend_tracker: Option<Arc<Mutex<DailySpendTracker>>>,
    pub last_budget_warning: Option<BudgetWarningAlert>,
    pub last_blocked_alternatives: Vec<FallbackAlternative>,
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
        let project_id = gatekeeper.profile().name.clone();
        let mut local_snapshot = LocalSystemSnapshot::default();
        local_snapshot.project_id = project_id.clone();
        local_snapshot.active_model_display = config.model.clone();
        let session_manager =
            AgentSessionManager::new(tool_registry, gatekeeper, session_token.to_string());
        Self {
            config,
            secret_manager,
            session_manager,
            session_id: session_id.to_string(),
            session_token: session_token.to_string(),
            project_id,
            current_key_index: 0,
            current_step: 0,
            conversation_history: Vec::new(),
            needs_confirmation: false,
            pending_tool_calls: VecDeque::new(),
            simulated_tool_handler: None,
            local_routing_enabled: true,
            local_snapshot,
            static_prefix: None,
            prefix_cached_in_session: false,
            last_request_payload: Vec::new(),
            spend_tracker: None,
            last_budget_warning: None,
            last_blocked_alternatives: Vec::new(),
        }
    }

    pub fn set_simulated_tool_handler<F>(&mut self, handler: F)
    where
        F: Fn(&str, &str) -> Result<String, String> + Send + Sync + 'static,
    {
        self.simulated_tool_handler = Some(Arc::new(handler));
    }

    /// Configures a deterministic `StaticSystemPrefix` (from `miku-memory` Step 1.6)
    /// using the agent's soul directives and sorted tool registry.
    pub fn set_static_system_prefix_from_directives(&mut self, soul_directives: &str) {
        let prefix = build_static_system_prefix_from_registry(
            &self.session_id,
            soul_directives,
            self.session_manager.registry(),
        );
        self.static_prefix = Some(prefix);
        self.prefix_cached_in_session = false;
    }

    pub fn set_spend_tracker(&mut self, tracker: Arc<Mutex<DailySpendTracker>>) {
        self.spend_tracker = Some(tracker);
    }

    /// Switches the harness to a free or local fallback alternative when paid daily budget reaches 100%.
    pub fn switch_to_fallback_alternative(&mut self, alternative: &FallbackAlternative) {
        self.config.provider = alternative.provider;
        self.config.model = alternative.model.clone();
        self.config.endpoint_url = alternative.endpoint_url.clone();
        self.config.key_aliases.clear();
        self.local_snapshot.active_model_display = alternative.model.clone();
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
    ) -> Result<(ProviderResponse, Vec<u8>), HarnessError> {
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
                let parsed = parse_provider_response(self.config.provider, &body)?;
                return Ok((parsed, body));
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
                        self.current_key_index = alias_idx;
                        let parsed = parse_provider_response(self.config.provider, &body)
                            .map_err(|e| self.sanitize_error(e, secret_buf.as_slice()))?;
                        return Ok((parsed, body));
                    } else if status == 429 || (500..=599).contains(&status) {
                        if attempt + 1 < total_keys {
                            if backoff_ms > 0 {
                                sleep(Duration::from_millis(backoff_ms)).await;
                                backoff_ms = (backoff_ms * 2).min(self.config.max_backoff_ms);
                            }
                            self.current_key_index = (alias_idx + 1) % total_keys;
                            continue;
                        }
                    } else {
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

    /// Executes an agent turn with bounded step iteration and 0-token local routing.
    pub async fn execute_turn(
        &mut self,
        user_prompt: &str,
        cancel_handle: Option<&CooperativeCancelHandle>,
    ) -> Result<TurnStepOutcome, HarnessError> {
        self.execute_turn_with_options(user_prompt, false, cancel_handle)
            .await
    }

    /// Executes an agent turn allowing explicit `force_llm` override (bypassing local 0-token router
    /// while still enforcing the per-project daily budget hard cap).
    pub async fn execute_turn_with_options(
        &mut self,
        user_prompt: &str,
        force_llm: bool,
        cancel_handle: Option<&CooperativeCancelHandle>,
    ) -> Result<TurnStepOutcome, HarnessError> {
        if self.local_routing_enabled {
            if let Some(ref tracker_arc) = self.spend_tracker {
                let mut tracker = tracker_arc.lock().await;
                let snap = tracker.snapshot(&self.project_id);
                self.local_snapshot.daily_spent_micros = snap.spent_micros;
                self.local_snapshot.daily_limit_micros = snap.limit_micros;
            }
            match LocalIntentRouter::classify_and_route(
                user_prompt,
                force_llm,
                &self.local_snapshot,
            ) {
                RouteDecision::LocalZeroToken(local_match) => {
                    if let Some(ref tracker_arc) = self.spend_tracker {
                        let mut tracker = tracker_arc.lock().await;
                        tracker.record_local_zero_token_hit(&self.project_id);
                    }
                    self.conversation_history
                        .push(("user".to_string(), user_prompt.to_string()));
                    self.conversation_history
                        .push(("assistant".to_string(), local_match.reply.clone()));
                    return Ok(TurnStepOutcome::LocalZeroTokenReply {
                        category: local_match.category.as_str().to_string(),
                        intent_id: local_match.intent_id,
                        reply: local_match.reply,
                        tokens_used: local_match.tokens_used,
                    });
                }
                RouteDecision::ForwardToLlm {
                    stripped_prompt, ..
                } => {
                    self.conversation_history
                        .push(("user".to_string(), stripped_prompt));
                }
            }
        } else {
            self.conversation_history
                .push(("user".to_string(), user_prompt.to_string()));
        }

        self.run_turn_loop(cancel_handle).await
    }

    /// Resumes execution after the user approves additional steps beyond the original max_steps.
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

            // 3. F-09 Pre-flight Daily Spend Evaluation BEFORE reading secrets or opening sockets
            if let Some(ref tracker_arc) = self.spend_tracker {
                let decision = tracker_arc.lock().await.evaluate_pre_flight(
                    &self.project_id,
                    self.config.provider,
                    &self.config.model,
                );
                match decision {
                    PreFlightBudgetDecision::BlockedHardCap {
                        project_id, day_key, spent_micros, limit_micros, alternatives, ..
                    } => {
                        let suggested_alternatives =
                            alternatives.iter().map(|a| a.label.clone()).collect();
                        self.last_blocked_alternatives = alternatives;
                        return Err(HarnessError::DailyBudgetExceeded {
                            project_id, day_key, spent_micros, limit_micros, suggested_alternatives,
                        });
                    }
                    PreFlightBudgetDecision::Allowed { warning, .. } => {
                        if warning.is_some() {
                            self.last_budget_warning = warning;
                        }
                    }
                }
            }

            let payload = format_provider_request_with_stable_prefix(
                self.config.provider,
                &self.config.model,
                self.static_prefix.as_ref(),
                &self.conversation_history,
            );
            self.last_request_payload = payload.clone();

            let (model_response, raw_response_body) = self
                .call_model_with_key_rotation(&payload, cancel_handle)
                .await?;

            if let Some(ref tracker_arc) = self.spend_tracker {
                let dynamic_text = self.conversation_history.last().map(|(_, c)| c.as_str()).unwrap_or("");
                let reply_str = match &model_response {
                    ProviderResponse::Text(t) => t.as_str(),
                    ProviderResponse::ToolCalls(_) => "tool_call",
                };
                let usage = extract_or_estimate_token_usage(
                    &raw_response_body,
                    self.static_prefix.as_ref(),
                    dynamic_text,
                    reply_str,
                    self.prefix_cached_in_session,
                );
                let (_cost, post_warning) = tracker_arc.lock().await.record_turn_usage(
                    &self.project_id,
                    self.config.provider,
                    &self.config.model,
                    usage,
                );
                if post_warning.is_some() {
                    self.last_budget_warning = post_warning;
                }
            }
            if self.static_prefix.is_some() {
                self.prefix_cached_in_session = true;
            }

            match model_response {
                ProviderResponse::Text(reply) => {
                    self.current_step += 1;
                    self.conversation_history.push(("assistant".to_string(), reply.clone()));
                    return Ok(TurnStepOutcome::Completed { reply, total_steps: self.current_step });
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
    pub async fn dispatch_tool_call(
        &mut self,
        tool_call: &ToolCallRequest,
    ) -> Result<String, HarnessError> {
        let repaired_val = repair_tool_call_json(&tool_call.raw_args).map_err(|e| {
            HarnessError::IrreparableToolJson {
                tool_name: tool_call.tool_name.clone(),
                detail: e.to_string(),
            }
        })?;
        let canonical_args = repaired_val.to_string();
        let target = repaired_val
            .get("target")
            .or_else(|| repaired_val.get("path"))
            .and_then(|t| t.as_str())
            .unwrap_or(".")
            .to_string();

        if let Some(ref handler) = self.simulated_tool_handler {
            let _ = self.session_manager.registry().get(&tool_call.tool_name).map_err(|e| {
                HarnessError::ToolExecutionFailed {
                    tool_name: tool_call.tool_name.clone(),
                    detail: e.to_string(),
                }
            })?;
            return handler(&tool_call.tool_name, &canonical_args).map_err(|e| {
                HarnessError::ToolExecutionFailed {
                    tool_name: tool_call.tool_name.clone(),
                    detail: e,
                }
            });
        }

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
            Ok(ToolDispatchOutcome::Completed(output)) => match tool_call.tool_name.as_str() {
                "fs_read" => {
                    let path = &output.canonical_target;
                    std::fs::read_to_string(path).map_err(|e| HarnessError::ToolExecutionFailed {
                        tool_name: tool_call.tool_name.clone(),
                        detail: format!("failed to read file '{}': {}", path, e),
                    })
                }
                _ => Err(HarnessError::ToolExecutionFailed {
                    tool_name: tool_call.tool_name.clone(),
                    detail: format!("tool executor for '{}' is not implemented", tool_call.tool_name),
                }),
            },
            Ok(ToolDispatchOutcome::Denied { reason, detail, audit_seq, audit_hash, .. }) => {
                Err(HarnessError::GatekeeperDenied(format!(
                    "{}: {} (audit_seq={}, audit_hash={})",
                    reason.as_code(), detail, audit_seq, audit_hash
                )))
            }
            Ok(ToolDispatchOutcome::SuspendedAsk { ticket, audit_seq, audit_hash, .. }) => {
                Err(HarnessError::GatekeeperDenied(format!(
                    "confirmation_pending: req_hash={} (audit_seq={}, audit_hash={})",
                    ticket.req_hash, audit_seq, audit_hash
                )))
            }
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
