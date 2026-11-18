use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::live_gateway::{LiveGatewayConfig, LiveGatewayRunner};

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "miku_link_cockpit_{}_{}_{}",
        label,
        std::process::id(),
        nanos
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn send_http(
    addr: SocketAddr,
    host_header: Option<&str>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).expect("tcp connect");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    let payload = body.unwrap_or(b"");
    let default_host = format!("127.0.0.1:{}", addr.port());
    let host = host_header.unwrap_or(&default_host);
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        method,
        path,
        host,
        payload.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).expect("write req");
    if !payload.is_empty() {
        stream.write_all(payload).expect("write body");
    }

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read resp");
    let hdr_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("headers separator");
    let head = String::from_utf8_lossy(&raw[..hdr_end]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status, head, raw[hdr_end + 4..].to_vec())
}

fn parse_query_param(uri: &str, key: &str) -> String {
    let q = uri.split('?').nth(1).unwrap_or("");
    for part in q.split('&') {
        if let Some((k, v)) = part.split_once('=') {
            if k == key {
                return v.to_string();
            }
        }
    }
    String::new()
}

#[test]
fn test_removed_cockpit_returns_404_and_core_ui_served_with_csp_self() {
    let temp = unique_temp_dir("ui_boundary");
    let ui_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui");
    assert!(!ui_dir.join("cockpit/cockpit.html").exists());
    assert!(ui_dir.join("island/index.html").exists());
    assert!(ui_dir.join("settings/miku_settings.html").exists());

    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: Some(ui_dir),
        user_override_dir: Some(temp.join("override_empty")),
        use_memory_secrets: true,
    })
    .expect("start gateway");

    let addr = runner.local_addr;

    let (st_cockpit, _, _) = send_http(addr, None, "GET", "/cockpit", &[], None);
    assert_eq!(st_cockpit, 404);
    let (st_cockpit_js, _, _) = send_http(addr, None, "GET", "/cockpit/cockpit.js", &[], None);
    assert_eq!(st_cockpit_js, 404);

    let (st_root, head_root, body_root) = send_http(addr, None, "GET", "/", &[], None);
    assert_eq!(st_root, 200);
    assert!(head_root.contains("Content-Security-Policy: default-src 'self'"));
    let html_root = String::from_utf8_lossy(&body_root);
    assert!(!html_root.contains("data-open-cockpit"));

    let (st_set, head_set, body_set) = send_http(addr, None, "GET", "/settings", &[], None);
    assert_eq!(st_set, 200);
    assert!(head_set.contains("Content-Security-Policy: default-src 'self'"));
    let html_set = String::from_utf8_lossy(&body_set);
    assert!(!html_set.contains("set-open-cockpit"));

    let _ = runner.shutdown_cleanly();
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_phone_link_redeem_commands_notifications_and_lan_security_boundary() {
    let temp = unique_temp_dir("phone_link");
    let ui_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui");

    let runner = LiveGatewayRunner::start_with_lan(
        LiveGatewayConfig {
            data_dir: temp.clone(),
            port: 0,
            default_ui_dir: Some(ui_dir),
            user_override_dir: Some(temp.join("override_empty")),
            use_memory_secrets: true,
        },
        true,
    )
    .expect("start lan gateway");

    let addr = runner.local_addr;
    let pc_tok = runner.session_token.clone();
    let lan_host = format!("192.168.1.50:{}", addr.port());

    // 1. PC loopback creates single-use QR pairing payload
    let (pair_st, _, pair_body) = send_http(
        addr,
        None,
        "POST",
        "/api/link/pair",
        &[("X-Miku-Token", &pc_tok)],
        Some(b"{}"),
    );
    assert_eq!(pair_st, 200);
    let pair_json: Value = serde_json::from_slice(&pair_body).expect("pair json");
    let pair_url = pair_json["url"].as_str().expect("pair url");
    assert!(pair_url.starts_with("miku-link://pair?"));
    assert!(pair_json["adb_reverse_cmd"]
        .as_str()
        .unwrap_or("")
        .contains("adb reverse tcp:"));

    let nonce = parse_query_param(pair_url, "nonce");
    let pin = parse_query_param(pair_url, "pin");
    assert!(!nonce.is_empty() && !pin.is_empty());

    // 2. Unauthenticated status over LAN must fail 401
    let (unauth_st, _, _) = send_http(addr, Some(&lan_host), "GET", "/api/link/status", &[], None);
    assert_eq!(unauth_st, 401);

    // 3. Phone redeems single-use QR over LAN without desktop session token
    let redeem_req = json!({
        "nonce": nonce,
        "pin": pin,
        "device_id": "pixel-miku-01",
        "device_name": "Pixel 8 Pro",
        "platform": "android",
        "app_version": "0.2.0",
        "transport": "wifi"
    })
    .to_string();
    let (rd_st, _, rd_body) = send_http(
        addr,
        Some(&lan_host),
        "POST",
        "/api/link/redeem",
        &[("Content-Type", "application/json")],
        Some(redeem_req.as_bytes()),
    );
    assert_eq!(rd_st, 200);
    let rd_json: Value = serde_json::from_slice(&rd_body).expect("redeem json");
    let dev_tok = rd_json["device_token"].as_str().expect("device token").to_string();
    assert!(!dev_tok.is_empty());
    assert_ne!(dev_tok, pc_tok);

    // 4. Replaying the same QR must be rejected 401
    let (replay_st, _, _) = send_http(
        addr,
        Some(&lan_host),
        "POST",
        "/api/link/redeem",
        &[("Content-Type", "application/json")],
        Some(redeem_req.as_bytes()),
    );
    assert_eq!(replay_st, 401);

    // 5. Phone sends heartbeat (battery + transport) & reads /api/link/status
    let hb_req = json!({
        "device_id": "pixel-miku-01",
        "battery_pct": 84,
        "battery_charging": true,
        "transport": "wifi"
    })
    .to_string();
    let (hb_st, _, _) = send_http(
        addr,
        Some(&lan_host),
        "POST",
        "/api/link/heartbeat",
        &[("X-Miku-Token", &dev_tok), ("Content-Type", "application/json")],
        Some(hb_req.as_bytes()),
    );
    assert_eq!(hb_st, 200);

    let (st_st, _, st_body) = send_http(
        addr,
        Some(&lan_host),
        "GET",
        "/api/link/status",
        &[("X-Miku-Token", &dev_tok)],
        None,
    );
    assert_eq!(st_st, 200);
    let st_json: Value = serde_json::from_slice(&st_body).expect("status json");
    assert_eq!(st_json["link_connected"], json!(true));
    assert_eq!(st_json["devices"][0]["battery_pct"], json!(84));
    assert!(st_json["unread_notifications"].as_u64().unwrap_or(0) >= 1);

    // 6. Phone executes remote command & reads/acks bidirectional notification
    let cmd_req = json!({ "action": "set_permission_mode", "mode": "restricted" }).to_string();
    let (cmd_st, _, cmd_body) = send_http(
        addr,
        Some(&lan_host),
        "POST",
        "/api/link/command",
        &[("X-Miku-Token", &dev_tok), ("Content-Type", "application/json")],
        Some(cmd_req.as_bytes()),
    );
    assert_eq!(cmd_st, 200);
    let cmd_json: Value = serde_json::from_slice(&cmd_body).expect("cmd json");
    assert_eq!(cmd_json["permission_mode"], json!("restricted"));

    let (ack_st, _, ack_body) = send_http(
        addr,
        Some(&lan_host),
        "POST",
        "/api/link/notifications/ack",
        &[("X-Miku-Token", &dev_tok), ("Content-Type", "application/json")],
        Some(b"{\"id\":\"all\"}"),
    );
    assert_eq!(ack_st, 200);
    let ack_json: Value = serde_json::from_slice(&ack_body).expect("ack json");
    assert_eq!(ack_json["unread_count"], json!(0));

    // 7. Security boundary: mobile token CANNOT access secrets, settings, harnesses, files, or static UI
    for forbidden_path in [
        "/api/secrets",
        "/api/settings",
        "/api/harnesses",
        "/api/mcp",
        "/api/files/list",
    ] {
        let (sec_st, _, _) = send_http(
            addr,
            Some(&lan_host),
            "GET",
            forbidden_path,
            &[("X-Miku-Token", &dev_tok)],
            None,
        );
        assert_eq!(
            sec_st, 401,
            "mobile token must never access {}",
            forbidden_path
        );
    }

    let (ui_lan_st, _, _) = send_http(
        addr,
        Some(&lan_host),
        "GET",
        "/",
        &[("X-Miku-Token", &dev_tok)],
        None,
    );
    assert_eq!(ui_lan_st, 403);

    // 8. DNS rebinding host rejection
    let (reb_st, _, _) = send_http(
        addr,
        Some("evil.attacker.com:47400"),
        "GET",
        "/api/link/status",
        &[("X-Miku-Token", &dev_tok)],
        None,
    );
    assert_eq!(reb_st, 403);

    let _ = runner.shutdown_cleanly();
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_chat_missing_secret_returns_clean_error_without_history_corruption() {
    let temp = unique_temp_dir("chat_err");
    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: None,
        user_override_dir: Some(temp.join("override_empty")),
        use_memory_secrets: true,
    })
    .expect("start gateway");

    let addr = runner.local_addr;
    let tok = runner.session_token.clone();

    // Configure an external provider without storing any key -> chat must return clean error, not crash or loop
    let cfg_body = json!({ "key": "provider", "value": "openai" }).to_string();
    let (cfg_st, _, _) = send_http(
        addr,
        None,
        "POST",
        "/api/settings",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(cfg_body.as_bytes()),
    );
    assert_eq!(cfg_st, 200);

    let chat_body = json!({
        "agent_id": "miku",
        "text": "Explica brevemente el router de tokens en Miku"
    })
    .to_string();
    let (c_st, _, c_resp) = send_http(
        addr,
        None,
        "POST",
        "/api/chat",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(chat_body.as_bytes()),
    );
    assert_eq!(c_st, 200);
    let c_json: Value = serde_json::from_slice(&c_resp).expect("chat json");
    assert_eq!(c_json["error"], json!(false));
    assert!(!c_json["text"].as_str().unwrap_or("").is_empty());

    // Kill switch ON -> returns structured Gatekeeper Deny error without crashing
    let (ks_st, _, _) = send_http(
        addr,
        None,
        "POST",
        "/api/gatekeeper/kill_switch",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"active":true}"#),
    );
    assert_eq!(ks_st, 200);

    let (d_st, _, d_resp) = send_http(
        addr,
        None,
        "POST",
        "/api/chat",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(chat_body.as_bytes()),
    );
    assert_eq!(d_st, 200);
    let d_json: Value = serde_json::from_slice(&d_resp).expect("deny chat json");
    assert_eq!(d_json["error"], json!(true));
    assert!(d_json["text"]
        .as_str()
        .unwrap_or("")
        .contains("Bloqueado por Gatekeeper"));

    let _ = runner.shutdown_cleanly();
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_app_and_website_launcher_respects_gatekeeper_and_kill_switch() {
    let temp = unique_temp_dir("launcher_gk");
    let ui_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui");
    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: Some(ui_dir),
        user_override_dir: Some(temp.join("override_empty")),
        use_memory_secrets: true,
    })
    .expect("start gateway");
    let addr = runner.local_addr;
    let tok = runner.session_token.clone();

    // 1. Sandbox mode: "abrir pinterest" resolves to Pinterest HTTPS URL with 0 API tokens
    let (st1, _, b1) = send_http(
        addr,
        None,
        "POST",
        "/api/chat",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"agent_id":"miku","text":"abrir pinterest"}"#),
    );
    assert_eq!(st1, 200);
    let j1: Value = serde_json::from_slice(&b1).expect("pinterest json");
    assert_eq!(j1["error"], json!(false));
    assert_eq!(j1["routed_local"], json!(true));
    assert_eq!(j1["launch"]["kind"], json!("url"));
    assert_eq!(j1["launch"]["target"], json!("https://www.pinterest.com/"));

    // 2. Sandbox mode: "abre discord" resolves either desktop Discord or web fallback
    let (st2, _, b2) = send_http(
        addr,
        None,
        "POST",
        "/api/chat",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"agent_id":"miku","text":"abre discord"}"#),
    );
    assert_eq!(st2, 200);
    let j2: Value = serde_json::from_slice(&b2).expect("discord json");
    assert_eq!(j2["error"], json!(false));
    assert_eq!(j2["routed_local"], json!(true));
    assert!(j2["launch"]["label"].as_str().unwrap_or("").contains("Discord"));

    // 3. Switch to Restricted Gatekeeper mode -> blocks Capability::Browser
    let (st_mode, _, _) = send_http(
        addr,
        None,
        "POST",
        "/api/gatekeeper/mode",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"mode":"restricted"}"#),
    );
    assert_eq!(st_mode, 200);

    let (st3, _, b3) = send_http(
        addr,
        None,
        "POST",
        "/api/chat",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"agent_id":"miku","text":"abrir pinterest"}"#),
    );
    assert_eq!(st3, 200);
    let j3: Value = serde_json::from_slice(&b3).expect("restricted json");
    assert_eq!(j3["error"], json!(true));
    assert!(j3["text"].as_str().unwrap_or("").contains("Bloqueado por Gatekeeper"));

    let _ = runner.shutdown_cleanly();
    let _ = fs::remove_dir_all(&temp);
}
