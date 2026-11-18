use std::fs;
use std::path::PathBuf;

use miku_gatekeeper::{Capability, GatekeeperVerdict};
use serde_json::{json, Value};

use super::api_handler::LiveMikuApiHandler;
use super::llm_client::{execute_llm_chat, LlmRequestConfig};
use super::runtime_state::persist_json;
use super::storage_actor::{now_epoch_ms, McpServerEntry};

struct McpCatalogEntry {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    command: &'static str,
    args: &'static [&'static str],
    tag: &'static str,
}

const MCP_CATALOG: &[McpCatalogEntry] = &[
    McpCatalogEntry {
        id: "filesystem",
        name: "Filesystem (Local)",
        description: "Lectura y edición segura de archivos dentro de directorios permitidos.",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-filesystem", "."],
        tag: "archivos",
    },
    McpCatalogEntry {
        id: "fetch",
        name: "Fetch / Web",
        description: "Descarga y conversión limpia de páginas web a Markdown.",
        command: "uvx",
        args: &["mcp-server-fetch"],
        tag: "red",
    },
    McpCatalogEntry {
        id: "memory",
        name: "Knowledge Graph",
        description: "Grafo de memoria persistente complementario a SQLite FTS5.",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-memory"],
        tag: "memoria",
    },
    McpCatalogEntry {
        id: "git",
        name: "Git Repository",
        description: "Inspección de diffs, commits y ramas en repositorios locales.",
        command: "uvx",
        args: &["mcp-server-git"],
        tag: "código",
    },
    McpCatalogEntry {
        id: "sqlite",
        name: "SQLite Query",
        description: "Consultas y análisis sobre bases de datos SQLite locales.",
        command: "uvx",
        args: &["mcp-server-sqlite"],
        tag: "datos",
    },
    McpCatalogEntry {
        id: "sequential-thinking",
        name: "Sequential Thinking",
        description: "Descomposición estructurada por pasos para tareas complejas.",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-sequential-thinking"],
        tag: "razonamiento",
    },
];

pub(super) fn try_dispatch_extended_api(
    handler: &LiveMikuApiHandler,
    method: &str,
    path: &str,
    parsed: &Value,
) -> Option<(u16, Value)> {
    match (method, path) {
        ("POST", "/api/agents/delegate") => Some(handle_delegate(handler, parsed)),
        ("POST", "/api/ui/choice") => Some(handle_ui_choice(handler, parsed)),
        ("GET", "/api/audio/devices")
        | ("GET", "/api/media/now_playing")
        | ("POST", "/api/media/control") => {
            super::media_routes::try_dispatch_media_api(handler, method, path, parsed)
        }
        ("GET", "/api/mcp/catalog") => Some(handle_mcp_catalog(handler)),
        ("POST", "/api/mcp/catalog/install") => Some(handle_mcp_catalog_install(handler, parsed)),
        ("GET", "/api/sounds/list") => Some(handle_sounds_list()),
        ("POST", "/api/ui/open_settings") => Some(handle_open_settings(handler, parsed)),
        ("POST", "/api/gatekeeper/mode") => Some(handle_gatekeeper_mode(handler, parsed)),
        ("GET", "/api/telemetry/usage") => Some(handle_telemetry_usage(handler)),
        ("POST", "/api/telemetry/reset") => Some(handle_telemetry_reset(handler)),
        ("POST", "/api/settings/reset") => Some(handle_settings_reset(handler)),
        ("POST", "/api/agent/state") => Some(handle_post_agent_state(handler, parsed)),
        ("POST", "/api/voice/transcript") => Some(handle_post_voice_transcript(handler, parsed)),
        _ => super::harness_routes::try_dispatch_harness_api(handler, method, path, parsed),
    }
}

fn handle_open_settings(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let section = parsed
        .get("section")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    handler.request_open_settings(section.clone());
    (200, json!({ "ok": true, "section": section }))
}

fn handle_delegate(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let from_agent = parsed
        .get("from_agent")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("miku");
    let to_agent = parsed
        .get("to_agent")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("miku");
    let task = parsed
        .get("task")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if task.is_empty() {
        return (400, json!({ "ok": false, "error": "task required" }));
    }
    let mode = parsed
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("code");
    let now_ms = now_epoch_ms();

    let (seq, verdict, target_name, provider, model, base_url, api_key) = {
        let mut st = handler.state.lock().expect("state mutex");
        let seq = st.next_seq;
        st.next_seq = st.next_seq.saturating_add(2);
        let verdict = st.gatekeeper.evaluate_request(
            from_agent,
            Capability::Network,
            "agent.delegate",
            task,
            now_ms,
        );
        let target_name = st
            .agents
            .iter()
            .find(|a| a.id == to_agent)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| to_agent.to_string());
        let provider = st
            .settings
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("openai")
            .to_string();
        let model = st
            .settings
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("gpt-4o-mini")
            .to_string();
        let base_url = st
            .settings
            .get("custom_base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let api_key = st
            .resolve_chat_credential(&provider)
            .map(|(_, k)| k)
            .unwrap_or_default();
        (seq, verdict, target_name, provider, model, base_url, api_key)
    };

    if let GatekeeperVerdict::Deny { detail, .. } = verdict {
        let err = json!({
            "type": "error_card",
            "title": "Delegación bloqueada por Gatekeeper",
            "exit_code": 403,
            "stderr": detail,
            "source": from_agent
        });
        let _ = handler.events_tx.send(err);
        return (403, json!({ "ok": false, "error": detail }));
    }

    let task_id = format!("task-{}", seq);
    let session_id = format!("ui-{}", to_agent);
    let docs = handler
        .storage
        .record_turn(
            &session_id,
            "user",
            &format!("[Delegado por {} | {}] {}", from_agent, mode, task),
            seq,
            now_ms,
        )
        .unwrap_or(0);

    let phases_init = json!([
        { "label": "Contexto SQLite", "state": "done" },
        { "label": format!("Agente {}", target_name), "state": "active" },
        { "label": "Validación Gatekeeper", "state": "pending" }
    ]);
    let _ = handler.events_tx.send(json!({
        "type": "agent_work",
        "task_id": task_id,
        "agent_id": to_agent,
        "agent_name": target_name,
        "from_agent": from_agent,
        "mode": mode,
        "title": task,
        "status": "running",
        "phases": phases_init
    }));

    let query_tok = task
        .split(|c: char| !c.is_alphanumeric())
        .find(|w| w.len() >= 2)
        .unwrap_or("miku");
    let hits = handler.storage.search_fts5(query_tok, 4).unwrap_or_default();

    let result_text = if !api_key.is_empty() || provider.eq_ignore_ascii_case("ollama") {
        let _ = handler.events_tx.send(json!({
            "type": "ai_inference",
            "provider": provider,
            "model": model,
            "status": "running",
            "task_id": task_id
        }));
        let sys = format!(
            "Eres el subagente {} de Miku (modo={}). Responde de forma concisa y accionable en español.",
            target_name, mode
        );
        match execute_llm_chat(&LlmRequestConfig {
            provider: provider.clone(),
            model: model.clone(),
            api_key,
            base_url,
            prompt: task.to_string(),
            system_prompt: sys,
            attachment: None,
        }) {
            Ok(resp) => {
                let _ = handler.events_tx.send(json!({
                    "type": "ai_inference",
                    "provider": provider,
                    "model": model,
                    "status": "done",
                    "latency_ms": resp.latency_ms,
                    "input_tokens": resp.input_tokens,
                    "output_tokens": resp.output_tokens,
                    "host": resp.endpoint_host
                }));
                resp.text
            }
            Err(err) => {
                let _ = handler.events_tx.send(json!({
                    "type": "error_card",
                    "title": format!("Error LLM ({}/{})", provider, model),
                    "exit_code": err.status_code.unwrap_or(502),
                    "stderr": err.message,
                    "source": to_agent
                }));
                return (
                    502,
                    json!({ "ok": false, "task_id": task_id, "error": err.message }),
                );
            }
        }
    } else {
        let hit_summary = if hits.is_empty() {
            "Sin coincidencias previas en memoria FTS5.".to_string()
        } else {
            hits.iter()
                .map(|h| format!("- [{}] {}", h.title, h.snippet.chars().take(90).collect::<String>()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "Tarea delegada a {} en SQLite WAL ({} docs).\nModo: {}\nContexto FTS5 ('{}'):\n{}\nConfigura una API key en Ajustes > API Keys o selecciona Ollama local para ejecución generativa completa.",
            target_name, docs, mode, query_tok, hit_summary
        )
    };

    let _ = handler.storage.record_turn(
        &session_id,
        "assistant",
        &result_text,
        seq.saturating_add(1),
        now_epoch_ms(),
    );

    let done_evt = json!({
        "type": "agent_work",
        "task_id": task_id,
        "agent_id": to_agent,
        "agent_name": target_name,
        "from_agent": from_agent,
        "mode": mode,
        "title": task,
        "status": "done",
        "result": result_text,
        "phases": [
            { "label": "Contexto SQLite", "state": "done" },
            { "label": format!("Agente {}", target_name), "state": "done" },
            { "label": "Validación Gatekeeper", "state": "done" }
        ]
    });
    let _ = handler.events_tx.send(done_evt.clone());
    (200, json!({ "ok": true, "work": done_evt }))
}

fn handle_ui_choice(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let kind = parsed
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("question");
    let decision = parsed
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if decision.is_empty() {
        return (400, json!({ "ok": false, "error": "decision required" }));
    }
    let option_index = parsed.get("option_index").and_then(|v| v.as_u64());
    let detail = parsed.get("detail").and_then(|v| v.as_str()).unwrap_or("");
    let seq = {
        let mut st = handler.state.lock().expect("state mutex");
        let s = st.next_seq;
        st.next_seq = st.next_seq.saturating_add(1);
        s
    };
    let _ = handler.storage.record_turn(
        "ui-audit",
        "system",
        &format!("choice kind={} decision={} detail={}", kind, decision, detail),
        seq,
        now_epoch_ms(),
    );
    let evt = json!({
        "type": "choice_resolved",
        "kind": kind,
        "decision": decision,
        "option_index": option_index,
        "detail": detail
    });
    let _ = handler.events_tx.send(evt);
    (
        200,
        json!({
            "ok": true,
            "kind": kind,
            "decision": decision,
            "option_index": option_index
        }),
    )
}

fn handle_mcp_catalog(handler: &LiveMikuApiHandler) -> (u16, Value) {
    let st = handler.state.lock().expect("state mutex");
    let catalog: Vec<Value> = MCP_CATALOG
        .iter()
        .map(|entry| {
            let installed = st.mcp_servers.iter().any(|m| m.name == entry.id);
            json!({
                "id": entry.id,
                "name": entry.name,
                "description": entry.description,
                "command": entry.command,
                "args": entry.args,
                "tag": entry.tag,
                "installed": installed
            })
        })
        .collect();
    (200, json!({ "ok": true, "catalog": catalog }))
}

fn handle_mcp_catalog_install(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let id = parsed
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let Some(entry) = MCP_CATALOG.iter().find(|e| e.id == id) else {
        return (404, json!({ "ok": false, "error": "catalog entry not found" }));
    };

    let _ = handler.events_tx.send(json!({
        "type": "mcp_download",
        "id": entry.id,
        "name": entry.name,
        "progress": 35,
        "status": "configuring"
    }));

    let snap = {
        let mut st = handler.state.lock().expect("state mutex");
        st.mcp_servers.retain(|m| m.name != entry.id);
        st.mcp_servers.push(McpServerEntry {
            name: entry.id.to_string(),
            command: entry.command.to_string(),
            args: entry.args.iter().map(|s| s.to_string()).collect(),
        });
        persist_json(&st.data_dir.join("mcp.json"), &st.mcp_servers);
        st.snapshot_json()
    };

    let _ = handler.events_tx.send(json!({
        "type": "mcp_download",
        "id": entry.id,
        "name": entry.name,
        "progress": 100,
        "status": "installed"
    }));
    handler.broadcast_state(&snap);
    (200, json!({ "ok": true, "installed": entry.id }))
}

fn handle_sounds_list() -> (u16, Value) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(local_app) = std::env::var("LOCALAPPDATA") {
        let ui_dir = PathBuf::from(local_app).join("Miku").join("ui");
        dirs.push(ui_dir.join("shared").join("audio").join("sounds"));
        dirs.push(ui_dir.join("audio").join("sounds"));
    }
    let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("ui")
        .join("shared")
        .join("audio")
        .join("sounds");
    dirs.push(bundled);

    let mut files = Vec::new();
    let mut manifest = Value::Null;
    for d in dirs {
        if !d.exists() {
            continue;
        }
        if manifest.is_null() {
            if let Ok(raw) = fs::read_to_string(d.join("manifest.json")) {
                if let Ok(val) = serde_json::from_str::<Value>(&raw) {
                    manifest = val;
                }
            }
        }
        if let Ok(rd) = fs::read_dir(&d) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                        if name != "manifest.json" && !files.iter().any(|x| x == name) {
                            files.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    (200, json!({ "ok": true, "manifest": manifest, "files": files }))
}

fn handle_gatekeeper_mode(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let raw_mode = parsed.get("mode").and_then(|v| v.as_str()).unwrap_or("");
    let (applied, rules, ks, snap) = {
        let mut st = handler.state.lock().expect("state mutex");
        match st.apply_permission_mode(raw_mode) {
            Ok(mode) => (
                mode,
                st.rules_json(),
                st.gatekeeper.is_kill_switch_active(),
                st.snapshot_json(),
            ),
            Err(e) => return (400, json!({ "ok": false, "error": e })),
        }
    };
    handler.broadcast_state(&snap);
    (
        200,
        json!({
            "ok": true,
            "permission_mode": applied,
            "kill_switch": ks,
            "rules": rules
        }),
    )
}

fn handle_telemetry_usage(handler: &LiveMikuApiHandler) -> (u16, Value) {
    let st = handler.state.lock().expect("state mutex");
    (200, st.telemetry_snapshot())
}

fn handle_telemetry_reset(handler: &LiveMikuApiHandler) -> (u16, Value) {
    let snap = {
        let mut st = handler.state.lock().expect("state mutex");
        st.reset_telemetry();
        st.snapshot_json()
    };
    handler.broadcast_state(&snap);
    (200, json!({ "ok": true }))
}

fn handle_settings_reset(handler: &LiveMikuApiHandler) -> (u16, Value) {
    let snap = {
        let mut st = handler.state.lock().expect("state mutex");
        st.settings.clear();
        let mode = st.permission_mode.clone();
        st.settings.insert("permission_mode".to_string(), json!(mode));
        st.settings.insert("lang".to_string(), json!("es"));
        st.settings.insert("autoclose".to_string(), json!(15));
        st.settings.insert("voice".to_string(), json!(1));
        st.settings.insert("sfx_enabled".to_string(), json!(1));
        st.settings.insert("voice_volume".to_string(), json!(80));
        st.settings.insert("sfx_volume".to_string(), json!(60));
        st.settings.insert("hover_open".to_string(), json!(0));
        st.settings.insert("voice_stt_enabled".to_string(), json!(false));
        st.settings.insert("voice_stt_engine".to_string(), json!("off"));
        st.settings.insert("voice_auto_send".to_string(), json!(false));
        miku_ui::webview::set_webview_mic_permission_allowed(false);
        persist_json(&st.data_dir.join("settings.json"), &st.settings);
        st.snapshot_json()
    };
    handler.broadcast_state(&snap);
    (200, json!({ "ok": true }))
}

fn handle_post_agent_state(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let state = parsed
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("idle");
    handler.broadcast_agent_state(state);
    (200, json!({ "ok": true, "state": state }))
}

fn handle_post_voice_transcript(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let text = parsed.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let is_final = parsed.get("is_final").and_then(|v| v.as_bool()).unwrap_or(false);
    let engine = parsed.get("engine").and_then(|v| v.as_str()).unwrap_or("off");
    handler.broadcast_voice_transcript(text, is_final, engine);
    (200, json!({ "ok": true, "text": text, "is_final": is_final, "engine": engine }))
}
