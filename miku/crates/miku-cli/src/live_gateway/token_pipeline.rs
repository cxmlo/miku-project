use serde_json::{json, Value};

use miku_agent::token_saver::{LocalIntentRouter, LocalSystemSnapshot, RouteDecision};

use super::runtime_state::LiveRuntimeState;
use super::storage_actor::{now_epoch_ms, AgentProfileDto};

const EST_ROUTER_SAVED_TOKENS: u64 = 420;
const EST_PRUNED_TOOLS_SAVED_TOKENS: u64 = 480;

pub(super) struct AgentModelResolution {
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key_opt: Option<String>,
    pub fallback_used: bool,
}

/// Evaluates `LocalIntentRouter` for 0-token casual greetings, status queries, or safe commands.
/// Returns `Some(reply_text, intent_id, tokens_saved)` when handled locally without calling an LLM.
pub(super) fn try_local_zero_token_route(
    st: &LiveRuntimeState,
    clean_text: &str,
    has_attachment: bool,
    global_model_display: &str,
) -> Option<(String, String, u64)> {
    let enabled = st
        .settings
        .get("token_saver_local_router")
        .map(|v| match v {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(n) => n.as_i64().unwrap_or(1) != 0,
            _ => true,
        })
        .unwrap_or(true);
    if !enabled || has_attachment {
        return None;
    }

    let limit_usd = setting_f64(&st.settings, "daily_budget_usd", 5.0).max(0.0);
    let snap = LocalSystemSnapshot {
        project_id: "miku-desktop".to_string(),
        current_time_iso: format_epoch_utc_iso(now_epoch_ms()),
        power_mode: st.permission_mode.clone(),
        battery_percent: 100,
        active_agents_count: st.agents.iter().filter(|a| a.active).count().max(1),
        active_model_display: global_model_display.to_string(),
        daily_spent_micros: (st.telemetry.daily_usd_spent * 1_000_000.0).round() as u64,
        daily_limit_micros: (limit_usd * 1_000_000.0).round() as u64,
    };

    match LocalIntentRouter::classify_and_route(clean_text, false, &snap) {
        RouteDecision::LocalZeroToken(m) => {
            Some((m.reply, m.intent_id, EST_ROUTER_SAVED_TOKENS))
        }
        RouteDecision::ForwardToLlm { .. } => None,
    }
}

/// Resolves effective `(provider, model, api_key, base_url)` giving priority to per-agent
/// customization while safely falling back to global settings if the agent's preferred
/// provider has no configured API key yet.
pub(super) fn resolve_agent_model_and_key(
    st: &mut LiveRuntimeState,
    agent_opt: Option<&AgentProfileDto>,
    global_provider: &str,
    global_model: &str,
    global_base_url: Option<String>,
) -> AgentModelResolution {
    let raw_spec = agent_opt
        .map(|a| a.model.trim())
        .unwrap_or("")
        .to_string();
    let explicit_prov = agent_opt
        .map(|a| a.provider.trim().to_ascii_lowercase())
        .unwrap_or_default();

    let model_is_auto = raw_spec.is_empty()
        || raw_spec.eq_ignore_ascii_case("auto")
        || raw_spec.eq_ignore_ascii_case("miku-local")
        || raw_spec.eq_ignore_ascii_case("custom-agent");

    if model_is_auto && explicit_prov.is_empty() {
        let api_key_opt = st.resolve_chat_credential(global_provider).map(|(_, k)| k);
        return AgentModelResolution {
            provider: global_provider.to_string(),
            model: global_model.to_string(),
            base_url: global_base_url,
            api_key_opt,
            fallback_used: false,
        };
    }

    let (pref_prov, pref_model) = if let Some((p, m)) = raw_spec.split_once('/') {
        (p.trim().to_ascii_lowercase(), m.trim().to_string())
    } else if !explicit_prov.is_empty() {
        let default_m = if !model_is_auto {
            raw_spec
        } else if explicit_prov.eq_ignore_ascii_case(global_provider) {
            global_model.to_string()
        } else {
            default_model_for_provider(&explicit_prov).to_string()
        };
        (explicit_prov, default_m)
    } else {
        (infer_provider_from_model(&raw_spec, global_provider), raw_spec)
    };

    if pref_prov.eq_ignore_ascii_case(global_provider) {
        let api_key_opt = st.resolve_chat_credential(&pref_prov).map(|(_, k)| k);
        return AgentModelResolution {
            provider: pref_prov,
            model: pref_model,
            base_url: global_base_url,
            api_key_opt,
            fallback_used: false,
        };
    }

    if pref_prov == "ollama" {
        return AgentModelResolution {
            provider: "ollama".to_string(),
            model: if pref_model.is_empty() {
                "qwen2.5-coder:7b".to_string()
            } else {
                pref_model
            },
            base_url: None,
            api_key_opt: None,
            fallback_used: false,
        };
    }

    // Check if an explicit key for `pref_prov` exists in vault before overriding away from global.
    let has_specific_key = st
        .secret_aliases
        .iter()
        .any(|(alias, meta)| {
            meta.provider.eq_ignore_ascii_case(&pref_prov)
                || alias.eq_ignore_ascii_case(&pref_prov)
        })
        || st
            .secrets
            .list_summaries()
            .iter()
            .any(|s| s.provider.eq_ignore_ascii_case(&pref_prov) || s.alias.eq_ignore_ascii_case(&pref_prov));

    if has_specific_key {
        if let Some((_, key)) = st.resolve_chat_credential(&pref_prov) {
            return AgentModelResolution {
                provider: pref_prov,
                model: pref_model,
                base_url: None,
                api_key_opt: Some(key),
                fallback_used: false,
            };
        }
    }

    let api_key_opt = st.resolve_chat_credential(global_provider).map(|(_, k)| k);
    AgentModelResolution {
        provider: global_provider.to_string(),
        model: global_model.to_string(),
        base_url: global_base_url,
        api_key_opt,
        fallback_used: true,
    }
}

fn default_model_for_provider(provider: &str) -> &'static str {
    match provider.trim().to_ascii_lowercase().as_str() {
        "anthropic" => "claude-haiku-4-5",
        "gemini" => "gemini-2.5-flash",
        "openai" => "gpt-4o-mini",
        "ollama" => "qwen2.5-coder:7b",
        _ => "deepseek-chat",
    }
}

fn infer_provider_from_model(model: &str, fallback: &str) -> String {
    let m = model.trim().to_ascii_lowercase();
    if m.starts_with("deepseek") {
        "deepseek".to_string()
    } else if m.starts_with("claude") {
        "anthropic".to_string()
    } else if m.starts_with("gemini") {
        "gemini".to_string()
    } else if m.starts_with("gpt-") || m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        "openai".to_string()
    } else if m.starts_with("qwen") || m.starts_with("llama") || m.starts_with("mistral") {
        "ollama".to_string()
    } else {
        fallback.to_string()
    }
}

/// Builds a strictly STATIC KV-cacheable system prompt containing ONLY agent identity
/// and custom persona (`prompt`) — zero dynamic timestamps, file names, or FTS5 results.
pub(super) fn build_static_system_prompt(
    agent_name: &str,
    agent_role: &str,
    custom_soul: &str,
    prefix_cache_enabled: bool,
) -> String {
    let persona = custom_soul.trim();
    let persona_block = if persona.is_empty() {
        String::new()
    } else {
        let clamped: String = persona.chars().take(1200).collect();
        format!("\nDirectiva del agente: {}", clamped)
    };
    let anchor = if prefix_cache_enabled {
        "[MIKU-SYS-V1] "
    } else {
        ""
    };
    format!(
        "{}Eres {}, {} en Miku Desktop Orchestrator. Responde en español claro, preciso y conciso. Usa herramientas reales solo cuando haga falta consultar o actuar en el equipo y nunca inventes resultados.{}",
        anchor, agent_name, agent_role, persona_block
    )
}

/// Picks a high-signal FTS5 search query by skipping common Spanish/English stopwords.
pub(super) fn extract_fts_query(prompt: &str) -> String {
    const STOPWORDS: &[&str] = &[
        "que", "qué", "como", "cómo", "para", "por", "con", "sin", "una", "uno", "unos", "unas",
        "los", "las", "del", "al", "el", "la", "en", "de", "y", "o", "es", "son", "esta", "está",
        "este", "esto", "esa", "ese", "eso", "sobre", "hola", "miku", "favor", "puedes", "puedo",
        "quiero", "dime", "haz", "hazme", "ayuda", "hay", "the", "and", "for", "with", "this",
        "that", "from", "what", "how", "can", "you", "please", "help", "are", "was", "have",
    ];
    let terms: Vec<String> = prompt
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .map(|w| w.trim().to_ascii_lowercase())
        .filter(|w| w.len() >= 3 && !STOPWORDS.contains(&w.as_str()))
        .take(2)
        .collect();
    if terms.is_empty() {
        prompt
            .split(|c: char| !c.is_alphanumeric())
            .find(|w| w.len() >= 2)
            .unwrap_or("miku")
            .to_string()
    } else {
        terms.join(" ")
    }
}

/// Prepends dynamic SQLite FTS5 hits and attached file name into the user turn rather than
/// `system_prompt` so provider KV prefix caching stays warm across turns.
pub(super) fn build_dynamic_user_turn(
    clean_text: &str,
    hits: &[miku_storage::Fts5SearchHit],
    file_name: Option<&str>,
) -> String {
    let mut prefix = String::new();
    if !hits.is_empty() {
        let snippets: Vec<String> = hits
            .iter()
            .take(3)
            .map(|h| {
                let compact: String = h.snippet.split_whitespace().collect::<Vec<_>>().join(" ");
                let cut: String = compact.chars().take(100).collect();
                format!("[{}] {}", h.title, cut)
            })
            .collect();
        prefix.push_str(&format!("[Memoria FTS5: {}]\n", snippets.join(" | ")));
    }
    if let Some(f) = file_name {
        prefix.push_str(&format!("[Archivo adjunto: {}]\n", f));
    }
    if prefix.is_empty() {
        clean_text.to_string()
    } else {
        format!("{}{}", prefix, clean_text)
    }
}

/// Determines whether tool JSON schemas should accompany this request (`~480` tokens saved
/// when omitted on pure conversational / coding Q&A prompts).
pub(super) fn prompt_needs_tools(
    prompt: &str,
    tools_mode: &str,
    prune_tools_enabled: bool,
) -> (bool, u64) {
    match tools_mode.trim().to_ascii_lowercase().as_str() {
        "always" => return (true, 0),
        "never" => return (false, EST_PRUNED_TOOLS_SAVED_TOKENS),
        _ => {}
    }
    if !prune_tools_enabled {
        return (true, 0);
    }
    let lower = prompt.to_ascii_lowercase();
    let file_or_path_hint = lower.contains('\\')
        || lower.contains('/')
        || [".rs", ".js", ".ts", ".py", ".json", ".toml", ".md", ".kt", ".gradle"]
            .iter()
            .any(|ext| lower.contains(ext));
    if file_or_path_hint {
        return (true, 0);
    }
    const ACTION_MARKERS: &[&str] = &[
        "archivo", "carpeta", "directorio", "lee ", "leer ", "escribe", "guardar", "crea ",
        "edita", "modifica", "lista ", "listar", "ejecuta", "comando", "terminal", "shell",
        "powershell", "cmd", "git ", "cargo", "npm", "gradle", "memoria", "fts5", "recuerda",
        "busca ", "buscar", "navegador", "chrome", "edge", "mcp", "skill", "portapapeles",
        "clipboard", "musica", "música", "volumen", "mute", "silencia", "pausa", "reproduc",
        "siguiente", "cancion", "canción", "sistema", "diagnostico", "diagnóstico", "hardware",
        "estado pc", "read ", "write ", "list ", "run ", "search ", "file ", "folder",
        "herramienta", "tool", "edicion", "refactor", "git", "abrir", "abre", "inicia",
        "iniciar", "lanza", "lanzar", "open ", "launch ", "spotify", "chrome", "edge",
        "pantalla", "screen", "captura", "screenshot", "analiza", "analizar", "veo", "mira",
        "clic", "click", "mouse", "raton", "ratón", "tecla", "teclado", "atajo", "shortcut",
        "ventana", "interfaz", "boton", "botón", "uia", "resum", "navegador", "browser",
        "whatsapp", "youtube", "aplicacion", "aplicación", "web ", "pagina", "página",
    ];
    if ACTION_MARKERS.iter().any(|m| lower.contains(m)) {
        (true, 0)
    } else {
        (false, EST_PRUNED_TOOLS_SAVED_TOKENS)
    }
}

pub(super) fn is_daily_budget_exceeded(st: &LiveRuntimeState, provider: &str) -> Option<(f64, f64)> {
    if provider.eq_ignore_ascii_case("ollama") {
        return None;
    }
    let limit = setting_f64(&st.settings, "daily_budget_usd", 5.0);
    if limit <= 0.0 {
        return None;
    }
    let spent = if st.telemetry.daily_day_index == now_epoch_ms() / 86_400_000 {
        st.telemetry.daily_usd_spent
    } else {
        0.0
    };
    if spent >= limit {
        Some((spent, limit))
    } else {
        None
    }
}

pub(super) fn setting_bool(settings: &serde_json::Map<String, serde_json::Value>, key: &str, default: bool) -> bool {
    settings
        .get(key)
        .map(|v| match v {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(n) => n.as_i64().unwrap_or(if default { 1 } else { 0 }) != 0,
            serde_json::Value::String(s) => matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on" | "yes"),
            _ => default,
        })
        .unwrap_or(default)
}

pub(super) fn setting_u32(
    settings: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: u32,
    min: u32,
    max: u32,
) -> u32 {
    settings
        .get(key)
        .and_then(|v| v.as_u64())
        .map(|n| (n as u32).clamp(min, max))
        .unwrap_or(default)
}

fn setting_f64(settings: &serde_json::Map<String, serde_json::Value>, key: &str, default: f64) -> f64 {
    settings
        .get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)))
        .unwrap_or(default)
}

fn format_epoch_utc_iso(epoch_ms: u64) -> String {
    let total_secs = epoch_ms / 1_000;
    let secs_in_day = total_secs % 86_400;
    let h = secs_in_day / 3_600;
    let m = (secs_in_day % 3_600) / 60;
    let s = secs_in_day % 60;
    let days = (total_secs / 86_400) as i64;
    // Civil date from days since 1970-01-01 (Howard Hinnant algorithm).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = y + if mo <= 2 { 1 } else { 0 };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", yr, mo, d, h, m, s)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_llm_success_reply_json(
    reply_id: &str,
    agent_id: &str,
    model_str: &str,
    fallback_note: bool,
    outcome: super::agent_tools::AgentLoopOutcome,
    stored_docs: usize,
    fts_query: &str,
    pruned_saved: u64,
) -> (Value, String, u64) {
    let resp = outcome.chat;
    let cached_tok = outcome.cached_tokens;
    let mut steps = vec![
        format!(
            "Gatekeeper: OK ({}{})",
            model_str,
            if fallback_note { " · fallback global" } else { "" }
        ),
        format!(
            "LLM: {} ({} ms, {} rondas)",
            resp.endpoint_host, resp.latency_ms, outcome.rounds
        ),
        format!("SQLite WAL+FTS5: {} docs (query '{}')", stored_docs, fts_query),
    ];
    if pruned_saved > 0 {
        steps.push(format!("Token Saver: tools omitidas (~{} tok ahorrados)", pruned_saved));
    }
    if cached_tok > 0 {
        steps.push(format!("KV Prefix Cache hit: {} tokens", cached_tok));
    }
    steps.extend(outcome.tool_steps);
    if let (Some(i), Some(o)) = (resp.input_tokens, resp.output_tokens) {
        steps.push(format!("Tokens: {} in / {} out", i, o));
    }
    let reply = json!({
        "type": "agent_reply",
        "reply_id": reply_id,
        "agent_id": agent_id,
        "error": false,
        "text": resp.text,
        "steps": steps,
        "latency_ms": resp.latency_ms,
        "cached_tokens": cached_tok,
        "tokens_saved": pruned_saved
    });
    (reply, resp.text, cached_tok)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_offline_fallback_reply(
    reply_id: &str,
    agent_id: &str,
    agent_name: &str,
    seq: u64,
    stored_docs: usize,
    fts_query: &str,
    fts_hits: usize,
    file_name: Option<&str>,
    vault_aliases_count: usize,
    model_str: &str,
    skills_count: usize,
) -> (Value, String) {
    let file_note = file_name
        .map(|f| format!(" | archivo={}", f))
        .unwrap_or_default();
    let vault_note = if vault_aliases_count > 0 {
        format!("bóveda={} alias activos", vault_aliases_count)
    } else {
        "sin API key guardada aún en Ajustes > API Keys".to_string()
    };
    let reply_text = format!(
        "[{}] Turno #{} indexado en SQLite WAL+FTS5 (docs={}, hits='{}' -> {}){}. Gatekeeper OK | {} | modelo={} | skills={}.",
        agent_name, seq, stored_docs, fts_query, fts_hits, file_note, vault_note, model_str, skills_count
    );
    let reply = json!({
        "type": "agent_reply",
        "reply_id": reply_id,
        "agent_id": agent_id,
        "error": false,
        "text": reply_text,
        "steps": [
            format!("Gatekeeper: OK ({})", model_str),
            format!("SQLite WAL+FTS5: {} docs", stored_docs),
            format!("Recall '{}': {} coincidencias", fts_query, fts_hits)
        ]
    });
    (reply, reply_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live_gateway::agent_roster::{default_agents, reconcile_agents};

    #[test]
    fn reconcile_agents_repairs_broken_ids_and_invalid_outfits() {
        let mut stale = default_agents();
        if let Some(gh) = stale.iter_mut().find(|a| a.id == "github") {
            gh.id = "github-dead".to_string();
            gh.outfit = "street".to_string();
        }
        stale.retain(|a| a.id != "antigravity");

        let (reconciled, changed) = reconcile_agents(stale);
        assert!(changed);
        let gh = reconciled.iter().find(|a| a.id == "github").expect("github restored");
        assert_eq!(gh.outfit, "beanie");
        assert!(reconciled.iter().any(|a| a.id == "antigravity"));
        assert!(reconciled.iter().all(|a| a.id != "github-dead"));
    }

    #[test]
    fn static_prefix_is_deterministic_and_extracts_stopwords() {
        let s1 = build_static_system_prompt("Miku", "Orquestadora", "Sé directa.", true);
        let s2 = build_static_system_prompt("Miku", "Orquestadora", "Sé directa.", true);
        assert_eq!(s1, s2);
        assert!(s1.starts_with("[MIKU-SYS-V1] "));
        assert!(!s1.contains("2026"));

        let q = extract_fts_query("Hola Miku por favor dime cómo compilar rust gateway");
        assert_eq!(q, "compilar rust");
    }

    #[test]
    fn prompt_needs_tools_prunes_casual_and_enables_for_actions() {
        let (chat_tools, saved) = prompt_needs_tools("Explica qué es un préstamo en Rust", "auto", true);
        assert!(!chat_tools);
        assert_eq!(saved, 480);

        let (file_tools, file_saved) = prompt_needs_tools("Lee el archivo Cargo.toml del proyecto", "auto", true);
        assert!(file_tools);
        assert_eq!(file_saved, 0);

        let (never_tools, never_saved) = prompt_needs_tools("Lee Cargo.toml", "never", true);
        assert!(!never_tools);
        assert_eq!(never_saved, 480);

        let (always_tools, always_saved) = prompt_needs_tools("Hola", "always", true);
        assert!(always_tools);
        assert_eq!(always_saved, 0);
    }

    #[test]
    fn local_router_routes_casual_greetings_zero_token() {
        let dir = std::env::temp_dir().join(format!("miku_tok_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let st = LiveRuntimeState::new(&dir, 47400, true).expect("init state");
        let routed = try_local_zero_token_route(&st, "hola", false, "deepseek-chat");
        assert!(routed.is_some());
        let (reply, _intent, saved) = routed.unwrap();
        assert!(!reply.is_empty());
        assert_eq!(saved, 420);

        let non_routed = try_local_zero_token_route(&st, "Refactoriza el módulo de autenticación en Rust", false, "deepseek-chat");
        assert!(non_routed.is_none());
        drop(st);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
