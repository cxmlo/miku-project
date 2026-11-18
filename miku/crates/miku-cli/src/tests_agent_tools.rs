use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::live_gateway::{LiveGatewayConfig, LiveGatewayRunner};

fn unique_temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "miku_agent_tools_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn post_api_json(addr: SocketAddr, token: &str, path: &str, body: &Value) -> (u16, Value) {
    let bytes = serde_json::to_vec(body).expect("json body");
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).expect("connect");
    let req = format!(
        "POST {} HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Miku-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        path,
        token,
        bytes.len()
    );
    stream.write_all(req.as_bytes()).expect("write hdr");
    stream.write_all(&bytes).expect("write body");
    let _ = stream.flush();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read resp");
    let hdr_end = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("hdr end");
    let status = String::from_utf8_lossy(&raw[..hdr_end])
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let parsed = serde_json::from_slice(&raw[hdr_end + 4..]).expect("json resp");
    (status, parsed)
}

fn read_http_json_body(stream: &mut TcpStream) -> Value {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut raw = Vec::new();
    let mut buf = [0u8; 2048];
    let mut hdr_end = None;
    let mut len = 0usize;
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if hdr_end.is_none() {
                    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        hdr_end = Some(pos);
                        let head = String::from_utf8_lossy(&raw[..pos]);
                        for line in head.lines() {
                            if let Some((k, v)) = line.split_once(':') {
                                if k.trim().eq_ignore_ascii_case("content-length") {
                                    len = v.trim().parse().unwrap_or(0);
                                }
                            }
                        }
                    }
                }
                if let Some(pos) = hdr_end {
                    if raw.len() >= pos + 4 + len {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
    let pos = hdr_end.expect("mock hdr");
    serde_json::from_slice(&raw[pos + 4..pos + 4 + len]).expect("mock req json")
}

fn send_http_json(stream: &mut TcpStream, body: &Value) {
    let raw = body.to_string();
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        raw.len(),
        raw
    );
    stream.write_all(resp.as_bytes()).expect("write mock");
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

#[test]
fn test_step2_deepseek_multi_round_tool_calls_gatekeeper_and_history() {
    let temp = unique_temp_dir("multi_round");
    let mock = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let port = mock.local_addr().expect("mock addr").port();

    let server = thread::spawn(move || {
        // Call 1 (turn 1, round 1): emit tool_calls exercising allow + ask + deny
        let (mut s1, _) = mock.accept().expect("accept 1");
        let req1 = read_http_json_body(&mut s1);
        assert_eq!(req1["model"], "deepseek-chat");
        assert!(req1["tools"].as_array().expect("tools").len() >= 10);
        send_http_json(
            &mut s1,
            &json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [
                            { "id": "t1", "type": "function", "function": { "name": "system_info", "arguments": "{}" } },
                            { "id": "t2", "type": "function", "function": { "name": "fs_write", "arguments": r#"{"path":"workspace/demo.txt","content":"hola_miku"}"# } },
                            { "id": "t3", "type": "function", "function": { "name": "fs_write", "arguments": r#"{"path":"notes/req.txt","content":"pedir"}"# } },
                            { "id": "t4", "type": "function", "function": { "name": "fs_write", "arguments": r#"{"path":"src/secrets/leak.txt","content":"nope"}"# } },
                            { "id": "t5", "type": "function", "function": { "name": "shell_exec", "arguments": r#"{"command":"git push origin main"}"# } },
                            { "id": "t6", "type": "function", "function": { "name": "shell_exec", "arguments": r#"{"command":"rm -rf /"}"# } }
                        ]
                    }
                }],
                "usage": { "prompt_tokens": 50, "completion_tokens": 20 }
            }),
        );

        // Call 2 (turn 1, round 2): verify tool outputs and emit second round fs_read + apps_detect + skills_list + fs_list + clipboard_read + media_control
        let (mut s2, _) = mock.accept().expect("accept 2");
        let req2 = read_http_json_body(&mut s2);
        let msgs2 = req2["messages"].as_array().expect("msgs2");
        let tool_msgs: Vec<&Value> = msgs2.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 6);
        assert!(tool_msgs[0]["content"].as_str().unwrap().contains("\"ok\":true"));
        assert!(tool_msgs[1]["content"].as_str().unwrap().contains("\"bytes_written\":9"));
        assert!(tool_msgs[2]["content"].as_str().unwrap().contains("\"approval_required\""));
        assert!(tool_msgs[3]["content"].as_str().unwrap().contains("\"denied\""));
        assert!(tool_msgs[4]["content"].as_str().unwrap().contains("\"denied\""));
        assert!(tool_msgs[5]["content"].as_str().unwrap().contains("immutable_shell_denylist"));
        send_http_json(
            &mut s2,
            &json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [
                            { "id": "t7", "type": "function", "function": { "name": "fs_read", "arguments": r#"{"path":"workspace/demo.txt"}"# } },
                            { "id": "t8", "type": "function", "function": { "name": "apps_detect", "arguments": "{}" } },
                            { "id": "t9", "type": "function", "function": { "name": "skills_list", "arguments": "{}" } },
                            { "id": "t10", "type": "function", "function": { "name": "fs_list", "arguments": r#"{"path":"workspace"}"# } },
                            { "id": "t11", "type": "function", "function": { "name": "clipboard_read", "arguments": "{}" } },
                            { "id": "t12", "type": "function", "function": { "name": "media_control", "arguments": r#"{"action":"mute"}"# } },
                            { "id": "t13", "type": "function", "function": { "name": "memory_search", "arguments": r#"{"query":"herramientas","limit":3}"# } }
                        ]
                    }
                }],
                "usage": { "prompt_tokens": 60, "completion_tokens": 12 }
            }),
        );

        // Call 3 (turn 1, round 3): verify fs_read, fs_list, clipboard_read AskPending, media_control OK, memory_search OK
        let (mut s3, _) = mock.accept().expect("accept 3");
        let req3 = read_http_json_body(&mut s3);
        let msgs3 = req3["messages"].as_array().expect("msgs3");
        let round2_tools: Vec<&Value> = msgs3.iter().rev().take(7).collect();
        let combined: String = round2_tools
            .iter()
            .filter_map(|m| m["content"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("hola_miku"), "fs_read: {}", combined);
        assert!(combined.contains("demo.txt"), "fs_list: {}", combined);
        assert!(combined.contains("\"approval_required\""), "clipboard_read: {}", combined);
        assert!(combined.contains("\"action\":\"mute\""), "media_control: {}", combined);
        assert!(combined.contains("\"hits\""), "memory_search: {}", combined);
        send_http_json(
            &mut s3,
            &json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "Archivo leído y Gatekeeper verificado." }
                }],
                "usage": { "prompt_tokens": 70, "completion_tokens": 10 }
            }),
        );

        // Call 4 (turn 2): verify prior history turns are included + test memory_search
        let (mut s4, _) = mock.accept().expect("accept 4");
        let req4 = read_http_json_body(&mut s4);
        let msgs4 = req4["messages"].as_array().expect("msgs4");
        assert!(msgs4.iter().any(|m| m["role"] == "assistant" && m["content"].as_str().unwrap_or("").contains("Archivo leído")));
        send_http_json(
            &mut s4,
            &json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "Historial confirmado en SQLite." }
                }],
                "usage": { "prompt_tokens": 40, "completion_tokens": 8 }
            }),
        );
    });

    let runner = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir: temp.clone(),
        port: 0,
        default_ui_dir: None,
        user_override_dir: Some(temp.join("ui")),
        use_memory_secrets: true,
    })
    .expect("start gateway");
    let addr = runner.local_addr;
    let tok = runner.session_token.clone();

    let (st_sec, sec_resp) = post_api_json(
        addr,
        &tok,
        "/api/secrets",
        &json!({
            "alias": "deepseek-test",
            "provider": "deepseek",
            "model": "deepseek-chat",
            "secret": "sk-mock-deepseek"
        }),
    );
    assert_eq!(st_sec, 200);
    assert_eq!(sec_resp["provider"], "deepseek");

    let (st_cfg, _) = post_api_json(
        addr,
        &tok,
        "/api/settings",
        &json!({ "key": "custom_base_url", "value": format!("http://127.0.0.1:{}", port) }),
    );
    assert_eq!(st_cfg, 200);
    let (st_tok, _) = post_api_json(
        addr,
        &tok,
        "/api/settings",
        &json!({ "key": "max_tokens", "value": 256 }),
    );
    assert_eq!(st_tok, 200);

    let (st1, r1) = post_api_json(
        addr,
        &tok,
        "/api/chat",
        &json!({ "agent_id": "miku", "text": "prueba herramientas" }),
    );
    assert_eq!(st1, 200);
    assert_eq!(r1["error"], false);
    assert_eq!(r1["text"], "Archivo leído y Gatekeeper verificado.");
    let steps = r1["steps"].as_array().expect("steps");
    assert!(steps.iter().any(|s| s.as_str().unwrap_or("").contains("Tool fs_write -> OK")));
    assert!(steps.iter().any(|s| s.as_str().unwrap_or("").contains("Tool fs_read -> OK")));
    assert!(temp.join("workspace").join("demo.txt").is_file());

    let (st2, r2) = post_api_json(
        addr,
        &tok,
        "/api/chat",
        &json!({ "agent_id": "miku", "text": "recuerdas el turno anterior?" }),
    );
    assert_eq!(st2, 200);
    assert_eq!(r2["text"], "Historial confirmado en SQLite.");

    server.join().expect("mock server");
    runner.shutdown_cleanly().expect("shutdown");
    let _ = fs::remove_dir_all(&temp);
}
