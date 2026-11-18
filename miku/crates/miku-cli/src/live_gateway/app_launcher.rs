//! Gatekeeper-evaluated local launcher for websites and installed Windows applications.
//! Resolves direct launch commands ("abrir pinterest", "abre discord") with 0 LLM tokens,
//! and backs the `open_app_or_url` agent tool for multi-step prompts.

use std::env;

use miku_gatekeeper::{Capability, GatekeeperVerdict};
use serde_json::{json, Value};

use super::api_handler::LiveMikuApiHandler;
use super::app_catalog::{
    find_exe_on_path, find_known_windows_desktop_app, find_windows_start_menu_app,
    known_app_web_fallback, known_website_url,
};
use super::storage_actor::now_epoch_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ResolvedTarget {
    Url { label: String, url: String },
    AppPath { label: String, path: String, args: Option<String> },
    Protocol { label: String, uri: String },
}

impl ResolvedTarget {
    pub(super) fn gatekeeper_target(&self) -> String {
        match self {
            ResolvedTarget::Url { url, .. } => format!("open_url:{}", url),
            ResolvedTarget::AppPath { label, .. } | ResolvedTarget::Protocol { label, .. } => {
                format!("open_app:{}", slugify_label(label))
            }
        }
    }

    pub(super) fn display_target(&self) -> &str {
        match self {
            ResolvedTarget::Url { url, .. } => url.as_str(),
            ResolvedTarget::AppPath { path, .. } => path.as_str(),
            ResolvedTarget::Protocol { uri, .. } => uri.as_str(),
        }
    }

    pub(super) fn kind_str(&self) -> &'static str {
        match self {
            ResolvedTarget::Url { .. } => "url",
            ResolvedTarget::AppPath { .. } => "app",
            ResolvedTarget::Protocol { .. } => "protocol",
        }
    }

    pub(super) fn label(&self) -> &str {
        match self {
            ResolvedTarget::Url { label, .. }
            | ResolvedTarget::AppPath { label, .. }
            | ResolvedTarget::Protocol { label, .. } => label.as_str(),
        }
    }
}

pub(super) fn strip_es_accents(input: &str) -> String {
    input
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'Á' => 'a',
            'é' | 'è' | 'ë' | 'ê' | 'É' => 'e',
            'í' | 'ì' | 'ï' | 'î' | 'Í' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'Ó' => 'o',
            'ú' | 'ù' | 'ü' | 'û' | 'Ú' => 'u',
            'ñ' | 'Ñ' => 'n',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

fn slugify_label(label: &str) -> String {
    let s: String = strip_es_accents(label)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    s.trim_matches('-').to_string()
}

fn contains_shell_metachars(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            ';' | '&' | '|' | '`' | '$' | '<' | '>' | '\r' | '\n' | '\0'
        )
    })
}

/// Parses standalone launch commands like "abre pinterest", "abrir discord por favor",
/// or "open https://github.com". Returns `None` for multi-step prompts or non-launch text.
pub(super) fn parse_direct_launch_request(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches(|c| c == '"' || c == '\'' || c == '¿' || c == '?' || c == '¡' || c == '!');
    if trimmed.is_empty() || trimmed.len() > 140 || contains_shell_metachars(trimmed) {
        return None;
    }

    let lower = strip_es_accents(trimmed);
    const MULTI_STEP_MARKERS: &[&str] = &[
        " y luego ",
        " y despues ",
        " y busca ",
        " y dime ",
        " y haz ",
        " y crea ",
        " y escribe ",
        " and then ",
        " and search ",
        "\n",
    ];
    if MULTI_STEP_MARKERS.iter().any(|m| lower.contains(m)) {
        return None;
    }

    const PREFIXES: &[&str] = &[
        "por favor abre ",
        "porfa abre ",
        "puedes abrir ",
        "podrias abrir ",
        "quiero abrir ",
        "abrime ",
        "abre ",
        "abrir ",
        "inicia ",
        "iniciar ",
        "lanza ",
        "lanzar ",
        "arranca ",
        "arrancar ",
        "open ",
        "launch ",
        "start ",
    ];

    let mut matched_len = None;
    for prefix in PREFIXES {
        if lower.starts_with(prefix) {
            matched_len = Some(prefix.len());
            break;
        }
    }
    let start_idx = matched_len?;
    let mut rest_raw = trimmed.get(start_idx..)?.trim();

    let filler_prefixes = [
        "la pagina de ",
        "la página de ",
        "la web de ",
        "el sitio de ",
        "la aplicacion de ",
        "la aplicación de ",
        "la aplicacion ",
        "la aplicación ",
        "la app de ",
        "la app ",
        "el programa ",
        "el navegador ",
        "el ",
        "la ",
        "the ",
        "app ",
    ];
    for fp in filler_prefixes {
        let rest_lower = strip_es_accents(rest_raw);
        let fp_lower = strip_es_accents(fp);
        if rest_lower.starts_with(&fp_lower) && rest_raw.len() > fp.len() {
            rest_raw = rest_raw[fp.len()..].trim();
            break;
        }
    }

    let filler_suffixes = [
        " por favor",
        " porfa",
        " plis",
        " please",
        " en el navegador",
        " web",
    ];
    for fsuf in filler_suffixes {
        let rest_lower = strip_es_accents(rest_raw);
        if rest_lower.ends_with(fsuf) && rest_raw.len() > fsuf.len() {
            rest_raw = rest_raw[..rest_raw.len() - fsuf.len()].trim();
        }
    }

    let cleaned = rest_raw.trim_matches(|c| c == '"' || c == '\'' || c == '.' && !rest_raw.contains(".com") && !rest_raw.contains(".org") && !rest_raw.contains(".net") && !rest_raw.contains(".io") && !rest_raw.contains(".dev") && !rest_raw.contains(".ai"));
    if cleaned.is_empty() || cleaned.len() > 110 {
        return None;
    }
    Some(cleaned.to_string())
}

fn looks_like_domain(raw: &str) -> bool {
    if raw.contains(' ') || raw.contains('\\') || raw.contains("..") {
        return false;
    }
    let host_part = raw.split('/').next().unwrap_or("");
    let Some((_, tld)) = host_part.rsplit_once('.') else {
        return false;
    };
    !host_part.is_empty()
        && host_part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == ':')
        && tld.len() >= 2
        && tld.len() <= 12
        && tld.chars().all(|c| c.is_ascii_alphabetic())
}

pub(super) fn resolve_launch_target(raw_target: &str) -> Result<ResolvedTarget, String> {
    let cleaned = raw_target.trim().trim_matches(|c| c == '"' || c == '\'');
    if cleaned.is_empty() || contains_shell_metachars(cleaned) {
        return Err("Destino vacío o con caracteres no permitidos.".to_string());
    }
    let lower = strip_es_accents(cleaned);

    for forbidden in ["file:", "javascript:", "vbscript:", "data:", "ms-msdt:", "cmd:", "powershell:"] {
        if lower.starts_with(forbidden) {
            return Err(format!("Esquema '{}' bloqueado por Gatekeeper.", forbidden));
        }
    }

    if lower.starts_with("https://") {
        return Ok(ResolvedTarget::Url {
            label: cleaned.to_string(),
            url: cleaned.to_string(),
        });
    }
    if lower.starts_with("http://localhost") || lower.starts_with("http://127.0.0.1") {
        return Ok(ResolvedTarget::Url {
            label: cleaned.to_string(),
            url: cleaned.to_string(),
        });
    }
    if lower.starts_with("http://") {
        let upgraded = format!("https://{}", &cleaned[7..]);
        return Ok(ResolvedTarget::Url {
            label: upgraded.clone(),
            url: upgraded,
        });
    }

    if let Some((label, url)) = known_website_url(&lower) {
        return Ok(ResolvedTarget::Url {
            label: label.to_string(),
            url: url.to_string(),
        });
    }

    if let Some((label, lnk)) = find_windows_start_menu_app(&lower) {
        return Ok(ResolvedTarget::AppPath {
            label,
            path: lnk.to_string_lossy().to_string(),
            args: None,
        });
    }

    if let Some(desktop) = find_known_windows_desktop_app(&lower) {
        return Ok(desktop);
    }

    if let Some((label, url)) = known_app_web_fallback(&lower) {
        return Ok(ResolvedTarget::Url {
            label: format!("{} (Web)", label),
            url: url.to_string(),
        });
    }

    if looks_like_domain(cleaned) {
        let url = format!("https://{}", cleaned);
        return Ok(ResolvedTarget::Url {
            label: cleaned.to_string(),
            url,
        });
    }

    let exe_cand = if lower.ends_with(".exe") {
        cleaned.to_string()
    } else {
        format!("{}.exe", cleaned)
    };
    if !exe_cand.contains('\\') && !exe_cand.contains('/') {
        if let Some(found) = find_exe_on_path(&exe_cand) {
            return Ok(ResolvedTarget::AppPath {
                label: cleaned.to_string(),
                path: found.to_string_lossy().to_string(),
                args: None,
            });
        }
    }

    Err(format!(
        "No encontré la app ni el sitio '{}'. Prueba con su nombre completo o dominio (ej. 'pinterest.com').",
        cleaned
    ))
}

#[cfg(target_os = "windows")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        lp_operation: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show_cmd: i32,
    ) -> isize;
}

pub(super) fn execute_resolved_launch(
    resolved: &ResolvedTarget,
    simulate_only: bool,
) -> Result<bool, String> {
    if simulate_only || env::var_os("MIKU_SKIP_OS_LAUNCH").is_some() {
        return Ok(true);
    }
    #[cfg(target_os = "windows")]
    unsafe {
        let op: Vec<u16> = "open\0".encode_utf16().collect();
        let file: Vec<u16> = resolved
            .display_target()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let params_buf: Option<Vec<u16>> = match resolved {
            ResolvedTarget::AppPath {
                args: Some(a), ..
            } => Some(a.encode_utf16().chain(std::iter::once(0)).collect()),
            _ => None,
        };
        let params_ptr = params_buf
            .as_ref()
            .map(|b| b.as_ptr())
            .unwrap_or(std::ptr::null());
        let rc = ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            params_ptr,
            std::ptr::null(),
            1,
        );
        if rc > 32 {
            Ok(false)
        } else {
            Err(format!(
                "Windows ShellExecuteW devolvió código {} al abrir '{}'",
                rc,
                resolved.label()
            ))
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(true)
    }
}

/// Evaluates `Capability::Browser` in Gatekeeper and executes a verified website/app launch.
pub(super) fn launch_with_gatekeeper(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    raw_target: &str,
) -> Result<(ResolvedTarget, bool), Value> {
    let resolved = match resolve_launch_target(raw_target) {
        Ok(r) => r,
        Err(msg) => {
            return Err(json!({
                "ok": false,
                "status": "not_found",
                "error": msg
            }));
        }
    };

    let gk_target = resolved.gatekeeper_target();
    let now_ms = now_epoch_ms();
    let (verdict, simulate_only) = {
        let mut st = handler.state.lock().expect("state mutex");
        let v = st.gatekeeper.evaluate_request(
            agent_id,
            Capability::Browser,
            &gk_target,
            raw_target,
            now_ms,
        );
        (v, st.use_memory_secrets)
    };

    match verdict {
        GatekeeperVerdict::Allow { .. } => match execute_resolved_launch(&resolved, simulate_only) {
            Ok(simulated) => Ok((resolved, simulated)),
            Err(err) => Err(json!({
                "ok": false,
                "status": "launch_failed",
                "error": err
            })),
        },
        GatekeeperVerdict::AskPending { ticket, .. } => {
            let _ = handler.events_tx.send(json!({
                "type": "approve",
                "id": ticket.req_hash,
                "title": format!("Gatekeeper: Abrir {}", resolved.label()),
                "command": format!("browser: {}", ticket.canonical_target),
                "diff": format!("Agente: {}\nDestino: {}", agent_id, resolved.display_target()),
                "capability": "browser",
                "target": ticket.canonical_target,
                "agent_id": agent_id
            }));
            Err(json!({
                "ok": false,
                "status": "approval_required",
                "ticket": ticket.req_hash,
                "error": "Gatekeeper requiere aprobación manual del usuario"
            }))
        }
        GatekeeperVerdict::Deny { detail, reason, .. } => Err(json!({
            "ok": false,
            "status": "denied",
            "reason": reason.as_code(),
            "error": format!("Bloqueado por Gatekeeper: {}", detail)
        })),
    }
}

/// Handles single-intent launch commands ("abrir pinterest", "abre discord") with 0 LLM tokens.
pub(super) fn try_local_launch_command(
    handler: &LiveMikuApiHandler,
    agent_id: &str,
    clean_text: &str,
    reply_id: &str,
    seq: u64,
    session_id: &str,
) -> Option<Value> {
    let raw_target = parse_direct_launch_request(clean_text)?;
    let started = std::time::Instant::now();
    let saved_tok = 680u64;

    let reply = match launch_with_gatekeeper(handler, agent_id, &raw_target) {
        Ok((resolved, _simulated)) => {
            if let Ok(mut st) = handler.state.lock() {
                st.record_local_route(saved_tok);
            }
            let text = match &resolved {
                ResolvedTarget::Url { label, url } => {
                    format!("Abriendo {} en tu navegador ({}).", label, url)
                }
                ResolvedTarget::AppPath { label, .. } | ResolvedTarget::Protocol { label, .. } => {
                    format!("Abriendo {}.", label)
                }
            };
            let _ = handler.storage.record_turn(
                session_id,
                "assistant",
                &text,
                seq.saturating_add(1),
                now_epoch_ms(),
            );
            json!({
                "type": "agent_reply",
                "reply_id": reply_id,
                "agent_id": agent_id,
                "error": false,
                "routed_local": true,
                "tokens_saved": saved_tok,
                "text": text,
                "launch": {
                    "kind": resolved.kind_str(),
                    "label": resolved.label(),
                    "target": resolved.display_target()
                },
                "steps": [],
                "latency_ms": started.elapsed().as_millis() as u64
            })
        }
        Err(err_val) => {
            let err_msg = err_val
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("No se pudo abrir el destino solicitado.")
                .to_string();
            let status = err_val
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("error");
            json!({
                "type": "agent_reply",
                "reply_id": reply_id,
                "agent_id": agent_id,
                "error": status != "approval_required",
                "routed_local": true,
                "retry_prompt": clean_text,
                "text": err_msg,
                "steps": []
            })
        }
    };

    let _ = handler.events_tx.send(reply.clone());
    Some(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spanish_and_english_launch_commands() {
        assert_eq!(
            parse_direct_launch_request("abrir pinterest"),
            Some("pinterest".to_string())
        );
        assert_eq!(
            parse_direct_launch_request("abre la pagina de youtube por favor"),
            Some("youtube".to_string())
        );
        assert_eq!(
            parse_direct_launch_request("inicia discord"),
            Some("discord".to_string())
        );
        assert_eq!(
            parse_direct_launch_request("open https://github.com/rust-lang"),
            Some("https://github.com/rust-lang".to_string())
        );
        assert_eq!(
            parse_direct_launch_request("abre youtube y luego busca música lofi"),
            None
        );
        assert_eq!(parse_direct_launch_request("hola miku cómo estás"), None);
    }

    #[test]
    fn resolves_known_websites_domains_and_blocks_unsafe_schemes() {
        assert_eq!(
            resolve_launch_target("pinterest").unwrap(),
            ResolvedTarget::Url {
                label: "Pinterest".to_string(),
                url: "https://www.pinterest.com/".to_string()
            }
        );
        assert_eq!(
            resolve_launch_target("dribbble.com").unwrap(),
            ResolvedTarget::Url {
                label: "dribbble.com".to_string(),
                url: "https://dribbble.com".to_string()
            }
        );
        assert!(resolve_launch_target("file:///C:/Windows/win.ini").is_err());
        assert!(resolve_launch_target("javascript:alert(1)").is_err());
        assert!(resolve_launch_target("calc.exe; whoami").is_err());
    }
}
