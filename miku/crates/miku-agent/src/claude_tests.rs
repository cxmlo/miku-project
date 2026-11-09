use std::time::{Duration, Instant};

use miku_core::build_current_thread_runtime;
use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};
use miku_protocol::{
    ClaudeBashInput, ClaudeHookDecisionKind, ClaudeHookEvent, ClaudeHookResult,
    ClaudeNotificationParams, ClaudePostToolUseParams, ClaudePreToolUseParams, ClaudeSessionPhase,
    JsonRpcId, JsonRpcRequest, JsonRpcResponse, MikuLinkMessageV1, PermissionDecisionKind,
    CLAUDE_HOOK_FAIL_OPEN_TIMEOUT_MS, JSONRPC_GATEKEEPER_DENIED, JSONRPC_KILL_SWITCH_ACTIVE,
};

use crate::{
    execute_hook_with_fail_open, AvatarActivityDispatcher, AvatarMoodState, ClaudeCodeAdapter,
    ClaudeCliSessionTracker, ClaudeProcessSnapshot, CLAUDE_ADDED_RAM_BUDGET_MB,
    CLAUDE_INTERCEPT_LATENCY_BUDGET_MS,
};

fn claude_adapter_test_profile() -> ProjectProfile {
    ProjectProfile::new("claude-code-adapter", "C:/Users/cmiranda/dev/miku", 60_000)
        .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "src/**", PolicyEffect::Ask, false)
        .with_rule(Capability::Shell, "cargo test*", PolicyEffect::Allow, false)
        .with_rule(Capability::Shell, "git status*", PolicyEffect::Allow, false)
        .with_rule(Capability::Shell, "cargo publish*", PolicyEffect::Ask, true)
        .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true)
        .with_rule(Capability::Shell, "*format C:*", PolicyEffect::Deny, true)
}

// LEDGER-3.1-1
#[test]
fn test_claude_code_cli_session_detection_and_process_polling() {
    let mut tracker = ClaudeCliSessionTracker::new();

    let snapshots = vec![
        ClaudeProcessSnapshot {
            pid: 4120,
            image_name: "claude.exe".to_string(),
            cmdline: "claude --session-id sess_alpha_01 --model claude-3-7-sonnet".to_string(),
            cwd: "C:/Users/cmiranda/dev/miku".to_string(),
        },
        ClaudeProcessSnapshot {
            pid: 5890,
            image_name: "node.exe".to_string(),
            cmdline: "node C:/AppData/Roaming/npm/node_modules/@anthropic-ai/claude-code/cli.js --resume sess_beta_02".to_string(),
            cwd: "C:/Users/cmiranda/dev/miku/crates".to_string(),
        },
        ClaudeProcessSnapshot {
            pid: 7001,
            image_name: "explorer.exe".to_string(),
            cmdline: "C:\\Windows\\explorer.exe".to_string(),
            cwd: "C:\\Windows".to_string(),
        },
        ClaudeProcessSnapshot {
            pid: 8010,
            image_name: "node.exe".to_string(),
            cmdline: "node vite.js --port 3000".to_string(),
            cwd: "C:/Users/cmiranda/dev/miku".to_string(),
        },
    ];

    let active = tracker.poll_processes(&snapshots, 10_000);
    assert_eq!(active.len(), 2);
    assert_eq!(tracker.active_session_count(), 2);

    let s1 = tracker.get_by_pid(4120).expect("pid 4120 must be tracked");
    assert_eq!(s1.session_id, "sess_alpha_01");
    assert_eq!(
        s1.model_override.as_deref(),
        Some("claude-3-7-sonnet")
    );
    assert_eq!(s1.cwd, "C:/Users/cmiranda/dev/miku");

    let s2 = tracker.get_by_pid(5890).expect("pid 5890 must be tracked");
    assert_eq!(s2.session_id, "sess_beta_02");

    // Second poll where pid 4120 exited -> tracker must prune terminated session automatically
    let active_after = tracker.poll_processes(&snapshots[1..], 10_500);
    assert_eq!(active_after.len(), 1);
    assert!(tracker.get_by_pid(4120).is_none());
    assert!(tracker.get_by_pid(5890).is_some());
}

// LEDGER-3.1-2
#[test]
fn test_claude_code_json_rpc_bidirectional_framing_and_fail_open_under_15ms() {
    let rt = build_current_thread_runtime().expect("current_thread runtime");
    rt.block_on(async {
        let req = JsonRpcRequest::new(
            Some(JsonRpcId::Number(1)),
            "pre_tool_use",
            ClaudePreToolUseParams {
                session_id: "sess_rpc_01".to_string(),
                tool_name: "Bash".to_string(),
                tool_input: serde_json::to_value(ClaudeBashInput {
                    command: "cargo test --workspace".to_string(),
                    timeout_ms: Some(30_000),
                    description: Some("Run workspace tests".to_string()),
                })
                .unwrap(),
                cwd: Some("C:/Users/cmiranda/dev/miku".to_string()),
                timestamp_ms: 12_000,
            },
        );

        let raw_line = req.encode_line().expect("encode json-rpc line");
        assert!(raw_line.ends_with('\n'));
        let decoded = JsonRpcRequest::<ClaudePreToolUseParams>::decode_line(&raw_line)
            .expect("decode json-rpc line");
        assert_eq!(decoded, req);

        // 1. Daemon unreachable / timeout (simulated 50 ms stall) -> MUST fail-open in < 15 ms
        let t0 = Instant::now();
        let out_line = execute_hook_with_fail_open(
            &raw_line,
            Duration::from_millis(5),
            |_parsed_req| async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(JsonRpcResponse::ok(
                    Some(JsonRpcId::Number(1)),
                    ClaudeHookResult::allow(1, "hash".to_string()),
                ))
            },
        )
        .await;
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert!(
            elapsed_ms < (CLAUDE_HOOK_FAIL_OPEN_TIMEOUT_MS as f64),
            "fail-open took {:.2} ms, exceeding 15.0 ms budget",
            elapsed_ms
        );

        let parsed_resp =
            JsonRpcResponse::<ClaudeHookResult>::decode_line(&out_line).expect("valid response");
        let res = parsed_resp.result.expect("must contain fail-open result");
        assert_eq!(res.decision, ClaudeHookDecisionKind::FailOpen);

        // 2. Daemon responds immediately -> returns exact Allow decision
        let out_ok_line = execute_hook_with_fail_open(
            &raw_line,
            Duration::from_millis(CLAUDE_HOOK_FAIL_OPEN_TIMEOUT_MS),
            |parsed_req| async move {
                Ok(JsonRpcResponse::ok(
                    parsed_req.id,
                    ClaudeHookResult::allow(7, "sha256:verified".to_string()),
                ))
            },
        )
        .await;
        let ok_resp =
            JsonRpcResponse::<ClaudeHookResult>::decode_line(&out_ok_line).expect("decode ok");
        assert_eq!(
            ok_resp.result.unwrap().decision,
            ClaudeHookDecisionKind::Allow
        );
    });
}

// LEDGER-3.1-3
#[test]
fn test_claude_code_tool_use_interception_maps_to_gatekeeper_ticket() {
    let gk = GatekeeperEngine::new(claude_adapter_test_profile()).expect("init gk");
    let mut adapter = ClaudeCodeAdapter::new(gk);

    // Case A: Pre-approved Bash command ("cargo test --lib") -> Allow immediately
    let req_allow = JsonRpcRequest::new(
        Some(JsonRpcId::Number(10)),
        "pre_tool_use",
        ClaudePreToolUseParams {
            session_id: "sess_gk_01".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({ "command": "cargo test --lib" }),
            cwd: Some("C:/Users/cmiranda/dev/miku".to_string()),
            timestamp_ms: 20_000,
        },
    );
    let out_allow = adapter.handle_pre_tool_use(&req_allow);
    assert!(out_allow.pending_ticket.is_none());
    assert_eq!(out_allow.avatar_mood, AvatarMoodState::Working);
    let res_allow = out_allow.rpc_response.result.expect("result allow");
    assert_eq!(res_allow.decision, ClaudeHookDecisionKind::Allow);
    assert!(res_allow.audit_hash.is_some());

    // Case B: Explicitly denied Bash command ("cargo test && rm -rf /") -> Deny win
    let req_deny = JsonRpcRequest::new(
        Some(JsonRpcId::Number(11)),
        "pre_tool_use",
        ClaudePreToolUseParams {
            session_id: "sess_gk_01".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({ "command": "cargo test && rm -rf /" }),
            cwd: Some("C:/Users/cmiranda/dev/miku".to_string()),
            timestamp_ms: 20_001,
        },
    );
    let out_deny = adapter.handle_pre_tool_use(&req_deny);
    assert!(out_deny.pending_ticket.is_none());
    let err_deny = out_deny.rpc_response.error.expect("must return JSON-RPC error on deny");
    assert_eq!(err_deny.code, JSONRPC_GATEKEEPER_DENIED);

    // Case C: Ask rule ("Bash" -> "cargo publish --dry-run") -> Issues PermissionTicket + Island envelope
    let req_ask = JsonRpcRequest::new(
        Some(JsonRpcId::Number(12)),
        "pre_tool_use",
        ClaudePreToolUseParams {
            session_id: "sess_gk_01".to_string(),
            tool_name: "Bash".to_string(),
            tool_input: serde_json::json!({ "command": "cargo publish --dry-run" }),
            cwd: Some("C:/Users/cmiranda/dev/miku".to_string()),
            timestamp_ms: 20_002,
        },
    );
    let out_ask = adapter.handle_pre_tool_use(&req_ask);
    let ticket = out_ask.pending_ticket.expect("must issue PermissionTicket");
    assert_eq!(ticket.capability, Capability::Shell);
    assert_eq!(out_ask.avatar_mood, AvatarMoodState::Alert);
    assert!(matches!(
        out_ask.island_envelope.msg,
        MikuLinkMessageV1::PermissionRequest { .. }
    ));

    // User approves [Y] on PC -> anti-TOCTOU check passes -> Allow
    let resolved_ok = adapter.resolve_pending_ticket(
        Some(JsonRpcId::Number(12)),
        "sess_gk_01",
        &ticket.req_hash,
        PermissionDecisionKind::Allow,
        false,
        false,
        Capability::Shell,
        "cargo publish --dry-run",
        "",
        20_100,
    );
    assert_eq!(
        resolved_ok.rpc_response.result.unwrap().decision,
        ClaudeHookDecisionKind::Allow
    );
    assert_eq!(resolved_ok.avatar_mood, AvatarMoodState::Working);

    // Case D: Global Kill Switch triggered -> blocks any subsequent command with -32002
    let _ = adapter.gatekeeper.trigger_kill_switch(20_200);
    let out_ks = adapter.handle_pre_tool_use(&req_allow);
    assert_eq!(
        out_ks.rpc_response.error.unwrap().code,
        JSONRPC_KILL_SWITCH_ACTIVE
    );
}

// LEDGER-3.1-4
#[test]
fn test_claude_code_activity_telemetry_updates_avatar_mood_working_and_thinking() {
    let mut dispatcher = AvatarActivityDispatcher::new();
    assert_eq!(dispatcher.current_mood(), AvatarMoodState::Idle);

    // 1. Notification(Thinking) -> AvatarMoodState::Thinking
    let ev_think = ClaudeHookEvent::Notification(ClaudeNotificationParams {
        session_id: "sess_av_01".to_string(),
        phase: ClaudeSessionPhase::Thinking,
        message: "Analyzing Rust workspace structure".to_string(),
    });
    let env_think = dispatcher.on_hook_event(&ev_think, 30_000);
    assert_eq!(dispatcher.current_mood(), AvatarMoodState::Thinking);
    assert!(matches!(
        env_think.msg,
        MikuLinkMessageV1::AgentEvent { ref kind, .. } if kind == "avatar_mood:thinking"
    ));

    // 2. PreToolUse(Bash) -> AvatarMoodState::Working
    let ev_pre = ClaudeHookEvent::PreToolUse(ClaudePreToolUseParams {
        session_id: "sess_av_01".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({ "command": "cargo test" }),
        cwd: None,
        timestamp_ms: 30_050,
    });
    let env_work = dispatcher.on_hook_event(&ev_pre, 30_050);
    assert_eq!(dispatcher.current_mood(), AvatarMoodState::Working);
    assert!(matches!(
        env_work.msg,
        MikuLinkMessageV1::AgentEvent { ref kind, .. } if kind == "avatar_mood:working"
    ));

    // 3. PostToolUse(success) -> AvatarMoodState::Happy
    let ev_post = ClaudeHookEvent::PostToolUse(ClaudePostToolUseParams {
        session_id: "sess_av_01".to_string(),
        tool_name: "Bash".to_string(),
        tool_input: serde_json::json!({ "command": "cargo test" }),
        tool_output: serde_json::json!({ "stdout": "test result: ok" }),
        duration_ms: 120,
        is_error: false,
    });
    let _ = dispatcher.on_hook_event(&ev_post, 30_170);
    assert_eq!(dispatcher.current_mood(), AvatarMoodState::Happy);

    // 4. Notification(Idle) -> returns to Idle (0 fps when asleep)
    let ev_idle = ClaudeHookEvent::Notification(ClaudeNotificationParams {
        session_id: "sess_av_01".to_string(),
        phase: ClaudeSessionPhase::Idle,
        message: "Turn completed".to_string(),
    });
    let _ = dispatcher.on_hook_event(&ev_idle, 30_300);
    assert_eq!(dispatcher.current_mood(), AvatarMoodState::Idle);
    assert_eq!(dispatcher.history().len(), 4);
}

// LEDGER-3.1-5
#[test]
fn benchmark_claude_code_adapter_latency_under_2ms_and_ram_under_5mb() {
    let gk = GatekeeperEngine::new(claude_adapter_test_profile()).expect("init gk");
    let mut adapter = ClaudeCodeAdapter::new(gk);

    let mut latencies_ms = Vec::with_capacity(20);
    for i in 0..20 {
        let req = JsonRpcRequest::new(
            Some(JsonRpcId::Number(i as i64)),
            "pre_tool_use",
            ClaudePreToolUseParams {
                session_id: format!("sess_bench_{}", i % 4),
                tool_name: "Bash".to_string(),
                tool_input: serde_json::json!({
                    "command": format!("cargo test --test suite_{}", i)
                }),
                cwd: Some("C:/Users/cmiranda/dev/miku".to_string()),
                timestamp_ms: 50_000 + (i as u64),
            },
        );
        let t0 = Instant::now();
        let out = adapter.handle_pre_tool_use(&req);
        let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(
            out.rpc_response.result.unwrap().decision,
            ClaudeHookDecisionKind::Allow
        );
        latencies_ms.push(dt_ms);
    }

    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95_ms = latencies_ms[18];
    let estimated_state_bytes = std::mem::size_of_val(&adapter)
        + std::mem::size_of_val(adapter.avatar_dispatcher.history())
        + adapter.gatekeeper.audit_log().len() * 256;
    let added_ram_mb = (estimated_state_bytes as f64) / (1024.0 * 1024.0);

    println!(
        "[BENCH-3.1] reps=20 intercept_p95_ms={:.4} added_ram_mb={:.4}",
        p95_ms, added_ram_mb
    );
    assert!(
        p95_ms < CLAUDE_INTERCEPT_LATENCY_BUDGET_MS,
        "interception p95 {:.4} ms exceeded {:.2} ms budget",
        p95_ms,
        CLAUDE_INTERCEPT_LATENCY_BUDGET_MS
    );
    assert!(
        added_ram_mb < CLAUDE_ADDED_RAM_BUDGET_MB,
        "added RAM {:.4} MB exceeded {:.2} MB budget",
        added_ram_mb,
        CLAUDE_ADDED_RAM_BUDGET_MB
    );
    assert!(adapter.gatekeeper.audit_log().verify_chain().is_ok());
}
