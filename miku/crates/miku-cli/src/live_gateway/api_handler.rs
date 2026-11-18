use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use miku_gatekeeper::{Capability, GatekeeperVerdict};
use miku_gateway::LoopbackApiHandler;
use miku_protocol::CaptureTransform;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::agent_tools::{run_openai_compatible_agent_loop, supports_openai_tools};
use super::api_routes::dispatch_http_api;
use super::llm_client::{execute_llm_chat_with_context, load_sanitized_history, LlmRequestConfig};
use super::llm_multimodal::LlmImageAttachment;
use super::runtime_state::LiveRuntimeState;
use super::storage_actor::{now_epoch_ms, StorageActorHandle};
use super::token_pipeline::{
    build_dynamic_user_turn, build_llm_success_reply_json, build_offline_fallback_reply,
    build_static_system_prompt, extract_fts_query, is_daily_budget_exceeded, prompt_needs_tools,
    resolve_agent_model_and_key, setting_bool, setting_u32, try_local_zero_token_route,
};

pub struct LiveMikuApiHandler {
    pub(super) state: Mutex<LiveRuntimeState>,
    pub(super) storage: StorageActorHandle,
    pub(super) events_tx: broadcast::Sender<Value>,
    pub(super) gateway_token: Mutex<Option<String>>,
    pub(super) gateway_port: Mutex<u16>,
    pub(super) pending_open_settings: Mutex<Option<Option<String>>>,
    pub(super) last_vision_attachment: Mutex<Option<LlmImageAttachment>>,
    pub(super) last_capture_transform: Mutex<CaptureTransform>,
    pub(super) voice_capture_stop: Arc<AtomicBool>,
}

impl LiveMikuApiHandler {
    pub fn new(data_dir: &Path, port: u16, use_memory_secrets: bool) -> Result<Self, String> {
        let mut state = LiveRuntimeState::new(data_dir, port, use_memory_secrets)?;
        let stt_init = state
            .settings
            .get("voice_stt_enabled")
            .map(|v| v.as_bool().unwrap_or_else(|| v.as_i64().unwrap_or(0) != 0))
            .unwrap_or(false);
        miku_ui::webview::set_webview_mic_permission_allowed(stt_init);
        let storage = StorageActorHandle::spawn(data_dir)?;
        state.next_seq = storage.initial_max_seq().saturating_add(1).max(state.next_seq);
        let (events_tx, _) = broadcast::channel(64);
        let init_port = if port == 0 {
            miku_gateway::DEFAULT_LOOPBACK_PORT
        } else {
            port
        };
        Ok(Self {
            state: Mutex::new(state),
            storage,
            events_tx,
            gateway_token: Mutex::new(None),
            gateway_port: Mutex::new(init_port),
            pending_open_settings: Mutex::new(None),
            last_vision_attachment: Mutex::new(None),
            last_capture_transform: Mutex::new(CaptureTransform::default()),
            voice_capture_stop: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn request_open_settings(&self, section: Option<String>) {
        if let Ok(mut guard) = self.pending_open_settings.lock() {
            *guard = Some(section);
        }
    }

    pub fn take_pending_open_settings(&self) -> Option<Option<String>> {
        self.pending_open_settings
            .lock()
            .ok()
            .and_then(|mut g| g.take())
    }

    pub fn set_gateway_endpoint(&self, port: u16, token: &str) {
        if let Ok(mut guard) = self.gateway_port.lock() {
            *guard = port;
        }
        if let Ok(mut guard) = self.gateway_token.lock() {
            *guard = Some(token.to_string());
        }
    }

    pub fn gateway_endpoint(&self) -> (u16, Option<String>) {
        let port = self
            .gateway_port
            .lock()
            .map(|g| *g)
            .unwrap_or(miku_gateway::DEFAULT_LOOPBACK_PORT);
        let tok = self.gateway_token.lock().ok().and_then(|g| g.clone());
        (port, tok)
    }

    pub fn checkpoint_storage(&self) -> Result<(), String> {
        self.storage.checkpoint()
    }

    pub fn set_kill_switch(&self, active: bool) -> bool {
        let now_ms = now_epoch_ms();
        let snap = {
            let mut st = self.state.lock().expect("state mutex");
            if active {
                let _ = st.gatekeeper.trigger_kill_switch(now_ms);
            } else {
                st.gatekeeper.reset_kill_switch(now_ms);
            }
            st.snapshot_json()
        };
        self.broadcast_state(&snap);
        active
    }

    pub fn toggle_kill_switch(&self) -> bool {
        let next = !self.is_kill_switch_active();
        self.set_kill_switch(next)
    }

    pub fn is_kill_switch_active(&self) -> bool {
        self.state
            .lock()
            .map(|st| st.gatekeeper.is_kill_switch_active())
            .unwrap_or(false)
    }

    pub fn has_any_api_key(&self) -> bool {
        self.state
            .lock()
            .map(|st| !st.secret_names().is_empty())
            .unwrap_or(false)
    }

    pub fn is_local_net_enabled(&self) -> bool {
        self.state.lock().ok().and_then(|st| {
            st.settings.get("local_net").map(|v| v.as_bool().unwrap_or_else(|| v.as_i64().unwrap_or(1) != 0))
        }).unwrap_or(true)
    }

    pub fn is_voice_stt_enabled(&self) -> bool {
        self.state.lock().ok().and_then(|st| {
            st.settings.get("voice_stt_enabled").map(|v| v.as_bool().unwrap_or_else(|| v.as_i64().unwrap_or(0) != 0))
        }).unwrap_or(false)
    }

    pub(super) fn current_voice_stt_engine(&self) -> String {
        self.state
            .lock()
            .ok()
            .and_then(|st| st.settings.get("voice_stt_engine").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| "off".to_string())
    }

    pub fn broadcast_agent_state(&self, state: &str) {
        let _ = self.events_tx.send(json!({ "type": "agent_state", "state": state }));
    }

    pub fn broadcast_voice_transcript(&self, text: &str, is_final: bool, engine: &str) {
        let _ = self.events_tx.send(json!({ "type": "voice_transcript", "text": text, "is_final": is_final, "engine": engine }));
    }

    pub fn push_event(&self, ev: Value) {
        let _ = self.events_tx.send(ev);
    }

    pub(super) fn broadcast_state(&self, snap: &Value) {
        let mut ev = snap.clone();
        if let Some(obj) = ev.as_object_mut() {
            obj.insert("type".to_string(), json!("state_sync"));
        }
        let _ = self.events_tx.send(ev);
    }

    pub(super) fn process_chat(
        &self,
        agent_id: &str,
        text: &str,
        file_name: Option<&str>,
    ) -> Value {
        let clean_text = text.trim();
        let now_ms = now_epoch_ms();
        let (
            seq,
            eval_res,
            provider,
            model,
            base_url,
            use_memory_secrets,
            agent_name,
            agent_role,
            agent_soul,
            agent_tools_mode,
            api_key_opt,
            vault_aliases_count,
            skills_count,
            local_route_hit,
            budget_exceeded,
            prefix_cache_enabled,
            prune_tools_enabled,
            max_tokens,
            history_limit,
            model_fallback_note,
        ) = {
            let mut st = match self.state.lock() {
                Ok(g) => g,
                Err(_) => {
                    return json!({
                        "type": "agent_reply",
                        "reply_id": format!("{}-err-{}", agent_id, now_ms),
                        "agent_id": agent_id,
                        "error": true,
                        "text": "Error interno al acceder al estado del runtime."
                    });
                }
            };
            let seq = st.next_seq;
            st.next_seq = st.next_seq.saturating_add(2);
            let verdict = st.gatekeeper.evaluate_request(
                agent_id,
                Capability::Network,
                "chat.prompt",
                clean_text,
                now_ms,
            );
            let global_provider = st.settings.get("provider").and_then(|v| v.as_str()).unwrap_or("openai").to_string();
            let global_model = st.settings.get("model").and_then(|v| v.as_str()).unwrap_or("gpt-4o-mini").to_string();
            let global_base_url = st.settings.get("custom_base_url").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let use_memory_secrets = st.use_memory_secrets;
            let agent_opt = st.agents.iter().find(|a| a.id == agent_id).cloned();
            let agent_name = agent_opt.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| "Miku".to_string());
            let agent_role = agent_opt.as_ref().map(|a| a.role.clone()).unwrap_or_else(|| "Orquestadora local".to_string());
            let agent_soul = agent_opt.as_ref().map(|a| a.prompt.clone()).unwrap_or_default();
            let agent_tools_mode = agent_opt.as_ref().map(|a| a.tools_mode.clone()).unwrap_or_else(|| "auto".to_string());
            let vault_aliases_count = st.secret_names().len();
            let skills_count = st.skill_engine.active_skill_count();
            let prefix_cache_enabled = setting_bool(&st.settings, "token_saver_prefix_cache", true);
            let prune_tools_enabled = setting_bool(&st.settings, "token_saver_prune_tools", true);
            let max_tokens = setting_u32(&st.settings, "max_tokens", 384, 64, 4096);
            let history_limit = setting_u32(&st.settings, "history_turns", 6, 0, 12) as usize;

            let resolved = resolve_agent_model_and_key(
                &mut st,
                agent_opt.as_ref(),
                &global_provider,
                &global_model,
                global_base_url,
            );
            let local_hit = try_local_zero_token_route(
                &st,
                clean_text,
                file_name.is_some(),
                &format!("{}/{}", resolved.provider, resolved.model),
            );
            let budget_over = if local_hit.is_none() {
                is_daily_budget_exceeded(&st, &resolved.provider)
            } else {
                None
            };
            (
                seq,
                verdict,
                resolved.provider,
                resolved.model,
                resolved.base_url,
                use_memory_secrets,
                agent_name,
                agent_role,
                agent_soul,
                agent_tools_mode,
                resolved.api_key_opt,
                vault_aliases_count,
                skills_count,
                local_hit,
                budget_over,
                prefix_cache_enabled,
                prune_tools_enabled,
                max_tokens,
                history_limit,
                resolved.fallback_used,
            )
        };
        let model_str = format!("{}/{}", provider, model);
        let reply_id = format!("{}-{}", agent_id, seq);
        self.broadcast_agent_state("thinking");

        if let GatekeeperVerdict::Deny { detail, .. } = eval_res {
            let err_reply = json!({
                "type": "agent_reply",
                "reply_id": reply_id,
                "agent_id": agent_id,
                "error": true,
                "retry_prompt": clean_text,
                "text": format!("Bloqueado por Gatekeeper: {}", detail),
                "steps": ["Gatekeeper: Deny"]
            });
            let _ = self.events_tx.send(err_reply.clone());
            self.broadcast_agent_state("error");
            return err_reply;
        }

        let session_id = format!("ui-{}", agent_id);
        let stored_docs = self
            .storage
            .record_turn(&session_id, "user", clean_text, seq, now_ms)
            .unwrap_or(0);

        if let Some((local_text, _intent_id, saved_tok)) = local_route_hit {
            if let Ok(mut st) = self.state.lock() {
                st.record_local_route(saved_tok);
            }
            let _ = self.storage.record_turn(&session_id, "assistant", &local_text, seq.saturating_add(1), now_epoch_ms());
            let local_reply = json!({
                "type": "agent_reply",
                "reply_id": reply_id,
                "agent_id": agent_id,
                "error": false,
                "routed_local": true,
                "tokens_saved": saved_tok,
                "text": local_text,
                "steps": [],
                "latency_ms": 0
            });
            let _ = self.events_tx.send(local_reply.clone());
            self.broadcast_agent_state("idle");
            return local_reply;
        }

        if file_name.is_none() {
            if let Some(launch_reply) = super::app_launcher::try_local_launch_command(self, agent_id, clean_text, &reply_id, seq, &session_id) {
                return launch_reply;
            }
        }

        if let Some((spent, limit)) = budget_exceeded {
            let msg = format!(
                "Presupuesto diario alcanzado (${:.3} / ${:.2} USD). Cambia el modelo a Ollama local o ajusta el límite en Ajustes > Ahorro de Tokens.",
                spent, limit
            );
            let budget_reply = json!({
                "type": "agent_reply",
                "reply_id": reply_id,
                "agent_id": agent_id,
                "error": true,
                "retry_prompt": clean_text,
                "text": msg,
                "steps": ["Budget Guard: límite diario alcanzado"]
            });
            let _ = self.events_tx.send(budget_reply.clone());
            self.broadcast_agent_state("error");
            return budget_reply;
        }

        let fts_query = extract_fts_query(clean_text);
        let hits = self.storage.search_fts5(&fts_query, 3).unwrap_or_default();
        let fts_hits = hits.len();

        let is_ollama = provider.eq_ignore_ascii_case("ollama");
        let should_call_live_llm =
            (!use_memory_secrets || base_url.is_some()) && (is_ollama || api_key_opt.is_some());

        if should_call_live_llm {
            let _ = self.events_tx.send(json!({
                "type": "ai_inference",
                "provider": provider,
                "model": model,
                "status": "running",
                "agent_id": agent_id
            }));
            let system_prompt = build_static_system_prompt(
                &agent_name,
                &agent_role,
                &agent_soul,
                prefix_cache_enabled,
            );
            let user_turn = build_dynamic_user_turn(clean_text, &hits, file_name);
            let (tools_enabled, pruned_saved) =
                prompt_needs_tools(clean_text, &agent_tools_mode, prune_tools_enabled);
            if pruned_saved > 0 {
                if let Ok(mut st) = self.state.lock() {
                    st.add_tokens_saved(pruned_saved);
                }
            }
            let api_key = api_key_opt.unwrap_or_default();
            self.clear_turn_vision_state();
            let llm_outcome = if supports_openai_tools(&provider) {
                run_openai_compatible_agent_loop(
                    self,
                    agent_id,
                    &session_id,
                    seq,
                    &provider,
                    &model,
                    &api_key,
                    base_url.as_deref(),
                    &system_prompt,
                    &user_turn,
                    max_tokens,
                    tools_enabled,
                    history_limit,
                )
            } else {
                let prior_history = load_sanitized_history(
                    &self.storage,
                    &session_id,
                    seq,
                    history_limit,
                    480,
                );
                let mut turn_att = self.take_pending_multimodal_attachment();
                let res = execute_llm_chat_with_context(
                    &LlmRequestConfig {
                        provider: provider.clone(),
                        model: model.clone(),
                        api_key,
                        base_url,
                        prompt: user_turn,
                        system_prompt,
                        attachment: turn_att.take(),
                    },
                    &prior_history,
                    max_tokens,
                );
                res.map(|res| super::agent_tools::AgentLoopOutcome {
                    chat: res.chat,
                    cached_tokens: res.cached_tokens,
                    tool_steps: Vec::new(),
                    rounds: 1,
                })
            };
            self.clear_turn_vision_state();
            match llm_outcome {
                Ok(outcome) => {
                    let cached_tok = outcome.cached_tokens;
                    if let Ok(mut st) = self.state.lock() {
                        st.record_llm_usage_with_cache(
                            &model_str,
                            outcome.chat.input_tokens.unwrap_or(0),
                            cached_tok,
                            outcome.chat.output_tokens.unwrap_or(0),
                            outcome.chat.latency_ms,
                            false,
                        );
                    }
                    let _ = self.events_tx.send(json!({
                        "type": "ai_inference",
                        "provider": provider,
                        "model": model,
                        "status": "done",
                        "latency_ms": outcome.chat.latency_ms,
                        "input_tokens": outcome.chat.input_tokens,
                        "cached_tokens": cached_tok,
                        "output_tokens": outcome.chat.output_tokens,
                        "host": outcome.chat.endpoint_host,
                        "agent_id": agent_id
                    }));
                    let (reply, assistant_text, _) = build_llm_success_reply_json(
                        &reply_id,
                        agent_id,
                        &model_str,
                        model_fallback_note,
                        outcome,
                        stored_docs,
                        &fts_query,
                        pruned_saved,
                    );
                    let _ = self.storage.record_turn(
                        &session_id,
                        "assistant",
                        &assistant_text,
                        seq.saturating_add(1),
                        now_epoch_ms(),
                    );
                    let _ = self.events_tx.send(reply.clone());
                    super::link_routes::notify_agent_reply_if_paired(self, &agent_name, &assistant_text, false);
                    self.broadcast_agent_state("idle");
                    return reply;
                }
                Err(err) => {
                    if let Ok(mut st) = self.state.lock() {
                        st.record_llm_usage_with_cache(&model_str, 0, 0, 0, 0, true);
                    }
                    let err_msg = format!("Error llamando a {}: {}", model_str, err.message);
                    let err_reply = json!({
                        "type": "agent_reply",
                        "reply_id": reply_id,
                        "agent_id": agent_id,
                        "error": true,
                        "status_code": err.status_code,
                        "retry_prompt": clean_text,
                        "text": err_msg,
                        "steps": [
                            format!("Gatekeeper: OK ({})", model_str),
                            format!("Error HTTP: {:?}", err.status_code)
                        ]
                    });
                    let _ = self.events_tx.send(err_reply.clone());
                    super::link_routes::notify_agent_reply_if_paired(self, &agent_name, &err_msg, true);
                    self.broadcast_agent_state("error");
                    return err_reply;
                }
            }
        }

        let (reply, reply_text) = build_offline_fallback_reply(
            &reply_id,
            agent_id,
            &agent_name,
            seq,
            stored_docs,
            &fts_query,
            fts_hits,
            file_name,
            vault_aliases_count,
            &model_str,
            skills_count,
        );
        let _ = self.storage.record_turn(
            &session_id,
            "assistant",
            &reply_text,
            seq.saturating_add(1),
            now_ms,
        );
        let _ = self.events_tx.send(reply.clone());
        super::link_routes::notify_agent_reply_if_paired(self, &agent_name, &reply_text, false);
        self.broadcast_agent_state(if reply.get("error").and_then(|v| v.as_bool()).unwrap_or(false) {
            "error"
        } else {
            "idle"
        });
        reply
    }
}

impl LoopbackApiHandler for LiveMikuApiHandler {
    fn bootstrap_json(&self) -> Value {
        let snap = self.state_json();
        json!({
            "ok": true,
            "app": "miku-live-gateway",
            "version": env!("CARGO_PKG_VERSION"),
            "state": snap
        })
    }

    fn state_json(&self) -> Value {
        match self.state.lock() {
            Ok(st) => st.snapshot_json(),
            Err(_) => json!({ "ok": false, "error": "state lock poisoned" }),
        }
    }

    fn handle_api(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> (u16, Value) {
        dispatch_http_api(self, method, path, query, body)
    }

    fn handle_ws_command(&self, cmd: &Value) -> Option<Value> {
        self.handle_ws_command_inner(cmd)
    }

    fn subscribe_events(&self) -> Option<broadcast::Receiver<Value>> {
        Some(self.events_tx.subscribe())
    }

    fn is_lan_public_endpoint(&self, method: &str, path: &str) -> bool {
        method == "POST" && path == "/api/link/redeem"
    }

    fn authorize_mobile_request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        query: Option<&str>,
    ) -> bool {
        super::link_routes::authorize_mobile_request(self, method, path, headers, query)
    }
}
