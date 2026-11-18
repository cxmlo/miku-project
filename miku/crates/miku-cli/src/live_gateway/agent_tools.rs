//! Native OpenAI/DeepSeek function-calling agent loop wired to Miku's real
//! Gatekeeper, SQLite WAL+FTS5, PcControl, Skills, Browser detection & Media subsystems.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::agent_tool_exec::{execute_single_tool, truncate_chars};
use super::api_handler::LiveMikuApiHandler;
use super::llm_client::{
    extract_cached_tokens, extract_openai_message_text, load_sanitized_history,
    openai_compatible_endpoint, post_json_endpoint, redact_err, uses_max_completion_tokens,
    LlmCallError, LlmChatResponse,
};

const MAX_TOOL_ROUNDS: usize = 4;
const MAX_TOOL_RESULT_CHARS: usize = 1_500;
const MAX_HISTORY_CHARS: usize = 480;

pub(super) struct AgentLoopOutcome {
    pub chat: LlmChatResponse,
    pub cached_tokens: u64,
    pub tool_steps: Vec<String>,
    pub rounds: usize,
}

pub(super) fn supports_openai_tools(provider: &str) -> bool {
    matches!(
        provider.trim().to_ascii_lowercase().as_str(),
        "deepseek" | "openai" | "groq" | "custom" | "openai_compatible"
    )
}

fn compact_tool_definitions() -> Value {
    json!([
        tool_spec("system_info", "Estado de Miku, proveedor, skills, MCP y Gatekeeper.", json!({ "type": "object", "properties": {} })),
        tool_spec("memory_search", "Busca en memoria SQLite FTS5.", json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 5 }
            },
            "required": ["query"]
        })),
        tool_spec("skills_list", "Lista Skills activas y sus capacidades.", json!({ "type": "object", "properties": {} })),
        tool_spec("apps_detect", "Detecta navegadores locales y servidores MCP.", json!({ "type": "object", "properties": {} })),
        tool_spec("fs_list", "Lista archivos dentro del directorio del proyecto.", json!({
            "type": "object",
            "properties": { "path": { "type": "string" } }
        })),
        tool_spec("fs_read", "Lee un archivo dentro del directorio permitido.", json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        })),
        tool_spec("fs_write", "Escribe un archivo bajo políticas de Gatekeeper.", json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })),
        tool_spec("shell_exec", "Ejecuta un comando evaluado por Gatekeeper.", json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"]
        })),
        tool_spec("clipboard_read", "Lee portapapeles de Windows (requiere Gatekeeper).", json!({ "type": "object", "properties": {} })),
        tool_spec("clipboard_write", "Escribe texto en portapapeles (requiere Gatekeeper).", json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
        })),
        tool_spec("media_control", "Controla reproducción multimedia en Windows.", json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["play_pause", "next", "prev", "mute", "vol_down", "vol_up"]
                }
            },
            "required": ["action"]
        })),
        tool_spec("open_app_or_url", "Abre una aplicación instalada en Windows o un sitio web https:// (evaluado por Gatekeeper).", json!({
            "type": "object",
            "properties": {
                "target": { "type": "string", "description": "Nombre de la app (ej. 'chrome', 'notepad') o URL/dominio (ej. 'github.com', 'https://wikipedia.org')." }
            },
            "required": ["target"]
        })),
        tool_spec("screen_capture", "Captura bajo demanda la ventana activa o región en RAM (sin auto-oclusión de Miku, bloquea credenciales/UAC), extrae OCR local + árbol UIA y guarda CaptureTransform.", json!({
            "type": "object",
            "properties": {
                "x": { "type": "integer" },
                "y": { "type": "integer" },
                "width": { "type": "integer" },
                "height": { "type": "integer" }
            }
        })),
        tool_spec("ui_inspect", "Inspecciona controles accesibles UI Automation (profundidad<=3, max 40, timeout 600ms) con coordenadas normalizadas 0..1000.", json!({
            "type": "object",
            "properties": {
                "window_title": { "type": "string" }
            }
        })),
        tool_spec("ui_interact", "Invoca un control UI Automation (Button/Edit/MenuItem) por automation_id o nombre visible.", json!({
            "type": "object",
            "properties": {
                "window_title": { "type": "string" },
                "automation_id": { "type": "string" },
                "control_type": { "type": "string" }
            },
            "required": ["automation_id"]
        })),
        tool_spec("pc_input", "Controla mouse o teclado vía SendInput (x_norm/y_norm en 0..1000 traducidos por CaptureTransform, click, right_click, type_text o key_combo con liberación RAII).", json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["click", "right_click", "type_text", "key_combo"] },
                "x_norm": { "type": "number" },
                "y_norm": { "type": "number" },
                "text": { "type": "string" },
                "keys": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["action"]
        })),
        tool_spec("browser_navigate", "Valida esquema https:// anti-SSRF y abre URL autorizada en navegador.", json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" }
            },
            "required": ["url"]
        })),
        tool_spec("skill_run", "Autoriza por manifiesto + Gatekeeper y ejecuta una Skill cargada.", json!({
            "type": "object",
            "properties": {
                "skill_id": { "type": "string" },
                "target": { "type": "string" },
                "args": { "type": "string" },
                "capability": { "type": "string" }
            },
            "required": ["skill_id"]
        })),
        tool_spec("antigravity_projects", "Lista los proyectos reales registrados por Antigravity IDE en ~/.gemini/config/projects/.", json!({
            "type": "object",
            "properties": {}
        })),
        tool_spec("git_status", "Ejecuta `git status -sb` y últimos 5 commits en un repositorio local.", json!({
            "type": "object",
            "properties": {
                "repo_path": { "type": "string" }
            }
        })),
        tool_spec("git_diff", "Muestra el diff resumido (`git diff --stat` + fragmento) de un repositorio local.", json!({
            "type": "object",
            "properties": {
                "repo_path": { "type": "string" }
            }
        }))
    ])
}

fn tool_spec(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters
        }
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_openai_compatible_agent_loop(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    session_id: &str,
    current_seq: u64,
    provider: &str,
    model: &str,
    api_key: &str,
    base_url: Option<&str>,
    system_prompt: &str,
    user_prompt: &str,
    max_tokens: u32,
    tools_enabled: bool,
    history_limit: usize,
) -> Result<AgentLoopOutcome, LlmCallError> {
    let clean_key = api_key.trim();
    let endpoint = openai_compatible_endpoint(provider, base_url);
    let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
    if !clean_key.is_empty() {
        headers.push(("Authorization".to_string(), format!("Bearer {}", clean_key)));
    }

    let mut messages: Vec<Value> = vec![json!({ "role": "system", "content": system_prompt })];
    let prior = load_sanitized_history(
        &handler.storage,
        session_id,
        current_seq,
        history_limit,
        MAX_HISTORY_CHARS,
    );
    for (role, content) in prior {
        messages.push(json!({ "role": role, "content": content }));
    }
    messages.push(json!({ "role": "user", "content": user_prompt }));

    let reasoner_model = uses_max_completion_tokens(model);
    let effective_tools_enabled = tools_enabled && !reasoner_model;
    let tools = if effective_tools_enabled {
        Some(compact_tool_definitions())
    } else {
        None
    };
    let mut tool_steps: Vec<String> = Vec::new();
    let mut seen_calls: BTreeSet<String> = BTreeSet::new();
    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut total_cached: u64 = 0;
    let mut any_usage = false;
    let mut total_latency_ms: u64 = 0;

    let max_rounds = if effective_tools_enabled { MAX_TOOL_ROUNDS } else { 0 };
    for round in 0..=max_rounds {
        let allow_tools = effective_tools_enabled && round < MAX_TOOL_ROUNDS;
        let mut payload = json!({
            "model": model,
            "messages": messages
        });
        if reasoner_model {
            payload["max_completion_tokens"] = json!(max_tokens.clamp(64, 4096));
        } else {
            payload["max_tokens"] = json!(max_tokens.clamp(64, 4096));
        }
        if allow_tools {
            if let Some(t) = tools.as_ref() {
                payload["tools"] = t.clone();
                payload["tool_choice"] = json!("auto");
            }
        }

        let (_status, val, host, latency_ms) =
            post_json_endpoint(&endpoint, &headers, &payload, clean_key)?;
        total_latency_ms = total_latency_ms.saturating_add(latency_ms);
        total_cached = total_cached.saturating_add(extract_cached_tokens(provider, &val));

        if let Some(usage) = val.get("usage") {
            any_usage = true;
            total_in = total_in.saturating_add(
                usage
                    .get("prompt_tokens")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0),
            );
            total_out = total_out.saturating_add(
                usage
                    .get("completion_tokens")
                    .and_then(|n| n.as_u64())
                    .unwrap_or(0),
            );
        }

        let msg_val = val
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c0| c0.get("message"))
            .cloned()
            .ok_or_else(|| {
                redact_err(
                    Some(200),
                    format!("el proveedor {} devolvió respuesta sin message", provider),
                    api_key,
                )
            })?;

        let tool_calls = msg_val
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        if tool_calls.is_empty() || !allow_tools {
            let text = extract_openai_message_text(&msg_val).ok_or_else(|| {
                redact_err(
                    Some(200),
                    format!("el proveedor {} devolvió respuesta sin texto", provider),
                    clean_key,
                )
            })?;

            return Ok(AgentLoopOutcome {
                chat: LlmChatResponse {
                    text,
                    input_tokens: any_usage.then_some(total_in),
                    output_tokens: any_usage.then_some(total_out),
                    latency_ms: total_latency_ms,
                    endpoint_host: host,
                },
                cached_tokens: total_cached,
                tool_steps,
                rounds: round + 1,
            });
        }

        let assistant_content = msg_val
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let mut assistant_msg = json!({
            "role": "assistant",
            "content": assistant_content,
            "tool_calls": tool_calls
        });
        if let Some(rc) = msg_val.get("reasoning_content").and_then(|v| v.as_str()) {
            assistant_msg["reasoning_content"] = json!(rc);
        }
        messages.push(assistant_msg);

        for (idx, tc) in tool_calls.iter().enumerate() {
            let call_id = tc
                .get("id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("call_{}_{}", round, idx));
            let fn_name = tc
                .pointer("/function/name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let raw_args = tc
                .pointer("/function/arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            let args: Value = serde_json::from_str(raw_args).unwrap_or_else(|_| json!({}));

            let _ = handler.events_tx.send(json!({
                "type": "ai_inference",
                "provider": provider,
                "model": model,
                "status": "tool_call",
                "tool": fn_name,
                "agent_id": agent_id
            }));

            let dedup_key = format!("{}:{}", fn_name, raw_args.trim());
            let tool_out = if !seen_calls.insert(dedup_key) {
                json!({ "ok": false, "error": "llamada duplicada evitada en la misma respuesta" })
            } else {
                execute_single_tool(handler, agent_id, fn_name, &args)
            };

            let status_tag = if tool_out.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                "OK"
            } else {
                tool_out
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("ERR")
            };
            tool_steps.push(format!("Tool {} -> {}", fn_name, status_tag));

            let serialized = truncate_chars(
                &serde_json::to_string(&tool_out).unwrap_or_else(|_| "{}".to_string()),
                MAX_TOOL_RESULT_CHARS,
            );
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": serialized
            }));
        }
        if let Some(mut att) = handler.take_pending_multimodal_attachment() {
            let content = if super::llm_multimodal::model_supports_vision(provider, model) {
                super::llm_multimodal::build_openai_user_content(
                    provider,
                    model,
                    "Observa la captura de pantalla adjunta y usa coordenadas 0..1000 si interactúas con controles.",
                    Some(&att),
                )
            } else {
                json!(super::llm_multimodal::format_vision_fallback_text(
                    &att,
                    "Modelo solo texto: analiza el OCR local y el árbol UIA adjuntos."
                ))
            };
            att.zeroize();
            messages.push(json!({ "role": "user", "content": content }));
        }
    }

    Err(redact_err(
        Some(500),
        "límite de iteraciones de herramientas excedido".to_string(),
        api_key,
    ))
}
