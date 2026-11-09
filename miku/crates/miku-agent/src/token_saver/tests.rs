use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};
use miku_memory::{ChunkKind, ContentChunk, SemanticChunker};
use miku_storage::{ConversationTurnRecord, Fts5SearchHit};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::harness::turn_loop::AgentHarness;
use crate::harness::types::{HarnessConfig, HarnessError, LlmProvider, TurnStepOutcome};
use crate::secrets::backend::EncryptedMemoryBackend;
use crate::secrets::manager::SecretManager;
use crate::token_saver::{
    assemble_turn_prompt_with_stable_prefix, build_static_system_prefix_from_registry,
    is_assembled_prefix_byte_stable, DailySpendTracker, ForwardReason, InjectableDayClock,
    LocalIntentRouter, LocalRouteCategory, LocalSystemSnapshot, ModelPricingRate,
    PreFlightBudgetDecision, ProjectDailyBudgetConfig, RouteDecision, TokenUsageRecord,
};
use crate::ToolRegistry;

const TEST_SESSION_TOKEN: &str = "test-session-token-f09-3-8";
const TOP_SECRET_KEY: &[u8] = b"sk-TOP-SECRET-3-8-NEVER-READ-OR-LEAK";

fn setup_f09_gatekeeper(project_id: &str) -> GatekeeperEngine {
    let profile = ProjectProfile::new(project_id, "C:/Users/cmiranda/dev/miku", 60_000)
        .with_rule(Capability::Network, "*", PolicyEffect::Allow, false)
        .with_rule(Capability::FsRead, "*", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "*", PolicyEffect::Deny, false)
        .with_rule(Capability::Shell, "*", PolicyEffect::Deny, false);
    GatekeeperEngine::new(profile).expect("Gatekeeper engine should build")
}

async fn setup_f09_secret_manager(project_id: &str) -> Arc<Mutex<SecretManager>> {
    let backend = Box::new(EncryptedMemoryBackend::new());
    let gk = setup_f09_gatekeeper(project_id);
    let mut sm = SecretManager::new(backend, TEST_SESSION_TOKEN.to_string(), gk);
    sm.set_secret("ds_primary", "deepseek", None, TOP_SECRET_KEY)
        .expect("set_secret in F-09 test");
    Arc::new(Mutex::new(sm))
}

async fn spawn_f09_mock_server<F>(handler: F) -> (SocketAddr, tokio::task::JoinHandle<()>)
where
    F: Fn(String, Vec<u8>) -> (u16, String) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind F-09 mock server");
    let addr = listener.local_addr().expect("local addr");
    let handler = Arc::new(handler);

    let handle = tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let handler = handler.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = buf[..n]
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|idx| buf[idx + 4..n].to_vec())
                    .unwrap_or_default();
                let (status, response_body) = handler(req_str, body);
                let response = format!(
                    "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
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

/// [LEDGER-3.8-1] Saludos, comandos internos y consultas locales resueltos sin red, sin leer secretos y con 0 tokens.
#[tokio::test]
async fn test_local_zero_token_router_resolves_greetings_commands_and_status_queries_with_zero_network_and_zero_secrets() {
    let http_hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = http_hits.clone();
    let (addr, server_handle) = spawn_f09_mock_server(move |_req, _body| {
        hits_clone.fetch_add(1, Ordering::SeqCst);
        (200, r#"{"choices":[{"message":{"content":"LLM response"}}]}"#.to_string())
    })
    .await;

    let sm = setup_f09_secret_manager("proj-local-0token").await;
    let clock = Arc::new(InjectableDayClock::new("2026-03-31"));
    let mut tracker = DailySpendTracker::new(clock);
    tracker.set_project_budget(ProjectDailyBudgetConfig::new("proj-local-0token", 500_000));
    let tracker_arc = Arc::new(Mutex::new(tracker));

    let endpoint = format!("http://127.0.0.1:{}/v1/chat/completions", addr.port());
    let config = HarnessConfig::new(
        LlmProvider::DeepSeek,
        "deepseek-chat",
        &endpoint,
        vec!["ds_primary".to_string()],
    );
    let mut harness = AgentHarness::new(
        config,
        sm.clone(),
        ToolRegistry::with_default_internal_tools(),
        setup_f09_gatekeeper("proj-local-0token"),
        "sess-3-8-1",
        TEST_SESSION_TOKEN,
    );
    harness.set_spend_tracker(tracker_arc.clone());

    let local_inputs = [
        ("¡Hola Miku!", LocalRouteCategory::CasualGreeting),
        ("buenos días", LocalRouteCategory::CasualGreeting),
        ("muchas gracias", LocalRouteCategory::CasualGreeting),
        ("/help", LocalRouteCategory::InternalCommand),
        ("/status", LocalRouteCategory::InternalCommand),
        ("/budget", LocalRouteCategory::InternalCommand),
        ("/agents", LocalRouteCategory::InternalCommand),
        ("/models", LocalRouteCategory::InternalCommand),
        ("¿qué hora es?", LocalRouteCategory::LocalStatusQuery),
        ("estado de batería", LocalRouteCategory::LocalStatusQuery),
        ("¿cuántos agentes activos?", LocalRouteCategory::LocalStatusQuery),
        ("gasto diario", LocalRouteCategory::LocalStatusQuery),
    ];

    for (prompt, expected_category) in local_inputs {
        let outcome = harness.execute_turn(prompt, None).await.expect("local 0-token turn");
        match outcome {
            TurnStepOutcome::LocalZeroTokenReply { category, reply, tokens_used, .. } => {
                assert_eq!(category, expected_category.as_str());
                assert_eq!(tokens_used, 0, "local route must consume 0 tokens");
                assert!(!reply.is_empty());
            }
            other => panic!("expected LocalZeroTokenReply for '{}', got {:?}", prompt, other),
        }
    }

    assert_eq!(http_hits.load(Ordering::SeqCst), 0, "zero HTTP requests for local queries");
    assert_eq!(sm.lock().await.authorized_read_attempts(), 0, "zero secret reads for local queries");

    let snap = tracker_arc.lock().await.snapshot("proj-local-0token");
    assert_eq!(snap.spent_micros, 0);
    assert_eq!(snap.local_zero_token_hits, 12);

    server_handle.abort();
}

/// [LEDGER-3.8-2] Prefijo estático de sistema y herramientas idéntico byte a byte entre turnos aunque cambie el contexto dinámico.
#[tokio::test]
async fn test_stable_system_and_tools_prefix_remains_byte_for_byte_identical_across_turns_preserving_kv_cache() {
    let registry = ToolRegistry::with_default_internal_tools();
    let soul = "Eres Miku Desktop Orchestrator. Prioriza precisión, seguridad Gatekeeper y mínimo consumo.";
    let prefix = build_static_system_prefix_from_registry("agent-miku-core", soul, &registry);

    let chunks_1 = SemanticChunker::chunk_document(
        "docs/design/F-08.md",
        "# Key Rotation\nRotates keys on HTTP 429 and 5xx with bounded backoff.",
        ChunkKind::Markdown,
        16,
    );
    let mut lookup_1: BTreeMap<String, ContentChunk> = BTreeMap::new();
    for ch in chunks_1 {
        lookup_1.insert(ch.chunk_id.clone(), ch);
    }
    let first_doc_id = lookup_1.keys().next().cloned().unwrap_or_else(|| "chk_1".to_string());

    let rag_turn_1 = vec![Fts5SearchHit {
        doc_id: first_doc_id,
        title: "Key Rotation".to_string(),
        tags: "f08 harness".to_string(),
        bm25_score: 4.2,
        snippet: "Rotates keys on HTTP 429 and 5xx.".to_string(),
        updated_at_ms: 1_700_000_000_000,
    }];
    let history_turn_1 = vec![ConversationTurnRecord {
        turn_id: "t_1".to_string(),
        session_id: "sess-kv".to_string(),
        seq: 1,
        role: "user".to_string(),
        content: "Revisa la rotación de claves".to_string(),
        token_estimate: 8,
        ts_ms: 1_700_000_001_000,
    }];
    let assembled_1 = assemble_turn_prompt_with_stable_prefix(
        &prefix,
        &rag_turn_1,
        &lookup_1,
        &history_turn_1,
        "Explica cómo funciona el backoff exponencial en turn_loop.rs",
        4096,
    )
    .expect("assemble turn 1 prompt");

    let chunks_2 = SemanticChunker::chunk_document(
        "docs/design/F-09.md",
        "# Daily Spend Cap\nBlocks paid provider calls at 100% of project budget before reading secrets.",
        ChunkKind::Markdown,
        16,
    );
    let mut lookup_2: BTreeMap<String, ContentChunk> = BTreeMap::new();
    for ch in chunks_2 {
        lookup_2.insert(ch.chunk_id.clone(), ch);
    }
    let second_doc_id = lookup_2.keys().next().cloned().unwrap_or_else(|| "chk_2".to_string());

    let rag_turn_2 = vec![Fts5SearchHit {
        doc_id: second_doc_id,
        title: "Daily Spend Cap".to_string(),
        tags: "f09 budget".to_string(),
        bm25_score: 5.1,
        snippet: "Blocks paid provider calls at 100% of project budget.".to_string(),
        updated_at_ms: 1_700_000_005_000,
    }];
    let history_turn_2 = vec![
        history_turn_1[0].clone(),
        ConversationTurnRecord {
            turn_id: "t_2".to_string(),
            session_id: "sess-kv".to_string(),
            seq: 2,
            role: "assistant".to_string(),
            content: "El backoff exponencial duplica initial_backoff_ms.".to_string(),
            token_estimate: 16,
            ts_ms: 1_700_000_002_000,
        },
    ];
    let assembled_2 = assemble_turn_prompt_with_stable_prefix(
        &prefix,
        &rag_turn_2,
        &lookup_2,
        &history_turn_2,
        "Ahora analiza el cálculo de micro-USD en budget.rs",
        4096,
    )
    .expect("assemble turn 2 prompt");

    assert!(is_assembled_prefix_byte_stable(&prefix, &assembled_1));
    assert!(is_assembled_prefix_byte_stable(&prefix, &assembled_2));
    assert_eq!(assembled_1.prefix_sha256, assembled_2.prefix_sha256);
    assert_eq!(assembled_1.static_prefix.as_bytes(), assembled_2.static_prefix.as_bytes());

    let prefix_bytes_len = prefix.rendered_prefix.len();
    assert_eq!(
        &assembled_1.full_prompt.as_bytes()[..prefix_bytes_len],
        &assembled_2.full_prompt.as_bytes()[..prefix_bytes_len],
        "static system + tools prefix must be byte-for-byte identical across turns"
    );
    assert_ne!(assembled_1.full_prompt.as_bytes(), assembled_2.full_prompt.as_bytes());

    let captured_bodies: Arc<std::sync::Mutex<Vec<Vec<u8>>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_clone = captured_bodies.clone();
    let (addr, server_handle) = spawn_f09_mock_server(move |_req, body| {
        captured_clone.lock().unwrap().push(body);
        (200, r#"{"choices":[{"message":{"content":"Respuesta verificada"}}]}"#.to_string())
    })
    .await;

    let sm = setup_f09_secret_manager("proj-kv-wire").await;
    let endpoint = format!("http://127.0.0.1:{}/v1/chat/completions", addr.port());
    let config = HarnessConfig::new(
        LlmProvider::DeepSeek,
        "deepseek-chat",
        &endpoint,
        vec!["ds_primary".to_string()],
    );
    let mut harness = AgentHarness::new(
        config,
        sm,
        registry,
        setup_f09_gatekeeper("proj-kv-wire"),
        "agent-miku-core",
        TEST_SESSION_TOKEN,
    );
    harness.set_static_system_prefix_from_directives(soul);

    harness.execute_turn("Analiza la estructura del crate miku-memory", None).await.expect("turn 1");
    harness.execute_turn("Ahora revisa la integración con miku-agent", None).await.expect("turn 2");

    let bodies = captured_bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    let json_1: serde_json::Value = serde_json::from_slice(&bodies[0]).unwrap();
    let json_2: serde_json::Value = serde_json::from_slice(&bodies[1]).unwrap();
    let sys_1 = json_1["messages"][0]["content"].as_str().unwrap();
    let sys_2 = json_2["messages"][0]["content"].as_str().unwrap();
    assert_eq!(sys_1.as_bytes(), sys_2.as_bytes());
    assert_eq!(sys_1.as_bytes(), prefix.rendered_prefix.as_bytes());

    server_handle.abort();
}

/// [LEDGER-3.8-3] Aviso al 80 % y bloqueo al 100 % del presupuesto diario antes de leer secretos o enviar petición de red.
#[tokio::test]
async fn test_daily_budget_emits_warning_at_80_pct_and_blocks_paid_calls_at_100_pct_before_secret_read_or_network() {
    let http_hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = http_hits.clone();
    let (addr, server_handle) = spawn_f09_mock_server(move |_req, _body| {
        let call_num = hits_clone.fetch_add(1, Ordering::SeqCst) + 1;
        if call_num == 1 {
            (
                200,
                r#"{"choices":[{"message":{"content":"Turno 1"}}],"usage":{"prompt_tokens":1000,"completion_tokens":1000}}"#.to_string(),
            )
        } else {
            (
                200,
                r#"{"choices":[{"message":{"content":"Turno 2"}}],"usage":{"prompt_tokens":250,"completion_tokens":250}}"#.to_string(),
            )
        }
    })
    .await;

    let sm = setup_f09_secret_manager("proj-budget-cap").await;
    let clock = Arc::new(InjectableDayClock::new("2026-03-31"));
    let mut tracker = DailySpendTracker::new(clock);
    tracker.set_project_budget(ProjectDailyBudgetConfig::new("proj-budget-cap", 1_000));
    tracker.set_pricing_rate(ModelPricingRate::new_paid(LlmProvider::DeepSeek, "deepseek-chat", 400, 100, 400));
    let tracker_arc = Arc::new(Mutex::new(tracker));

    let endpoint = format!("http://127.0.0.1:{}/v1/chat/completions", addr.port());
    let config = HarnessConfig::new(
        LlmProvider::DeepSeek,
        "deepseek-chat",
        &endpoint,
        vec!["ds_primary".to_string()],
    );
    let mut harness = AgentHarness::new(
        config,
        sm.clone(),
        ToolRegistry::with_default_internal_tools(),
        setup_f09_gatekeeper("proj-budget-cap"),
        "sess-3-8-3",
        TEST_SESSION_TOKEN,
    );
    harness.set_spend_tracker(tracker_arc.clone());

    let out1 = harness.execute_turn("Analiza el diseño del módulo de memoria", None).await.expect("turn 1 at 80%");
    assert!(matches!(out1, TurnStepOutcome::Completed { .. }));
    let warn = harness.last_budget_warning.as_ref().expect("warning emitted at 80%");
    assert_eq!(warn.usage_pct, 80);
    assert_eq!(warn.spent_micros, 800);

    let out2 = harness.execute_turn("Revisa los contratos de miku-protocol", None).await.expect("turn 2 reaches 100%");
    assert!(matches!(out2, TurnStepOutcome::Completed { .. }));

    let reads_before_blocked = sm.lock().await.authorized_read_attempts();
    let hits_before_blocked = http_hits.load(Ordering::SeqCst);
    assert_eq!(reads_before_blocked, 2);
    assert_eq!(hits_before_blocked, 2);

    let err = harness
        .execute_turn("Intenta otra consulta paga después del 100%", None)
        .await
        .expect_err("turn 3 must be blocked by 100% daily budget hard cap");

    match err {
        HarnessError::DailyBudgetExceeded { project_id, spent_micros, limit_micros, suggested_alternatives, .. } => {
            assert_eq!(project_id, "proj-budget-cap");
            assert_eq!(spent_micros, 1_000);
            assert_eq!(limit_micros, 1_000);
            assert!(!suggested_alternatives.is_empty());
        }
        other => panic!("expected DailyBudgetExceeded, got {:?}", other),
    }

    assert_eq!(sm.lock().await.authorized_read_attempts(), reads_before_blocked, "0 secret reads on blocked call");
    assert_eq!(http_hits.load(Ordering::SeqCst), hits_before_blocked, "0 network calls on blocked call");

    server_handle.abort();
}

/// [LEDGER-3.8-4] Alternativa gratuita/local al bloquear el gasto al 100 % y opción explícita de forzar LLM dentro del presupuesto.
#[tokio::test]
async fn test_budget_hard_cap_offers_free_local_alternatives_and_supports_explicit_force_llm_within_budget() {
    let http_hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = http_hits.clone();
    let (addr, server_handle) = spawn_f09_mock_server(move |_req, _body| {
        hits_clone.fetch_add(1, Ordering::SeqCst);
        (
            200,
            r#"{"choices":[{"message":{"content":"Respuesta proveedor"}}],"usage":{"prompt_tokens":500,"completion_tokens":500}}"#.to_string(),
        )
    })
    .await;

    let sm = setup_f09_secret_manager("proj-fallback").await;
    let clock = Arc::new(InjectableDayClock::new("2026-03-31"));
    let mut tracker = DailySpendTracker::new(clock);
    tracker.set_project_budget(ProjectDailyBudgetConfig::new("proj-fallback", 1_000));
    tracker.set_pricing_rate(ModelPricingRate::new_paid(LlmProvider::DeepSeek, "deepseek-chat", 500, 100, 500));
    let tracker_arc = Arc::new(Mutex::new(tracker));

    let endpoint = format!("http://127.0.0.1:{}/v1/chat/completions", addr.port());
    let config = HarnessConfig::new(
        LlmProvider::DeepSeek,
        "deepseek-chat",
        &endpoint,
        vec!["ds_primary".to_string()],
    );
    let mut harness = AgentHarness::new(
        config,
        sm.clone(),
        ToolRegistry::with_default_internal_tools(),
        setup_f09_gatekeeper("proj-fallback"),
        "sess-3-8-4",
        TEST_SESSION_TOKEN,
    );
    harness.set_spend_tracker(tracker_arc.clone());

    let local_res = harness.execute_turn("hola", None).await.unwrap();
    assert!(matches!(local_res, TurnStepOutcome::LocalZeroTokenReply { .. }));
    assert_eq!(http_hits.load(Ordering::SeqCst), 0);

    let forced_res = harness.execute_turn_with_options("hola", true, None).await.expect("force_llm within budget");
    assert!(matches!(forced_res, TurnStepOutcome::Completed { .. }));
    assert_eq!(http_hits.load(Ordering::SeqCst), 1);

    let prefix_forced_res = harness.execute_turn("/llm hola", None).await.expect("/llm prefix within budget");
    assert!(matches!(prefix_forced_res, TurnStepOutcome::Completed { .. }));
    assert_eq!(http_hits.load(Ordering::SeqCst), 2);

    let blocked_err = harness.execute_turn_with_options("hola", true, None).await.expect_err("blocked at 100%");
    assert!(matches!(blocked_err, HarnessError::DailyBudgetExceeded { .. }));
    assert!(!harness.last_blocked_alternatives.is_empty());

    let mut free_alt = harness.last_blocked_alternatives[0].clone();
    free_alt.endpoint_url = endpoint.clone();
    harness.switch_to_fallback_alternative(&free_alt);

    let reads_before_free = sm.lock().await.authorized_read_attempts();
    let free_turn = harness
        .execute_turn("Explica el módulo miku-gatekeeper usando el modelo local", None)
        .await
        .expect("free/local fallback alternative must succeed at 100% paid cap");
    assert!(matches!(free_turn, TurnStepOutcome::Completed { .. }));
    assert_eq!(sm.lock().await.authorized_read_attempts(), reads_before_free);
    let snap = tracker_arc.lock().await.snapshot("proj-fallback");
    assert_eq!(snap.spent_micros, 1_000);

    server_handle.abort();
}

/// [LEDGER-3.8-5] Clasificación incierta o técnica derivada al LLM sin falsos positivos locales ni bypass de Gatekeeper.
#[test]
fn test_uncertain_or_ambiguous_inputs_route_to_llm_without_local_false_positives() {
    let snapshot = LocalSystemSnapshot::default();
    let ambiguous_or_technical_inputs = [
        "hola, revisa el bug en src/main.rs",
        "buenos días, ejecuta cargo test --workspace",
        "estado de la memoria en miku-memory/src/lib.rs",
        "bateria && rm -rf /",
        "¿por qué falla el test de hora actual?",
        "cómo estás implementando el parser JSON en turn_loop.rs",
        "gracias, ahora crea un archivo nuevo en crates/miku-agent",
        "/deploy production",
        "/exec ls -la",
        "lee el archivo Cargo.toml y dime el gasto diario",
        "explica qué hace LocalIntentRouter",
    ];

    for input in ambiguous_or_technical_inputs {
        let decision = LocalIntentRouter::classify_and_route(input, false, &snapshot);
        match decision {
            RouteDecision::ForwardToLlm { reason, .. } => {
                assert!(matches!(
                    reason,
                    ForwardReason::AmbiguousOrTechnical
                        | ForwardReason::MultiIntentOrComplex
                        | ForwardReason::UnknownCommandOrQuery
                ));
            }
            RouteDecision::LocalZeroToken(m) => {
                panic!("FALSE POSITIVE: '{}' routed locally as {:?}", input, m);
            }
        }
    }
}

/// [LEDGER-3.8-6] Reinicio diario determinista mediante reloj inyectable y cero fuga de secretos en diagnósticos.
#[tokio::test]
async fn test_deterministic_daily_budget_reset_with_injectable_clock_and_zero_secret_leak() {
    let clock = Arc::new(InjectableDayClock::new("2026-03-31"));
    let mut tracker = DailySpendTracker::new(clock.clone());
    tracker.set_project_budget(ProjectDailyBudgetConfig::new("proj-reset", 1_000));
    tracker.set_pricing_rate(ModelPricingRate::new_paid(LlmProvider::DeepSeek, "deepseek-chat", 500, 100, 500));

    let (cost, warn) = tracker.record_turn_usage(
        "proj-reset",
        LlmProvider::DeepSeek,
        "deepseek-chat",
        TokenUsageRecord { uncached_input_tokens: 1_000, cached_input_tokens: 0, output_tokens: 1_000 },
    );
    assert_eq!(cost, 1_000);
    assert!(warn.is_some());

    let preflight_day1 = tracker.evaluate_pre_flight("proj-reset", LlmProvider::DeepSeek, "deepseek-chat");
    assert!(matches!(preflight_day1, PreFlightBudgetDecision::BlockedHardCap { .. }));

    clock.set_day("2026-04-01");

    let snap_day2 = tracker.snapshot("proj-reset");
    assert_eq!(snap_day2.day_key, "2026-04-01");
    assert_eq!(snap_day2.spent_micros, 0);
    assert!(!snap_day2.warning_emitted);
    assert_eq!(snap_day2.hard_cap_blocked_count, 0);

    let preflight_day2 = tracker.evaluate_pre_flight("proj-reset", LlmProvider::DeepSeek, "deepseek-chat");
    assert!(matches!(preflight_day2, PreFlightBudgetDecision::Allowed { spent_micros: 0, warning: None, .. }));

    let err = HarnessError::DailyBudgetExceeded {
        project_id: "proj-reset".to_string(),
        day_key: "2026-03-31".to_string(),
        spent_micros: 1_000,
        limit_micros: 1_000,
        suggested_alternatives: vec!["Ollama Local".to_string()],
    };
    let rendered = format!("{} {:?}", err, err);
    assert!(!rendered.contains(std::str::from_utf8(TOP_SECRET_KEY).unwrap()));
}
