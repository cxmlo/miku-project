//! Fast Coucou-style hook relay for `miku hook relay --agent <agent> <event>`.
//! Reads stdin JSON from the host IDE/CLI (e.g. Antigravity), normalizes fields,
//! forwards to Miku's local gateway (`127.0.0.1:47400`), and prints the decision JSON.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::{json, Map, Value};

const DEFAULT_PORT: u16 = 47400;
const MAX_STDIN_BYTES: usize = 512 * 1024;

const ARG_ALIASES: &[(&str, &str)] = &[
    ("CommandLine", "command"),
    ("FilePath", "file_path"),
    ("Path", "path"),
    ("Url", "url"),
    ("Query", "query"),
    ("Pattern", "pattern"),
];

pub fn normalize_event_name(raw: &str) -> &str {
    match raw.trim() {
        "PreInvocation" => "UserPromptSubmit",
        "PostInvocation" => "PostToolUse",
        "BeforeTool" | "BeforeToolSelection" | "preToolUse" | "pre_tool_use" => "PreToolUse",
        "AfterTool" | "AfterModel" | "postToolUse" | "post_tool_use" => "PostToolUse",
        "BeforeAgent" | "userPromptSubmitted" | "beforeSubmitPrompt" => "UserPromptSubmit",
        "AfterAgent" | "agentStop" | "stop" => "Stop",
        "sessionStart" | "session_start" | "startup" => "SessionStart",
        "sessionEnd" | "session_end" | "exit" => "SessionEnd",
        other => other,
    }
}

pub fn normalize_hook_payload(raw_bytes: &[u8], agent: &str, cli_event: &str) -> Value {
    let text = String::from_utf8_lossy(raw_bytes);
    let trimmed = text.trim_start_matches('\u{FEFF}').trim();
    let mut map: Map<String, Value> = if trimmed.is_empty() {
        Map::new()
    } else {
        serde_json::from_str::<Value>(trimmed)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    };

    let raw_event = map
        .get("hook_event_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(cli_event)
        .to_string();
    let canonical_event = normalize_event_name(&raw_event).to_string();

    if let Some(tc) = map.get("toolCall").and_then(Value::as_object).cloned() {
        if !map.contains_key("tool_name") {
            if let Some(name) = tc.get("name").and_then(Value::as_str) {
                map.insert("tool_name".into(), Value::String(name.to_string()));
            }
        }
        if !map.contains_key("tool_input") {
            let mut input = tc
                .get("args")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            for (src, dst) in ARG_ALIASES {
                if !input.contains_key(*dst) {
                    if let Some(v) = input.get(*src).cloned() {
                        input.insert((*dst).to_string(), v);
                    }
                }
            }
            map.insert("tool_input".into(), Value::Object(input));
        }
    }

    let tool_name = map
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let command = map
        .get("tool_input")
        .and_then(|v| v.get("command"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let file_path = map
        .get("tool_input")
        .and_then(|v| v.get("file_path").or_else(|| v.get("path")))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let cwd = map
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let session_id = map
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    json!({
        "agent": agent,
        "event": canonical_event,
        "raw_event": raw_event,
        "tool_name": tool_name,
        "command": command,
        "file_path": file_path,
        "cwd": cwd,
        "session_id": session_id,
        "payload": Value::Object(map)
    })
}

pub fn run_hook_relay(agent: &str, cli_event: &str) -> Result<String, String> {
    let mut buf = Vec::new();
    let stdin = std::io::stdin();
    let _ = stdin.lock().take(MAX_STDIN_BYTES as u64).read_to_end(&mut buf);

    let normalized = normalize_hook_payload(&buf, agent, cli_event);
    let event_name = normalized
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or(cli_event);
    let is_pre_tool = event_name == "PreToolUse";

    let mut port: u16 = std::env::var("MIKU_GATEWAY_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    let mut token = std::env::var("MIKU_SESSION_TOKEN").unwrap_or_default();
    if let Some(local_app) = std::env::var_os("LOCALAPPDATA") {
        let relay_cfg = std::path::PathBuf::from(local_app)
            .join("Miku")
            .join("data")
            .join("hook_relay.json");
        if let Ok(raw) = std::fs::read_to_string(relay_cfg) {
            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                if let Some(p) = v.get("port").and_then(Value::as_u64) {
                    port = p as u16;
                }
                if token.is_empty() {
                    if let Some(t) = v.get("token").and_then(Value::as_str) {
                        token = t.to_string();
                    }
                }
            }
        }
    }
    let addr: SocketAddr = format!("127.0.0.1:{}", port)
        .parse()
        .map_err(|e| format!("invalid relay addr: {}", e))?;

    let fallback = if is_pre_tool {
        r#"{"decision":"ask"}"#.to_string()
    } else {
        "{}".to_string()
    };

    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(180)) else {
        return Ok(fallback);
    };
    let read_wait = if is_pre_tool {
        Duration::from_secs(115)
    } else {
        Duration::from_secs(3)
    };
    let _ = stream.set_read_timeout(Some(read_wait));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    let body = serde_json::to_string(&normalized).unwrap_or_else(|_| "{}".to_string());
    let token_hdr = if token.is_empty() {
        String::new()
    } else {
        format!("X-Miku-Token: {}\r\n", token)
    };
    let req = format!(
        "POST /api/hooks/event HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        port,
        token_hdr,
        body.len(),
        body
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return Ok(fallback);
    }

    let mut raw_resp = String::new();
    if stream.read_to_string(&mut raw_resp).is_err() {
        return Ok(fallback);
    }
    let Some((_, json_part)) = raw_resp.split_once("\r\n\r\n") else {
        return Ok(fallback);
    };
    let parsed = serde_json::from_str::<Value>(json_part.trim()).unwrap_or_else(|_| json!({}));
    if is_pre_tool {
        let dec = parsed
            .get("decision")
            .and_then(Value::as_str)
            .unwrap_or("ask");
        return Ok(json!({ "decision": dec }).to_string());
    }
    Ok("{}".to_string())
}
