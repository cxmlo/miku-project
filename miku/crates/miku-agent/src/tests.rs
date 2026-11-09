use super::*;
use std::time::{Duration, Instant};
use miku_core::{build_current_thread_runtime, spawn_orchestrator};
use miku_gatekeeper::{Capability, DenyReason, GatekeeperEngine, PolicyEffect, ProjectProfile};
use miku_gateway::{start_gateway_server, GatewayConfig, LoopbackWsClient};
use miku_protocol::{BaseContractBundle, PermissionDecisionKind};

pub(crate) fn sample_agent_profile() -> ProjectProfile {
    ProjectProfile::new("miku-agent-test", "C:/Users/cmiranda/dev/miku", 60_000)
        .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
        .with_rule(Capability::FsRead, "memory/**", PolicyEffect::Allow, false)
        .with_rule(
            Capability::FsRead,
            "src/secrets/**",
            PolicyEffect::Deny,
            true,
        )
        .with_rule(Capability::FsWrite, "src/**", PolicyEffect::Ask, false)
        .with_rule(Capability::Shell, "cargo test*", PolicyEffect::Allow, false)
        .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true)
        .with_rule(
            Capability::Browser,
            "https://docs.rs/*",
            PolicyEffect::Allow,
            false,
        )
}

// LEDGER-1.4-1
#[test]
fn test_deterministic_tool_registry_internal_tools_and_duplicate_rejection() {
    let mut reg = ToolRegistry::with_default_internal_tools();
    assert_eq!(reg.len(), 5);

    // Verify deterministic BTreeMap ordering
    let names: Vec<String> = reg
        .list_deterministic()
        .into_iter()
        .map(|d| d.name.clone())
        .collect();
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

    // Verify exact family and capability mappings for FS, Shell, Browser, Memory
    assert_eq!(
        reg.get("fs_read").unwrap().required_capability,
        Capability::FsRead
    );
    assert_eq!(
        reg.get("fs_write").unwrap().required_capability,
        Capability::FsWrite
    );
    assert_eq!(
        reg.get("shell_exec").unwrap().required_capability,
        Capability::Shell
    );
    assert_eq!(
        reg.get("browser_action").unwrap().required_capability,
        Capability::Browser
    );
    assert_eq!(
        reg.get("memory_query").unwrap().family,
        InternalToolFamily::Memory
    );

    // Duplicate registration must fail deterministically
    let dup_err = reg
        .register(ToolDescriptor {
            name: "FS_READ".to_string(),
            family: InternalToolFamily::Fs,
            required_capability: Capability::FsRead,
            requires_non_empty_target: true,
            description: "duplicate".to_string(),
        })
        .expect_err("duplicate tool must be rejected");
    assert!(matches!(dup_err, ToolError::DuplicateTool(ref n) if n == "fs_read"));

    // Unknown tool must fail with ToolNotFound
    assert!(matches!(
        reg.get("unknown_tool"),
        Err(ToolError::ToolNotFound(_))
    ));

    // Empty target or null bytes must fail schema validation
    assert!(matches!(
        reg.validate_invocation("fs_read", "   ", ""),
        Err(ToolError::InvalidArguments(_))
    ));
    assert!(matches!(
        reg.validate_invocation("fs_read", "src/lib.rs\0", ""),
        Err(ToolError::InvalidArguments(_))
    ));
}

// LEDGER-1.4-2
#[test]
fn test_pre_execution_gatekeeper_interception_and_capability_token_validation() {
    let rt = build_current_thread_runtime().expect("current_thread runtime");
    rt.block_on(async {
        let gk = GatekeeperEngine::new(sample_agent_profile()).expect("init gk");
        let mut mgr = AgentSessionManager::new(
            ToolRegistry::with_default_internal_tools(),
            gk,
            "miku_session_secret_01".to_string(),
        );

        // 1. Invalid session token -> Unauthorized (0 handler executions)
        let out_unauth = mgr
            .dispatch_immediate(ToolInvocationRequest {
                task_id: "t-unauth".to_string(),
                session_id: "s1".to_string(),
                agent: "claude_code".to_string(),
                session_token: "wrong_token".to_string(),
                tool_name: "fs_read".to_string(),
                target: "src/lib.rs".to_string(),
                args: "".to_string(),
                simulated_work_ms: 0,
                timestamp_ms: 1_000,
            })
            .await
            .expect("dispatch unauth");
        assert!(matches!(out_unauth, ToolDispatchOutcome::Unauthorized { .. }));
        assert_eq!(mgr.handler_execution_count(), 0);
        assert_eq!(mgr.task_state("t-unauth"), Some(TaskLifecycleState::Denied));

        // 2. Gatekeeper ExplicitDenyRule ("src/secrets/**") -> Denied (0 handler executions)
        let out_deny = mgr
            .dispatch_immediate(ToolInvocationRequest {
                task_id: "t-deny".to_string(),
                session_id: "s1".to_string(),
                agent: "claude_code".to_string(),
                session_token: "miku_session_secret_01".to_string(),
                tool_name: "fs_read".to_string(),
                target: "src/secrets/key.pem".to_string(),
                args: "".to_string(),
                simulated_work_ms: 0,
                timestamp_ms: 1_001,
            })
            .await
            .expect("dispatch deny");
        assert!(matches!(
            out_deny,
            ToolDispatchOutcome::Denied {
                reason: DenyReason::ExplicitDenyRule,
                ..
            }
        ));
        assert_eq!(mgr.handler_execution_count(), 0);

        // 3. Gatekeeper PathJailViolation ("..\..\Windows\System32\cmd.exe") -> Denied
        let out_jail = mgr
            .dispatch_immediate(ToolInvocationRequest {
                task_id: "t-jail".to_string(),
                session_id: "s1".to_string(),
                agent: "cursor".to_string(),
                session_token: "miku_session_secret_01".to_string(),
                tool_name: "fs_write".to_string(),
                target: r"..\..\Windows\System32\cmd.exe".to_string(),
                args: "pwn".to_string(),
                simulated_work_ms: 0,
                timestamp_ms: 1_002,
            })
            .await
            .expect("dispatch jail");
        assert!(matches!(
            out_jail,
            ToolDispatchOutcome::Denied {
                reason: DenyReason::PathJailViolation,
                ..
            }
        ));
        assert_eq!(mgr.handler_execution_count(), 0);

        // 4. Gatekeeper AskPending ("fs_write" on "src/main.rs") -> Suspends execution until approved
        let out_ask = mgr
            .dispatch_immediate(ToolInvocationRequest {
                task_id: "t-ask".to_string(),
                session_id: "s1".to_string(),
                agent: "claude_code".to_string(),
                session_token: "miku_session_secret_01".to_string(),
                tool_name: "fs_write".to_string(),
                target: "src/main.rs".to_string(),
                args: "fn verified() {}".to_string(),
                simulated_work_ms: 0,
                timestamp_ms: 1_003,
            })
            .await
            .expect("dispatch ask");
        let ticket = match out_ask {
            ToolDispatchOutcome::SuspendedAsk { ticket, .. } => ticket,
            other => panic!("expected SuspendedAsk, got {:?}", other),
        };
        assert_eq!(
            mgr.task_state("t-ask"),
            Some(TaskLifecycleState::SuspendedAsk)
        );
        assert_eq!(mgr.handler_execution_count(), 0);

        // Resume with matching args -> Completed (handler_execution_count becomes 1)
        let out_resumed = mgr
            .resume_suspended_ask_task(
                "t-ask",
                &ticket.req_hash,
                PermissionDecisionKind::Allow,
                false,
                false,
                "src/main.rs",
                "fn verified() {}",
                1_004,
            )
            .await
            .expect("resume ask");
        assert!(matches!(out_resumed, ToolDispatchOutcome::Completed(_)));
        assert_eq!(mgr.handler_execution_count(), 1);
        assert_eq!(
            mgr.task_state("t-ask"),
            Some(TaskLifecycleState::Completed)
        );
    });
}

// LEDGER-1.4-3
#[test]
fn test_agent_session_serial_queue_and_cooperative_cancellation_lifecycle() {
    let rt = build_current_thread_runtime().expect("current_thread runtime");
    rt.block_on(async {
        let gk = GatekeeperEngine::new(sample_agent_profile()).expect("init gk");
        let mut mgr = AgentSessionManager::new(
            ToolRegistry::with_default_internal_tools(),
            gk,
            "miku_session_secret_01".to_string(),
        );

        // Enqueue 3 tasks in session "sess_fifo"
        let _h1 = mgr
            .enqueue_task(ToolInvocationRequest {
                task_id: "fifo-1".to_string(),
                session_id: "sess_fifo".to_string(),
                agent: "claude_code".to_string(),
                session_token: "miku_session_secret_01".to_string(),
                tool_name: "fs_read".to_string(),
                target: "src/one.rs".to_string(),
                args: "".to_string(),
                simulated_work_ms: 0,
                timestamp_ms: 2_000,
            })
            .expect("enqueue 1");

        let h2 = mgr
            .enqueue_task(ToolInvocationRequest {
                task_id: "fifo-2".to_string(),
                session_id: "sess_fifo".to_string(),
                agent: "claude_code".to_string(),
                session_token: "miku_session_secret_01".to_string(),
                tool_name: "shell_exec".to_string(),
                target: "cargo test --lib".to_string(),
                args: "--nocapture".to_string(),
                simulated_work_ms: 250,
                timestamp_ms: 2_001,
            })
            .expect("enqueue 2");

        let _h3 = mgr
            .enqueue_task(ToolInvocationRequest {
                task_id: "fifo-3".to_string(),
                session_id: "sess_fifo".to_string(),
                agent: "claude_code".to_string(),
                session_token: "miku_session_secret_01".to_string(),
                tool_name: "memory_query".to_string(),
                target: "memory/architecture.md".to_string(),
                args: "ADR-007".to_string(),
                simulated_work_ms: 0,
                timestamp_ms: 2_002,
            })
            .expect("enqueue 3");

        // 1st task completes in FIFO order
        let r1 = mgr
            .dispatch_next_in_session("sess_fifo")
            .await
            .unwrap()
            .expect("run 1");
        assert!(matches!(r1, ToolDispatchOutcome::Completed(ref o) if o.task_id == "fifo-1"));

        // Trigger cooperative cancellation on 2nd task while in flight (250 ms simulated work, cancelled after 1 ms)
        let h2_clone = h2.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            h2_clone.cancel();
        });

        let t_cancel = Instant::now();
        let r2 = mgr
            .dispatch_next_in_session("sess_fifo")
            .await
            .unwrap()
            .expect("run 2");
        let cancel_elapsed_ms = t_cancel.elapsed().as_secs_f64() * 1000.0;
        assert!(matches!(r2, ToolDispatchOutcome::Cancelled { ref task_id, .. } if task_id == "fifo-2"));
        assert!(
            cancel_elapsed_ms < 50.0,
            "cooperative cancellation must abort well before 250 ms work finishes"
        );
        assert_eq!(
            mgr.task_state("fifo-2"),
            Some(TaskLifecycleState::Cancelled)
        );

        // Trigger Global Kill Switch -> cancels remaining queued task "fifo-3"
        let cancelled_count = mgr.trigger_global_kill_switch(2_050);
        assert_eq!(cancelled_count, 1);
        assert_eq!(
            mgr.task_state("fifo-3"),
            Some(TaskLifecycleState::Cancelled)
        );
        assert!(mgr.dispatch_next_in_session("sess_fifo").await.is_none());
    });
}

// LEDGER-1.4-4
#[test]
fn test_end_to_end_tool_dispatch_over_gateway_and_core_event_bus() {
    let rt = build_current_thread_runtime().expect("current_thread runtime");
    rt.block_on(async {
        let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
        let (orch_handle, orch_join) =
            spawn_orchestrator(bundle, "miku_agent_ws_tok".to_string(), 64);
        let (gw_handle, gw_join) =
            start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                .await
                .expect("start gateway");

        let gk = GatekeeperEngine::new(sample_agent_profile()).expect("init gk");
        let mut mgr = AgentSessionManager::new(
            ToolRegistry::with_default_internal_tools(),
            gk,
            "miku_agent_ws_tok".to_string(),
        );

        let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
            .await
            .expect("connect WS");
        let hello = r##"{"v":1,"id":"h-agent","msg":{"type":"hello","data":{"client":"miku_island","token":"miku_agent_ws_tok","capabilities":["command","agent_events"]}}}"##;
        client.send_text(hello).await.expect("send hello");
        let _ = client.recv_envelope().await.expect("recv hello_ack");

        // Send command over Gateway WS and execute all 4 internal tool families (FS, Shell, Browser, Memory)
        let cmd = r##"{"v":1,"id":"cmd-1","msg":{"type":"command","data":{"session_id":"sess_01","action":"dispatch_tool","text":"browser_action https://docs.rs/tokio"}}}"##;
        client.send_text(cmd).await.expect("send command");
        let gw_ack = client.recv_envelope().await.expect("recv gw ack");
        assert_eq!(gw_ack.reply_to.as_deref(), Some("cmd-1"));

        for (idx, (tool, target, args)) in [
            ("fs_read", "src/lib.rs", ""),
            ("shell_exec", "cargo test --workspace", ""),
            ("browser_action", "https://docs.rs/tokio", "navigate"),
            ("memory_query", "memory/notes.md", "tokio current_thread"),
        ]
        .iter()
        .enumerate()
        {
            let req_id = format!("req-{}", idx + 1);
            let outcome = mgr
                .dispatch_immediate(ToolInvocationRequest {
                    task_id: format!("task-{}", idx + 1),
                    session_id: "sess_01".to_string(),
                    agent: "claude_code".to_string(),
                    session_token: "miku_agent_ws_tok".to_string(),
                    tool_name: (*tool).to_string(),
                    target: (*target).to_string(),
                    args: (*args).to_string(),
                    simulated_work_ms: 0,
                    timestamp_ms: 3_000 + (idx as u64),
                })
                .await
                .expect("dispatch internal tool");
            assert!(matches!(outcome, ToolDispatchOutcome::Completed(_)));

            let env = mgr.outcome_to_protocol_envelope(&req_id, "claude_code", &outcome);
            assert_eq!(env.reply_to.as_deref(), Some(req_id.as_str()));
        }

        assert_eq!(mgr.handler_execution_count(), 4);
        assert!(mgr.gatekeeper().audit_log().verify_chain().is_ok());

        client.close_cleanly().await.expect("close WS");
        gw_handle.trigger_shutdown();
        gw_join.await.expect("gw join");
        orch_handle.trigger_system_shutdown_signal();
        orch_join.await.expect("orch join");
    });
}
