//! Miku-Link (estilo Phone Link): vinculación QR LAN / USB adb reverse,
//! heartbeat de batería/dispositivo, comandos remotos y notificaciones en vivo.

use std::collections::VecDeque;
use std::net::UdpSocket;

use miku_gatekeeper::{Capability, GatekeeperVerdict};
use miku_gateway::lan_pairing::{encode_pairing_qr_matrix, render_pairing_qr_svg};
use miku_gateway::{http_loopback::extract_session_token, DEFAULT_LOOPBACK_PORT};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use super::api_handler::LiveMikuApiHandler;
use super::media_routes::handle_media_control;
use super::runtime_state::{parse_query_param, persist_json, LiveRuntimeState};
use super::storage_actor::now_epoch_ms;

const MAX_LINK_NOTIFICATIONS: usize = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LinkDeviceMeta {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub app_version: String,
    pub transport: String,
    pub battery_pct: Option<u8>,
    pub battery_charging: Option<bool>,
    pub paired_at_ms: u64,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct LinkNotificationItem {
    pub id: String,
    pub ts_ms: u64,
    pub level: String,
    pub category: String,
    pub title: String,
    pub body: String,
    pub source: String,
    pub read: bool,
}

fn jstr<'a>(v: &'a Value, key: &str, default: &'a str) -> &'a str {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(default)
}

pub(super) fn detect_primary_lan_ip() -> String {
    for target in ["8.8.8.8:80", "192.168.1.1:80", "10.0.0.1:80"] {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
            if sock.connect(target).is_ok() {
                if let Ok(addr) = sock.local_addr() {
                    let ip = addr.ip();
                    if !ip.is_loopback() && !ip.is_unspecified() {
                        return ip.to_string();
                    }
                }
            }
        }
    }
    "127.0.0.1".to_string()
}

pub(super) fn init_notification_queue() -> VecDeque<LinkNotificationItem> {
    VecDeque::with_capacity(MAX_LINK_NOTIFICATIONS)
}

pub(super) fn build_qr_json_with_meta(
    uri: &str,
    lan_ip: &str,
    port: u16,
    ttl_ms: u64,
) -> Value {
    let mat = encode_pairing_qr_matrix(uri);
    let svg = mat
        .as_ref()
        .map(|m| m.to_svg(6, 4))
        .unwrap_or_else(|| render_pairing_qr_svg(uri));
    let data_url = format!("data:image/svg+xml;utf8,{}", svg.replace('#', "%23"));
    json!({
        "ok": true,
        "url": uri,
        "lan_ip": lan_ip,
        "port": port,
        "lan_base_url": format!("http://{}:{}", lan_ip, port),
        "adb_reverse_cmd": format!("adb reverse tcp:{} tcp:{}", port, port),
        "expires_in_sec": ttl_ms / 1000,
        "matrix_size": mat.as_ref().map(|m| m.size).unwrap_or(0),
        "version": mat.as_ref().map(|m| m.version).unwrap_or(0),
        "svg": svg,
        "qr_svg_data_url": data_url
    })
}

pub(super) fn push_notification_with_state(
    st: &mut LiveRuntimeState,
    events_tx: &broadcast::Sender<Value>,
    level: &str,
    category: &str,
    title: &str,
    body: &str,
    source: &str,
) -> Value {
    st.link_notif_seq = st.link_notif_seq.saturating_add(1);
    let item = LinkNotificationItem {
        id: format!("ln-{}", st.link_notif_seq),
        ts_ms: now_epoch_ms(),
        level: level.to_string(),
        category: category.to_string(),
        title: title.to_string(),
        body: body.chars().take(280).collect(),
        source: source.to_string(),
        read: false,
    };
    while st.link_notifications.len() >= MAX_LINK_NOTIFICATIONS {
        st.link_notifications.pop_front();
    }
    st.link_notifications.push_back(item.clone());
    let unread = st.link_notifications.iter().filter(|n| !n.read).count();
    let val = serde_json::to_value(&item).unwrap_or_else(|_| json!({}));
    let _ = events_tx.send(json!({
        "type": "link_notification",
        "notification": val,
        "unread_count": unread
    }));
    val
}

pub(super) fn notify_agent_reply_if_paired(
    handler: &LiveMikuApiHandler,
    agent_name: &str,
    reply_text: &str,
    is_error: bool,
) {
    let mut st = handler.state.lock().expect("state mutex");
    if st.pairing.active_paired_device_count() == 0 {
        return;
    }
    let clean: String = reply_text.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview: String = clean.chars().take(160).collect();
    let title = if is_error {
        format!("Error en {}", agent_name)
    } else {
        format!("{} respondió", agent_name)
    };
    let level = if is_error { "error" } else { "info" };
    push_notification_with_state(
        &mut st,
        &handler.events_tx,
        level,
        "agent",
        &title,
        &preview,
        "pc-gateway",
    );
}

fn extract_mobile_token(headers: &[(String, String)], query: Option<&str>) -> Option<String> {
    for (k, v) in headers {
        if k == "x-miku-link-token" {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    extract_session_token(headers, query)
}

fn is_mobile_allowed_route(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        ("WS", "/ws")
            | ("GET", "/api/state")
            | ("GET", "/api/bootstrap")
            | ("GET", "/api/media/now_playing")
            | ("GET", "/api/link/status")
            | ("GET", "/api/link/notifications")
            | ("GET", "/api/telemetry/usage")
            | ("GET", "/api/doctor")
            | ("POST", "/api/chat")
            | ("POST", "/api/media/control")
            | ("POST", "/api/link/heartbeat")
            | ("POST", "/api/link/command")
            | ("POST", "/api/link/notifications")
            | ("POST", "/api/link/notifications/ack")
            | ("POST", "/api/gatekeeper/kill_switch")
            | ("POST", "/api/gatekeeper/mode")
            | ("POST", "/api/ui/choice")
    )
}

pub(super) fn authorize_mobile_request(
    handler: &LiveMikuApiHandler,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    query: Option<&str>,
) -> bool {
    if !is_mobile_allowed_route(method, path) {
        return false;
    }
    let Some(token) = extract_mobile_token(headers, query) else {
        return false;
    };
    let Ok(st) = handler.state.lock() else {
        return false;
    };
    let pin = st.pairing.pinned_cert_sha256().to_string();
    st.pairing.authenticate_mobile_session(&token, &pin).is_ok()
}

fn resolve_status_json(handler: &LiveMikuApiHandler) -> Value {
    let (port, _) = handler.gateway_endpoint();
    let active_port = if port == 0 { DEFAULT_LOOPBACK_PORT } else { port };
    let lan_ip = detect_primary_lan_ip();
    let st = handler.state.lock().expect("state mutex");
    let devices: Vec<Value> = st
        .pairing
        .list_paired_sessions()
        .iter()
        .map(|s| {
            st.link_devices
                .get(&s.device_id)
                .map(|m| json!(m))
                .unwrap_or_else(|| {
                    json!({
                        "device_id": s.device_id,
                        "device_name": s.device_id,
                        "platform": "android",
                        "app_version": "unknown",
                        "transport": "wifi",
                        "battery_pct": null,
                        "battery_charging": null,
                        "paired_at_ms": s.paired_at_ms,
                        "last_seen_ms": s.paired_at_ms
                    })
                })
        })
        .collect();
    let unread = st.link_notifications.iter().filter(|n| !n.read).count();
    let desktop_name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Miku-PC".to_string());
    json!({
        "ok": true,
        "link_connected": !devices.is_empty(),
        "lan_enabled": st.pairing.is_lan_enabled(),
        "lan_ip": lan_ip,
        "port": active_port,
        "lan_base_url": format!("http://{}:{}", lan_ip, active_port),
        "adb_reverse_cmd": format!("adb reverse tcp:{} tcp:{}", active_port, active_port),
        "paired_count": devices.len(),
        "devices": devices,
        "unread_notifications": unread,
        "total_notifications": st.link_notifications.len(),
        "desktop_name": desktop_name,
        "kill_switch": st.gatekeeper.is_kill_switch_active(),
        "permission_mode": st.permission_mode,
        "pair_url": st.pair_url
    })
}

fn generate_qr_response(handler: &LiveMikuApiHandler, host_override: Option<&str>) -> (u16, Value) {
    let (port, _) = handler.gateway_endpoint();
    let active_port = if port == 0 { DEFAULT_LOOPBACK_PORT } else { port };
    let lan_ip = host_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(detect_primary_lan_ip);

    let mut st = handler.state.lock().expect("state mutex");
    if !st.pairing.is_lan_enabled() {
        st.pairing.set_lan_enabled(true);
    }
    st.pairing.set_lan_bind_host(&lan_ip);
    match st.pairing.generate_single_use_qr(now_epoch_ms()) {
        Ok(mut payload) => {
            payload.lan_host = lan_ip.clone();
            payload.port = active_port;
            let uri = payload.to_pairing_uri();
            st.pair_url = uri.clone();
            if st.link_devices.is_empty() {
                st.link_device = "pairing-pending".to_string();
            }
            let snap = st.snapshot_json();
            drop(st);
            handler.broadcast_state(&snap);
            (
                200,
                build_qr_json_with_meta(&uri, &lan_ip, active_port, 60_000),
            )
        }
        Err(e) => (400, json!({ "ok": false, "error": e.to_string() })),
    }
}

fn handle_redeem(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let nonce = jstr(parsed, "nonce", "");
    if nonce.is_empty() {
        return (400, json!({ "ok": false, "error": "nonce_required" }));
    }
    let raw_pin = jstr(parsed, "pin", "");
    let clean_pin = raw_pin.strip_prefix("sha256:").unwrap_or(raw_pin);
    let dev_id: String = jstr(parsed, "device_id", "android-phone").chars().take(64).collect();
    let dev_name: String = jstr(parsed, "device_name", &dev_id).chars().take(80).collect();
    let platform: String = jstr(parsed, "platform", "android").chars().take(24).collect();
    let app_version: String = jstr(parsed, "app_version", "0.2.0").chars().take(24).collect();
    let transport: String = jstr(parsed, "transport", "wifi").chars().take(16).collect();

    let (port, _) = handler.gateway_endpoint();
    let active_port = if port == 0 { DEFAULT_LOOPBACK_PORT } else { port };
    let lan_ip = detect_primary_lan_ip();
    let now_ms = now_epoch_ms();

    let (session, snap) = {
        let mut st = handler.state.lock().expect("state mutex");
        if !st.pairing.is_lan_enabled() {
            st.pairing.set_lan_enabled(true);
        }
        match st.pairing.redeem_qr_pairing(&dev_id, nonce, clean_pin, now_ms) {
            Ok(sess) => {
                let meta = LinkDeviceMeta {
                    device_id: dev_id.clone(),
                    device_name: dev_name.clone(),
                    platform,
                    app_version,
                    transport,
                    battery_pct: None,
                    battery_charging: None,
                    paired_at_ms: now_ms,
                    last_seen_ms: now_ms,
                };
                st.link_devices.insert(dev_id, meta);
                st.link_device = dev_name.clone();
                push_notification_with_state(
                    &mut st,
                    &handler.events_tx,
                    "info",
                    "link",
                    "Teléfono vinculado",
                    &format!("{} se conectó por Miku-Link.", dev_name),
                    "mobile-link",
                );
                (sess, st.snapshot_json())
            }
            Err(e) => return (401, json!({ "ok": false, "error": e.to_string() })),
        }
    };

    handler.broadcast_state(&snap);
    let _ = handler.events_tx.send(json!({ "type": "link_status" }));
    let desktop_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Miku-PC".to_string());
    (
        200,
        json!({
            "ok": true,
            "device_token": session.session_token,
            "device_id": session.device_id,
            "device_name": dev_name,
            "epoch": session.epoch,
            "desktop_name": desktop_name,
            "lan_ip": lan_ip,
            "port": active_port,
            "base_url": format!("http://{}:{}", lan_ip, active_port),
            "ws_url": format!("ws://{}:{}/ws", lan_ip, active_port)
        }),
    )
}

fn handle_heartbeat(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let dev_id = jstr(parsed, "device_id", "android-phone");
    let battery_pct = parsed
        .get("battery_pct")
        .and_then(|v| v.as_u64())
        .map(|n| n.min(100) as u8);
    let charging = parsed.get("battery_charging").and_then(|v| v.as_bool());
    let transport = jstr(parsed, "transport", "");

    let mut st = handler.state.lock().expect("state mutex");
    let now_ms = now_epoch_ms();
    if let Some(meta) = st.link_devices.get_mut(dev_id) {
        meta.last_seen_ms = now_ms;
        if let Some(b) = battery_pct {
            meta.battery_pct = Some(b);
        }
        if charging.is_some() {
            meta.battery_charging = charging;
        }
        if !transport.is_empty() {
            meta.transport = transport.to_string();
        }
    }
    drop(st);
    let _ = handler.events_tx.send(json!({ "type": "link_status" }));
    (200, json!({ "ok": true, "ts_ms": now_ms }))
}

fn handle_link_command(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let action = jstr(parsed, "action", "");
    match action {
        "media_play_pause" => handle_media_control(handler, &json!({ "action": "play_pause" })),
        "media_next" => handle_media_control(handler, &json!({ "action": "next" })),
        "media_prev" => handle_media_control(handler, &json!({ "action": "prev" })),
        "volume_up" => handle_media_control(handler, &json!({ "action": "vol_up" })),
        "volume_down" => handle_media_control(handler, &json!({ "action": "vol_down" })),
        "volume_mute" => handle_media_control(handler, &json!({ "action": "mute" })),
        "island_expand" | "island_collapse" | "island_ping" => {
            let sub = action.strip_prefix("island_").unwrap_or("ping");
            let _ = handler.events_tx.send(json!({ "type": "island_control", "action": sub }));
            (200, json!({ "ok": true, "action": action }))
        }
        "open_settings" => {
            let sec = parsed.get("section").and_then(|v| v.as_str()).map(ToString::to_string);
            handler.request_open_settings(sec);
            (200, json!({ "ok": true, "action": action }))
        }
        "set_permission_mode" => {
            let mode = jstr(parsed, "mode", "sandbox");
            let mut st = handler.state.lock().expect("state mutex");
            match st.apply_permission_mode(mode) {
                Ok(canonical) => {
                    persist_json(&st.data_dir.join("settings.json"), &st.settings);
                    push_notification_with_state(
                        &mut st,
                        &handler.events_tx,
                        "info",
                        "gatekeeper",
                        "Modo Gatekeeper actualizado",
                        &format!("Modo activo cambiado a {} desde Miku-Link.", canonical),
                        "mobile-link",
                    );
                    let snap = st.snapshot_json();
                    drop(st);
                    handler.broadcast_state(&snap);
                    (200, json!({ "ok": true, "permission_mode": canonical }))
                }
                Err(e) => (400, json!({ "ok": false, "error": e })),
            }
        }
        "toggle_kill_switch" => {
            let active = parsed
                .get("active")
                .and_then(|v| v.as_bool())
                .unwrap_or_else(|| !handler.is_kill_switch_active());
            let next = handler.set_kill_switch(active);
            (200, json!({ "ok": true, "kill_switch": next }))
        }
        "notify_pc" => {
            let title = jstr(parsed, "title", "Notificación del Móvil");
            let body = jstr(parsed, "body", "Mensaje recibido desde Miku-Link");
            let mut st = handler.state.lock().expect("state mutex");
            let item = push_notification_with_state(
                &mut st,
                &handler.events_tx,
                "info",
                "mobile",
                title,
                body,
                "mobile-link",
            );
            drop(st);
            let _ = handler.events_tx.send(json!({ "type": "notify", "title": title, "body": body }));
            (200, json!({ "ok": true, "notification": item }))
        }
        "clipboard_push" => {
            let text = jstr(parsed, "text", "");
            if text.is_empty() {
                return (400, json!({ "ok": false, "error": "empty_clipboard_text" }));
            }
            let mut st = handler.state.lock().expect("state mutex");
            let verdict = st.gatekeeper.evaluate_request(
                "miku-link",
                Capability::Clipboard,
                "clipboard:write",
                "mobile clipboard push",
                now_epoch_ms(),
            );
            let bio_ok = parsed.get("biometric_verified").and_then(|v| v.as_bool()).unwrap_or(false);
            match verdict {
                GatekeeperVerdict::Deny { detail, .. } => {
                    return (403, json!({ "ok": false, "error": detail }));
                }
                GatekeeperVerdict::AskPending { .. } if !bio_ok => {
                    return (403, json!({ "ok": false, "error": "gatekeeper_biometric_required" }));
                }
                _ => {}
            }
            let item = push_notification_with_state(
                &mut st,
                &handler.events_tx,
                "info",
                "clipboard",
                "Portapapeles desde Android",
                text,
                "mobile-link",
            );
            drop(st);
            let _ = handler.events_tx.send(json!({
                "type": "notify",
                "title": "Portapapeles recibido",
                "body": text.chars().take(120).collect::<String>()
            }));
            (200, json!({ "ok": true, "notification": item }))
        }
        _ => (400, json!({ "ok": false, "error": "unsupported_link_command" })),
    }
}

pub(super) fn try_dispatch_link_api(
    handler: &LiveMikuApiHandler,
    method: &str,
    path: &str,
    query: Option<&str>,
    parsed: &Value,
) -> Option<(u16, Value)> {
    match (method, path) {
        ("POST", "/api/link/pair") => Some(generate_qr_response(handler, parsed.get("host").and_then(|v| v.as_str()))),
        ("GET", "/api/link/qr") => {
            let h = parse_query_param(query, "host");
            Some(generate_qr_response(handler, if h.trim().is_empty() { None } else { Some(h.as_str()) }))
        }
        ("POST", "/api/link/redeem") => Some(handle_redeem(handler, parsed)),
        ("POST", "/api/link/unpair") => {
            let snap = {
                let mut st = handler.state.lock().expect("state mutex");
                let _ = st.pairing.pair_again_revoke_all();
                st.pairing.set_lan_enabled(false);
                st.pair_url.clear();
                st.link_device.clear();
                st.link_devices.clear();
                st.snapshot_json()
            };
            handler.broadcast_state(&snap);
            let _ = handler.events_tx.send(json!({ "type": "link_status" }));
            Some((200, json!({ "ok": true, "link_connected": false })))
        }
        ("GET", "/api/link/status") => Some((200, resolve_status_json(handler))),
        ("POST", "/api/link/heartbeat") => Some(handle_heartbeat(handler, parsed)),
        ("POST", "/api/link/command") => Some(handle_link_command(handler, parsed)),
        ("GET", "/api/link/notifications") => {
            let st = handler.state.lock().expect("state mutex");
            let list: Vec<&LinkNotificationItem> = st.link_notifications.iter().rev().take(40).collect();
            let unread = st.link_notifications.iter().filter(|n| !n.read).count();
            Some((200, json!({ "ok": true, "notifications": list, "unread_count": unread })))
        }
        ("POST", "/api/link/notifications") => {
            let title = jstr(parsed, "title", "Aviso Miku PC");
            let body = jstr(parsed, "body", "Sincronización activa con tu PC");
            let level = jstr(parsed, "level", "info");
            let mut st = handler.state.lock().expect("state mutex");
            let item = push_notification_with_state(&mut st, &handler.events_tx, level, "pc", title, body, "pc-desktop");
            Some((200, json!({ "ok": true, "notification": item })))
        }
        ("POST", "/api/link/notifications/ack") => {
            let id = jstr(parsed, "id", "all");
            let mut st = handler.state.lock().expect("state mutex");
            for item in &mut st.link_notifications {
                if id == "all" || item.id == id {
                    item.read = true;
                }
            }
            let unread = st.link_notifications.iter().filter(|n| !n.read).count();
            drop(st);
            let _ = handler.events_tx.send(json!({ "type": "link_status" }));
            Some((200, json!({ "ok": true, "unread_count": unread })))
        }
        _ => None,
    }
}
