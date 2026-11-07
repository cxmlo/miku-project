#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 8. Step 2.3: Bidirectional Typed JSON-RPC IPC Bridge between WebView2 Host
//    and Core/Gateway via window.chrome.webview.postMessage (ADR-001 / ADR-005 / F-02)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiInteractionKind {
    HoverEnter,
    HoverLeave,
    PinToggle,
    EscapePressed,
    ActionClick,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum WebViewIpcPayloadV1 {
    MikuLink(MikuLinkMessageV1),
    UiInteraction {
        event: UiInteractionKind,
        target_id: String,
        x: i32,
        y: i32,
        timestamp_ms: u64,
    },
    TelemetrySnapshot {
        island_state: IslandState,
        webview_state: String,
        ram_private_mb_x100: u32,
        rtt_us: u32,
        active_agents: u32,
    },
    DomSafeTextRender {
        element_id: String,
        safe_text: String,
        csp_enforced: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebViewIpcEnvelopeV1 {
    pub jsonrpc: String,
    pub v: u8,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub origin: String,
    pub payload: WebViewIpcPayloadV1,
}

/// Strictly validates the WebView2 `WebMessageReceived` sender origin.
/// Only `https://miku.local` and `https://miku.local/*` are authorized.
pub fn validate_ipc_sender_origin(origin: &str) -> Result<String, UiError> {
    let trimmed = origin.trim();
    if trimmed.is_empty() {
        return Err(UiError::UnauthorizedIpcOrigin(
            "empty sender origin is forbidden".to_string(),
        ));
    }
    if trimmed
        .chars()
        .any(|c| c == '\0' || c == '\r' || c == '\n' || c == '@' || c == '\\')
    {
        return Err(UiError::UnauthorizedIpcOrigin(
            "origin contains forbidden control, authority (@), or backslash characters".to_string(),
        ));
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower == "https://miku.local" {
        return Ok("https://miku.local/".to_string());
    }
    if let Some(tail) = lower.strip_prefix("https://miku.local/") {
        if tail.contains("..") {
            return Err(UiError::UnauthorizedIpcOrigin(
                "origin path traversal '..' is forbidden".to_string(),
            ));
        }
        return Ok(trimmed.to_string());
    }

    Err(UiError::UnauthorizedIpcOrigin(format!(
        "untrusted origin '{}'; expected 'https://miku.local/*'",
        trimmed
    )))
}

/// Validates that the Content-Security-Policy applied to the local WebView2 shell
/// enforces `default-src 'self'`, `script-src 'self'`, `connect-src 'none'`, `object-src 'none'`,
/// and prohibits `'unsafe-eval'` or wildcard script origins.
pub fn validate_webview_csp_policy(csp: &str) -> Result<(), UiError> {
    let lower = csp.to_ascii_lowercase();
    for required in [
        "default-src 'self'",
        "script-src 'self'",
        "connect-src 'none'",
        "object-src 'none'",
    ] {
        if !lower.contains(required) {
            return Err(UiError::CspViolation(format!(
                "missing mandatory CSP directive '{}'",
                required
            )));
        }
    }
    if lower.contains("'unsafe-eval'") || lower.contains("script-src *") {
        return Err(UiError::CspViolation(
            "CSP contains forbidden 'unsafe-eval' or wildcard script-src".to_string(),
        ));
    }
    Ok(())
}

/// Escapes untrusted text for safe DOM `textContent` display, stripping null/control bytes
/// and neutralizing `<`, `>`, `&`, `"`, and `'`.
pub fn escape_html_entities_for_text_content(untrusted_text: &str) -> String {
    let mut safe_text = String::with_capacity(untrusted_text.len());
    for ch in untrusted_text.chars() {
        match ch {
            '\0' | '\r' => {}
            '<' => safe_text.push_str("&lt;"),
            '>' => safe_text.push_str("&gt;"),
            '&' => safe_text.push_str("&amp;"),
            '"' => safe_text.push_str("&quot;"),
            '\'' => safe_text.push_str("&#x27;"),
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => safe_text.push(c),
        }
    }
    safe_text
}

/// Prepares an untrusted string (such as an agent title, file name, or tool prompt)
/// for safe rendering exclusively via DOM `textContent` (never `innerHTML`), stripping
/// null/control bytes and neutralizing angle brackets so `<img src=x onerror=alert(1)>`
/// cannot execute even if inspected outside a text node.
pub fn sanitize_for_text_content(
    element_id: &str,
    untrusted_text: &str,
) -> Result<WebViewIpcPayloadV1, UiError> {
    let elem = element_id.trim();
    if elem.is_empty()
        || !elem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(UiError::IpcFrameError {
            code: "invalid_element_id".to_string(),
            detail: format!("invalid DOM target element_id '{}'", element_id),
        });
    }

    let safe_text = escape_html_entities_for_text_content(untrusted_text);

    Ok(WebViewIpcPayloadV1::DomSafeTextRender {
        element_id: elem.to_string(),
        safe_text,
        csp_enforced: true,
    })
}

impl WebViewIpcEnvelopeV1 {
    pub fn new(
        id: impl Into<String>,
        reply_to: Option<String>,
        origin: impl Into<String>,
        payload: WebViewIpcPayloadV1,
    ) -> Self {
        Self {
            jsonrpc: MIKU_JSONRPC_V2.to_string(),
            v: MIKU_PROTOCOL_VERSION_V1,
            id: id.into(),
            reply_to,
            origin: origin.into(),
            payload,
        }
    }

    pub fn encode_post_message_json(&self) -> Result<String, UiError> {
        let encoded = serde_json::to_string(self).map_err(|e| UiError::IpcFrameError {
            code: "serialization_error".to_string(),
            detail: e.to_string(),
        })?;
        if encoded.len() > MAX_IPC_MESSAGE_BYTES {
            return Err(UiError::IpcFrameError {
                code: "frame_too_large".to_string(),
                detail: format!(
                    "encoded IPC frame {} bytes exceeds max {} bytes",
                    encoded.len(),
                    MAX_IPC_MESSAGE_BYTES
                ),
            });
        }
        Ok(encoded)
    }

    /// Validates and deserializes a raw JSON message received from `window.chrome.webview.postMessage`.
    /// Never panics on malformed, truncated, oversized, or injected frames.
    pub fn decode_and_validate(raw_json: &str) -> Result<Self, UiError> {
        let size_bytes = raw_json.len();
        if size_bytes == 0 {
            return Err(UiError::IpcFrameError {
                code: "empty_frame".to_string(),
                detail: "IPC frame cannot be empty".to_string(),
            });
        }
        if size_bytes > MAX_IPC_MESSAGE_BYTES {
            return Err(UiError::IpcFrameError {
                code: "frame_too_large".to_string(),
                detail: format!(
                    "IPC frame size {} exceeds max {} bytes",
                    size_bytes, MAX_IPC_MESSAGE_BYTES
                ),
            });
        }
        if raw_json.as_bytes().contains(&0) {
            return Err(UiError::IpcFrameError {
                code: "null_byte_detected".to_string(),
                detail: "IPC frame contains forbidden null byte".to_string(),
            });
        }

        let val: serde_json::Value =
            serde_json::from_str(raw_json).map_err(|e| UiError::IpcFrameError {
                code: "malformed_json".to_string(),
                detail: e.to_string(),
            })?;

        let jsonrpc = val
            .get("jsonrpc")
            .and_then(|v| v.as_str())
            .ok_or_else(|| UiError::IpcFrameError {
                code: "invalid_jsonrpc".to_string(),
                detail: "missing string field 'jsonrpc'".to_string(),
            })?;
        if jsonrpc != MIKU_JSONRPC_V2 {
            return Err(UiError::IpcFrameError {
                code: "invalid_jsonrpc".to_string(),
                detail: format!("expected jsonrpc '{}', got '{}'", MIKU_JSONRPC_V2, jsonrpc),
            });
        }

        let v_u64 = val.get("v").and_then(|v| v.as_u64()).ok_or_else(|| {
            UiError::IpcFrameError {
                code: "missing_version".to_string(),
                detail: "missing numeric field 'v'".to_string(),
            }
        })?;
        let v = u8::try_from(v_u64).unwrap_or(u8::MAX);
        if v != MIKU_PROTOCOL_VERSION_V1 {
            return Err(UiError::IpcFrameError {
                code: "unsupported_version".to_string(),
                detail: format!(
                    "unsupported protocol version {}; supported: {:?}",
                    v, SUPPORTED_PROTOCOL_VERSIONS
                ),
            });
        }

        let env: WebViewIpcEnvelopeV1 =
            serde_json::from_value(val).map_err(|e| UiError::IpcFrameError {
                code: "schema_validation_failed".to_string(),
                detail: e.to_string(),
            })?;

        if env.id.trim().is_empty() {
            return Err(UiError::IpcFrameError {
                code: "empty_id".to_string(),
                detail: "envelope 'id' cannot be empty".to_string(),
            });
        }

        let _ = validate_ipc_sender_origin(&env.origin)?;

        if let WebViewIpcPayloadV1::MikuLink(
            MikuLinkMessageV1::Hello { .. } | MikuLinkMessageV1::PermissionDecision { .. },
        ) = &env.payload
        {
            if size_bytes > MAX_AUTH_FRAME_BYTES {
                return Err(UiError::IpcFrameError {
                    code: "auth_frame_too_large".to_string(),
                    detail: format!(
                        "auth frame size {} exceeds max {} bytes",
                        size_bytes, MAX_AUTH_FRAME_BYTES
                    ),
                });
            }
        }

        Ok(env)
    }
}

