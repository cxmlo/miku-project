use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::live_gateway::{LiveGatewayConfig, LiveGatewayRunner};

static TEST_86_SEQ: AtomicU64 = AtomicU64::new(1);

fn unique_temp_dir(tag: &str) -> PathBuf {
    let n = TEST_86_SEQ.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!(
        "miku_8_6_{}_{}_{}",
        tag,
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).expect("create temp dir");
    p
}

fn raw_http_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).expect("connect loopback");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set timeout");
    let body_bytes = body.unwrap_or(&[]);
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\nContent-Length: {}\r\n",
        method,
        path,
        addr.port(),
        body_bytes.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{}: {}\r\n", k, v));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).expect("write header");
    if !body_bytes.is_empty() {
        stream.write_all(body_bytes).expect("write body");
    }
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    let split_pos = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("http header separator");
    let hdr_txt = String::from_utf8_lossy(&raw[..split_pos]).to_string();
    let status = hdr_txt
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .expect("status line");
    (status, hdr_txt, raw[split_pos + 4..].to_vec())
}

#[test]
fn test_8_6_permission_mode_selector_updates_gatekeeper_policy_consistently() {
    let temp = unique_temp_dir("perm_mode");
    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: None,
        user_override_dir: Some(temp.join("ui_override")),
        use_memory_secrets: true,
    })
    .expect("start gateway");
    let addr = runner.local_addr;
    let tok = runner.session_token.clone();

    // 1. Default permission mode must be "sandbox"
    let (st_code, _, st_body) = raw_http_request(
        addr,
        "GET",
        "/api/state",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(st_code, 200);
    let st_json: Value = serde_json::from_slice(&st_body).expect("state json");
    assert_eq!(st_json["permission_mode"], "sandbox");

    // 2. Reject invalid mode
    let (bad_code, _, _) = raw_http_request(
        addr,
        "POST",
        "/api/gatekeeper/mode",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"mode":"godmode"}"#),
    );
    assert_eq!(bad_code, 400);

    // 3. Switch to "restricted": shell & write rules become deny
    let (res_code, _, res_body) = raw_http_request(
        addr,
        "POST",
        "/api/gatekeeper/mode",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"mode":"restricted"}"#),
    );
    assert_eq!(res_code, 200);
    let res_json: Value = serde_json::from_slice(&res_body).expect("restricted json");
    assert_eq!(res_json["permission_mode"], "restricted");
    let rules = res_json["rules"].as_array().expect("rules array");
    assert!(rules
        .iter()
        .any(|r| r["name"] == "fs_write: **" && r["tier"] == "deny"));
    assert!(rules
        .iter()
        .any(|r| r["name"] == "shell: *" && r["tier"] == "deny"));
    assert!(rules
        .iter()
        .any(|r| r["name"] == "fs_read: **" && r["tier"] == "allow"));

    // 4. Turn Kill Switch ON, switch mode to "full": Kill Switch must remain active,
    //    AND immutable denylist rules on shell remain present.
    let (ks_code, _, _) = raw_http_request(
        addr,
        "POST",
        "/api/gatekeeper/kill_switch",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"active":true}"#),
    );
    assert_eq!(ks_code, 200);

    let (full_code, _, full_body) = raw_http_request(
        addr,
        "POST",
        "/api/gatekeeper/mode",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(br#"{"mode":"Toda mi PC"}"#),
    );
    assert_eq!(full_code, 200);
    let full_json: Value = serde_json::from_slice(&full_body).expect("full json");
    assert_eq!(full_json["permission_mode"], "full");
    assert_eq!(full_json["kill_switch"], true);
    let full_rules = full_json["rules"].as_array().expect("full rules");
    assert!(full_rules
        .iter()
        .any(|r| r["name"] == "fs_write: **" && r["tier"] == "allow"));
    assert!(full_rules
        .iter()
        .any(|r| r["name"] == "shell: *git push*" && r["tier"] == "deny"));
    assert!(full_rules
        .iter()
        .any(|r| r["name"] == "shell: *rm -rf*" && r["tier"] == "deny"));

    runner.shutdown_cleanly().expect("shutdown first");

    // 5. Restart gateway on same data_dir -> "permission_mode": "full" must persist
    let runner2 = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: None,
        user_override_dir: Some(temp.join("ui_override")),
        use_memory_secrets: true,
    })
    .expect("restart gateway");
    let (s2, _, b2) = raw_http_request(
        runner2.local_addr,
        "GET",
        "/api/state",
        &[("X-Miku-Token", &runner2.session_token)],
        None,
    );
    assert_eq!(s2, 200);
    let j2: Value = serde_json::from_slice(&b2).expect("restarted state");
    assert_eq!(j2["permission_mode"], "full");

    runner2.shutdown_cleanly().expect("shutdown second");
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_8_6_telemetry_usage_and_settings_reset_roundtrip() {
    let temp = unique_temp_dir("telemetry_reset");
    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: None,
        user_override_dir: Some(temp.join("ui_override")),
        use_memory_secrets: true,
    })
    .expect("start gateway");
    let addr = runner.local_addr;
    let tok = runner.session_token.clone();

    let (t_code, _, t_body) = raw_http_request(
        addr,
        "GET",
        "/api/telemetry/usage",
        &[("X-Miku-Token", &tok)],
        None,
    );
    assert_eq!(t_code, 200);
    let t_json: Value = serde_json::from_slice(&t_body).expect("telemetry json");
    assert_eq!(t_json["ok"], true);
    assert_eq!(t_json["total_requests"], 0);

    let (tr_code, _, tr_body) = raw_http_request(
        addr,
        "POST",
        "/api/telemetry/reset",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(b"{}"),
    );
    assert_eq!(tr_code, 200);
    assert_eq!(json!(serde_json::from_slice::<Value>(&tr_body).unwrap())["ok"], true);

    let (sr_code, _, _) = raw_http_request(
        addr,
        "POST",
        "/api/settings/reset",
        &[("X-Miku-Token", &tok), ("Content-Type", "application/json")],
        Some(b"{}"),
    );
    assert_eq!(sr_code, 200);

    let (_, _, st_body) = raw_http_request(
        addr,
        "GET",
        "/api/state",
        &[("X-Miku-Token", &tok)],
        None,
    );
    let st_json: Value = serde_json::from_slice(&st_body).expect("state after reset");
    assert_eq!(st_json["settings"]["lang"], "es");
    assert_eq!(st_json["settings"]["autoclose"], 15);

    runner.shutdown_cleanly().expect("shutdown");
    let _ = fs::remove_dir_all(&temp);
}
