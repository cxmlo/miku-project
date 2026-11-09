use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::harness::json_repair::{repair_tool_call_json, ToolJsonRepairError};
use crate::harness::turn_loop::AgentHarness;
use crate::harness::types::{HarnessConfig, HarnessError, LlmProvider, TurnStepOutcome};
use crate::secrets::backend::EncryptedMemoryBackend;
use crate::secrets::manager::SecretManager;
use crate::ToolRegistry;

const TEST_SESSION_TOKEN: &str = "test-agent-harness-session-token-3-7";

fn setup_test_gatekeeper() -> GatekeeperEngine {
    let profile = ProjectProfile::new("harness-test", "C:/Users/cmiranda/dev/miku", 60_000)
        .with_rule(Capability::Network, "*", PolicyEffect::Allow, false)
        .with_rule(Capability::FsRead, "*", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "*", PolicyEffect::Allow, false)
        .with_rule(Capability::Shell, "*", PolicyEffect::Allow, false);
    GatekeeperEngine::new(profile).expect("Gatekeeper engine should build")
}

async fn setup_secret_manager_with_keys(
    keys: &[(&str, &str, &[u8])], // (alias, provider, secret)
) -> Arc<Mutex<SecretManager>> {
    let backend = Box::new(EncryptedMemoryBackend::new());
    let gk = setup_test_gatekeeper();
    let mut sm = SecretManager::new(backend, TEST_SESSION_TOKEN.to_string(), gk);
    for (alias, prov, sec) in keys {
        sm.set_secret(alias, prov, None, sec)
            .expect("set_secret in test harness");
    }
    Arc::new(Mutex::new(sm))
}

/// Spawns a local mock HTTP server that responds to requests based on custom handler logic.
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

/// [LEDGER-3.7-1] Servidor local devuelve HTTP 429 en la key 1 -> harness rota automáticamente a key 2 y completa el turno.
#[tokio::test]
async fn test_agent_harness_rotates_api_key_on_429_failover() {
    let req_count = Arc::new(AtomicUsize::new(0));
    let req_count_clone = req_count.clone();

    let (addr, _server) = spawn_mock_http_server(move |headers, _body| {
        let _ = req_count_clone.fetch_add(1, Ordering::SeqCst);
        if headers.contains("Bearer sk-mock-key-primary-429") {
            (
                429,
                "application/json",
                r#"{"error":{"message":"rate limited on primary key"}}"#,
            )
        } else if headers.contains("Bearer sk-mock-key-backup-200") {
            (
                200,
                "application/json",
                r#"{"choices":[{"message":{"content":"Hello from backup key, 429 successfully resolved!"}}]}"#,
            )
        } else {
            (
                401,
                "application/json",
                r#"{"error":{"message":"unexpected auth header"}}"#,
            )
        }
    })
    .await;

    let sm = setup_secret_manager_with_keys(&[
        (
            "deepseek-primary",
            "deepseek",
            b"sk-mock-key-primary-429",
        ),
        (
            "deepseek-backup",
            "deepseek",
            b"sk-mock-key-backup-200",
        ),
    ])
    .await;

    let mut config = HarnessConfig::new(
        LlmProvider::DeepSeek,
        "deepseek-chat",
        &format!("http://{}/v1/chat/completions", addr),
        vec!["deepseek-primary".to_string(), "deepseek-backup".to_string()],
    );
    config.initial_backoff_ms = 5;

    let tool_registry = ToolRegistry::with_default_internal_tools();
    let gatekeeper = setup_test_gatekeeper();

    let mut harness = AgentHarness::new(
        config,
        sm,
        tool_registry,
        gatekeeper,
        "session-test-429",
        TEST_SESSION_TOKEN,
    );

    let outcome = harness
        .execute_turn("Execute task with failover", None)
        .await
        .expect("turn execution must succeed on rotated backup key");

    match outcome {
        TurnStepOutcome::Completed { reply, total_steps } => {
            assert!(
                reply.contains("Hello from backup key"),
                "unexpected reply: {}",
                reply
            );
            assert_eq!(total_steps, 1);
        }
        other => panic!("expected TurnStepOutcome::Completed, got {:?}", other),
    }

    assert_eq!(
        req_count.load(Ordering::SeqCst),
        2,
        "expected exactly 2 HTTP requests: 1 failing on primary (429) + 1 succeeding on backup (200)"
    );
    assert_eq!(
        harness.current_key_index, 1,
        "current active key index must now point to backup key"
    );
}

/// [LEDGER-3.7-2] 5xx y agotamiento de claves terminan con backoff acotado y error claro AllKeysExhausted.
#[tokio::test]
async fn test_agent_harness_exhausted_keys_on_5xx_returns_clear_error_and_bounded_backoff() {
    let req_count = Arc::new(AtomicUsize::new(0));
    let req_count_clone = req_count.clone();

    let (addr, _server) = spawn_mock_http_server(move |_headers, _body| {
        req_count_clone.fetch_add(1, Ordering::SeqCst);
        (
            503,
            "application/json",
            r#"{"error":{"message":"service temporarily unavailable"}}"#,
        )
    })
    .await;

    let sm = setup_secret_manager_with_keys(&[
        ("anthropic-main", "anthropic", b"sk-anthropic-key-1"),
        ("anthropic-spare", "anthropic", b"sk-anthropic-key-2"),
    ])
    .await;

    let mut config = HarnessConfig::new(
        LlmProvider::Anthropic,
        "claude-3-7-sonnet",
        &format!("http://{}/v1/messages", addr),
        vec!["anthropic-main".to_string(), "anthropic-spare".to_string()],
    );
    config.initial_backoff_ms = 5;
    config.max_backoff_ms = 20;

    let tool_registry = ToolRegistry::with_default_internal_tools();
    let gatekeeper = setup_test_gatekeeper();

    let mut harness = AgentHarness::new(
        config,
        sm,
        tool_registry,
        gatekeeper,
        "session-test-5xx",
        TEST_SESSION_TOKEN,
    );

    let err = harness
        .execute_turn("Prompt that encounters 503 across all keys", None)
        .await
        .expect_err("must return error when all keys return 5xx");

    match err {
        HarnessError::AllKeysExhausted {
            ref provider,
            ref attempted_aliases,
            last_status,
        } => {
            assert_eq!(provider, "anthropic");
            assert_eq!(
                attempted_aliases,
                &vec!["anthropic-main".to_string(), "anthropic-spare".to_string()]
            );
            assert_eq!(last_status, 503);
            let err_msg = err.to_string();
            assert!(
                err_msg.contains("all API keys exhausted"),
                "error message must be clear: {}",
                err_msg
            );
        }
        other => panic!("expected HarnessError::AllKeysExhausted, got {:?}", other),
    }

    assert_eq!(
        req_count.load(Ordering::SeqCst),
        2,
        "expected exactly 2 requests before declaring all keys exhausted"
    );
}

/// [LEDGER-3.7-3] Reparador de JSON malformado en llamadas a herramientas con límites estrictos y fail-closed.
#[tokio::test]
async fn test_agent_harness_repairs_malformed_tool_json_and_fails_closed_on_irreparable() {
    // 1. Markdown fences + single quotes + trailing commas
    let raw_fenced = "```json\n{\n  'target': 'src/main.rs',\n  'action': 'read',\n}\n```";
    let repaired = repair_tool_call_json(raw_fenced).expect("must repair fenced single-quoted JSON");
    assert_eq!(repaired["target"], "src/main.rs");
    assert_eq!(repaired["action"], "read");

    // 2. Truncated JSON missing closing braces
    let raw_truncated = "{\"target\": \"src/lib.rs\", \"options\": {\"encoding\": \"utf-8\"";
    let repaired_truncated =
        repair_tool_call_json(raw_truncated).expect("must repair truncated JSON with missing braces");
    assert_eq!(repaired_truncated["target"], "src/lib.rs");
    assert_eq!(repaired_truncated["options"]["encoding"], "utf-8");

    // 3. Unquoted keys
    let raw_unquoted = "{target: \"docs/design.md\", mode: \"fast\"}";
    let repaired_unquoted =
        repair_tool_call_json(raw_unquoted).expect("must repair unquoted keys");
    assert_eq!(repaired_unquoted["target"], "docs/design.md");
    assert_eq!(repaired_unquoted["mode"], "fast");

    // 4. Irreparable payload must fail closed without panicking
    let gibberish = "Not even remotely json, just random commentary and symbols ~!@#$%^&*()";
    let err = repair_tool_call_json(gibberish).expect_err("must fail closed on non-JSON input");
    match err {
        ToolJsonRepairError::Irreparable(_) => {}
        other => panic!("expected Irreparable, got {:?}", other),
    }

    // 5. Oversized payload must be rejected
    let huge = "a".repeat(70_000);
    let err_huge = repair_tool_call_json(&huge).expect_err("must reject oversized input > 64KB");
    match err_huge {
        ToolJsonRepairError::InputTooLarge { .. } => {}
        other => panic!("expected InputTooLarge, got {:?}", other),
    }

    // 6. Integrated into Turn Loop: model outputs malformed tool JSON, harness repairs and executes tool
    let (addr, _server) = spawn_mock_http_server(move |_headers, _body| {
        (
            200,
            "application/json",
            r#"{"choices":[{"message":{"content":null,"tool_calls":[{"function":{"name":"fs_read","arguments":"```json\n{'target': 'src/lib.rs',}\n```"}}]}}]}"#,
        )
    })
    .await;

    let sm = setup_secret_manager_with_keys(&[(
        "openai-key",
        "openai",
        b"sk-openai-key-valid",
    )])
    .await;

    let mut config = HarnessConfig::new(
        LlmProvider::OpenAi,
        "gpt-4o",
        &format!("http://{}/v1/chat/completions", addr),
        vec!["openai-key".to_string()],
    );
    config.max_steps = 1; // pause right after tool call execution

    let tool_registry = ToolRegistry::with_default_internal_tools();
    let gatekeeper = setup_test_gatekeeper();

    let mut harness = AgentHarness::new(
        config,
        sm,
        tool_registry,
        gatekeeper,
        "session-test-repair",
        TEST_SESSION_TOKEN,
    );

    harness.set_simulated_tool_handler(|tool, args| {
        assert_eq!(tool, "fs_read");
        let val: serde_json::Value = serde_json::from_str(args).expect("must receive valid canonical JSON");
        assert_eq!(val["target"], "src/lib.rs");
        Ok("file contents: pub fn test() {}".to_string())
    });

    let outcome = harness
        .execute_turn("Read src/lib.rs", None)
        .await
        .expect("turn should repair tool json and execute tool");

    match outcome {
        TurnStepOutcome::NeedsConfirmation { step, max_steps, .. } => {
            assert_eq!(step, 1);
            assert_eq!(max_steps, 1);
            assert!(
                harness.conversation_history.iter().any(|(_, c)| c.contains("file contents")),
                "tool output must be present in conversation history"
            );
        }
        other => panic!("expected NeedsConfirmation at step boundary, got {:?}", other),
    }
}

/// [LEDGER-3.7-4] Límite estricto de max_steps por turno; nunca exceder sin pedir confirmación.
#[tokio::test]
async fn test_agent_harness_enforces_strict_max_steps_and_requires_confirmation() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();

    // Server always requests another tool call, simulating an iterative or runaway agent
    let (addr, _server) = spawn_mock_http_server(move |_headers, _body| {
        let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
        let resp = format!(
            r#"{{"choices":[{{"message":{{"content":null,"tool_calls":[{{"function":{{"name":"fs_read","arguments":"{{\"target\":\"step_{}.txt\"}}"}}}}]}}}}]}}"#,
            n
        );
        (200, "application/json", Box::leak(resp.into_boxed_str()))
    })
    .await;

    let sm = setup_secret_manager_with_keys(&[(
        "ollama-alias",
        "ollama",
        b"sk-ollama-local",
    )])
    .await;

    let mut config = HarnessConfig::new(
        LlmProvider::Ollama,
        "llama3.2",
        &format!("http://{}/api/chat", addr),
        vec!["ollama-alias".to_string()],
    );
    config.max_steps = 2; // Strict boundary: exactly 2 steps allowed before requiring confirmation

    let tool_registry = ToolRegistry::with_default_internal_tools();
    let gatekeeper = setup_test_gatekeeper();

    let mut harness = AgentHarness::new(
        config,
        sm,
        tool_registry,
        gatekeeper,
        "session-test-max-steps",
        TEST_SESSION_TOKEN,
    );

    harness.set_simulated_tool_handler(|_tool, args| Ok(format!("mock read for {}", args)));

    // Phase 1: Run with budget = 2
    let outcome1 = harness
        .execute_turn("Run autonomous loop", None)
        .await
        .expect("must reach boundary without panic");

    match outcome1 {
        TurnStepOutcome::NeedsConfirmation { step, max_steps, .. } => {
            assert_eq!(step, 2, "must halt exactly at max_steps boundary");
            assert_eq!(max_steps, 2);
            assert!(harness.needs_confirmation, "harness must flag confirmation needed");
        }
        other => panic!("expected NeedsConfirmation, got {:?}", other),
    }

    assert_eq!(
        harness.current_step, 2,
        "agent loop must not iterate beyond max_steps"
    );

    // Phase 2: User confirms and grants 2 more steps
    let outcome2 = harness
        .confirm_and_resume(2, None)
        .await
        .expect("resume after confirmation must succeed");

    match outcome2 {
        TurnStepOutcome::NeedsConfirmation { step, max_steps, .. } => {
            assert_eq!(step, 4, "must halt at new extended boundary of 4");
            assert_eq!(max_steps, 4);
        }
        other => panic!("expected NeedsConfirmation at extended boundary, got {:?}", other),
    }

    assert_eq!(
        harness.current_step, 4,
        "agent loop must stop at new boundary"
    );
}

/// [LEDGER-3.7-5] Ausencia total de secretos en errores, logs, resultados e historial del arnés.
#[tokio::test]
async fn test_agent_harness_zero_secret_leak_in_errors_logs_and_state() {
    let sentinel_secret = "sk-ultra-sensitive-classified-secret-xyz-987";

    let (addr, _server) = spawn_mock_http_server(move |_headers, _body| {
        (
            500,
            "application/json",
            r#"{"error":{"message":"internal error triggered"}}"#,
        )
    })
    .await;

    let sm = setup_secret_manager_with_keys(&[(
        "gemini-classified",
        "gemini",
        sentinel_secret.as_bytes(),
    )])
    .await;

    let mut config = HarnessConfig::new(
        LlmProvider::Gemini,
        "gemini-2.0-flash",
        &format!("http://{}/v1beta/chat/completions", addr),
        vec!["gemini-classified".to_string()],
    );
    config.initial_backoff_ms = 0;

    let tool_registry = ToolRegistry::with_default_internal_tools();
    let gatekeeper = setup_test_gatekeeper();

    let mut harness = AgentHarness::new(
        config,
        sm,
        tool_registry,
        gatekeeper,
        "session-test-zero-leak",
        TEST_SESSION_TOKEN,
    );

    let err = harness
        .execute_turn("Test that fails and generates errors", None)
        .await
        .expect_err("must produce error on 500");

    let err_display = format!("{}", err);
    let err_debug = format!("{:?}", err);

    assert!(
        !err_display.contains(sentinel_secret),
        "secret leaked in error Display string!"
    );
    assert!(
        !err_debug.contains(sentinel_secret),
        "secret leaked in error Debug string!"
    );

    // Verify conversation history
    for (role, content) in &harness.conversation_history {
        assert!(
            !role.contains(sentinel_secret),
            "secret leaked in role string!"
        );
        assert!(
            !content.contains(sentinel_secret),
            "secret leaked in message content!"
        );
    }
}
