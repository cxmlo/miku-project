use std::path::PathBuf;

use miku_browser::detect_installed_browsers;
use miku_gateway::LoopbackApiHandler;
use miku_skills::SkillEngine;
use miku_ui::query_current_process_private_ram_mb;
use serde_json::{json, Value};

use crate::{
    probe_custom_agents_directory, query_live_windows_webview2_version, run_doctor_suite,
    DoctorRunConfig, WebView2ProbeSource, BUDGET_MAX_RELEASE_BINARY_MB,
};

use super::api_handler::LiveMikuApiHandler;
use super::extended_routes::try_dispatch_extended_api;
use super::runtime_state::{
    parse_query_param, percent_decode_lossy, persist_json, reload_skills_from_root,
};
use super::storage_actor::{AgentProfileDto, McpServerEntry, SecretAliasIndexEntry};

pub(super) fn dispatch_http_api(
    handler: &LiveMikuApiHandler,
    method: &str,
    path: &str,
    query: Option<&str>,
    body: &[u8],
) -> (u16, Value) {
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body).unwrap_or(Value::Null)
    };

    match (method, path) {
        ("GET", "/api/state") => (200, handler.state_json()),
        ("GET", "/api/bootstrap") => (200, handler.bootstrap_json()),
        ("POST", "/api/chat") => {
            let text = parsed
                .get("text")
                .or_else(|| parsed.get("prompt"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if text.trim().is_empty() {
                return (400, json!({ "ok": false, "error": "prompt text required" }));
            }
            let agent_id = parsed
                .get("agent_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("miku");
            let file_name = parsed
                .get("file")
                .and_then(|f| f.get("name"))
                .and_then(|v| v.as_str());
            (200, handler.process_chat(agent_id, text, file_name))
        }
        ("POST", "/api/settings") => {
            let key = parsed.get("key").and_then(|v| v.as_str()).unwrap_or("");
            if key.trim().is_empty() {
                return (400, json!({ "ok": false, "error": "setting key required" }));
            }
            let val = parsed.get("value").cloned().unwrap_or(Value::Null);
            let snap = {
                let mut st = handler.state.lock().expect("state mutex");
                st.settings.insert(key.to_string(), val.clone());
                if key == "local_net" {
                    let enabled = val.as_bool().unwrap_or(val.as_i64().unwrap_or(0) != 0);
                    st.pairing.set_lan_enabled(enabled);
                }
                if key == "voice_stt_enabled" {
                    let enabled = val.as_bool().unwrap_or(val.as_i64().unwrap_or(0) != 0);
                    miku_ui::webview::set_webview_mic_permission_allowed(enabled);
                }
                persist_json(&st.data_dir.join("settings.json"), &st.settings);
                st.snapshot_json()
            };
            handler.broadcast_state(&snap);
            (200, json!({ "ok": true, "key": key, "value": val }))
        }
        ("POST", "/api/gatekeeper/kill_switch") => {
            let active = parsed
                .get("active")
                .or_else(|| parsed.get("armed"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let out = handler.set_kill_switch(active);
            (200, json!({ "ok": true, "kill_switch": out }))
        }
        ("POST", "/api/secrets") => {
            let alias = parsed
                .get("alias")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let secret = parsed
                .get("secret")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if alias.is_empty() || secret.is_empty() {
                return (
                    400,
                    json!({ "ok": false, "error": "alias and secret required" }),
                );
            }
            let provider_opt = parsed.get("provider").and_then(|v| v.as_str());
            let model_opt = parsed.get("model").and_then(|v| v.as_str());

            let mut st = handler.state.lock().expect("state mutex");
            let provider = provider_opt
                .or_else(|| st.settings.get("provider").and_then(|v| v.as_str()))
                .unwrap_or("custom")
                .to_string();
            let model_owned = model_opt
                .or_else(|| st.settings.get("model").and_then(|v| v.as_str()))
                .map(|s| s.to_string());

            if let Err(e) = st.secrets.set_secret(
                alias,
                &provider,
                model_owned.as_deref(),
                secret.as_bytes(),
            ) {
                return (400, json!({ "ok": false, "error": e.to_string() }));
            }
            st.secret_aliases.insert(
                alias.to_string(),
                SecretAliasIndexEntry {
                    alias: alias.to_string(),
                    provider: provider.clone(),
                    model: model_owned.clone(),
                },
            );
            persist_json(&st.data_dir.join("secret_aliases.json"), &st.secret_aliases);
            if provider_opt.is_some() || model_opt.is_some() {
                if let Some(p) = provider_opt.map(str::trim).filter(|s| !s.is_empty()) {
                    st.settings.insert("provider".to_string(), json!(p));
                }
                if let Some(m) = model_opt.map(str::trim).filter(|s| !s.is_empty()) {
                    st.settings.insert("model".to_string(), json!(m));
                }
                persist_json(&st.data_dir.join("settings.json"), &st.settings);
            }
            let snap = st.snapshot_json();
            drop(st);
            handler.broadcast_state(&snap);
            (
                200,
                json!({ "ok": true, "alias": alias, "provider": provider, "status": "configured" }),
            )
        }
        ("DELETE", p) if p.starts_with("/api/secrets/") => {
            let alias = percent_decode_lossy(p.trim_start_matches("/api/secrets/"));
            let mut st = handler.state.lock().expect("state mutex");
            match st.secrets.delete_secret(&alias) {
                Ok(_) => {
                    st.secret_aliases.remove(&alias);
                    persist_json(&st.data_dir.join("secret_aliases.json"), &st.secret_aliases);
                    let snap = st.snapshot_json();
                    drop(st);
                    handler.broadcast_state(&snap);
                    (200, json!({ "ok": true, "alias": alias }))
                }
                Err(e) => (400, json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("POST", "/api/skills/reload") => {
            let (warnings, skills_list, snap) = {
                let mut st = handler.state.lock().expect("state mutex");
                let dir = st.skills_dir.clone();
                let mut engine = SkillEngine::new();
                let warnings = reload_skills_from_root(&dir, &mut engine, &st.gatekeeper);
                st.skill_engine = engine;
                let list = st.skills_json();
                let snap = st.snapshot_json();
                (warnings, list, snap)
            };
            handler.broadcast_state(&snap);
            (
                200,
                json!({
                    "ok": true,
                    "count": skills_list.len(),
                    "skills": skills_list,
                    "diagnostics": warnings
                }),
            )
        }
        ("POST", "/api/mcp") => {
            let name = parsed
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let command = parsed
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if name.is_empty() || command.is_empty() {
                return (
                    400,
                    json!({ "ok": false, "error": "name and command required" }),
                );
            }
            let args: Vec<String> = parsed
                .get("args")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let snap = {
                let mut st = handler.state.lock().expect("state mutex");
                st.mcp_servers.retain(|m| m.name != name);
                st.mcp_servers.push(McpServerEntry {
                    name: name.to_string(),
                    command: command.to_string(),
                    args,
                });
                persist_json(&st.data_dir.join("mcp.json"), &st.mcp_servers);
                st.snapshot_json()
            };
            handler.broadcast_state(&snap);
            (200, json!({ "ok": true }))
        }
        ("GET", "/api/memory/search") => {
            let q = parse_query_param(query, "q");
            match handler.storage.search_fts5(&q, 10) {
                Ok(hits) => {
                    let rows: Vec<Value> = hits
                        .into_iter()
                        .map(|h| {
                            json!({
                                "doc_id": h.doc_id,
                                "title": h.title,
                                "tags": h.tags,
                                "bm25_score": h.bm25_score,
                                "snippet": h.snippet,
                                "updated_at_ms": h.updated_at_ms
                            })
                        })
                        .collect();
                    (200, Value::Array(rows))
                }
                Err(e) => (500, json!({ "ok": false, "error": e })),
            }
        }
        (_, p) if p.starts_with("/api/link/") => {
            if let Some(resp) =
                super::link_routes::try_dispatch_link_api(handler, method, p, query, &parsed)
            {
                return resp;
            }
            (404, json!({ "ok": false, "error": "link endpoint not found" }))
        }
        ("GET", "/api/doctor") => {
            let agents_dir = handler
                .state
                .lock()
                .map(|s| s.agents_dir.clone())
                .unwrap_or_else(|_| PathBuf::from(".miku/agents"));
            let (port, gw_token) = handler.gateway_endpoint();
            let report_val = std::thread::spawn(move || {
                let t0 = std::time::Instant::now();
                let live_wv2 = query_live_windows_webview2_version();
                let mut cfg = DoctorRunConfig {
                    port,
                    webview2_source: WebView2ProbeSource::SimulatedVersion(
                        live_wv2.or_else(|| Some("131.0.2903.86".to_string())),
                    ),
                    ..DoctorRunConfig::default()
                };
                if let Some(tok) = gw_token.filter(|t| !t.is_empty()) {
                    cfg.auth_token = tok;
                }
                let mut report = run_doctor_suite(&cfg);
                let (custom_probe, _) = probe_custom_agents_directory(&agents_dir, 1_024);
                report.probes.push(custom_probe);
                let finalized = crate::DoctorReportV0::from_probes(
                    report.probes,
                    t0.elapsed().as_secs_f64() * 1_000.0,
                );
                serde_json::to_value(finalized).unwrap_or_else(|_| json!({ "ok": false }))
            })
            .join()
            .unwrap_or_else(|_| json!({ "ok": false, "error": "doctor thread failed" }));
            (200, report_val)
        }
        ("GET", "/api/update/check") => (
            200,
            json!({
                "ok": true,
                "version": env!("CARGO_PKG_VERSION"),
                "channel": "stable",
                "status": "up_to_date",
                "max_binary_mb": BUDGET_MAX_RELEASE_BINARY_MB,
                "process_private_mb": query_current_process_private_ram_mb()
            }),
        ),
        ("GET", "/api/apps") => {
            let candidates = detect_installed_browsers();
            let list: Vec<Value> = candidates
                .into_iter()
                .map(|b| {
                    json!({
                        "name": b.engine.slug(),
                        "path": b.executable_path,
                        "protocol": format!("{:?}", b.protocol)
                    })
                })
                .collect();
            (200, Value::Array(list))
        }
        ("POST", "/api/files/upload") => {
            let name = parsed
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
                return (
                    400,
                    json!({ "ok": false, "error": "invalid single-segment upload filename" }),
                );
            }
            let st = handler.state.lock().expect("state mutex");
            match st.uploads_jail.resolve_and_confine(name) {
                Ok(canon) => (200, json!({ "ok": true, "name": name, "safe_path": canon })),
                Err(e) => (400, json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("POST", "/api/agents") => {
            let list: Vec<AgentProfileDto> = parsed
                .get("agents")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            if list.is_empty() {
                return (
                    400,
                    json!({ "ok": false, "error": "agents array must not be empty" }),
                );
            }
            let snap = {
                let mut st = handler.state.lock().expect("state mutex");
                let (mut next, _) = super::agent_roster::reconcile_agents(list);
                let agents_dir = st.agents_dir.clone();
                super::agent_roster::discover_custom_agents(&agents_dir, &mut next);
                st.agents = next;
                persist_json(&st.data_dir.join("agents.json"), &st.agents);
                st.snapshot_json()
            };
            handler.broadcast_state(&snap);
            (200, json!({ "ok": true }))
        }
        ("GET", "/api/providers/models") => super::model_sync::handle_get_provider_models(handler),
        ("POST", "/api/providers/models/sync") => {
            super::model_sync::handle_sync_provider_models(handler, &parsed)
        }
        ("GET", "/api/tts/voices") => super::neural_tts::handle_tts_voices(handler),
        ("POST", "/api/tts/speak") => super::neural_tts::handle_tts_speak(handler, &parsed),
        _ => {
            if let Some(resp) = try_dispatch_extended_api(handler, method, path, &parsed) {
                resp
            } else {
                (404, json!({ "ok": false, "error": "unknown api route" }))
            }
        }
    }
}
