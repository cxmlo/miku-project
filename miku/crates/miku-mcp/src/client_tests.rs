use std::collections::BTreeMap;
use std::time::Instant;

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};

use crate::{
    ExternalMcpClientManager, ExternalMcpServerConfig, McpClientError, McpToolDefinition,
    MockServerBehavior,
};

fn test_gatekeeper_profile() -> ProjectProfile {
    ProjectProfile::new("mcp-client-test", "C:/miku/test", 30_000)
        .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "src/safe/**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "src/secrets/**", PolicyEffect::Deny, false)
        .with_rule(Capability::Shell, "deny_target", PolicyEffect::Deny, false)
        .with_rule(Capability::Shell, "*", PolicyEffect::Allow, false)
}

fn sample_tool(name: &str, cap: &str) -> McpToolDefinition {
    McpToolDefinition {
        name: name.to_string(),
        description: format!("Test tool {}", name),
        capability: cap.to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "target": { "type": "string" },
                "args": { "type": "string" }
            },
            "required": ["target"]
        }),
    }
}

// LEDGER-3.4-1: Timeout
#[tokio::test]
async fn test_mcp_client_timeout_cancels_pending_request_without_blocking_reactor() {
    let gk = GatekeeperEngine::new(test_gatekeeper_profile()).expect("gatekeeper init");
    let mut manager = ExternalMcpClientManager::new(gk);

    let cfg = ExternalMcpServerConfig {
        name: "slow_server".to_string(),
        command: "mock_slow".to_string(),
        args: vec![],
        env: BTreeMap::new(),
        timeout_ms: 60,
        idle_ttl_ms: 10_000,
        auto_start: false,
    };

    let behavior = MockServerBehavior {
        response_delay_ms: 350,
        return_corrupt_json: false,
        disconnect_on_call: false,
        tools: vec![sample_tool("allow_tool", "shell")],
    };

    manager
        .register_mock_server(cfg, behavior)
        .expect("register server");

    let t0 = Instant::now();
    let res = manager
        .call_tool(
            "slow_server",
            "allow_tool",
            "test_target",
            "",
            "sess_timeout",
            1_000,
        )
        .await;

    let elapsed = t0.elapsed();
    assert!(
        elapsed.as_millis() >= 50 && elapsed.as_millis() < 300,
        "Timeout must cancel promptly around 60 ms without waiting 350 ms (elapsed = {:?})",
        elapsed
    );

    match res {
        Err(McpClientError::Timeout { server, timeout_ms }) => {
            assert_eq!(server, "slow_server");
            assert_eq!(timeout_ms, 60);
        }
        other => panic!("expected McpClientError::Timeout, got: {:?}", other),
    }
}

// LEDGER-3.4-2: Capability control (Gatekeeper deny-by-default)
#[tokio::test]
async fn test_mcp_client_capability_control_enforces_gatekeeper_deny_and_zero_handler_calls() {
    let gk = GatekeeperEngine::new(test_gatekeeper_profile()).expect("gatekeeper init");
    let mut manager = ExternalMcpClientManager::new(gk);

    let cfg = ExternalMcpServerConfig {
        name: "gatekeeper_server".to_string(),
        command: "mock_gk".to_string(),
        args: vec![],
        env: BTreeMap::new(),
        timeout_ms: 1_000,
        idle_ttl_ms: 10_000,
        auto_start: false,
    };

    let behavior = MockServerBehavior {
        response_delay_ms: 0,
        return_corrupt_json: false,
        disconnect_on_call: false,
        tools: vec![
            sample_tool("fs_write_secret", "fs_write"),
            sample_tool("allow_tool", "shell"),
        ],
    };

    manager
        .register_mock_server(cfg, behavior)
        .expect("register server");

    // 1. Tool denied by Gatekeeper (src/secrets/** -> Deny)
    let denied_res = manager
        .call_tool(
            "gatekeeper_server",
            "fs_write_secret",
            "src/secrets/token.key",
            "new_value",
            "sess_gk",
            2_000,
        )
        .await;

    match denied_res {
        Err(McpClientError::GatekeeperDenied { tool, reason, .. }) => {
            assert_eq!(tool, "fs_write_secret");
            assert!(
                reason.contains("policy_deny") || reason.contains("deny"),
                "expected policy deny reason, got: {}",
                reason
            );
        }
        other => panic!("expected GatekeeperDenied, got {:?}", other),
    }

    // Verify SHA-256 audit log recorded the deny event
    let audit = manager.gatekeeper().audit_log();
    assert!(audit.verify_chain().is_ok());
    let last = audit.entries().last().expect("audit entry");
    assert_eq!(last.verdict, "deny");
    assert_eq!(last.capability, "fs_write");

    // 2. Allowed tool completes and audits 'allow'
    let allowed_res = manager
        .call_tool(
            "gatekeeper_server",
            "allow_tool",
            "src/main.rs",
            "",
            "sess_gk",
            2_010,
        )
        .await;

    assert!(allowed_res.is_ok(), "allowed tool must succeed: {:?}", allowed_res);
    let audit2 = manager.gatekeeper().audit_log();
    let last2 = audit2.entries().last().expect("allow audit entry");
    assert_eq!(last2.verdict, "allow");
}

// LEDGER-3.4-3: On-demand lifecycle and idle reaping
#[tokio::test]
async fn test_mcp_client_on_demand_lifecycle_spawns_on_session_and_reaps_on_idle_ttl() {
    let gk = GatekeeperEngine::new(test_gatekeeper_profile()).expect("gatekeeper init");
    let mut manager = ExternalMcpClientManager::new(gk);

    let cfg = ExternalMcpServerConfig {
        name: "ondemand_server".to_string(),
        command: "mock_ondemand".to_string(),
        args: vec![],
        env: BTreeMap::new(),
        timeout_ms: 1_000,
        idle_ttl_ms: 100, // 100 ms TTL
        auto_start: false,
    };

    let behavior = MockServerBehavior {
        response_delay_ms: 0,
        return_corrupt_json: false,
        disconnect_on_call: false,
        tools: vec![sample_tool("allow_tool", "shell")],
    };

    manager
        .register_mock_server(cfg, behavior)
        .expect("register server");

    // Initially 0 active processes
    assert_eq!(manager.active_process_count(), 0);
    assert!(!manager.is_server_running("ondemand_server"));

    // Session 1 acquires server -> process starts
    manager
        .acquire_session("ondemand_server", "session_1", 3_000)
        .await
        .expect("acquire session 1");
    assert_eq!(manager.active_process_count(), 1);
    assert!(manager.is_server_running("ondemand_server"));

    // Session 2 also acquires server -> process remains 1
    manager
        .acquire_session("ondemand_server", "session_2", 3_010)
        .await
        .expect("acquire session 2");
    assert_eq!(manager.active_process_count(), 1);

    // Reaping before sessions release reaps 0
    let reaped = manager.reap_idle_servers(3_050).await;
    assert_eq!(reaped, 0);
    assert_eq!(manager.active_process_count(), 1);

    // Session 1 releases
    manager.release_session("session_1", 3_060).await;
    assert_eq!(manager.active_process_count(), 1);

    // Session 2 releases at 3_080 ms
    manager.release_session("session_2", 3_080).await;
    assert_eq!(manager.active_process_count(), 1);

    // Reaping at 3_120 ms (< 100 ms elapsed) reaps 0
    let reaped_early = manager.reap_idle_servers(3_120).await;
    assert_eq!(reaped_early, 0);
    assert_eq!(manager.active_process_count(), 1);

    // Reaping at 3_200 ms (> 100 ms TTL) terminates process and reaps 1
    let reaped_late = manager.reap_idle_servers(3_200).await;
    assert_eq!(reaped_late, 1);
    assert_eq!(manager.active_process_count(), 0);
    assert!(!manager.is_server_running("ondemand_server"));
}

// LEDGER-3.4-4: Shutdown and Kill Switch terminate processes with 0 orphans
#[tokio::test]
async fn test_mcp_client_kill_switch_and_graceful_shutdown_terminate_all_child_processes_zero_orphans() {
    let gk = GatekeeperEngine::new(test_gatekeeper_profile()).expect("gatekeeper init");
    let mut manager = ExternalMcpClientManager::new(gk);

    let cfg1 = ExternalMcpServerConfig {
        name: "srv1".to_string(),
        command: "mock1".to_string(),
        args: vec![],
        env: BTreeMap::new(),
        timeout_ms: 1_000,
        idle_ttl_ms: 10_000,
        auto_start: false,
    };
    let cfg2 = ExternalMcpServerConfig {
        name: "srv2".to_string(),
        command: "mock2".to_string(),
        args: vec![],
        env: BTreeMap::new(),
        timeout_ms: 1_000,
        idle_ttl_ms: 10_000,
        auto_start: false,
    };

    manager
        .register_mock_server(cfg1, MockServerBehavior::default())
        .expect("reg 1");
    manager
        .register_mock_server(cfg2, MockServerBehavior::default())
        .expect("reg 2");

    manager
        .acquire_session("srv1", "s1", 4_000)
        .await
        .expect("start srv1");
    manager
        .acquire_session("srv2", "s2", 4_000)
        .await
        .expect("start srv2");
    assert_eq!(manager.active_process_count(), 2);

    let tracked_pids = manager.active_process_pids();
    assert_eq!(tracked_pids.len(), 2);

    // Trip Kill Switch
    manager.trip_kill_switch().await.expect("trip kill switch");
    assert_eq!(manager.active_process_count(), 0);
    assert!(!manager.is_server_running("srv1"));
    assert!(!manager.is_server_running("srv2"));

    // Invocations during Kill Switch are rejected immediately
    let res = manager
        .call_tool("srv1", "allow_tool", "target", "", "s1", 4_050)
        .await;
    match res {
        Err(McpClientError::KillSwitchActive) => {}
        other => panic!("expected KillSwitchActive, got {:?}", other),
    }

    // Graceful shutdown on already stopped manager is idempotent and clean
    manager.shutdown().await.expect("shutdown clean");
    assert_eq!(manager.active_process_count(), 0);
}

// LEDGER-3.4-5: Protocol errors, disconnect recovery, and zero secret leaks
#[tokio::test]
async fn test_mcp_client_protocol_errors_disconnect_recovery_and_zero_secret_leak() {
    let gk = GatekeeperEngine::new(test_gatekeeper_profile()).expect("gatekeeper init");
    let mut manager = ExternalMcpClientManager::new(gk);

    let mut secret_env = BTreeMap::new();
    secret_env.insert("API_SECRET".to_string(), "SUPER_SECRET_TOKEN_12345".to_string());

    let cfg = ExternalMcpServerConfig {
        name: "resilient_server".to_string(),
        command: "mock_resilient".to_string(),
        args: vec![],
        env: secret_env,
        timeout_ms: 1_000,
        idle_ttl_ms: 10_000,
        auto_start: false,
    };

    let corrupt_behavior = MockServerBehavior {
        response_delay_ms: 0,
        return_corrupt_json: true,
        disconnect_on_call: false,
        tools: vec![sample_tool("allow_tool", "shell")],
    };

    manager
        .register_mock_server(cfg.clone(), corrupt_behavior)
        .expect("register server");

    // 1. Corrupt JSON produces typed ProtocolError and does NOT leak secret
    let err = manager
        .call_tool(
            "resilient_server",
            "allow_tool",
            "target",
            "",
            "sess_err",
            5_000,
        )
        .await
        .expect_err("must fail on corrupt JSON");

    let err_str = err.to_string();
    assert!(
        matches!(err, McpClientError::ProtocolError(_)),
        "expected ProtocolError, got: {:?}",
        err
    );
    assert!(
        !err_str.contains("SUPER_SECRET_TOKEN_12345"),
        "error must never leak environment secrets: {}",
        err_str
    );

    // 2. Disconnect produces TransportError
    let disconnect_behavior = MockServerBehavior {
        response_delay_ms: 0,
        return_corrupt_json: false,
        disconnect_on_call: true,
        tools: vec![sample_tool("allow_tool", "shell")],
    };
    manager
        .register_mock_server(cfg.clone(), disconnect_behavior)
        .expect("register disconnect server");

    let disc_err = manager
        .call_tool(
            "resilient_server",
            "allow_tool",
            "target",
            "",
            "sess_disc",
            5_010,
        )
        .await
        .expect_err("must fail on disconnect");

    assert!(
        matches!(disc_err, McpClientError::TransportError(_)),
        "expected TransportError, got: {:?}",
        disc_err
    );

    // 3. Subsequent session recovers cleanly once server returns valid responses
    let healthy_behavior = MockServerBehavior {
        response_delay_ms: 0,
        return_corrupt_json: false,
        disconnect_on_call: false,
        tools: vec![sample_tool("allow_tool", "shell")],
    };
    manager
        .register_mock_server(cfg, healthy_behavior)
        .expect("register healthy server");

    let recovered = manager
        .call_tool(
            "resilient_server",
            "allow_tool",
            "src/recovered.rs",
            "",
            "sess_rec",
            5_020,
        )
        .await;

    assert!(recovered.is_ok(), "must recover on subsequent session: {:?}", recovered);
}
