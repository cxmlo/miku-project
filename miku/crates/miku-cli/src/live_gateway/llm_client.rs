use std::time::Instant;

use serde_json::{json, Value};

use super::winhttp_https::{parse_http_url, post_https, post_plain_http};

pub use super::llm_multimodal::LlmImageAttachment;

#[derive(Debug, Clone)]
pub struct LlmRequestConfig {
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub base_url: Option<String>,
    pub prompt: String,
    pub system_prompt: String,
    pub attachment: Option<LlmImageAttachment>,
}

#[derive(Debug, Clone)]
pub struct LlmChatResponse {
    pub text: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub latency_ms: u64,
    pub endpoint_host: String,
}

#[derive(Debug, Clone)]
pub(super) struct LlmChatResult {
    pub chat: LlmChatResponse,
    pub cached_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct LlmCallError {
    pub status_code: Option<u16>,
    pub message: String,
}

pub fn execute_llm_chat(cfg: &LlmRequestConfig) -> Result<LlmChatResponse, LlmCallError> {
    execute_llm_chat_with_context(cfg, &[], 1024).map(|r| r.chat)
}

pub(super) fn execute_llm_chat_with_context(
    cfg: &LlmRequestConfig,
    history: &[(String, String)],
    max_tokens: u32,
) -> Result<LlmChatResult, LlmCallError> {
    let (url, headers, payload_val) =
        build_provider_request_with_context(cfg, history, max_tokens.clamp(16, 4096));
    let (_status, val, host, latency_ms) =
        post_json_endpoint(&url, &headers, &payload_val, &cfg.api_key)?;

    let (text, input_tokens, output_tokens) = extract_provider_reply(&cfg.provider, &val)
        .ok_or_else(|| {
            redact_err(
                Some(200),
                format!("el proveedor {} devolvió respuesta sin texto", cfg.provider),
                &cfg.api_key,
            )
        })?;
    let cached_tokens = extract_cached_tokens(&cfg.provider, &val);

    Ok(LlmChatResult {
        chat: LlmChatResponse {
            text,
            input_tokens,
            output_tokens,
            latency_ms,
            endpoint_host: host,
        },
        cached_tokens,
    })
}

pub(super) fn probe_provider_key(
    provider: &str,
    model: &str,
    api_key: &str,
    base_url: Option<&str>,
) -> Result<(u64, String), LlmCallError> {
    let cfg = LlmRequestConfig {
        provider: provider.to_string(),
        model: model.to_string(),
        api_key: api_key.to_string(),
        base_url: base_url.map(|s| s.to_string()),
        prompt: "Responde OK".to_string(),
        system_prompt: "Responde solo OK.".to_string(),
        attachment: None,
    };
    let res = execute_llm_chat_with_context(&cfg, &[], 16)?;
    Ok((res.chat.latency_ms, res.chat.endpoint_host))
}

pub(super) fn post_json_endpoint(
    url: &str,
    headers: &[(String, String)],
    payload_val: &Value,
    secret: &str,
) -> Result<(u16, Value, String, u64), LlmCallError> {
    let started = Instant::now();
    let (is_https, host, port, path) =
        parse_http_url(url).map_err(|msg| redact_err(None, msg, secret))?;
    let body_bytes =
        serde_json::to_vec(payload_val).map_err(|e| redact_err(None, e.to_string(), secret))?;

    let (status, resp_bytes) = if is_https {
        post_https(&host, port, &path, headers, &body_bytes)
            .map_err(|msg| redact_err(None, msg, secret))?
    } else {
        post_plain_http(&host, port, &path, headers, &body_bytes)
            .map_err(|msg| redact_err(None, msg, secret))?
    };

    let latency_ms = started.elapsed().as_millis() as u64;
    let raw_str = String::from_utf8_lossy(&resp_bytes).to_string();
    let parsed: Option<Value> = serde_json::from_slice(&resp_bytes).ok();

    if !(200..300).contains(&status) {
        let err_detail = parsed
            .as_ref()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.get("message").or(Some(e)))
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| raw_str.chars().take(280).collect());
        return Err(redact_err(
            Some(status),
            format!("HTTP {} ({}): {}", status, host, err_detail),
            secret,
        ));
    }

    let val = parsed.ok_or_else(|| {
        redact_err(
            Some(status),
            format!("respuesta JSON inválida desde {}", host),
            secret,
        )
    })?;

    Ok((status, val, host, latency_ms))
}

pub(super) fn openai_compatible_endpoint(provider: &str, base_url: Option<&str>) -> String {
    let prov = provider.trim().to_ascii_lowercase();
    let custom_base = base_url.map(str::trim).filter(|s| !s.is_empty());
    match prov.as_str() {
        "deepseek" => custom_base
            .map(|b| {
                let t = b.trim_end_matches('/');
                if t.ends_with("/chat/completions") {
                    t.to_string()
                } else {
                    format!("{}/chat/completions", t)
                }
            })
            .unwrap_or_else(|| "https://api.deepseek.com/chat/completions".to_string()),
        _ => custom_base
            .map(|b| {
                let t = b.trim_end_matches('/');
                if t.ends_with("/chat/completions") {
                    t.to_string()
                } else {
                    format!("{}/v1/chat/completions", t)
                }
            })
            .unwrap_or_else(|| "https://api.openai.com/v1/chat/completions".to_string()),
    }
}

pub(super) fn redact_err(status_code: Option<u16>, message: String, secret: &str) -> LlmCallError {
    let cleaned = if !secret.is_empty() {
        message.replace(secret, "[REDACTED]")
    } else {
        message
    };
    LlmCallError {
        status_code,
        message: cleaned,
    }
}

pub(super) fn uses_max_completion_tokens(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4")
}

pub(super) fn normalize_chat_history(raw: &[(String, String)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (role, text) in raw {
        let trimmed = text.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Error llamando a ")
            || (trimmed.starts_with('[') && trimmed.contains("indexado en SQLite WAL+FTS5"))
        {
            continue;
        }
        let norm_role = if role == "assistant" { "assistant" } else { "user" };
        if out.is_empty() && norm_role == "assistant" {
            continue;
        }
        if let Some((last_role, last_text)) = out.last_mut() {
            if last_role == norm_role {
                if last_text != trimmed {
                    last_text.push('\n');
                    last_text.push_str(trimmed);
                }
                continue;
            }
        }
        out.push((norm_role.to_string(), trimmed.to_string()));
    }
    if out.last().map(|(r, _)| r.as_str()) == Some("user") {
        out.pop();
    }
    out
}

pub(super) fn load_sanitized_history(
    storage: &super::storage_actor::StorageActorHandle,
    session_id: &str,
    current_seq: u64,
    history_limit: usize,
    max_chars_per_turn: usize,
) -> Vec<(String, String)> {
    let capped = history_limit.min(12);
    if capped == 0 {
        return Vec::new();
    }
    let Ok(turns) = storage.list_recent_turns(session_id, capped + 6) else {
        return Vec::new();
    };
    let filtered: Vec<(String, String)> = turns
        .into_iter()
        .filter(|t| t.seq < current_seq)
        .map(|t| {
            let clipped: String = t.content.chars().take(max_chars_per_turn).collect();
            (t.role, clipped)
        })
        .collect();
    let normalized = normalize_chat_history(&filtered);
    let skip = normalized.len().saturating_sub(capped);
    normalized.into_iter().skip(skip).collect()
}

fn build_provider_request_with_context(
    cfg: &LlmRequestConfig,
    history: &[(String, String)],
    max_tokens: u32,
) -> (String, Vec<(String, String)>, Value) {
    let prov = cfg.provider.trim().to_ascii_lowercase();
    let custom_base = cfg
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let clean_history = normalize_chat_history(history);
    let turns: Vec<Value> = clean_history
        .iter()
        .map(|(role, content)| {
            json!({
                "role": if role == "assistant" { "assistant" } else { "user" },
                "content": content
            })
        })
        .collect();

    match prov.as_str() {
        "anthropic" => {
            let url = custom_base
                .map(|b| format!("{}/v1/messages", b.trim_end_matches('/')))
                .unwrap_or_else(|| "https://api.anthropic.com/v1/messages".to_string());
            let mut hdrs = vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ];
            if !cfg.api_key.is_empty() {
                hdrs.push(("x-api-key".to_string(), cfg.api_key.clone()));
            }
            let mut msg_turns = turns;
            msg_turns.push(json!({
                "role": "user",
                "content": super::llm_multimodal::build_anthropic_user_content(
                    &cfg.model,
                    &cfg.prompt,
                    cfg.attachment.as_ref(),
                )
            }));
            let body = json!({
                "model": cfg.model,
                "max_tokens": max_tokens,
                "system": cfg.system_prompt,
                "messages": msg_turns
            });
            (url, hdrs, body)
        }
        "gemini" => {
            let clean_model = cfg
                .model
                .trim()
                .trim_start_matches("models/")
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                .collect::<String>();
            let url = match custom_base {
                Some(b) => b.to_string(),
                None => format!(
                    "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
                    clean_model
                ),
            };
            let mut hdrs = vec![("Content-Type".to_string(), "application/json".to_string())];
            if !cfg.api_key.is_empty() {
                hdrs.push(("x-goog-api-key".to_string(), cfg.api_key.clone()));
            }
            let mut contents: Vec<Value> = history
                .iter()
                .map(|(role, content)| {
                    json!({
                        "role": if role == "assistant" { "model" } else { "user" },
                        "parts": [{ "text": content }]
                    })
                })
                .collect();
            contents.push(json!({
                "role": "user",
                "parts": super::llm_multimodal::build_gemini_user_parts(
                    &cfg.model,
                    &cfg.prompt,
                    cfg.attachment.as_ref(),
                )
            }));
            let body = json!({
                "system_instruction": { "parts": [{ "text": cfg.system_prompt }] },
                "generationConfig": { "maxOutputTokens": max_tokens },
                "contents": contents
            });
            (url, hdrs, body)
        }
        "ollama" => {
            let url = custom_base
                .map(|b| {
                    let trimmed = b.trim_end_matches('/');
                    if trimmed.ends_with("/api/chat") || trimmed.ends_with("/v1/chat/completions") {
                        trimmed.to_string()
                    } else {
                        format!("{}/api/chat", trimmed)
                    }
                })
                .unwrap_or_else(|| "http://127.0.0.1:11434/api/chat".to_string());
            let hdrs = vec![("Content-Type".to_string(), "application/json".to_string())];
            let mut msgs = Vec::with_capacity(turns.len() + 2);
            msgs.push(json!({ "role": "system", "content": cfg.system_prompt }));
            msgs.extend(turns);
            msgs.push(super::llm_multimodal::build_ollama_user_turn(
                &cfg.model,
                &cfg.prompt,
                cfg.attachment.as_ref(),
            ));
            let body = json!({
                "model": cfg.model,
                "stream": false,
                "options": { "num_predict": max_tokens },
                "messages": msgs
            });
            (url, hdrs, body)
        }
        _ => {
            let url = openai_compatible_endpoint(&prov, custom_base);
            let clean_key = cfg.api_key.trim();
            let mut hdrs = vec![("Content-Type".to_string(), "application/json".to_string())];
            if !clean_key.is_empty() {
                hdrs.push(("Authorization".to_string(), format!("Bearer {}", clean_key)));
            }
            let mut msgs = Vec::with_capacity(turns.len() + 2);
            msgs.push(json!({ "role": "system", "content": cfg.system_prompt }));
            msgs.extend(turns);
            msgs.push(json!({
                "role": "user",
                "content": super::llm_multimodal::build_openai_user_content(
                    &prov,
                    &cfg.model,
                    &cfg.prompt,
                    cfg.attachment.as_ref(),
                )
            }));
            let mut body = json!({
                "model": cfg.model,
                "messages": msgs
            });
            if uses_max_completion_tokens(&cfg.model) {
                body["max_completion_tokens"] = json!(max_tokens);
            } else {
                body["max_tokens"] = json!(max_tokens);
            }
            (url, hdrs, body)
        }
    }
}

pub(super) fn extract_cached_tokens(provider: &str, v: &Value) -> u64 {
    let prov = provider.trim().to_ascii_lowercase();
    if prov == "gemini" {
        return v
            .get("usageMetadata")
            .and_then(|u| u.get("cachedContentTokenCount"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
    }
    if let Some(usage) = v.get("usage") {
        return usage
            .get("prompt_cache_hit_tokens")
            .or_else(|| usage.get("cache_read_input_tokens"))
            .or_else(|| {
                usage
                    .get("prompt_tokens_details")
                    .and_then(|d| d.get("cached_tokens"))
            })
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
    }
    0
}

fn extract_provider_reply(provider: &str, v: &Value) -> Option<(String, Option<u64>, Option<u64>)> {
    let prov = provider.trim().to_ascii_lowercase();
    match prov.as_str() {
        "anthropic" => {
            let text = v
                .get("content")
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            let is_text = item.get("type").and_then(|t| t.as_str()).unwrap_or("text") == "text";
                            if is_text { item.get("text").and_then(|t| t.as_str()) } else { None }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|s| !s.trim().is_empty())?;
            let in_tok = v.get("usage").and_then(|u| u.get("input_tokens")).and_then(|n| n.as_u64());
            let out_tok = v.get("usage").and_then(|u| u.get("output_tokens")).and_then(|n| n.as_u64());
            Some((text, in_tok, out_tok))
        }
        "gemini" => {
            let text = v
                .get("candidates")
                .and_then(|c| c.get(0))
                .and_then(|c0| c0.get("content"))
                .and_then(|ct| ct.get("parts"))
                .and_then(|p| p.as_array())
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|s| !s.trim().is_empty())?;
            let usage = v.get("usageMetadata");
            let in_tok = usage.and_then(|u| u.get("promptTokenCount")).and_then(|n| n.as_u64());
            let out_tok = usage.and_then(|u| u.get("candidatesTokenCount")).and_then(|n| n.as_u64());
            Some((text, in_tok, out_tok))
        }
        "ollama" => {
            if let Some(txt) = v
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                let in_tok = v.get("prompt_eval_count").and_then(|n| n.as_u64());
                let out_tok = v.get("eval_count").and_then(|n| n.as_u64());
                return Some((txt.to_string(), in_tok, out_tok));
            }
            extract_openai_shape(v)
        }
        _ => extract_openai_shape(v),
    }
}

fn extract_openai_shape(v: &Value) -> Option<(String, Option<u64>, Option<u64>)> {
    let msg = v.get("choices").and_then(|c| c.get(0)).and_then(|c0| c0.get("message"))?;
    let text = extract_openai_message_text(msg)?;
    let usage = v.get("usage");
    let in_tok = usage.and_then(|u| u.get("prompt_tokens")).and_then(|n| n.as_u64());
    let out_tok = usage.and_then(|u| u.get("completion_tokens")).and_then(|n| n.as_u64());
    Some((text, in_tok, out_tok))
}

pub(super) fn extract_openai_message_text(msg: &Value) -> Option<String> {
    if let Some(s) = msg.get("content").and_then(|c| c.as_str()) {
        if !s.trim().is_empty() {
            return Some(s.trim().to_string());
        }
    }
    if let Some(arr) = msg.get("content").and_then(|c| c.as_array()) {
        let joined = arr
            .iter()
            .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        if !joined.trim().is_empty() {
            return Some(joined.trim().to_string());
        }
    }
    msg.get("reasoning_content")
        .and_then(|r| r.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

#[cfg(test)]
#[path = "llm_client_tests.rs"]
mod tests;
