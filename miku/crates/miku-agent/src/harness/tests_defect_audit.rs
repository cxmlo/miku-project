use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::harness::providers::{parse_endpoint, send_provider_http_post};
use crate::harness::turn_loop::AgentHarness;
use crate::harness::types::{HarnessConfig, HarnessError, LlmProvider, ToolCallRequest, TurnStepOutcome};
use crate::secrets::backend::EncryptedMemoryBackend;
use crate::secrets::manager::SecretManager;
use crate::ToolRegistry;

const TEST_SESSION_TOKEN: &str = "test-agent-harness-audit-token";

fn setup_audit_gatekeeper() -> GatekeeperEngine {
    let manifest_dir = env!("CARGO_MANIFEST_DIR").replace('\\', "/");
    let profile = ProjectProfile::new("harness-audit-test", &manifest_dir, 60_000)
        .with_rule(Capability::Network, "*", PolicyEffect::Allow, false)
        .with_rule(Capability::FsRead, "*", PolicyEffect::Allow, false)
        .with_rule(Capability::FsRead, "*/restricted/*", PolicyEffect::Deny, false)
        .with_rule(Capability::Shell, "*", PolicyEffect::Allow, false);
    GatekeeperEngine::new(profile).expect("Gatekeeper engine should build")
}

async fn setup_secret_manager_with_keys(
    keys: &[(&str, &str, &[u8])],
) -> Arc<Mutex<SecretManager>> {
    let backend = Box::new(EncryptedMemoryBackend::new());
    let gk = setup_audit_gatekeeper();
    let mut sm = SecretManager::new(backend, TEST_SESSION_TOKEN.to_string(), gk);
    for (alias, prov, sec) in keys {
        sm.set_secret(alias, prov, None, sec)
            .expect("set_secret in test harness");
    }
    Arc::new(Mutex::new(sm))
}

async fn spawn_mock_http_server<F>(handler: F) -> (SocketAddr, tokio::task::JoinHandle<()>)
where
    F: Fn(String, Vec<u8>) -> (u16, &'static str, &'static str) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let addr = listener.local_addr().expect("local addr");
    let handler = Arc::new(handler);

    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let handler = handler.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if let Some(idx) = buf[..n].windows(4).position(|w| w == b"\r\n\r\n") {
                    buf[idx + 4..n].to_vec()
                } else {
                    Vec::new()
                };

                let (status, content_type, response_body) = handler(req_str, body);
                let response = format!(
                    "HTTP/1.1 {} OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    content_type,
                    response_body.len(),
                    response_body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });

    (addr, handle)
}

/// [DEFECT-1A] parse_endpoint preserves scheme, validates port and rejects remote plaintext HTTP
#[test]
fn test_parse_endpoint_preserves_scheme_and_rejects_insecure_remote_http() {
    // 1. Remote HTTPS: scheme https, default port 443
    let target = parse_endpoint("https://api.deepseek.com/v1/chat/completions")
        .expect("remote HTTPS must parse successfully");
    assert_eq!(target.scheme, "https");
    assert_eq!(target.host, "api.deepseek.com");
    assert_eq!(target.port, 443);
    assert_eq!(target.path, "/v1/chat/completions");

    // 2. Local loopback HTTP (Ollama): scheme http, custom port
    let target_ollama = parse_endpoint("http://127.0.0.1:11434/api/chat")
        .expect("local loopback HTTP must parse successfully");
    assert_eq!(target_ollama.scheme, "http");
    assert_eq!(target_ollama.host, "127.0.0.1");
    assert_eq!(target_ollama.port, 11434);
    assert_eq!(target_ollama.path, "/api/chat");

    // 3. Localhost HTTP
    let target_local = parse_endpoint("http://localhost:8080/v1/completions")
        .expect("localhost HTTP must parse successfully");
    assert_eq!(target_local.scheme, "http");
    assert_eq!(target_local.host, "localhost");
    assert_eq!(target_local.port, 8080);

    // 4. Remote HTTP is forbidden and fails closed with InsecureTransport
    let err = parse_endpoint("http://api.deepseek.com/v1/chat/completions")
        .expect_err("remote plaintext HTTP must be rejected");
    match err {
        HarnessError::InsecureTransport(msg) => {
            assert!(
                msg.contains("remote") || msg.contains("plaintext") || msg.contains("rejected"),
                "error message should indicate remote HTTP rejection: {}",
                msg
            );
        }
        other => panic!("expected InsecureTransport error, got {:?}", other),
    }

    // 5. Unsupported scheme rejected
    let err_scheme = parse_endpoint("ftp://example.com/api")
        .expect_err("unsupported scheme must be rejected");
    assert!(matches!(err_scheme, HarnessError::InsecureTransport(_)));

    // 6. 0.0.0.0 and :: (unspecified addresses) MUST NOT be treated as loopback
    let err_unspec4 = parse_endpoint("http://0.0.0.0:8000/api")
        .expect_err("http://0.0.0.0 must be rejected as non-loopback");
    assert!(matches!(err_unspec4, HarnessError::InsecureTransport(_)));

    let err_unspec6 = parse_endpoint("http://[::]:8000/api")
        .expect_err("http://[::] must be rejected as non-loopback");
    assert!(matches!(err_unspec6, HarnessError::InsecureTransport(_)));
}

#[tokio::test]
async fn test_send_provider_http_post_rejects_0_0_0_0_and_ipv6_unspecified() {
    let err_v4 = send_provider_http_post(
        "http://0.0.0.0:8000/api",
        LlmProvider::Ollama,
        Some(b"sk-secret-key-4"),
        b"{}",
        1000,
    )
    .await
    .expect_err("0.0.0.0 HTTP must be rejected");
    assert!(matches!(err_v4, HarnessError::InsecureTransport(_)));

    let err_v6 = send_provider_http_post(
        "http://[::]:8000/api",
        LlmProvider::Ollama,
        Some(b"sk-secret-key-6"),
        b"{}",
        1000,
    )
    .await
    .expect_err("[::] HTTP must be rejected");
    assert!(matches!(err_v6, HarnessError::InsecureTransport(_)));
}

/// [DEFECT-1B] HTTPS transport executes real TLS handshake over wire; raw wire bytes contain no plaintext secret
#[tokio::test]
async fn test_https_transport_enforces_tls_handshake_and_no_plaintext_secret_on_wire() {
    let sentinel_secret = "sk-wire-tls-sentinel-key-SECRET999";

    // Bind a raw TCP listener. When WinHTTP connects with https://, it must send a TLS ClientHello
    let raw_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind raw listener");
    let port = raw_listener.local_addr().expect("port").port();

    let server_handle = tokio::spawn(async move {
        let (mut socket, _) = raw_listener.accept().await.expect("accept raw socket");
        let mut raw_buf = vec![0u8; 4096];
        let n = socket.read(&mut raw_buf).await.unwrap_or(0);
        raw_buf.truncate(n);
        // Immediately drop connection without completing TLS handshake to fail closed safely
        drop(socket);
        raw_buf
    });

    let endpoint = format!("https://127.0.0.1:{}/v1/chat/completions", port);
    let send_result = send_provider_http_post(
        &endpoint,
        LlmProvider::DeepSeek,
        Some(sentinel_secret.as_bytes()),
        b"{}",
        2_000,
    )
    .await;

    // Client must fail-closed because raw mock server didn't complete trusted TLS handshake
    assert!(
        send_result.is_err(),
        "untrusted/incomplete TLS connection must fail closed"
    );

    let wire_bytes = server_handle.await.expect("join server handle");
    assert!(
        !wire_bytes.is_empty(),
        "client must have initiated TCP connection and sent data"
    );

    // In a real TLS connection, byte 0 is Record Type 22 (0x16 = Handshake)
    // and byte 1 is SSL 3.0 / TLS 1.x major version 3 (0x03)
    assert_eq!(
        wire_bytes[0], 0x16,
        "first byte on wire must be TLS Handshake record type 0x16"
    );
    assert_eq!(
        wire_bytes[1], 0x03,
        "second byte on wire must be TLS major version 0x03"
    );

    // INVARIANT: Plaintext secret must NEVER be visible anywhere in the raw wire bytes!
    let wire_text = String::from_utf8_lossy(&wire_bytes);
    assert!(
        !wire_text.contains(sentinel_secret),
        "CRITICAL: plaintext secret was found on the raw wire bytes!"
    );
}

/// [DEFECT-2] dispatch_tool_call connects to real AgentSessionManager, executes real tools, and Gatekeeper blocks denied
#[tokio::test]
async fn test_dispatch_tool_call_connects_to_real_session_manager_and_gatekeeper_blocks_denied() {
    let registry = ToolRegistry::with_default_internal_tools();
    let gatekeeper = setup_audit_gatekeeper();
    let sm = setup_secret_manager_with_keys(&[]).await;

    let config = HarnessConfig::new(
        LlmProvider::Ollama,
        "llama3",
        "http://127.0.0.1:11434/api/chat",
        vec![],
    );

    let mut harness = AgentHarness::new(
        config,
        sm,
        registry,
        gatekeeper,
        "session-real-tool-dispatch",
        TEST_SESSION_TOKEN,
    );

    // 1. Authorized tool call: fs_read on a REAL file on disk
    let cargo_toml_path = format!("{}/Cargo.toml", env!("CARGO_MANIFEST_DIR").replace('\\', "/"));
    let call_allowed = ToolCallRequest {
        tool_name: "fs_read".to_string(),
        raw_args: format!(r#"{{"path": "{}"}}"#, cargo_toml_path),
    };
    let res = harness
        .dispatch_tool_call(&call_allowed)
        .await
        .expect("authorized tool call must succeed through real executor");
    assert!(
        res.contains("name = \"miku-agent\""),
        "result must contain real Cargo.toml content, not synthetic ok: {}",
        res
    );
    assert!(
        !res.contains("fs_ok"),
        "result must NOT contain synthetic fs_ok: {}",
        res
    );

    // 2. Tool without real executor (e.g. shell_exec): must fail explicitly as unimplemented, NOT fake shell_ok
    let call_unimplemented = ToolCallRequest {
        tool_name: "shell_exec".to_string(),
        raw_args: r#"{"command": "echo hello"}"#.to_string(),
    };
    let err_unimpl = harness
        .dispatch_tool_call(&call_unimplemented)
        .await
        .expect_err("tool without executor must return explicit error");
    match err_unimpl {
        HarnessError::ToolExecutionFailed { tool_name, detail } => {
            assert_eq!(tool_name, "shell_exec");
            assert!(
                detail.contains("not implemented") || detail.contains("unimplemented"),
                "detail should state not implemented: {}",
                detail
            );
        }
        other => panic!("expected ToolExecutionFailed with not implemented, got {:?}", other),
    }

    // Real Gatekeeper audit chain must have recorded entries
    assert!(
        !harness.gatekeeper().audit_log().is_empty(),
        "audit log chain must be non-empty after execution"
    );

    // 3. Denied tool call: fs_read on restricted path
    let call_denied = ToolCallRequest {
        tool_name: "fs_read".to_string(),
        raw_args: r#"{"path": "C:/Users/cmiranda/dev/miku/restricted/secret.key"}"#.to_string(),
    };
    let err_denied = harness
        .dispatch_tool_call(&call_denied)
        .await
        .expect_err("restricted target must be denied by Gatekeeper");
    assert!(
        matches!(err_denied, HarnessError::GatekeeperDenied(_)),
        "expected GatekeeperDenied, got: {:?}",
        err_denied
    );

    // 3. Unregistered tool call
    let call_unknown = ToolCallRequest {
        tool_name: "nonexistent_custom_tool".to_string(),
        raw_args: r#"{"arg": 1}"#.to_string(),
    };
    let err_unknown = harness
        .dispatch_tool_call(&call_unknown)
        .await
        .expect_err("unregistered tool must fail with ToolExecutionFailed");
    assert!(
        matches!(err_unknown, HarnessError::ToolExecutionFailed { .. }),
        "expected ToolExecutionFailed, got: {:?}",
        err_unknown
    );
}

/// [DEFECT-3] Agent harness sanitizes reflected secret in upstream HTTP error messages
#[tokio::test]
async fn test_agent_harness_sanitizes_reflected_secret_in_http_error_response() {
    let sentinel_secret = "sk-reflected-test-secret-ABCXYZ456";

    // Server deliberately reflects the secret in its error response body
    let (addr, _server) = spawn_mock_http_server(move |_headers, _body| {
        (
            400,
            "application/json",
            r#"{"error":{"message":"Invalid authentication: key sk-reflected-test-secret-ABCXYZ456 has been revoked"}}"#,
        )
    })
    .await;

    let sm = setup_secret_manager_with_keys(&[(
        "test-reflect-alias",
        "deepseek",
        sentinel_secret.as_bytes(),
    )])
    .await;

    let mut config = HarnessConfig::new(
        LlmProvider::DeepSeek,
        "deepseek-chat",
        &format!("http://{}/v1/chat/completions", addr),
        vec!["test-reflect-alias".to_string()],
    );
    config.initial_backoff_ms = 0;

    let tool_registry = ToolRegistry::with_default_internal_tools();
    let gatekeeper = setup_audit_gatekeeper();

    let mut harness = AgentHarness::new(
        config,
        sm,
        tool_registry,
        gatekeeper,
        "session-test-sanitize-reflect",
        TEST_SESSION_TOKEN,
    );

    let err = harness
        .execute_turn("Test query", None)
        .await
        .expect_err("must produce error on 400 with reflected secret");

    let err_string = format!("{}", err);
    let err_debug = format!("{:?}", err);

    assert!(
        !err_string.contains(sentinel_secret),
        "secret was reflected in Display error string: {}",
        err_string
    );
    assert!(
        !err_debug.contains(sentinel_secret),
        "secret was reflected in Debug error string: {}",
        err_debug
    );
    assert!(
        err_string.contains("[REDACTED_SECRET]") || err_debug.contains("[REDACTED_SECRET]"),
        "error message should contain [REDACTED_SECRET]: {}",
        err_string
    );
}

/// [DEFECT-4] Multiple tool calls in single response halt at max_steps and resume with saturating_add
#[tokio::test]
async fn test_agent_harness_multiple_tool_calls_halt_at_max_steps_and_resume_saturating() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    // First model response returns 3 tool calls.
    // Second model response (after resumption) returns text completion.
    let (addr, _server) = spawn_mock_http_server(move |_headers, _body| {
        let n = cc.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            (
                200,
                "application/json",
                r#"{
                    "choices": [
                        {
                            "message": {
                                "tool_calls": [
                                    {"function": {"name": "fs_read", "arguments": "{\"path\":\"C:/Users/cmiranda/dev/miku/allowed/a.txt\"}"}},
                                    {"function": {"name": "fs_read", "arguments": "{\"path\":\"C:/Users/cmiranda/dev/miku/allowed/b.txt\"}"}},
                                    {"function": {"name": "fs_read", "arguments": "{\"path\":\"C:/Users/cmiranda/dev/miku/allowed/c.txt\"}"}}
                                ]
                            }
                        }
                    ]
                }"#,
            )
        } else {
            (
                200,
                "application/json",
                r#"{"choices":[{"message":{"content":"All 3 tools completed successfully!"}}]}"#,
            )
        }
    })
    .await;

    let sm = setup_secret_manager_with_keys(&[]).await;

    let mut config = HarnessConfig::new(
        LlmProvider::Ollama,
        "mock-model",
        &format!("http://{}/v1/chat/completions", addr),
        vec![],
    );
    config.max_steps = 2; // Allow only 2 steps before requiring confirmation!

    let tool_registry = ToolRegistry::with_default_internal_tools();
    let gatekeeper = setup_audit_gatekeeper();

    let mut harness = AgentHarness::new(
        config,
        sm,
        tool_registry,
        gatekeeper,
        "session-multi-tools",
        TEST_SESSION_TOKEN,
    );

    harness.set_simulated_tool_handler(|_tool, _args| {
        Ok("mock read completed".to_string())
    });

    // Turn should stop at step 2 with NeedsConfirmation
    let outcome = harness
        .execute_turn("Process all files", None)
        .await
        .expect("turn execution should return NeedsConfirmation");

    match outcome {
        TurnStepOutcome::NeedsConfirmation {
            step,
            max_steps,
            pending_tool,
        } => {
            assert_eq!(step, 2, "must halt exactly at max_steps (2)");
            assert_eq!(max_steps, 2);
            assert_eq!(pending_tool, Some("fs_read".to_string()));
            assert!(
                harness.needs_confirmation,
                "harness must be in needs_confirmation state"
            );
        }
        other => panic!("expected NeedsConfirmation at step 2, got {:?}", other),
    }

    // Resume with 5 additional steps
    let resumed_outcome = harness
        .confirm_and_resume(5, None)
        .await
        .expect("resuming should complete remaining tools and finish turn");

    match resumed_outcome {
        TurnStepOutcome::Completed { reply, total_steps } => {
            assert!(
                reply.contains("All 3 tools completed successfully"),
                "reply: {}",
                reply
            );
            assert_eq!(total_steps, 4); // 1 initial LLM query + 3 tools executed
        }
        other => panic!("expected Completed outcome after resumption, got {:?}", other),
    }

    // Verify saturating_add prevents overflow even near usize::MAX
    harness.config.max_steps = usize::MAX - 2;
    let _ = harness.confirm_and_resume(10, None).await;
    assert_eq!(harness.config.max_steps, usize::MAX);
}
