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
        "miku_cli_live_{}_{}_{}",
        label,
        std::process::id(),
        nanos
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

fn raw_http_request(
    addr: SocketAddr,
    method: &str,
    path_with_query: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).expect("connect to live gateway");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    let payload = body.unwrap_or(b"");
    let host_hdr = format!("127.0.0.1:{}", addr.port());
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        method,
        path_with_query,
        host_hdr,
        payload.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).expect("write head");
    if !payload.is_empty() {
        stream.write_all(payload).expect("write body");
    }

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read http response");
    let hdr_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http headers terminator");
    let head = String::from_utf8_lossy(&raw[..hdr_end]).to_string();
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status, head, raw[hdr_end + 4..].to_vec())
}

#[test]
fn test_6_2_live_gateway_serves_ui_and_enforces_real_sqlite_fts5_gatekeeper_secrets() {
    let temp = unique_temp_dir("full_stack");
    let skill_dir = temp.join("skills").join("echo-notes");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(skill_dir.join("run.ps1"), "Write-Output 'notes ok'\n").expect("write entrypoint");
    fs::write(
        skill_dir.join("skill.toml"),
        "schema_version = 1\n\n[skill]\nid = \"echo-notes\"\nname = \"Echo Notes\"\nversion = \"0.1.0\"\ndescription = \"Safe note skill\"\nentrypoint = \"run.ps1\"\ncapabilities = [\"fs_read\"]\nallowed_targets = [\"notes/**\"]\n",
    )
    .expect("write skill.toml");

    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: None,
        user_override_dir: Some(temp.join("ui_override")),
        use_memory_secrets: true,
    })
    .expect("start live gateway");

    let addr = runner.local_addr;
    let tok = runner.session_token.clone();

    // 1. Serves modular index.html with CSP & Set-Cookie token
    let (status, head, html_bytes) = raw_http_request(addr, "GET", "/", &[], None);
    assert_eq!(status, 200);
    assert!(head.contains("Content-Security-Policy: default-src 'self'"));
    assert!(head.contains("Set-Cookie: miku_token="));
    let html = String::from_utf8_lossy(&html_bytes);
    assert!(html.contains("<script defer src=\"/island/app.js\"></script>"));

    // 2. Store API secret securely (no value ever echoed back!)
    let secret_payload = json!({
        "alias": "openai-prod",
        "provider": "openai",
        "model": "gpt-4o-mini",
        "secret": "SENSITIVE_TEST_SECRET_VALUE_47400"
    })
    .to_string();
    let (s_status, _, s_body) = raw_http_request(
        addr,
        "POST",
        "/api/secrets",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(secret_payload.as_bytes()),
    );
    assert_eq!(s_status, 200);
    let s_str = String::from_utf8_lossy(&s_body);
    assert!(!s_str.contains("SENSITIVE_TEST_SECRET_VALUE_47400"));

    // 3. Reload local skills from disk
    let (sk_status, _, sk_body) = raw_http_request(
        addr,
        "POST",
        "/api/skills/reload",
        &[("X-Miku-Token", &tok)],
        Some(b"{}"),
    );
    assert_eq!(sk_status, 200);
    let sk_json: Value = serde_json::from_slice(&sk_body).expect("skills json");
    assert_eq!(sk_json["count"], 1, "skills response: {:?}", sk_json);

    // 4. Submit chat prompt -> stored in SQLite WAL + FTS5 searchable
    let chat_payload = json!({
        "agent_id": "miku",
        "text": "verificar memoria obsidian_wal_keyword"
    })
    .to_string();
    let (c_status, _, c_body) = raw_http_request(
        addr,
        "POST",
        "/api/chat",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(chat_payload.as_bytes()),
    );
    assert_eq!(c_status, 200);
    let c_json: Value = serde_json::from_slice(&c_body).expect("chat json");
    assert_eq!(c_json["error"], false);

    let (m_status, _, m_body) = raw_http_request(
        addr,
        "GET",
        "/api/memory/search?q=obsidian_wal_keyword",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(m_status, 200);
    let m_arr: Value = serde_json::from_slice(&m_body).expect("fts5 hits json");
    assert!(!m_arr.as_array().expect("array").is_empty());

    // 5. Gatekeeper Kill Switch blocks subsequent prompt fail-closed
    let (k_status, _, _) = raw_http_request(
        addr,
        "POST",
        "/api/gatekeeper/kill_switch",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"active":true}"#),
    );
    assert_eq!(k_status, 200);

    let (c_blocked_status, _, c_blocked_body) = raw_http_request(
        addr,
        "POST",
        "/api/chat",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(chat_payload.as_bytes()),
    );
    assert_eq!(c_blocked_status, 200);
    let blocked_json: Value = serde_json::from_slice(&c_blocked_body).expect("blocked chat json");
    assert_eq!(blocked_json["error"], true);
    assert!(blocked_json["text"]
        .as_str()
        .unwrap_or("")
        .contains("Gatekeeper"));

    // 6. Pairing QR + unpair + apps + doctor + update endpoints
    let (p_status, _, p_body) = raw_http_request(
        addr,
        "POST",
        "/api/link/pair",
        &[("X-Miku-Token", &tok)],
        Some(b"{}"),
    );
    assert_eq!(p_status, 200);
    let p_json: Value = serde_json::from_slice(&p_body).expect("pair json");
    assert!(p_json["url"].as_str().unwrap_or("").starts_with("miku-link://pair"));
    assert!(p_json["svg"].as_str().unwrap_or("").contains("<svg"));

    let (u_status, _, _) = raw_http_request(
        addr,
        "POST",
        "/api/link/unpair",
        &[("X-Miku-Token", &tok)],
        Some(b"{}"),
    );
    assert_eq!(u_status, 200);

    let (d_status, _, d_body) = raw_http_request(
        addr,
        "GET",
        "/api/doctor",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(d_status, 200);
    let d_json: Value = serde_json::from_slice(&d_body).expect("doctor json");
    assert!(d_json["probes"].as_array().map(|a| a.len()).unwrap_or(0) >= 5);

    let (up_status, _, _) = raw_http_request(
        addr,
        "GET",
        "/api/update/check",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(up_status, 200);

    let (apps_status, _, _) = raw_http_request(
        addr,
        "GET",
        "/api/apps",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(apps_status, 200);

    runner.shutdown_cleanly().expect("clean shutdown");
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_6_2_live_gateway_rejects_path_traversal_upload_and_missing_token() {
    let temp = unique_temp_dir("sec_guard");
    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: None,
        user_override_dir: Some(temp.join("ui_override")),
        use_memory_secrets: true,
    })
    .expect("start live gateway");

    let addr = runner.local_addr;
    let tok = runner.session_token.clone();

    let (unauth_status, _, _) = raw_http_request(addr, "GET", "/api/state", &[], None);
    assert_eq!(unauth_status, 401);

    let bad_upload = json!({ "name": "../escape.txt", "data_url": "" }).to_string();
    let (up_status, _, _) = raw_http_request(
        addr,
        "POST",
        "/api/files/upload",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(bad_upload.as_bytes()),
    );
    assert_eq!(up_status, 400);

    runner.shutdown_cleanly().expect("clean shutdown");
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_6_3_pairing_qr_svg_and_uri_roundtrip_over_loopback_and_tailscale() {
    let temp = unique_temp_dir("qr_roundtrip");
    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: None,
        user_override_dir: Some(temp.join("ui_override")),
        use_memory_secrets: true,
    })
    .expect("start live gateway");

    let addr = runner.local_addr;
    let tok = runner.session_token.clone();

    // Loopback / LAN QR endpoint
    let (lan_status, _, lan_body) = raw_http_request(
        addr,
        "GET",
        "/api/link/qr",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(lan_status, 200);
    let lan_json: Value = serde_json::from_slice(&lan_body).expect("lan qr json");
    assert_eq!(lan_json["ok"], true);
    assert!(lan_json["matrix_size"].as_u64().unwrap_or(0) >= 29);
    assert!(lan_json["version"].as_u64().unwrap_or(0) >= 3);
    assert!(lan_json["url"].as_str().unwrap_or("").starts_with("miku-link://pair"));
    assert!(lan_json["svg"].as_str().unwrap_or("").contains("<path d=\"M"));
    assert!(lan_json["qr_svg_data_url"]
        .as_str()
        .unwrap_or("")
        .starts_with("data:image/svg+xml;utf8,"));

    // Tailscale MagicDNS host QR endpoint
    let (ts_status, _, ts_body) = raw_http_request(
        addr,
        "GET",
        "/api/link/qr?host=miku-pc.tailnet-alpha.ts.net",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(ts_status, 200);
    let ts_json: Value = serde_json::from_slice(&ts_body).expect("ts qr json");
    let ts_url = ts_json["url"].as_str().unwrap_or("");
    assert!(ts_url.contains("host=miku-pc.tailnet-alpha.ts.net"));
    assert!(ts_url.contains("pin=sha256:"));
    assert!(ts_json["svg"].as_str().unwrap_or("").contains("Miku-Link Pairing QR"));

    runner.shutdown_cleanly().expect("clean shutdown");
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_7_4_live_gateway_routes_llm_chat_delegate_choice_audio_and_mcp_catalog() {
    use std::net::TcpListener;
    use std::thread;

    let temp = unique_temp_dir("phase7_ext");
    let mock_llm = TcpListener::bind("127.0.0.1:0").expect("bind mock llm");
    let mock_port = mock_llm.local_addr().expect("mock addr").port();

    let server_handle = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = mock_llm.accept().expect("accept llm client");
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
            let mut req_buf = Vec::new();
            let mut chunk = [0u8; 2048];
            let mut hdr_end = None;
            let mut content_len = 0usize;
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        req_buf.extend_from_slice(&chunk[..n]);
                        if hdr_end.is_none() {
                            if let Some(pos) = req_buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                hdr_end = Some(pos);
                                let head = String::from_utf8_lossy(&req_buf[..pos]);
                                for line in head.lines() {
                                    if let Some((k, v)) = line.split_once(':') {
                                        if k.trim().eq_ignore_ascii_case("content-length") {
                                            content_len = v.trim().parse().unwrap_or(0);
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(pos) = hdr_end {
                            if req_buf.len() >= pos + 4 + content_len {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            let body = json!({
                "choices": [{ "message": { "content": "Respuesta real LLM desde gateway Rust" } }],
                "usage": { "prompt_tokens": 22, "completion_tokens": 8 }
            })
            .to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(resp.as_bytes()).expect("write mock llm");
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    });

    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: None,
        user_override_dir: Some(temp.join("ui_override")),
        use_memory_secrets: true,
    })
    .expect("start live gateway");

    let addr = runner.local_addr;
    let tok = runner.session_token.clone();

    // 0. Bridge sync contract: GET /api/state + /api/bootstrap must be routed (never 404)
    for p in ["/api/state", "/api/bootstrap"] {
        let (st, _, body) = raw_http_request(addr, "GET", p, &[("X-Miku-Token", &tok)], None);
        assert_eq!(st, 200, "{} must be routed by live gateway, body={}", p, String::from_utf8_lossy(&body));
    }

    // Configure provider + mock custom_base_url + vault key
    let _ = raw_http_request(
        addr,
        "POST",
        "/api/secrets",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(
            json!({
                "alias": "openai-local",
                "provider": "openai",
                "model": "gpt-4o-mini",
                "secret": "sk-mock-live-gateway"
            })
            .to_string()
            .as_bytes(),
        ),
    );
    for (k, v) in [
        ("provider", json!("openai")),
        ("model", json!("gpt-4o-mini")),
        ("custom_base_url", json!(format!("http://127.0.0.1:{}", mock_port))),
    ] {
        let (set_status, _, _) = raw_http_request(
            addr,
            "POST",
            "/api/settings",
            &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
            Some(json!({ "key": k, "value": v }).to_string().as_bytes()),
        );
        assert_eq!(set_status, 200);
    }

    // 1. Live LLM roundtrip over POST /api/chat
    let (chat_status, _, chat_body) = raw_http_request(
        addr,
        "POST",
        "/api/chat",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(
            json!({ "agent_id": "miku", "text": "Hola Miku, confirma estado" })
                .to_string()
                .as_bytes(),
        ),
    );
    assert_eq!(chat_status, 200);
    let chat_json: Value = serde_json::from_slice(&chat_body).expect("chat json");
    assert_eq!(chat_json["error"], false);
    assert_eq!(chat_json["text"], "Respuesta real LLM desde gateway Rust");

    // 2. Agent delegation route over POST /api/agents/delegate
    let (del_status, _, del_body) = raw_http_request(
        addr,
        "POST",
        "/api/agents/delegate",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(
            json!({ "from_agent": "miku", "to_agent": "miku", "task": "Revisar arquitectura", "mode": "code" })
                .to_string()
                .as_bytes(),
        ),
    );
    assert_eq!(del_status, 200);
    let del_json: Value = serde_json::from_slice(&del_body).expect("delegate json");
    assert_eq!(del_json["ok"], true);
    assert_eq!(
        del_json["work"]["result"],
        "Respuesta real LLM desde gateway Rust"
    );

    // 3. UI choice persistence over POST /api/ui/choice
    let (ch_status, _, ch_body) = raw_http_request(
        addr,
        "POST",
        "/api/ui/choice",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(
            json!({ "kind": "question", "decision": "OpciÃ³n A", "option_index": 0 })
                .to_string()
                .as_bytes(),
        ),
    );
    assert_eq!(ch_status, 200);
    let ch_json: Value = serde_json::from_slice(&ch_body).expect("choice json");
    assert_eq!(ch_json["decision"], "OpciÃ³n A");

    // 4. Audio devices & custom sound slots
    let (aud_status, _, aud_body) = raw_http_request(
        addr,
        "GET",
        "/api/audio/devices",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(aud_status, 200);
    let aud_json: Value = serde_json::from_slice(&aud_body).expect("audio json");
    assert_eq!(aud_json["ok"], true);
    assert!(aud_json["devices"].is_array());

    let (snd_status, _, snd_body) = raw_http_request(
        addr,
        "GET",
        "/api/sounds/list",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(snd_status, 200);
    let snd_json: Value = serde_json::from_slice(&snd_body).expect("sounds json");
    assert_eq!(snd_json["ok"], true);

    // 5. MCP catalog query & real persistence install
    let (cat_status, _, cat_body) = raw_http_request(
        addr,
        "GET",
        "/api/mcp/catalog",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(cat_status, 200);
    let cat_json: Value = serde_json::from_slice(&cat_body).expect("catalog json");
    assert!(!cat_json["catalog"].as_array().expect("catalog").is_empty());

    let (inst_status, _, inst_body) = raw_http_request(
        addr,
        "POST",
        "/api/mcp/catalog/install",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"id":"filesystem"}"#),
    );
    assert_eq!(inst_status, 200);
    let inst_json: Value = serde_json::from_slice(&inst_body).expect("install json");
    assert_eq!(inst_json["installed"], "filesystem");

    server_handle.join().expect("mock llm join");
    runner.shutdown_cleanly().expect("clean shutdown");
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_step1_live_doctor_uses_runtime_token_and_reports_6_of_6_pass() {
    let temp = unique_temp_dir("live_doctor");
    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: None,
        user_override_dir: Some(temp.join("ui_override")),
        use_memory_secrets: true,
    })
    .expect("start live gateway");

    let addr = runner.local_addr;
    let tok = runner.session_token.clone();

    let (status, _, body) = raw_http_request(
        addr,
        "GET",
        "/api/doctor",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(status, 200);
    let doc: Value = serde_json::from_slice(&body).expect("doctor json");
    assert_eq!(doc["overall_status"], "PASS", "doctor={}", doc);
    assert_eq!(doc["failed_count"], 0, "doctor={}", doc);
    assert_eq!(doc["passed_count"], 6, "doctor={}", doc);
    let probes = doc["probes"].as_array().expect("probes array");
    assert_eq!(probes.len(), 6);
    assert_eq!(probes[0]["probe_id"], "loopback_port");
    assert_eq!(probes[0]["status"], "PASS", "loopback probe={}", probes[0]);

    runner.shutdown_cleanly().expect("clean shutdown");
    let _ = fs::remove_dir_all(&temp);
}
