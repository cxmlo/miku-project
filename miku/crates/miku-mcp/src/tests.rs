use std::net::SocketAddr;
use std::time::Instant;

use miku_agent::ToolRegistry;
use miku_gatekeeper::{Capability, PolicyEffect, ProjectProfile};
use miku_protocol::{
    JSONRPC_GATEKEEPER_DENIED, JSONRPC_INVALID_PARAMS, JSONRPC_INVALID_REQUEST,
    JSONRPC_KILL_SWITCH_ACTIVE, JSONRPC_METHOD_NOT_FOUND, JSONRPC_PARSE_ERROR,
};

use crate::{
    start_mcp_loopback_host, McpHostConfig, McpHostEngine, McpHostError, McpTestClient,
    McpToolCallArguments, MAX_MCP_DISPATCH_LATENCY_MS, MCP_PROTOCOL_VERSION_2024_11_05,
};

fn build_test_profile() -> ProjectProfile {
    ProjectProfile::new("miku-mcp-test", "C:/Users/cmiranda/miku-workspace", 30_000)
        .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsRead, "README.md", PolicyEffect::Allow, false)
        .with_rule(Capability::Shell, "cargo test*", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "src/**", PolicyEffect::Ask, false)
        .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true)
}

// LEDGER-3.3-1
#[tokio::test]
async fn test_mcp_host_startup_loopback_bind_and_external_ip_rejection() {
    // 1. Non-loopback bind addresses (0.0.0.0, LAN IP) MUST be rejected before opening a socket
    for external in ["0.0.0.0:47410", "192.168.1.50:47410", "10.0.0.1:47410"] {
        let addr: SocketAddr = external.parse().unwrap();
        let bad_cfg = McpHostConfig {
            bind_addr: addr,
            client_agent_id: "cursor".to_string(),
            required_session_token: None,
        };
        let err = bad_cfg.validate_loopback_bind().unwrap_err();
        assert!(matches!(err, McpHostError::NonLoopbackBindForbidden(_)));

        let engine = McpHostEngine::new(
            bad_cfg,
            build_test_profile(),
            ToolRegistry::with_default_internal_tools(),
        )
        .unwrap();
        let start_res = start_mcp_loopback_host(engine).await;
        assert!(matches!(
            start_res,
            Err(McpHostError::NonLoopbackBindForbidden(_))
        ));
    }

    // 2. Ephemeral loopback bind (127.0.0.1:0) succeeds and negotiates MCP 2024-11-05 initialize
    let cfg = McpHostConfig::loopback(0).with_agent_id("cursor");
    cfg.validate_loopback_bind().expect("loopback bind valid");
    let engine = McpHostEngine::new(
        cfg,
        build_test_profile(),
        ToolRegistry::with_default_internal_tools(),
    )
    .unwrap();

    let (handle, join) = start_mcp_loopback_host(engine)
        .await
        .expect("start loopback MCP host");
    assert!(handle.local_addr.ip().is_loopback());
    assert!(handle.local_addr.port() > 0);

    let mut client = McpTestClient::connect(handle.local_addr)
        .await
        .expect("connect test client");
    let init = client
        .initialize("cursor-mcp-test", None)
        .await
        .expect("MCP initialize");
    assert_eq!(init.protocol_version, MCP_PROTOCOL_VERSION_2024_11_05);
    assert_eq!(init.server_info.name, "miku-mcp-host");

    handle.trigger_shutdown();
    join.await.unwrap();
}

// LEDGER-3.3-2
#[tokio::test]
async fn test_mcp_tools_list_returns_authorized_catalog_via_test_client() {
    let cfg = McpHostConfig::loopback(0).with_agent_id("cursor");
    let engine = McpHostEngine::new(
        cfg,
        build_test_profile(),
        ToolRegistry::with_default_internal_tools(),
    )
    .unwrap();

    let (handle, join) = start_mcp_loopback_host(engine)
        .await
        .expect("start loopback MCP host");
    let mut client = McpTestClient::connect(handle.local_addr)
        .await
        .expect("connect test client");
    let _ = client.initialize("cursor-mcp-test", None).await.unwrap();

    let catalog = client.list_tools().await.expect("MCP tools/list");
    assert_eq!(catalog.tools.len(), 5);
    let names: Vec<&str> = catalog.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "browser_action",
            "fs_read",
            "fs_write",
            "memory_query",
            "shell_exec"
        ]
    );
    for tool in &catalog.tools {
        assert!(!tool.description.is_empty());
        assert_eq!(tool.input_schema["type"], "object");
        assert!(tool.input_schema["properties"]["target"].is_object());
    }

    let metrics = handle.metrics().await;
    assert_eq!(metrics.tools_list_count, 1);
    assert_eq!(metrics.handler_executions, 0);

    handle.trigger_shutdown();
    join.await.unwrap();
}

// LEDGER-3.3-3
#[tokio::test]
async fn test_mcp_tools_call_authorized_executes_handler_after_gatekeeper_and_audits_under_5ms() {
    let cfg = McpHostConfig::loopback(0)
        .with_agent_id("cursor")
        .with_session_token("mcp-secret-session-token-v1");
    let engine = McpHostEngine::new(
        cfg,
        build_test_profile(),
        ToolRegistry::with_default_internal_tools(),
    )
    .unwrap();
    let exec_counter = engine.handler_executions_counter();

    let (handle, join) = start_mcp_loopback_host(engine)
        .await
        .expect("start loopback MCP host");
    let mut client = McpTestClient::connect(handle.local_addr)
        .await
        .expect("connect test client");
    let _ = client
        .initialize("cursor-mcp-test", Some("mcp-secret-session-token-v1"))
        .await
        .unwrap();

    // Warm-up + 20 measured calls to certify dispatch latency < 5.0 ms (excluding human approval)
    let mut latencies_ms = Vec::with_capacity(20);
    for i in 0..20 {
        let t0 = Instant::now();
        let resp = client
            .call_tool(
                "fs_read",
                McpToolCallArguments {
                    target: "src/lib.rs".to_string(),
                    args: format!("--line={}", i),
                    session_token: Some("mcp-secret-session-token-v1".to_string()),
                    timestamp_ms: Some(10_000 + i),
                },
            )
            .await
            .expect("call_tool fs_read");
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        latencies_ms.push(elapsed_ms);

        assert!(resp.error.is_none(), "unexpected error: {:?}", resp.error);
        let res = resp.result.expect("tool call result");
        assert!(!res.is_error);
        assert_eq!(
            res.canonical_target,
            "C:/Users/cmiranda/miku-workspace/src/lib.rs"
        );
        assert_eq!(res.audit_seq, i + 1);
        assert_eq!(res.audit_hash.len(), 64);
        assert!(res.content[0].text.contains("MCP_TOOL_OK|tool=fs_read"));
    }

    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50_ms = latencies_ms[latencies_ms.len() / 2];
    let p95_ms = latencies_ms[(latencies_ms.len() * 95) / 100];
    println!(
        "[BENCH-3.3] MCP tools/call authorized dispatch (n=20): p50 = {:.4} ms | p95 = {:.4} ms (budget < {:.1} ms)",
        p50_ms, p95_ms, MAX_MCP_DISPATCH_LATENCY_MS
    );
    assert!(
        p95_ms < MAX_MCP_DISPATCH_LATENCY_MS,
        "MCP dispatch p95 {:.4} ms exceeded {:.1} ms",
        p95_ms,
        MAX_MCP_DISPATCH_LATENCY_MS
    );

    assert_eq!(
        exec_counter.load(std::sync::atomic::Ordering::SeqCst),
        20
    );
    let verified_entries = handle
        .verify_audit_chain()
        .await
        .expect("SHA-256 audit chain intact");
    assert_eq!(verified_entries, 20);

    handle.trigger_shutdown();
    join.await.unwrap();
}

// LEDGER-3.3-4
#[tokio::test]
async fn test_mcp_tools_call_denied_enforces_zero_handler_executions_and_audits_event() {
    let cfg = McpHostConfig::loopback(0)
        .with_agent_id("cursor")
        .with_session_token("mcp-secret-session-token-v1");
    let engine = McpHostEngine::new(
        cfg,
        build_test_profile(),
        ToolRegistry::with_default_internal_tools(),
    )
    .unwrap();
    let exec_counter = engine.handler_executions_counter();

    let (handle, join) = start_mcp_loopback_host(engine)
        .await
        .expect("start loopback MCP host");
    let mut client = McpTestClient::connect(handle.local_addr)
        .await
        .expect("connect test client");

    // 1. ExplicitDenyRule (`rm -rf /`)
    let deny_rule = client
        .call_tool(
            "shell_exec",
            McpToolCallArguments {
                target: "rm -rf /".to_string(),
                args: "--no-preserve-root".to_string(),
                session_token: Some("mcp-secret-session-token-v1".to_string()),
                timestamp_ms: Some(20_001),
            },
        )
        .await
        .unwrap();
    let err1 = deny_rule.error.expect("must return JSON-RPC error on deny");
    assert_eq!(err1.code, JSONRPC_GATEKEEPER_DENIED);
    assert_eq!(err1.data.as_ref().unwrap()["reason"], "deny_rule");

    // 2. PathJailViolation (`..\..\Windows\System32\cmd.exe`)
    let deny_jail = client
        .call_tool(
            "fs_read",
            McpToolCallArguments {
                target: r"..\..\Windows\System32\drivers\etc\hosts".to_string(),
                args: String::new(),
                session_token: Some("mcp-secret-session-token-v1".to_string()),
                timestamp_ms: Some(20_002),
            },
        )
        .await
        .unwrap();
    let err2 = deny_jail.error.expect("must return JSON-RPC error on jail escape");
    assert_eq!(err2.code, JSONRPC_GATEKEEPER_DENIED);
    assert_eq!(err2.data.as_ref().unwrap()["reason"], "deny_jail");

    // 3. DefaultDeny (`browser_action` to unlisted domain)
    let deny_default = client
        .call_tool(
            "browser_action",
            McpToolCallArguments {
                target: "https://untrusted.example.com".to_string(),
                args: "click".to_string(),
                session_token: Some("mcp-secret-session-token-v1".to_string()),
                timestamp_ms: Some(20_003),
            },
        )
        .await
        .unwrap();
    let err3 = deny_default.error.expect("must return JSON-RPC error on default deny");
    assert_eq!(err3.code, JSONRPC_GATEKEEPER_DENIED);
    assert_eq!(err3.data.as_ref().unwrap()["reason"], "deny_default");

    // 4. AskPending without interactive approval (`fs_write` on `src/main.rs`) -> must NOT execute handler
    let ask_pending = client
        .call_tool(
            "fs_write",
            McpToolCallArguments {
                target: "src/main.rs".to_string(),
                args: "fn main() {}".to_string(),
                session_token: Some("mcp-secret-session-token-v1".to_string()),
                timestamp_ms: Some(20_004),
            },
        )
        .await
        .unwrap();
    let err4 = ask_pending.error.expect("unapproved AskPending must not execute handler");
    assert_eq!(err4.code, JSONRPC_GATEKEEPER_DENIED);
    assert_eq!(err4.data.as_ref().unwrap()["reason"], "ask_pending_requires_approval");

    // 5. Invalid session token -> rejected without leaking the expected secret token
    let bad_auth = client
        .call_tool(
            "fs_read",
            McpToolCallArguments {
                target: "src/lib.rs".to_string(),
                args: String::new(),
                session_token: Some("wrong-token".to_string()),
                timestamp_ms: Some(20_005),
            },
        )
        .await
        .unwrap();
    let err5 = bad_auth.error.expect("wrong token must fail");
    assert_eq!(err5.code, JSONRPC_GATEKEEPER_DENIED);
    assert!(!err5.message.contains("mcp-secret-session-token-v1"));

    // 6. Global Kill Switch active -> rejects even an allowed `fs_read` with JSONRPC_KILL_SWITCH_ACTIVE (-32002)
    handle.trigger_kill_switch(20_010).await;
    let ks_deny = client
        .call_tool(
            "fs_read",
            McpToolCallArguments {
                target: "src/lib.rs".to_string(),
                args: String::new(),
                session_token: Some("mcp-secret-session-token-v1".to_string()),
                timestamp_ms: Some(20_011),
            },
        )
        .await
        .unwrap();
    let err6 = ks_deny.error.expect("Kill Switch must block MCP call");
    assert_eq!(err6.code, JSONRPC_KILL_SWITCH_ACTIVE);

    // ZERO handler executions across all 6 denied/unapproved calls!
    assert_eq!(
        exec_counter.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    let verified_entries = handle
        .verify_audit_chain()
        .await
        .expect("SHA-256 audit chain intact");
    assert!(verified_entries >= 6);

    handle.trigger_shutdown();
    join.await.unwrap();
}

// LEDGER-3.3-5
#[tokio::test]
async fn test_mcp_invalid_messages_unknown_methods_and_clean_host_teardown() {
    let cfg = McpHostConfig::loopback(0).with_agent_id("cursor");
    let engine = McpHostEngine::new(
        cfg,
        build_test_profile(),
        ToolRegistry::with_default_internal_tools(),
    )
    .unwrap();
    let exec_counter = engine.handler_executions_counter();

    let (handle, join) = start_mcp_loopback_host(engine)
        .await
        .expect("start loopback MCP host");
    let bound_addr = handle.local_addr;

    {
        let mut client = McpTestClient::connect(bound_addr)
            .await
            .expect("connect test client");

        // 1. Malformed JSON -> JSONRPC_PARSE_ERROR (-32700)
        let parse_err = client
            .send_raw_line("{not valid json")
            .await
            .expect("response for malformed json");
        assert_eq!(parse_err.error.unwrap().code, JSONRPC_PARSE_ERROR);

        // 2. Invalid JSON-RPC version ("1.0") -> JSONRPC_INVALID_REQUEST (-32600)
        let inv_req = client
            .send_raw_line(r#"{"jsonrpc":"1.0","id":2,"method":"tools/list","params":{}}"#)
            .await
            .expect("response for invalid jsonrpc version");
        assert_eq!(inv_req.error.unwrap().code, JSONRPC_INVALID_REQUEST);

        // 3. Unknown method ("resources/subscribe") -> JSONRPC_METHOD_NOT_FOUND (-32601)
        let unknown_method = client
            .send_raw_line(
                r#"{"jsonrpc":"2.0","id":3,"method":"resources/subscribe","params":{}}"#,
            )
            .await
            .expect("response for unknown method");
        assert_eq!(
            unknown_method.error.unwrap().code,
            JSONRPC_METHOD_NOT_FOUND
        );

        // 4. Unknown tool in `tools/call` -> JSONRPC_INVALID_PARAMS (-32602)
        let unknown_tool = client
            .call_tool(
                "non_existent_tool",
                McpToolCallArguments {
                    target: "src/lib.rs".to_string(),
                    args: String::new(),
                    session_token: None,
                    timestamp_ms: Some(30_000),
                },
            )
            .await
            .unwrap();
        assert_eq!(unknown_tool.error.unwrap().code, JSONRPC_INVALID_PARAMS);

        // 5. Empty target in `tools/call` -> JSONRPC_INVALID_PARAMS (-32602)
        let empty_target = client
            .call_tool(
                "fs_read",
                McpToolCallArguments {
                    target: "   ".to_string(),
                    args: String::new(),
                    session_token: None,
                    timestamp_ms: Some(30_001),
                },
            )
            .await
            .unwrap();
        assert_eq!(empty_target.error.unwrap().code, JSONRPC_INVALID_PARAMS);
    }

    assert_eq!(
        exec_counter.load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    // Trigger clean host shutdown and verify connection count drops to 0 and listener closes
    handle.trigger_shutdown();
    join.await.expect("host task joined cleanly");
    assert_eq!(handle.active_connections(), 0);
    assert!(McpTestClient::connect(bound_addr).await.is_err());
}
