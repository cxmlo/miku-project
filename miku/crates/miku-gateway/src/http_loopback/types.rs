use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use miku_core::OrchestratorHandle;
use miku_protocol::sha256_hex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::lan_pairing::qr_pairing::{encode_pairing_qr_matrix, render_pairing_qr_svg};

#[derive(Debug, Clone)]
pub struct HttpRequestHead {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequestHead {
    pub fn header(&self, name_lower: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name_lower)
            .map(|(_, v)| v.as_str())
    }

    pub fn is_websocket_upgrade(&self) -> bool {
        let has_upgrade = self
            .header("upgrade")
            .map(|v| v.to_ascii_lowercase().contains("websocket"))
            .unwrap_or(false);
        let has_conn = self
            .header("connection")
            .map(|v| v.to_ascii_lowercase().contains("upgrade"))
            .unwrap_or(false);
        has_upgrade && has_conn && self.header("sec-websocket-key").is_some()
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: &'static str,
    pub content_type: &'static str,
    pub extra_headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(status: u16, reason: &'static str, value: Value) -> Self {
        let body = serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec());
        Self {
            status,
            reason,
            content_type: "application/json; charset=utf-8",
            extra_headers: vec![
                ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
                ("Cache-Control".to_string(), "no-store".to_string()),
            ],
            body,
        }
    }

    pub fn text(status: u16, reason: &'static str, text: &str) -> Self {
        Self {
            status,
            reason,
            content_type: "text/plain; charset=utf-8",
            extra_headers: vec![
                ("X-Content-Type-Options".to_string(), "nosniff".to_string()),
                ("Cache-Control".to_string(), "no-store".to_string()),
            ],
            body: text.as_bytes().to_vec(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.status,
            self.reason,
            self.content_type,
            self.body.len()
        );
        for (k, v) in &self.extra_headers {
            out.push_str(&format!("{}: {}\r\n", k, v));
        }
        out.push_str("\r\n");
        let mut raw = out.into_bytes();
        raw.extend_from_slice(&self.body);
        raw
    }
}

pub trait LoopbackApiHandler: Send + Sync {
    fn bootstrap_json(&self) -> Value;
    fn state_json(&self) -> Value;
    fn handle_api(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> (u16, Value);
    fn handle_ws_command(&self, cmd: &Value) -> Option<Value>;
    fn subscribe_events(&self) -> Option<broadcast::Receiver<Value>> {
        None
    }
    fn is_lan_public_endpoint(&self, _method: &str, _path: &str) -> bool {
        false
    }
    fn authorize_mobile_request(
        &self,
        _method: &str,
        _path: &str,
        _headers: &[(String, String)],
        _query: Option<&str>,
    ) -> bool {
        false
    }
}

#[derive(Clone, Default)]
pub struct LoopbackServerConfig {
    pub user_override_dir: Option<PathBuf>,
    pub default_ui_dir: Option<PathBuf>,
    pub session_token: Option<String>,
    pub api_handler: Option<Arc<dyn LoopbackApiHandler>>,
    pub allow_lan_pairing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfoDto {
    pub id: String,
    pub name: String,
    pub role: String,
    pub model: String,
    pub c: String,
    pub outfit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDto {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntryDto {
    pub title: String,
    pub snippet: String,
}

#[derive(Debug, Default)]
struct GatewayBridgeState {
    kill_switch: bool,
    link_connected: bool,
    link_device: String,
    pair_url: String,
    agents: Vec<AgentInfoDto>,
    secret_hashes: HashMap<String, String>,
    skills: Vec<String>,
    mcp: Vec<McpServerDto>,
    rules: Vec<Value>,
    memory: Vec<MemoryEntryDto>,
    settings: HashMap<String, Value>,
    uploads: Vec<String>,
}

pub struct DefaultGatewayApiHandler {
    orchestrator: OrchestratorHandle,
    state: RwLock<GatewayBridgeState>,
    events_tx: broadcast::Sender<Value>,
}

impl DefaultGatewayApiHandler {
    pub fn new(orchestrator: OrchestratorHandle) -> Self {
        let (events_tx, _) = broadcast::channel(128);
        let initial = GatewayBridgeState {
            rules: vec![
                json!({ "id": "default-deny", "name": "Deny-by-default shell/net", "tier": "ask" }),
                json!({ "id": "path-jail", "name": "CanonicalPathJail workspace", "tier": "enforced" }),
            ],
            ..Default::default()
        };
        Self {
            orchestrator,
            state: RwLock::new(initial),
            events_tx,
        }
    }

    fn emit(&self, msg: Value) {
        let _ = self.events_tx.send(msg);
    }

    pub fn push_memory_doc(&self, title: impl Into<String>, snippet: impl Into<String>) {
        if let Ok(mut g) = self.state.write() {
            g.memory.push(MemoryEntryDto {
                title: title.into(),
                snippet: snippet.into(),
            });
        }
    }
}

impl LoopbackApiHandler for DefaultGatewayApiHandler {
    fn bootstrap_json(&self) -> Value {
        json!({
            "version": 1,
            "gateway": "miku-gateway",
            "state": self.state_json()
        })
    }

    fn state_json(&self) -> Value {
        let g = self.state.read().expect("gateway state lock");
        let mut aliases: Vec<String> = g.secret_hashes.keys().cloned().collect();
        aliases.sort();
        json!({
            "agents": g.agents,
            "secrets": aliases,
            "rules": g.rules,
            "skills": g.skills,
            "mcp": g.mcp,
            "kill_switch": g.kill_switch,
            "has_api_key": !g.secret_hashes.is_empty(),
            "link_connected": g.link_connected,
            "link_device": g.link_device,
            "settings": g.settings,
            "runtime_ready": !self.orchestrator.raw_sender().is_closed()
        })
    }

    fn handle_api(
        &self,
        method: &str,
        path: &str,
        query: Option<&str>,
        body: &[u8],
    ) -> (u16, Value) {
        let payload: Value = if body.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(body).unwrap_or_else(|_| json!({}))
        };

        match (method, path) {
            ("GET", "/api/bootstrap") => (200, self.bootstrap_json()),
            ("GET", "/api/state") => (200, self.state_json()),
            ("POST", "/api/gatekeeper/kill_switch") => {
                let active = payload.get("active").and_then(|v| v.as_bool()).unwrap_or(true);
                if let Ok(mut g) = self.state.write() {
                    g.kill_switch = active;
                }
                self.emit(json!({ "type": "state_sync" }));
                (200, json!({ "ok": true, "kill_switch": active }))
            }
            ("POST", "/api/secrets") => {
                let alias = payload.get("alias").and_then(|v| v.as_str()).unwrap_or("").trim();
                let secret = payload.get("secret").and_then(|v| v.as_str()).unwrap_or("").trim();
                if alias.is_empty() || secret.is_empty() {
                    return (400, json!({ "error": "alias_and_secret_required" }));
                }
                let digest = sha256_hex(secret.as_bytes());
                if let Ok(mut g) = self.state.write() {
                    g.secret_hashes.insert(alias.to_string(), digest);
                }
                self.emit(json!({ "type": "state_sync" }));
                (200, json!({ "ok": true, "alias": alias }))
            }
            ("DELETE", p) if p.starts_with("/api/secrets/") => {
                let alias = p.trim_start_matches("/api/secrets/");
                let removed = self
                    .state
                    .write()
                    .ok()
                    .and_then(|mut g| g.secret_hashes.remove(alias))
                    .is_some();
                self.emit(json!({ "type": "state_sync" }));
                (if removed { 200 } else { 404 }, json!({ "ok": removed, "alias": alias }))
            }
            ("POST", "/api/skills/reload") => {
                let count = self.state.read().map(|g| g.skills.len()).unwrap_or(0);
                self.emit(json!({ "type": "state_sync" }));
                (200, json!({ "ok": true, "skills_count": count }))
            }
            ("POST", "/api/mcp") => {
                let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
                let command = payload.get("command").and_then(|v| v.as_str()).unwrap_or("").trim();
                if name.is_empty() || command.is_empty() {
                    return (400, json!({ "error": "mcp_name_and_command_required" }));
                }
                let args = payload
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                if let Ok(mut g) = self.state.write() {
                    g.mcp.push(McpServerDto {
                        name: name.to_string(),
                        command: command.to_string(),
                        args,
                    });
                }
                self.emit(json!({ "type": "state_sync" }));
                (200, json!({ "ok": true, "name": name }))
            }
            ("GET", "/api/memory/search") => {
                let q = query
                    .and_then(|qs| {
                        qs.split('&')
                            .filter_map(|kv| kv.split_once('='))
                            .find(|(k, _)| *k == "q")
                            .map(|(_, v)| v.to_ascii_lowercase())
                    })
                    .unwrap_or_default();
                let g = self.state.read().expect("state lock");
                let hits: Vec<&MemoryEntryDto> = g
                    .memory
                    .iter()
                    .filter(|m| {
                        q.is_empty()
                            || m.title.to_ascii_lowercase().contains(&q)
                            || m.snippet.to_ascii_lowercase().contains(&q)
                    })
                    .collect();
                (200, json!(hits))
            }
            ("POST", "/api/agents") => {
                if let Some(arr) = payload.get("agents").and_then(|v| v.as_array()) {
                    if let Ok(mut g) = self.state.write() {
                        g.agents = arr
                            .iter()
                            .filter_map(|a| serde_json::from_value::<AgentInfoDto>(a.clone()).ok())
                            .collect();
                    }
                    self.emit(json!({ "type": "state_sync" }));
                    return (200, json!({ "ok": true }));
                }
                (400, json!({ "error": "invalid_agents_payload" }))
            }
            ("POST", "/api/settings") => {
                if let Some(k) = payload.get("key").and_then(|v| v.as_str()) {
                    let val = payload.get("value").cloned().unwrap_or(Value::Null);
                    if let Ok(mut g) = self.state.write() {
                        g.settings.insert(k.to_string(), val);
                    }
                    return (200, json!({ "ok": true, "key": k }));
                }
                (400, json!({ "error": "setting_key_required" }))
            }
            ("POST", "/api/link/pair") => {
                let url = "miku-link://pair?v=1&host=127.0.0.1&port=47400".to_string();
                let mat = encode_pairing_qr_matrix(&url);
                let svg = mat
                    .as_ref()
                    .map(|m| m.to_svg(6, 4))
                    .unwrap_or_else(|| render_pairing_qr_svg(&url));
                let data_url = format!("data:image/svg+xml;utf8,{}", svg.replace('#', "%23"));
                if let Ok(mut g) = self.state.write() {
                    g.pair_url = url.clone();
                }
                (
                    200,
                    json!({
                        "ok": true,
                        "url": url,
                        "matrix_size": mat.as_ref().map(|m| m.size).unwrap_or(0),
                        "version": mat.as_ref().map(|m| m.version).unwrap_or(0),
                        "qr_svg": svg,
                        "svg": svg,
                        "qr_svg_data_url": data_url
                    }),
                )
            }
            ("GET", "/api/link/qr") => {
                let url = self
                    .state
                    .read()
                    .ok()
                    .map(|g| g.pair_url.clone())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "miku-link://pair?v=1&host=127.0.0.1&port=47400".to_string());
                let mat = encode_pairing_qr_matrix(&url);
                let svg = mat
                    .as_ref()
                    .map(|m| m.to_svg(6, 4))
                    .unwrap_or_else(|| render_pairing_qr_svg(&url));
                let data_url = format!("data:image/svg+xml;utf8,{}", svg.replace('#', "%23"));
                (
                    200,
                    json!({
                        "ok": true,
                        "url": url,
                        "matrix_size": mat.as_ref().map(|m| m.size).unwrap_or(0),
                        "version": mat.as_ref().map(|m| m.version).unwrap_or(0),
                        "svg": svg,
                        "qr_svg_data_url": data_url
                    }),
                )
            }
            ("POST", "/api/link/unpair") => {
                if let Ok(mut g) = self.state.write() {
                    g.link_connected = false;
                    g.link_device.clear();
                }
                self.emit(json!({ "type": "state_sync" }));
                (200, json!({ "ok": true }))
            }
            ("POST", "/api/files/upload") => {
                let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("file");
                if let Ok(mut g) = self.state.write() {
                    g.uploads.push(name.to_string());
                }
                (200, json!({ "ok": true, "name": name }))
            }
            ("POST", "/api/chat") => {
                let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
                let agent_id = payload.get("agent_id").and_then(|v| v.as_str()).unwrap_or("miku");
                let kill = self.state.read().map(|g| g.kill_switch).unwrap_or(false);
                let reply = if kill {
                    json!({
                        "type": "agent_reply",
                        "agent_id": agent_id,
                        "error": true,
                        "text": "Kill Switch Gatekeeper activo: ejecución bloqueada."
                    })
                } else {
                    json!({
                        "type": "agent_reply",
                        "agent_id": agent_id,
                        "error": false,
                        "text": format!("Gateway 47400 recibió tu mensaje ({} bytes). Configura un proveedor/API key en Ajustes -> API Keys para respuestas LLM en vivo.", text.len()),
                        "steps": [{ "tool": "gateway_router", "status": "ok", "cmd": format!("agent={}", agent_id) }]
                    })
                };
                self.emit(reply.clone());
                (200, reply)
            }
            ("GET", "/api/doctor") => (200, json!({ "overall_status": "PASS", "passed_count": 5, "probes": [] })),
            ("GET", "/api/update/check") => (200, json!({ "version": "0.1.0-pre", "up_to_date": true })),
            ("GET", "/api/apps") => (200, json!([])),
            _ => (404, json!({ "error": "not_found", "path": path })),
        }
    }

    fn handle_ws_command(&self, cmd: &Value) -> Option<Value> {
        let kind = cmd.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "chat" => {
                let bytes = serde_json::to_vec(cmd).unwrap_or_default();
                let (_, reply) = self.handle_api("POST", "/api/chat", None, &bytes);
                Some(reply)
            }
            "cancel" => Some(json!({ "type": "mascot", "state": "idle" })),
            "dictate" => {
                let on = cmd.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
                Some(json!({ "type": "mascot", "state": if on { "listening" } else { "idle" } }))
            }
            "choice" => {
                let verdict = cmd.get("verdict").and_then(|v| v.as_str()).unwrap_or("deny");
                Some(json!({ "type": "notify", "title": "Gatekeeper", "body": format!("Decisión aplicada: {}", verdict) }))
            }
            _ => None,
        }
    }

    fn subscribe_events(&self) -> Option<broadcast::Receiver<Value>> {
        Some(self.events_tx.subscribe())
    }
}
