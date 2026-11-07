#![allow(unused_imports)]
use crate::prelude::*;

    // LEDGER-2.2-1
    #[test]
    fn test_isolated_user_data_folder_resolves_under_localappdata_and_blocks_traversal() {
        let valid = format_canonical_windows_webview2_data_dir(
            r"C:\Users\Camila Miranda\AppData\Local",
        )
        .expect("valid Windows LOCALAPPDATA path");
        assert_eq!(
            valid,
            r"C:\Users\Camila Miranda\AppData\Local\miku\webview2"
        );

        // Mixed forward slashes are normalized to canonical Windows backslashes
        let mixed =
            format_canonical_windows_webview2_data_dir("C:/Users/cmiranda/AppData/Local/")
                .expect("mixed slashes normalized");
        assert_eq!(mixed, r"C:\Users\cmiranda\AppData\Local\miku\webview2");

        // Idempotent if \miku\webview2 is already appended
        let idem = format_canonical_windows_webview2_data_dir(
            r"C:\Users\cmiranda\AppData\Local\miku\webview2",
        )
        .expect("idempotent isolated suffix");
        assert_eq!(idem, r"C:\Users\cmiranda\AppData\Local\miku\webview2");

        // Reject relative paths, traversal `..`, null bytes, and unisolated EBWebView folders
        assert!(matches!(
            format_canonical_windows_webview2_data_dir(r"AppData\Local"),
            Err(UiError::InvalidUserDataFolder(_))
        ));
        assert!(matches!(
            format_canonical_windows_webview2_data_dir(
                r"C:\Users\cmiranda\AppData\Local\..\..\Windows\System32"
            ),
            Err(UiError::InvalidUserDataFolder(_))
        ));
        assert!(matches!(
            format_canonical_windows_webview2_data_dir("C:\\Users\\cmiranda\0\\AppData\\Local"),
            Err(UiError::InvalidUserDataFolder(_))
        ));
        assert!(matches!(
            format_canonical_windows_webview2_data_dir(r"C:\Program Files\Miku\miku.exe.WebView2\EBWebView"),
            Err(UiError::InvalidUserDataFolder(_))
        ));

        // Verify resolve_isolated_webview2_user_data_dir with explicit override
        let override_path = Path::new(r"C:\Users\cmiranda\AppData\Local");
        let resolved =
            resolve_isolated_webview2_user_data_dir(Some(override_path)).expect("override dir");
        assert_eq!(
            resolved.to_string_lossy(),
            r"C:\Users\cmiranda\AppData\Local\miku\webview2"
        );
    }

    // LEDGER-2.2-2
    #[test]
    fn test_webview2_security_policy_disables_devtools_in_release_and_blocks_external_navigation() {
        let rel_policy = WebView2SecurityPolicy::release_hardened();
        rel_policy
            .validate_release_hardening()
            .expect("release hardening valid");
        assert!(!rel_policy.are_devtools_enabled);
        assert!(!rel_policy.are_default_context_menus_enabled);
        assert!(!rel_policy.is_status_bar_enabled);
        assert!(!rel_policy.is_zoom_control_enabled);
        assert!(!rel_policy.are_host_objects_allowed);

        // Enabling DevTools in release must fail validation
        let mut weakened = rel_policy.clone();
        weakened.are_devtools_enabled = true;
        assert!(weakened.validate_release_hardening().is_err());

        // Allowed local origins
        assert_eq!(
            evaluate_navigation_target("https://miku.local/index.html", &rel_policy),
            NavigationVerdict::AllowLocalOrigin {
                normalized_uri: "https://miku.local/index.html".to_string()
            }
        );
        assert_eq!(
            evaluate_navigation_target("about:blank", &rel_policy),
            NavigationVerdict::AllowLocalOrigin {
                normalized_uri: "about:blank".to_string()
            }
        );

        // Blocked external or unsafe URIs
        for forbidden_uri in [
            "https://evil.example.com/index.html",
            "http://miku.local/index.html",
            "file:///C:/Windows/System32/cmd.exe",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "https://miku.local/@evil.com/steal",
            "https://miku.local/../../secrets.txt",
        ] {
            assert!(
                matches!(
                    evaluate_navigation_target(forbidden_uri, &rel_policy),
                    NavigationVerdict::BlockExternalOrUnsafe { .. }
                ),
                "URI '{}' should have been blocked",
                forbidden_uri
            );
        }
    }

    // LEDGER-2.2-3
    #[test]
    fn test_mode_c_ttl_lifecycle_state_machine_lazy_spawn_warm_reuse_and_ttl_eviction() {
        let monitor = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY1".to_string(),
            is_primary: true,
            dpi: 96,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
        };
        let policy = WebView2SecurityPolicy::release_hardened();
        let driver = IsolatedWebView2ProcessDriver::new(2.51, 60.0);
        let override_dir = Path::new(r"C:\Users\cmiranda\AppData\Local");
        let mut host = WebView2HostStateMachine::new(
            monitor,
            0,
            Some(override_dir),
            policy,
            15_000,
            driver,
        )
        .expect("init Mode C host FSM");

        // 1. Initially Unloaded: 0 instances, 0 child PIDs, 0 timers
        assert_eq!(host.state(), WebView2LifecycleState::Unloaded);
        assert_eq!(host.active_webview2_instances(), 0);
        assert!(host.active_child_pids().is_empty());
        assert_eq!(host.active_timers(), 0);

        // 2. First expand at t=1000ms -> cold spawn
        let exp1 = host.expand_island(1_000).expect("cold expand");
        assert!(!exp1.reused_warm);
        assert_eq!(host.cold_spawns_count(), 1);
        assert_eq!(host.warm_reuses_count(), 0);
        assert_eq!(host.active_webview2_instances(), 1);
        assert_eq!(host.active_child_pids().len(), 3);
        assert_eq!(host.active_timers(), 0);

        // 3. Collapse at t=2000ms -> enters CollapsedWarmTtl with 1 countdown timer
        let col1 = host.collapse_island(2_000).expect("collapse to warm TTL");
        assert!(matches!(
            col1,
            WebView2LifecycleState::CollapsedWarmTtl {
                collapsed_at_ms: 2_000,
                ttl_ms: 15_000,
                ..
            }
        ));
        assert_eq!(host.active_timers(), 1);

        // 4. Re-expand at t=5000ms (within 15_000ms TTL) -> warm reuse without respawning!
        let exp2 = host.expand_island(5_000).expect("warm re-expand");
        assert!(exp2.reused_warm);
        assert_eq!(host.cold_spawns_count(), 1);
        assert_eq!(host.warm_reuses_count(), 1);
        assert_eq!(host.active_timers(), 0);

        // 5. Collapse again at t=6000ms and advance past TTL (t=21_500ms) -> evicts to EvictedIdle
        let _ = host.collapse_island(6_000).expect("second collapse");
        assert!(!host.poll_ttl(10_000).unwrap()); // Not yet expired
        assert!(host.poll_ttl(21_500).unwrap()); // Expired!
        assert_eq!(
            host.state(),
            WebView2LifecycleState::EvictedIdle {
                last_evicted_at_ms: 21_500,
                total_evictions: 1,
            }
        );
        assert_eq!(host.active_webview2_instances(), 0);
        assert!(host.active_child_pids().is_empty());
        assert_eq!(host.active_timers(), 0);
        assert!(host.current_private_ram_mb() <= MAX_IDLE_POST_TTL_RAM_MB);
    }

    // LEDGER-2.2-4
    #[test]
    fn test_five_consecutive_expand_collapse_cycles_reclaim_ram_and_leave_zero_child_processes() {
        let monitor = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY1".to_string(),
            is_primary: true,
            dpi: 96,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
        };
        let override_dir = Path::new(r"C:\Users\cmiranda\AppData\Local");
        let report = run_five_cycle_webview2_ttl_recovery_audit(monitor, Some(override_dir), 3.72)
            .expect("5 consecutive expand/collapse cycles pass");

        assert_eq!(report.cycles_completed, FIVE_CYCLE_STRESS_REPS);
        assert_eq!(report.cold_spawns, 5);
        assert_eq!(report.warm_reuses, 1);
        assert_eq!(report.ttl_evictions, 5);
        assert_eq!(report.blocked_external_navs, 5);
        assert_eq!(report.residual_webview2_pids, 0);
        assert_eq!(report.residual_idle_timers, 0);
        assert!(report.devtools_disabled_in_release);
        assert!(report.max_post_ttl_ram_mb <= MAX_IDLE_POST_TTL_RAM_MB);
        assert!(report.max_expand_latency_ms <= MAX_EXPAND_VISIBLE_LATENCY_MS);
        assert_eq!(
            report.isolated_user_data_dir,
            r"C:\Users\cmiranda\AppData\Local\miku\webview2"
        );
    }

    // LEDGER-2.3-1
    #[test]
    fn test_ipc_envelope_jsonrpc_v1_roundtrip_all_miku_link_and_ui_variants() {
        let payloads = vec![
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::Hello {
                client: "miku_webview2".to_string(),
                token: "tok_local_123".to_string(),
                capabilities: vec!["ui_bridge".to_string()],
            }),
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::HelloAck {
                session_id: "sess_wv2".to_string(),
                island_state: IslandState::Expanded,
            }),
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::Ping { ts: 1_762_400_100 }),
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::Pong { ts: 1_762_400_100 }),
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::AgentEvent {
                agent: "claude_code".to_string(),
                kind: "tool_running".to_string(),
                title: "cargo test --workspace".to_string(),
            }),
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::PermissionRequest {
                req_hash: "sha256:0123456789abcdef".to_string(),
                agent: "claude_code".to_string(),
                capability: "fs_write".to_string(),
                target: "src/lib.rs".to_string(),
                ttl_sec: 30,
                pc_only: true,
            }),
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::PermissionDecision {
                req_hash: "sha256:0123456789abcdef".to_string(),
                decision: PermissionDecisionKind::Allow,
                biometric_verified: false,
            }),
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::Command {
                session_id: "sess_wv2".to_string(),
                action: "enqueue_prompt".to_string(),
                text: "run clippy".to_string(),
            }),
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::Error {
                code: "gatekeeper_denied".to_string(),
                supported: Some(vec![1]),
                detail: Some("denied by policy".to_string()),
            }),
            WebViewIpcPayloadV1::UiInteraction {
                event: UiInteractionKind::HoverEnter,
                target_id: "island-pill".to_string(),
                x: 960,
                y: 22,
                timestamp_ms: 1_000,
            },
            WebViewIpcPayloadV1::TelemetrySnapshot {
                island_state: IslandState::Expanded,
                webview_state: "ActiveExpanded".to_string(),
                ram_private_mb_x100: 380,
                rtt_us: 14,
                active_agents: 2,
            },
            WebViewIpcPayloadV1::DomSafeTextRender {
                element_id: "agent-card-title".to_string(),
                safe_text: "Claude Code: Ready".to_string(),
                csp_enforced: true,
            },
        ];

        assert_eq!(payloads.len(), 12); // 9 MikuLinkMessageV1 + 3 native UI variants
        for (idx, payload) in payloads.into_iter().enumerate() {
            let env = WebViewIpcEnvelopeV1::new(
                format!("msg-2-3-{}", idx),
                if idx % 2 == 1 {
                    Some(format!("msg-2-3-{}", idx - 1))
                } else {
                    None
                },
                "https://miku.local/index.html",
                payload,
            );
            let json = env
                .encode_post_message_json()
                .expect("serialize WebViewIpcEnvelopeV1");
            let decoded = WebViewIpcEnvelopeV1::decode_and_validate(&json)
                .expect("deserialize WebViewIpcEnvelopeV1");
            assert_eq!(env, decoded);
        }
    }

    // LEDGER-2.3-2
    #[test]
    fn test_ipc_bridge_enforces_local_origin_csp_and_safe_text_content_xss_neutralization() {
        // 1. Strict local origin validation
        assert!(validate_ipc_sender_origin("https://miku.local").is_ok());
        assert!(validate_ipc_sender_origin("https://miku.local/index.html").is_ok());

        for bad_origin in [
            "",
            "null",
            "*",
            "http://miku.local/index.html",
            "https://miku.local.evil.com/index.html",
            "https://evil.example.com",
            "file:///C:/Windows/System32/cmd.exe",
            "https://miku.local/../../etc/passwd",
            "https://miku.local/@attacker.com",
        ] {
            assert!(
                matches!(
                    validate_ipc_sender_origin(bad_origin),
                    Err(UiError::UnauthorizedIpcOrigin(_))
                ),
                "origin '{}' should be rejected",
                bad_origin
            );
        }

        // 2. Strict CSP validation
        validate_webview_csp_policy(MIKU_WEBVIEW_CSP_HEADER).expect("default CSP valid");
        assert!(validate_webview_csp_policy("default-src 'self'; script-src *").is_err());
        assert!(validate_webview_csp_policy(
            "default-src 'self'; script-src 'self' 'unsafe-eval'; connect-src 'none'; object-src 'none'"
        )
        .is_err());

        // 3. XSS payload neutralization for textContent
        let xss_input = "<img src=x onerror=alert(1)><script>fetch('https://evil.com')</script>";
        let rendered = sanitize_for_text_content("agent-title-node", xss_input)
            .expect("sanitize XSS payload");
        match rendered {
            WebViewIpcPayloadV1::DomSafeTextRender {
                element_id,
                safe_text,
                csp_enforced,
            } => {
                assert_eq!(element_id, "agent-title-node");
                assert!(csp_enforced);
                assert!(!safe_text.contains('<'));
                assert!(!safe_text.contains('>'));
                assert!(safe_text.contains("&lt;img src=x onerror=alert(1)&gt;"));
            }
            other => panic!("unexpected payload variant: {:?}", other),
        }
    }

    // LEDGER-2.3-3
    #[test]
    fn test_ipc_bridge_discards_malformed_oversized_and_unknown_field_frames_without_panic() {
        let mut bridge = WebView2IpcBridge::new().expect("init bridge");
        let valid_origin = "https://miku.local/index.html";

        let oversized = "x".repeat(MAX_IPC_MESSAGE_BYTES + 128);
        let malformed_samples: Vec<&str> = vec![
            "",
            "{not_valid_json",
            r#"{"jsonrpc":"1.0","v":1,"id":"m1","origin":"https://miku.local/","payload":{"kind":"miku_link","data":{"type":"ping","data":{"ts":1}}}}"#,
            r#"{"jsonrpc":"2.0","v":99,"id":"m2","origin":"https://miku.local/","payload":{"kind":"miku_link","data":{"type":"ping","data":{"ts":1}}}}"#,
            r#"{"jsonrpc":"2.0","v":1,"id":"","origin":"https://miku.local/","payload":{"kind":"miku_link","data":{"type":"ping","data":{"ts":1}}}}"#,
            r#"{"jsonrpc":"2.0","v":1,"id":"m3","origin":"https://miku.local/","unexpected_field":true,"payload":{"kind":"miku_link","data":{"type":"ping","data":{"ts":1}}}}"#,
            "{\"jsonrpc\":\"2.0\",\"v\":1,\"id\":\"m4\0\",\"origin\":\"https://miku.local/\"}",
            &oversized,
        ];

        for (idx, bad_frame) in malformed_samples.iter().enumerate() {
            let res = bridge.handle_inbound_raw_message(valid_origin, bad_frame);
            assert!(
                res.is_err(),
                "malformed sample #{} should return Err without panicking",
                idx
            );
        }

        assert_eq!(
            bridge.dropped_malformed_frames(),
            malformed_samples.len() as u64
        );
        // Each dropped frame queued a typed error envelope for diagnostic correlation
        assert_eq!(bridge.pending_outbound_count(), malformed_samples.len());
        let _ = bridge.drain_all_buffers();
        assert_eq!(bridge.retained_buffer_bytes(), 0);
    }

    // LEDGER-2.3-4
    #[test]
    fn test_bidirectional_post_message_roundtrip_core_routing_and_zero_buffer_leak() {
        let mut bridge = WebView2IpcBridge::new().expect("init bridge");
        let origin = "https://miku.local/index.html";

        // 1. UI HoverEnter -> TelemetrySnapshot
        let ui_req = WebViewIpcEnvelopeV1::new(
            "ui-hover-1",
            None,
            origin,
            WebViewIpcPayloadV1::UiInteraction {
                event: UiInteractionKind::HoverEnter,
                target_id: "island-pill".to_string(),
                x: 960,
                y: 18,
                timestamp_ms: 500,
            },
        );
        let (ui_resp, rtt1) = bridge
            .handle_inbound_raw_message(origin, &ui_req.encode_post_message_json().unwrap())
            .expect("UI interaction roundtrip");
        assert_eq!(ui_resp.reply_to.as_deref(), Some("ui-hover-1"));
        assert!(rtt1 < MAX_IPC_RTT_BUDGET_MS || cfg!(debug_assertions));
        assert!(matches!(
            ui_resp.payload,
            WebViewIpcPayloadV1::TelemetrySnapshot {
                island_state: IslandState::Peek,
                ..
            }
        ));

        // 2. PermissionDecision -> Gatekeeper AgentEvent
        let perm_req = WebViewIpcEnvelopeV1::new(
            "perm-dec-1",
            Some("req-99".to_string()),
            origin,
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::PermissionDecision {
                req_hash: "sha256:deadbeef".to_string(),
                decision: PermissionDecisionKind::Allow,
                biometric_verified: false,
            }),
        );
        let (perm_resp, _) = bridge
            .handle_inbound_raw_message(origin, &perm_req.encode_post_message_json().unwrap())
            .expect("PermissionDecision roundtrip");
        assert_eq!(perm_resp.reply_to.as_deref(), Some("perm-dec-1"));
        assert_eq!(
            perm_resp.payload,
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::AgentEvent {
                agent: "miku_gatekeeper".to_string(),
                kind: "permission_allowed".to_string(),
                title: "sha256:deadbeef".to_string(),
            })
        );

        assert_eq!(bridge.roundtrips_completed(), 2);
        assert_eq!(bridge.pending_inbound_count(), 0);
        assert_eq!(bridge.pending_outbound_count(), 2);

        // 3. Simulate TTL eviction buffer drain -> 0 retained bytes
        let drained = bridge.drain_all_buffers();
        assert_eq!(drained, 2);
        assert_eq!(bridge.pending_inbound_count(), 0);
        assert_eq!(bridge.pending_outbound_count(), 0);
        assert_eq!(bridge.retained_buffer_bytes(), 0);
    }

