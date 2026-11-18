//! Live official model catalog synchronization per AI provider via DPAPI secrets.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde_json::{json, Value};

use super::api_handler::LiveMikuApiHandler;
use super::runtime_state::persist_json;
use super::storage_actor::now_epoch_ms;
use super::winhttp_https::get_https;

pub(super) fn handle_get_provider_models(handler: &LiveMikuApiHandler) -> (u16, Value) {
    let st = handler.state.lock().expect("state mutex");
    let synced = st
        .settings
        .get("synced_models")
        .cloned()
        .unwrap_or_else(|| json!({}));
    (200, json!({ "ok": true, "synced_models": synced }))
}

pub(super) fn handle_sync_provider_models(
    handler: &LiveMikuApiHandler,
    parsed: &Value,
) -> (u16, Value) {
    let provider = parsed
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let alias = parsed
        .get("alias")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if provider.is_empty() {
        return (400, json!({ "ok": false, "error": "provider required" }));
    }

    let targets: Vec<&str> = if provider == "all" {
        vec!["deepseek", "groq", "openai", "anthropic", "gemini", "ollama"]
    } else {
        vec![provider.as_str()]
    };

    let mut results = serde_json::Map::new();
    let mut errors = serde_json::Map::new();

    for prov in targets {
        let key_opt = {
            let mut st = handler.state.lock().expect("state mutex");
            alias
                .and_then(|a| st.resolve_secret_by_alias(a))
                .or_else(|| st.resolve_secret_by_alias(prov))
        };

        if prov != "ollama" && key_opt.is_none() {
            errors.insert(
                prov.to_string(),
                json!(format!("No hay API key DPAPI guardada para '{}'", prov)),
            );
            continue;
        }

        let fetched = match prov {
            "deepseek" => fetch_openai_models("api.deepseek.com", "/models", &key_opt.unwrap(), &[]),
            "groq" => {
                fetch_openai_models("api.groq.com", "/openai/v1/models", &key_opt.unwrap(), &[])
            }
            "openai" => fetch_openai_models(
                "api.openai.com",
                "/v1/models",
                &key_opt.unwrap(),
                &["gpt-", "o1", "o3", "o4", "chatgpt-"],
            ),
            "anthropic" => fetch_anthropic_models(&key_opt.unwrap()),
            "gemini" => fetch_gemini_models(&key_opt.unwrap()),
            "ollama" => fetch_ollama_models(),
            other => Err(format!("Proveedor no soportado para sync: {}", other)),
        };

        match fetched {
            Ok(models) => {
                results.insert(prov.to_string(), json!(models));
            }
            Err(err) => {
                errors.insert(prov.to_string(), json!(err));
            }
        }
    }

    if !results.is_empty() {
        let mut st = handler.state.lock().expect("state mutex");
        let mut current = st
            .settings
            .get("synced_models")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();
        for (k, v) in &results {
            current.insert(k.clone(), v.clone());
        }
        st.settings
            .insert("synced_models".to_string(), Value::Object(current));
        persist_json(&st.data_dir.join("settings.json"), &st.settings);
        let snap = st.snapshot_json();
        drop(st);
        handler.broadcast_state(&snap);
    }

    if results.is_empty() && !errors.is_empty() {
        let first_err = errors
            .values()
            .next()
            .and_then(|v| v.as_str())
            .unwrap_or("Error sincronizando modelos");
        return (
            502,
            json!({
                "ok": false,
                "error": first_err,
                "errors": errors,
                "synced_at_ms": now_epoch_ms()
            }),
        );
    }

    let first_models = results
        .get(&provider)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    (
        200,
        json!({
            "ok": true,
            "provider": provider,
            "models": first_models,
            "synced": results,
            "errors": errors,
            "synced_at_ms": now_epoch_ms()
        }),
    )
}

fn fetch_openai_models(
    host: &str,
    path: &str,
    api_key: &str,
    prefixes: &[&str],
) -> Result<Vec<String>, String> {
    let headers = vec![
        ("Authorization".to_string(), format!("Bearer {}", api_key)),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    let (status, bytes) = get_https(host, 443, path, &headers)?;
    if !(200..300).contains(&status) {
        let body = String::from_utf8_lossy(&bytes);
        return Err(format!("HTTP {} en {}: {}", status, host, body.chars().take(180).collect::<String>()));
    }
    let val: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("JSON inválido de {}: {}", host, e))?;
    let mut list: Vec<String> = val
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("id").and_then(|s| s.as_str()).map(str::to_string))
                .filter(|id| {
                    if prefixes.is_empty() {
                        true
                    } else {
                        prefixes.iter().any(|p| id.starts_with(p))
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    list.sort();
    list.dedup();
    Ok(list)
}

fn fetch_anthropic_models(api_key: &str) -> Result<Vec<String>, String> {
    let headers = vec![
        ("x-api-key".to_string(), api_key.to_string()),
        ("anthropic-version".to_string(), "2023-06-01".to_string()),
        ("Accept".to_string(), "application/json".to_string()),
    ];
    let (status, bytes) = get_https("api.anthropic.com", 443, "/v1/models", &headers)?;
    if !(200..300).contains(&status) {
        let body = String::from_utf8_lossy(&bytes);
        return Err(format!("HTTP {} en Anthropic: {}", status, body.chars().take(180).collect::<String>()));
    }
    let val: Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("JSON inválido de Anthropic: {}", e))?;
    let mut list: Vec<String> = val
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("id").and_then(|s| s.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    list.sort();
    list.dedup();
    Ok(list)
}

fn fetch_gemini_models(api_key: &str) -> Result<Vec<String>, String> {
    let path = format!("/v1beta/models?key={}&pageSize=100", api_key);
    let headers = vec![("Accept".to_string(), "application/json".to_string())];
    let (status, bytes) = get_https("generativelanguage.googleapis.com", 443, &path, &headers)?;
    if !(200..300).contains(&status) {
        let body = String::from_utf8_lossy(&bytes);
        return Err(format!("HTTP {} en Gemini: {}", status, body.chars().take(180).collect::<String>()));
    }
    let val: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("JSON inválido de Gemini: {}", e))?;
    let mut list: Vec<String> = val
        .get("models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|item| {
                    item.get("supportedGenerationMethods")
                        .and_then(|m| m.as_array())
                        .map(|methods| {
                            methods.iter().any(|x| x.as_str() == Some("generateContent"))
                        })
                        .unwrap_or(true)
                })
                .filter_map(|item| item.get("name").and_then(|s| s.as_str()))
                .map(|s| s.strip_prefix("models/").unwrap_or(s).to_string())
                .filter(|s| s.starts_with("gemini-"))
                .collect()
        })
        .unwrap_or_default();
    list.sort();
    list.dedup();
    Ok(list)
}

fn fetch_ollama_models() -> Result<Vec<String>, String> {
    let addr = "127.0.0.1:11434"
        .parse()
        .map_err(|_| "Dirección Ollama inválida".to_string())?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|_| "Ollama local no está ejecutándose en 127.0.0.1:11434".to_string())?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let req = "GET /api/tags HTTP/1.1\r\nHost: 127.0.0.1:11434\r\nConnection: close\r\n\r\n";
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("Error enviando a Ollama: {}", e))?;
    let mut raw = Vec::new();
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b)
        .unwrap_or(text.as_ref());
    let val: Value =
        serde_json::from_str(body).map_err(|e| format!("Respuesta Ollama inválida: {}", e))?;
    let mut list: Vec<String> = val
        .get("models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|s| s.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    list.sort();
    list.dedup();
    Ok(list)
}
