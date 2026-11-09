use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::harness::types::{HarnessError, LlmProvider, ToolCallRequest};
use crate::harness::winhttp_transport::send_winhttp_https_request;
use crate::secrets::types::Zeroizing;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderResponse {
    Text(String),
    ToolCalls(Vec<ToolCallRequest>),
}

/// Parsed endpoint components for secure dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointTarget {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,
}

/// Returns true if host represents a valid loopback address (127.0.0.0/8, ::1, or localhost).
/// Invariant: 0.0.0.0 and :: (unspecified addresses) are strictly excluded and never treated as loopback.
pub fn is_loopback(host: &str) -> bool {
    let clean = host.trim().trim_start_matches('[').trim_end_matches(']');
    if clean.eq_ignore_ascii_case("localhost")
        || clean.to_ascii_lowercase().ends_with(".localhost")
    {
        return true;
    }
    if let Ok(ip) = clean.parse::<std::net::IpAddr>() {
        ip.is_loopback()
    } else {
        false
    }
}

/// Parses an endpoint URL and validates transport safety.
/// Invariant: Remote endpoints MUST use HTTPS. Cleartext HTTP is strictly rejected
/// unless the host is a local loopback address (e.g. local Ollama or mock test server).
pub fn parse_endpoint(endpoint: &str) -> Result<EndpointTarget, HarnessError> {
    let trimmed = endpoint.trim();
    let (scheme, without_scheme, default_port) = if let Some(rest) = trimmed.strip_prefix("https://") {
        ("https".to_string(), rest, 443)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("http".to_string(), rest, 80)
    } else {
        return Err(HarnessError::InsecureTransport(format!(
            "invalid or unsupported scheme in endpoint '{}': only https:// and http:// are allowed",
            trimmed
        )));
    };

    let (host_port, path) = match without_scheme.find('/') {
        Some(idx) => (&without_scheme[..idx], &without_scheme[idx..]),
        None => (without_scheme, "/"),
    };

    let (host, port) = if host_port.starts_with('[') {
        // IPv6 literal [::1]:port
        match host_port.find(']') {
            Some(end_idx) => {
                let h = &host_port[1..end_idx];
                let after = &host_port[end_idx + 1..];
                let p = if let Some(colon_idx) = after.find(':') {
                    after[colon_idx + 1..].parse::<u16>().map_err(|e| {
                        HarnessError::NetworkError(format!("invalid IPv6 port: {}", e))
                    })?
                } else {
                    default_port
                };
                (h.to_string(), p)
            }
            None => {
                return Err(HarnessError::NetworkError(
                    "unclosed IPv6 literal in host".to_string(),
                ))
            }
        }
    } else {
        match host_port.find(':') {
            Some(idx) => {
                let h = &host_port[..idx];
                let p = host_port[idx + 1..].parse::<u16>().map_err(|e| {
                    HarnessError::NetworkError(format!("invalid port in endpoint: {}", e))
                })?;
                (h.to_string(), p)
            }
            None => (host_port.to_string(), default_port),
        }
    };

    if scheme == "http" && !is_loopback(&host) {
        return Err(HarnessError::InsecureTransport(format!(
            "remote plaintext HTTP rejected for host '{}': remote endpoints require HTTPS with TLS certificate validation",
            host
        )));
    }

    Ok(EndpointTarget {
        scheme,
        host,
        port,
        path: path.to_string(),
    })
}

/// Redacts any sensitive secret bytes found within an arbitrary string.
pub fn sanitize_text_with_secrets(text: &str, secrets: &[&[u8]]) -> String {
    let mut sanitized = text.to_string();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        if let Ok(sec_str) = std::str::from_utf8(secret) {
            if !sec_str.is_empty() && sanitized.contains(sec_str) {
                sanitized = sanitized.replace(sec_str, "[REDACTED_SECRET]");
            }
        }
    }
    sanitized
}

/// Constructs a provider-specific JSON request payload.
pub fn format_provider_request(
    provider: LlmProvider,
    model: &str,
    messages: &[(String, String)], // (role, content)
) -> Vec<u8> {
    match provider {
        LlmProvider::DeepSeek | LlmProvider::OpenAi => {
            let msgs: Vec<Value> = messages
                .iter()
                .map(|(role, content)| {
                    serde_json::json!({
                        "role": role,
                        "content": content
                    })
                })
                .collect();
            serde_json::json!({
                "model": model,
                "messages": msgs,
            })
            .to_string()
            .into_bytes()
        }
        LlmProvider::Anthropic => {
            let msgs: Vec<Value> = messages
                .iter()
                .map(|(role, content)| {
                    serde_json::json!({
                        "role": role,
                        "content": content
                    })
                })
                .collect();
            serde_json::json!({
                "model": model,
                "messages": msgs,
                "max_tokens": 2048
            })
            .to_string()
            .into_bytes()
        }
        LlmProvider::Gemini => {
            let contents: Vec<Value> = messages
                .iter()
                .map(|(role, content)| {
                    serde_json::json!({
                        "role": if role == "assistant" { "model" } else { "user" },
                        "parts": [{"text": content}]
                    })
                })
                .collect();
            serde_json::json!({
                "contents": contents
            })
            .to_string()
            .into_bytes()
        }
        LlmProvider::Ollama => {
            let msgs: Vec<Value> = messages
                .iter()
                .map(|(role, content)| {
                    serde_json::json!({
                        "role": role,
                        "content": content
                    })
                })
                .collect();
            serde_json::json!({
                "model": model,
                "messages": msgs,
                "stream": false
            })
            .to_string()
            .into_bytes()
        }
    }
}

/// Parses an HTTP response from an LLM provider into normalized text or tool calls.
pub fn parse_provider_response(
    provider: LlmProvider,
    body: &[u8],
) -> Result<ProviderResponse, HarnessError> {
    let json: Value = serde_json::from_slice(body).map_err(|e| {
        HarnessError::NetworkError(format!("malformed JSON response from provider: {}", e))
    })?;

    // Universal: check choices array first (supported by DeepSeek, OpenAI, vLLM, and Ollama proxy)
    if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
        if let Some(first) = choices.first() {
            let msg = first.get("message").unwrap_or(&Value::Null);

            // Check for structured tool_calls
            if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                let mut calls = Vec::new();
                for tc in tool_calls {
                    let func = tc.get("function").unwrap_or(tc);
                    let name = func
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let raw_args = if let Some(args_str) =
                        func.get("arguments").and_then(|a| a.as_str())
                    {
                        args_str.to_string()
                    } else if let Some(args_val) = func.get("arguments") {
                        args_val.to_string()
                    } else {
                        "{}".to_string()
                    };
                    calls.push(ToolCallRequest {
                        tool_name: name,
                        raw_args,
                    });
                }
                if !calls.is_empty() {
                    return Ok(ProviderResponse::ToolCalls(calls));
                }
            }

            if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                return Ok(ProviderResponse::Text(content.to_string()));
            }
        }
    }

    match provider {
        LlmProvider::DeepSeek | LlmProvider::OpenAi => {
            // Already handled above in universal choices parser
        }
        LlmProvider::Anthropic => {
            if let Some(content) = json.get("content").and_then(|c| c.as_array()) {
                let mut tool_calls = Vec::new();
                let mut texts = Vec::new();

                for item in content {
                    let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    if item_type == "tool_use" {
                        let name = item
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let input = item.get("input").cloned().unwrap_or(Value::Null);
                        tool_calls.push(ToolCallRequest {
                            tool_name: name,
                            raw_args: input.to_string(),
                        });
                    } else if item_type == "text" {
                        if let Some(t) = item.get("text").and_then(|s| s.as_str()) {
                            texts.push(t);
                        }
                    }
                }

                if !tool_calls.is_empty() {
                    return Ok(ProviderResponse::ToolCalls(tool_calls));
                }
                if !texts.is_empty() {
                    return Ok(ProviderResponse::Text(texts.join("\n")));
                }
            }
        }
        LlmProvider::Gemini => {
            if let Some(candidates) = json.get("candidates").and_then(|c| c.as_array()) {
                if let Some(first) = candidates.first() {
                    if let Some(parts) = first
                        .get("content")
                        .and_then(|c| c.get("parts"))
                        .and_then(|p| p.as_array())
                    {
                        let mut tool_calls = Vec::new();
                        let mut text_parts = Vec::new();

                        for part in parts {
                            if let Some(func_call) = part.get("functionCall") {
                                let name = func_call
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("unknown")
                                    .to_string();
                                let args = func_call.get("args").cloned().unwrap_or(Value::Null);
                                tool_calls.push(ToolCallRequest {
                                    tool_name: name,
                                    raw_args: args.to_string(),
                                });
                            } else if let Some(t) = part.get("text").and_then(|s| s.as_str()) {
                                text_parts.push(t);
                            }
                        }

                        if !tool_calls.is_empty() {
                            return Ok(ProviderResponse::ToolCalls(tool_calls));
                        }
                        if !text_parts.is_empty() {
                            return Ok(ProviderResponse::Text(text_parts.join("")));
                        }
                    }
                }
            }
        }
        LlmProvider::Ollama => {
            if let Some(msg) = json.get("message") {
                if let Some(tool_calls) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                    let mut calls = Vec::new();
                    for tc in tool_calls {
                        let func = tc.get("function").unwrap_or(tc);
                        let name = func
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let raw_args = func
                            .get("arguments")
                            .map(|a| a.to_string())
                            .unwrap_or_else(|| "{}".to_string());
                        calls.push(ToolCallRequest {
                            tool_name: name,
                            raw_args,
                        });
                    }
                    if !calls.is_empty() {
                        return Ok(ProviderResponse::ToolCalls(calls));
                    }
                }
                if let Some(content) = msg.get("content").and_then(|c| c.as_str()) {
                    return Ok(ProviderResponse::Text(content.to_string()));
                }
            }
        }
    }

    // Fallback: look for general text / message / error in JSON
    if let Some(err) = json.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
        return Err(HarnessError::NetworkError(format!("provider returned error: {}", err)));
    }

    if let Some(text) = json.get("text").and_then(|t| t.as_str()) {
        return Ok(ProviderResponse::Text(text.to_string()));
    }

    Ok(ProviderResponse::Text(json.to_string()))
}

/// Dispatches an HTTP/1.1 POST request.
/// Remote endpoints use HTTPS with native TLS certificate validation.
/// Loopback endpoints use TcpStream directly.
pub async fn send_provider_http_post(
    endpoint: &str,
    provider: LlmProvider,
    secret_bytes: Option<&[u8]>,
    body: &[u8],
    timeout_ms: u64,
) -> Result<(u16, Vec<u8>), HarnessError> {
    let target = parse_endpoint(endpoint)?;

    if target.scheme == "https" {
        let headers_w = {
            let mut headers_s = Zeroizing::new(String::with_capacity(512));
            headers_s.push_str("Content-Type: application/json\r\n");
            if let Some(sec) = secret_bytes {
                if !sec.is_empty() {
                    let sec_str = std::str::from_utf8(sec).unwrap_or("");
                    match provider {
                        LlmProvider::DeepSeek | LlmProvider::OpenAi => {
                            headers_s.push_str("Authorization: Bearer ");
                            headers_s.push_str(sec_str);
                            headers_s.push_str("\r\n");
                        }
                        LlmProvider::Anthropic => {
                            headers_s.push_str("x-api-key: ");
                            headers_s.push_str(sec_str);
                            headers_s.push_str("\r\nanthropic-version: 2023-06-01\r\n");
                        }
                        LlmProvider::Gemini => {
                            headers_s.push_str("x-goog-api-key: ");
                            headers_s.push_str(sec_str);
                            headers_s.push_str("\r\n");
                        }
                        LlmProvider::Ollama => {}
                    }
                }
            }
            headers_s.push_str("Connection: close\r\n");
            let wide: Vec<u16> = headers_s.encode_utf16().chain(std::iter::once(0)).collect();
            Zeroizing::new(wide)
        };

        send_winhttp_https_request(
            &target.host,
            target.port,
            &target.path,
            headers_w,
            body,
            timeout_ms,
        )
        .await
    } else {
        send_loopback_http_post(&target, provider, secret_bytes, body, timeout_ms).await
    }
}

/// Loopback HTTP POST over TcpStream for local mock servers and Ollama.
async fn send_loopback_http_post(
    target: &EndpointTarget,
    provider: LlmProvider,
    secret_bytes: Option<&[u8]>,
    body: &[u8],
    timeout_ms: u64,
) -> Result<(u16, Vec<u8>), HarnessError> {
    if !is_loopback(&target.host) {
        return Err(HarnessError::InsecureTransport(format!(
            "remote plaintext HTTP rejected for '{}': TLS required",
            target.host
        )));
    }

    let connect_fut = TcpStream::connect((target.host.as_str(), target.port));
    let mut stream = timeout(Duration::from_millis(timeout_ms), connect_fut)
        .await
        .map_err(|_| {
            HarnessError::NetworkError(format!(
                "connection to {}:{} timed out after {} ms",
                target.host, target.port, timeout_ms
            ))
        })?
        .map_err(|e| {
            HarnessError::NetworkError(format!(
                "failed to connect to {}:{} - {}",
                target.host, target.port, e
            ))
        })?;

    // Build complete HTTP/1.1 request (headers + body) wrapped in Zeroizing guard for a single atomic write
    let mut request_buf = Zeroizing::new(Vec::with_capacity(512 + body.len()));
    request_buf.extend_from_slice(b"POST ");
    request_buf.extend_from_slice(target.path.as_bytes());
    request_buf.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    request_buf.extend_from_slice(target.host.as_bytes());
    request_buf.extend_from_slice(b":");
    request_buf.extend_from_slice(target.port.to_string().as_bytes());
    request_buf.extend_from_slice(b"\r\nContent-Type: application/json\r\nContent-Length: ");
    request_buf.extend_from_slice(body.len().to_string().as_bytes());
    request_buf.extend_from_slice(b"\r\nConnection: close\r\n");

    if let Some(sec) = secret_bytes {
        if !sec.is_empty() {
            match provider {
                LlmProvider::DeepSeek | LlmProvider::OpenAi => {
                    request_buf.extend_from_slice(b"Authorization: Bearer ");
                    request_buf.extend_from_slice(sec);
                    request_buf.extend_from_slice(b"\r\n");
                }
                LlmProvider::Anthropic => {
                    request_buf.extend_from_slice(b"x-api-key: ");
                    request_buf.extend_from_slice(sec);
                    request_buf.extend_from_slice(b"\r\nanthropic-version: 2023-06-01\r\n");
                }
                LlmProvider::Gemini => {
                    request_buf.extend_from_slice(b"x-goog-api-key: ");
                    request_buf.extend_from_slice(sec);
                    request_buf.extend_from_slice(b"\r\n");
                }
                LlmProvider::Ollama => {}
            }
        }
    }
    request_buf.extend_from_slice(b"\r\n");
    request_buf.extend_from_slice(body);

    let write_res = stream.write_all(&request_buf).await;
    // Zeroizing drops and zeroizes request_buf automatically on any exit path or here
    drop(request_buf);

    write_res.map_err(|e| HarnessError::NetworkError(format!("failed to send HTTP request: {}", e)))?;

    stream
        .flush()
        .await
        .map_err(|e| HarnessError::NetworkError(format!("failed to flush stream: {}", e)))?;

    // Read HTTP response deterministically using Content-Length so we don't block or fail on late TCP RST
    let mut response_buf = Vec::with_capacity(4096);
    let read_res = timeout(Duration::from_millis(timeout_ms), async {
        let mut chunk = [0u8; 4096];
        let mut expected_total: Option<usize> = None;
        loop {
            let n = match stream.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if expected_total.is_some() && !response_buf.is_empty() => {
                    let _ = e;
                    break;
                }
                Err(e) => return Err(e),
            };
            response_buf.extend_from_slice(&chunk[..n]);

            if expected_total.is_none() {
                if let Some(hdr_end) = response_buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let body_start = hdr_end + 4;
                    let hdr_str = String::from_utf8_lossy(&response_buf[..hdr_end]);
                    let mut content_len = None;
                    for line in hdr_str.lines() {
                        if let Some((k, v)) = line.split_once(':') {
                            if k.trim().eq_ignore_ascii_case("content-length") {
                                if let Ok(len) = v.trim().parse::<usize>() {
                                    content_len = Some(len);
                                    break;
                                }
                            }
                        }
                    }
                    if let Some(cl) = content_len {
                        expected_total = Some(body_start + cl);
                    }
                }
            }

            if let Some(total) = expected_total {
                if response_buf.len() >= total {
                    break;
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| HarnessError::NetworkError("timed out reading HTTP response".to_string()))?;

    read_res.map_err(|e| HarnessError::NetworkError(format!("failed to read HTTP response: {}", e)))?;

    // Parse status line: HTTP/1.1 <status_code>
    let header_end = response_buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| {
            HarnessError::NetworkError("malformed HTTP response: missing header terminator".to_string())
        })?;

    let headers_part = String::from_utf8_lossy(&response_buf[..header_end]);
    let first_line = headers_part.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let _version = parts.next().unwrap_or("");
    let status_str = parts.next().unwrap_or("0");
    let status_code = status_str.parse::<u16>().map_err(|_| {
        HarnessError::NetworkError(format!("invalid HTTP status in response line '{}'", first_line))
    })?;

    let body_bytes = response_buf[header_end + 4..].to_vec();
    Ok((status_code, body_bytes))
}
