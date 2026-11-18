use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use miku_core::{build_current_thread_runtime, spawn_orchestrator};
use miku_protocol::BaseContractBundle;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::{
    start_gateway_server_with_loopback, GatewayConfig, LoopbackServerConfig, read_ws_frame_bounded,
    write_ws_close_frame, write_ws_frame, MAX_FRAME_BYTES,
};

fn unique_temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let p = env::temp_dir().join(format!("miku_6_2_{}_{}_{}", tag, std::process::id(), nanos));
    fs::create_dir_all(&p).expect("create temp dir");
    p
}

async fn raw_http(
    addr: SocketAddr,
    req_raw: &str,
) -> (u16, String, Vec<(String, String)>, Vec<u8>) {
    let mut s = TcpStream::connect(addr).await.expect("connect tcp");
    s.write_all(req_raw.as_bytes()).await.expect("write req");
    s.flush().await.expect("flush req");

    let mut all = Vec::new();
    let _ = s.read_to_end(&mut all).await.expect("read resp");

    let split = all
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http header separator");
    let head = String::from_utf8_lossy(&all[..split]).to_string();
    let body = all[split + 4..].to_vec();

    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    let mut hdrs = Vec::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            hdrs.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    (status, head, hdrs, body)
}

fn header_val(hdrs: &[(String, String)], key: &str) -> Option<String> {
    hdrs.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
}

// LEDGER-6.2-1
#[test]
fn test_6_2_loopback_http_serves_modular_ui_with_user_override_and_csp() {
    let rt = build_current_thread_runtime().expect("runtime");
    rt.block_on(async {
        let user_dir = unique_temp_dir("override");
        let custom_theme = ":root { --ac: #FF00AA; }\n/* custom user override theme */\n";
        fs::write(user_dir.join("theme.css"), custom_theme).expect("write override theme");

        let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
        let (orch, orch_join) = spawn_orchestrator(bundle, "miku_link_tok".to_string(), 64);

        let lb = LoopbackServerConfig {
            user_override_dir: Some(user_dir.clone()),
            ..LoopbackServerConfig::default()
        };
        let (gw, gw_join) =
            start_gateway_server_with_loopback(GatewayConfig::loopback(0), orch.clone(), lb)
                .await
                .expect("start gw");

        // 1. Fetch index.html via GET / -> 200 OK, CSP strict, nosniff, Set-Cookie token
        let req_index = format!(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            gw.local_addr.port()
        );
        let (st, _, hdrs, body) = raw_http(gw.local_addr, &req_index).await;
        assert_eq!(st, 200);
        let csp = header_val(&hdrs, "content-security-policy").expect("CSP header");
        assert!(csp.contains("script-src 'self'"));
        assert_eq!(header_val(&hdrs, "x-content-type-options").as_deref(), Some("nosniff"));
        let cookie = header_val(&hdrs, "set-cookie").expect("Set-Cookie token");
        assert!(cookie.contains("miku_token=") && cookie.contains("SameSite=Strict"));
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("<main class=\"island\""));

        // 2. Fetch /theme.css -> user override wins
        let req_theme = format!(
            "GET /theme.css HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            gw.local_addr.port()
        );
        let (st_t, _, hdrs_t, body_t) = raw_http(gw.local_addr, &req_theme).await;
        assert_eq!(st_t, 200);
        assert_eq!(
            header_val(&hdrs_t, "x-miku-ui-source").as_deref(),
            Some("user-override")
        );
        assert_eq!(String::from_utf8_lossy(&body_t), custom_theme);

        // 3. Fetch /app.js -> falls back cleanly to bundled modular UI
        let req_app = format!(
            "GET /app.js HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            gw.local_addr.port()
        );
        let (st_a, _, hdrs_a, body_a) = raw_http(gw.local_addr, &req_app).await;
        assert_eq!(st_a, 200);
        assert_eq!(
            header_val(&hdrs_a, "x-miku-ui-source").as_deref(),
            Some("bundled")
        );
        assert!(String::from_utf8_lossy(&body_a).contains("MikuMascot"));

        gw.trigger_shutdown();
        gw_join.await.expect("gw join");
        orch.trigger_system_shutdown_signal();
        orch_join.await.expect("orch join");
        let _ = fs::remove_dir_all(user_dir);
    });
}

// LEDGER-6.2-2
#[test]
fn test_6_2_api_bootstrap_agents_gatekeeper_mcp_skills_and_secrets_roundtrip() {
    let rt = build_current_thread_runtime().expect("runtime");
    rt.block_on(async {
        let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
        let (orch, orch_join) = spawn_orchestrator(bundle, "miku_link_tok".to_string(), 64);

        let handler = std::sync::Arc::new(crate::http_loopback::DefaultGatewayApiHandler::new(
            orch.clone(),
        ));
        handler.push_memory_doc("Gateway Architecture", "Loopback runs on port 47400");

        let lb = LoopbackServerConfig {
            api_handler: Some(handler),
            ..LoopbackServerConfig::default()
        };
        let (gw, gw_join) =
            start_gateway_server_with_loopback(GatewayConfig::loopback(0), orch.clone(), lb)
                .await
                .expect("start gw");
        let port = gw.local_addr.port();
        let tok = gw.session_token().expect("session token").to_string();

        // 1. Bootstrap & initial state
        let req_boot = format!(
            "GET /api/bootstrap HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Miku-Token: {}\r\nConnection: close\r\n\r\n",
            port, tok
        );
        let (st, _, _, b_boot) = raw_http(gw.local_addr, &req_boot).await;
        assert_eq!(st, 200);
        let boot_v: Value = serde_json::from_slice(&b_boot).unwrap();
        assert_eq!(boot_v["gateway"], "miku-gateway");

        // 2. Save secret alias -> NEVER expose raw secret in /api/state
        let sec_body = json!({ "alias": "deepseek-main", "secret": "sk-ultra-secret-value-999" }).to_string();
        let req_sec = format!(
            "POST /api/secrets HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Miku-Token: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            port, tok, sec_body.len(), sec_body
        );
        let (st_sec, _, _, _) = raw_http(gw.local_addr, &req_sec).await;
        assert_eq!(st_sec, 200);

        // 3. Arm kill switch, add MCP, add agent, search memory
        let ks_body = json!({ "active": true }).to_string();
        let req_ks = format!(
            "POST /api/gatekeeper/kill_switch HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Miku-Token: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            port, tok, ks_body.len(), ks_body
        );
        assert_eq!(raw_http(gw.local_addr, &req_ks).await.0, 200);

        let mcp_body = json!({ "name": "fs-local", "command": "npx", "args": ["-y", "mcp-fs"] }).to_string();
        let req_mcp = format!(
            "POST /api/mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Miku-Token: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            port, tok, mcp_body.len(), mcp_body
        );
        assert_eq!(raw_http(gw.local_addr, &req_mcp).await.0, 200);

        let ag_body = json!({ "agents": [{ "id": "coder", "name": "Rust Coder", "role": "Dev", "model": "deepseek-chat", "c": "#3B9EFF", "outfit": "none" }] }).to_string();
        let req_ag = format!(
            "POST /api/agents HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Miku-Token: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            port, tok, ag_body.len(), ag_body
        );
        assert_eq!(raw_http(gw.local_addr, &req_ag).await.0, 200);

        let req_mem = format!(
            "GET /api/memory/search?q=47400 HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Miku-Token: {}\r\nConnection: close\r\n\r\n",
            port, tok
        );
        let (st_m, _, _, b_m) = raw_http(gw.local_addr, &req_mem).await;
        assert_eq!(st_m, 200);
        let hits: Vec<Value> = serde_json::from_slice(&b_m).unwrap();
        assert_eq!(hits.len(), 1);

        // 4. Verify state_json reflects roundtrips & NEVER leaks plaintext secret
        let req_state = format!(
            "GET /api/state HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Miku-Token: {}\r\nConnection: close\r\n\r\n",
            port, tok
        );
        let (st_s, _, _, b_s) = raw_http(gw.local_addr, &req_state).await;
        assert_eq!(st_s, 200);
        let state_raw = String::from_utf8_lossy(&b_s);
        assert!(!state_raw.contains("sk-ultra-secret-value-999"));
        let st_v: Value = serde_json::from_slice(&b_s).unwrap();
        assert_eq!(st_v["kill_switch"], true);
        assert_eq!(st_v["secrets"][0], "deepseek-main");
        assert_eq!(st_v["mcp"][0]["name"], "fs-local");
        assert_eq!(st_v["agents"][0]["id"], "coder");

        // 5. Connect /ws?token=... and verify live chat roundtrip frame
        let mut ws = TcpStream::connect(gw.local_addr).await.expect("ws tcp");
        let ws_req = format!(
            "GET /ws?token={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            tok, port
        );
        ws.write_all(ws_req.as_bytes()).await.unwrap();
        let mut hdr_buf = Vec::with_capacity(256);
        let mut b = [0u8; 1];
        while !hdr_buf.ends_with(b"\r\n\r\n") {
            let n = ws.read(&mut b).await.unwrap();
            assert!(n > 0, "unexpected eof reading WS 101 headers");
            hdr_buf.push(b[0]);
        }
        assert!(String::from_utf8_lossy(&hdr_buf).starts_with("HTTP/1.1 101"));

        let f1 = read_ws_frame_bounded(&mut ws, MAX_FRAME_BYTES, false).await.unwrap();
        assert!(String::from_utf8_lossy(&f1.payload).contains("\"ok\":true"));
        let f2 = read_ws_frame_bounded(&mut ws, MAX_FRAME_BYTES, false).await.unwrap();
        assert!(String::from_utf8_lossy(&f2.payload).contains("state_sync"));

        write_ws_frame(
            &mut ws,
            0x1,
            br#"{"type":"chat","text":"hola","agent_id":"coder"}"#,
            true,
        )
        .await
        .unwrap();
        let reply = read_ws_frame_bounded(&mut ws, MAX_FRAME_BYTES, false).await.unwrap();
        let reply_str = String::from_utf8_lossy(&reply.payload);
        assert!(reply_str.contains("agent_reply"));
        assert!(reply_str.contains("Kill Switch"));
        let _ = write_ws_close_frame(&mut ws, 1000, true).await;

        gw.trigger_shutdown();
        gw_join.await.expect("gw join");
        orch.trigger_system_shutdown_signal();
        orch_join.await.expect("orch join");
    });
}

// LEDGER-6.2-3
#[test]
fn test_6_2_loopback_api_rejects_missing_token_and_non_local_host() {
    let rt = build_current_thread_runtime().expect("runtime");
    rt.block_on(async {
        let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
        let (orch, orch_join) = spawn_orchestrator(bundle, "miku_link_tok".to_string(), 64);

        let (gw, gw_join) = start_gateway_server_with_loopback(
            GatewayConfig::loopback(0),
            orch.clone(),
            LoopbackServerConfig::default(),
        )
        .await
        .expect("start gw");
        let port = gw.local_addr.port();

        // Missing token -> 401
        let req_no_tok = format!(
            "GET /api/state HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            port
        );
        assert_eq!(raw_http(gw.local_addr, &req_no_tok).await.0, 401);

        // Wrong token -> 401
        let req_bad_tok = format!(
            "POST /api/gatekeeper/kill_switch HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Miku-Token: wrong\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{{\"active\":true}}",
            port
        );
        assert_eq!(raw_http(gw.local_addr, &req_bad_tok).await.0, 401);

        // Non-local Host -> 403
        let req_bad_host = "GET / HTTP/1.1\r\nHost: evil.example.com:47400\r\nConnection: close\r\n\r\n";
        assert_eq!(raw_http(gw.local_addr, req_bad_host).await.0, 403);

        // Cross-origin CSRF attempt -> 403
        let req_bad_origin = format!(
            "GET /api/state HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: https://evil.example.com\r\nConnection: close\r\n\r\n",
            port
        );
        assert_eq!(raw_http(gw.local_addr, &req_bad_origin).await.0, 403);

        // Path traversal -> 403
        let req_trav = format!(
            "GET /../../Cargo.toml HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
            port
        );
        assert_eq!(raw_http(gw.local_addr, &req_trav).await.0, 403);

        // WS without valid token -> 401
        let req_ws_no_tok = format!(
            "GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
            port
        );
        assert_eq!(raw_http(gw.local_addr, &req_ws_no_tok).await.0, 401);

        gw.trigger_shutdown();
        gw_join.await.expect("gw join");
        orch.trigger_system_shutdown_signal();
        orch_join.await.expect("orch join");
    });
}
