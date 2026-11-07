#![allow(unused_imports)]
use crate::prelude::*;

    // LEDGER-2.6-1
    #[test]
    fn test_ui_gatekeeper_interceptor_enforces_deny_by_default_and_path_jail_pre_execution() {
        let profile =
            UiGatekeeperInterceptor::default_production_profile("C:/Users/cmiranda/miku-workspace");
        let mut interceptor = UiGatekeeperInterceptor::new(profile).expect("init UI Gatekeeper");
        let mut bridge = WebView2IpcBridge::new().expect("init IPC bridge");

        // 1. Unlisted capability/target -> DefaultDeny
        let out_default = interceptor
            .intercept_pre_execution(
                &mut bridge,
                "req-2-6-def",
                "cursor",
                Capability::Network,
                "https://untrusted-tracker.example.org/collect",
                "POST",
                &[],
                1_000,
            )
            .expect("intercept default deny");
        assert!(matches!(
            out_default.verdict,
            GatekeeperVerdict::Deny {
                reason: DenyReason::DefaultDeny,
                ..
            }
        ));
        assert_eq!(out_default.avatar_mood, AvatarMood::Error);

        // 2. Explicit Deny rule overrides general Allow/Ask (Deny > Ask > Allow precedence)
        for (cap, target, args) in [
            (Capability::Shell, "rm -rf /", "--no-preserve-root"),
            (Capability::Shell, "git push --force origin main", ""),
            (Capability::FsWrite, "src/secrets/api_keys.toml", "secret=123"),
        ] {
            let out_rule = interceptor
                .intercept_pre_execution(&mut bridge, "req-2-6-rule", "claude_code", cap, target, args, &[], 1_010)
                .expect("intercept explicit deny rule");
            assert!(
                matches!(
                    out_rule.verdict,
                    GatekeeperVerdict::Deny {
                        reason: DenyReason::ExplicitDenyRule,
                        ..
                    }
                ),
                "target '{}' should match ExplicitDenyRule",
                target
            );
        }

        // 3. PathJail escapes (traversal ..\, UNC paths, reserved DOS devices, NTFS ADS)
        for bad_path in [
            r"..\..\Windows\System32\cmd.exe",
            r"\\attacker-server\share\payload.dll",
            "src/CON.txt",
            "src/NUL",
            "src/main.rs:hidden_stream",
        ] {
            let out_jail = interceptor
                .intercept_pre_execution(
                    &mut bridge,
                    "req-2-6-jail",
                    "claude_code",
                    Capability::FsWrite,
                    bad_path,
                    "data",
                    &[bad_path],
                    1_020,
                )
                .expect("intercept path jail escape");
            assert!(
                matches!(
                    out_jail.verdict,
                    GatekeeperVerdict::Deny {
                        reason: DenyReason::PathJailViolation,
                        ..
                    }
                ),
                "path '{}' should be blocked by CanonicalPathJail",
                bad_path
            );
        }

        // 4. AskPending generates ImpactSummaryCard with 100% textContent-sanitized fields (neutralizing XSS)
        let out_ask = interceptor
            .intercept_pre_execution(
                &mut bridge,
                "req-2-6-ask-xss",
                "claude_code",
                Capability::FsWrite,
                "src/lib.rs",
                "<script>fetch('https://evil.com')</script>",
                &["src/lib.rs", "<img src=x onerror=alert(1)>"],
                1_030,
            )
            .expect("intercept AskPending with hostile args");
        match &out_ask.modal_state {
            ConfirmationModalState::ActiveCountdown { impact, .. } => {
                assert_eq!(impact.severity, RiskSeverityLevel::High);
                assert!(!impact.exact_command_or_args.contains("<script"));
                assert!(impact.exact_command_or_args.contains("&lt;script&gt;"));
                for render in &impact.dom_renders {
                    if let WebViewIpcPayloadV1::DomSafeTextRender {
                        safe_text,
                        csp_enforced,
                        ..
                    } = render
                    {
                        assert!(*csp_enforced);
                        assert!(!safe_text.contains('<'));
                        assert!(!safe_text.contains('>'));
                    }
                }
            }
            other => panic!("expected ActiveCountdown modal, got {:?}", other),
        }
    }

    // LEDGER-2.6-2
    #[test]
    fn test_confirmation_modal_ticket_ttl_countdown_and_auto_deny_on_expiration() {
        let profile =
            UiGatekeeperInterceptor::default_production_profile("C:/Users/cmiranda/miku-workspace");
        let mut interceptor = UiGatekeeperInterceptor::new(profile).unwrap();
        let mut bridge = WebView2IpcBridge::new().unwrap();

        // 1. Trigger destructive Ask request (`cargo publish` -> pc_only = true, Destructive severity)
        let ask_out = interceptor
            .intercept_pre_execution(
                &mut bridge,
                "req-publish-1",
                "claude_code",
                Capability::Shell,
                "cargo publish",
                "--allow-dirty",
                &["Cargo.toml", "src/lib.rs"],
                5_000,
            )
            .expect("intercept cargo publish");
        assert_eq!(ask_out.avatar_mood, AvatarMood::Surprised);

        let req_hash = match &ask_out.modal_state {
            ConfirmationModalState::ActiveCountdown {
                ticket,
                impact,
                remaining_ms,
            } => {
                assert_eq!(*remaining_ms, DEFAULT_CONFIRMATION_MODAL_TTL_MS);
                assert_eq!(impact.severity, RiskSeverityLevel::Destructive);
                assert!(impact.pc_only);
                ticket.req_hash.clone()
            }
            other => panic!("expected ActiveCountdown, got {:?}", other),
        };
        assert_eq!(interceptor.engine().pending_ticket_count(), 1);

        // 2. Poll at t = 20_000 ms (15_000 ms elapsed -> 15_000 ms remaining, still active)
        let mid_poll = interceptor.poll_modal_ttl(&mut bridge, 20_000).unwrap();
        assert!(mid_poll.is_none());
        match interceptor.modal_state() {
            ConfirmationModalState::ActiveCountdown { remaining_ms, .. } => {
                assert_eq!(*remaining_ms, 15_000);
            }
            other => panic!("expected ActiveCountdown at mid-TTL, got {:?}", other),
        }

        // 3. Poll past TTL expiration (t = 35_001 ms > expires_at_ms = 35_000 ms) -> Auto-Deny (TicketExpired)
        let expired = interceptor
            .poll_modal_ttl(&mut bridge, 35_001)
            .unwrap()
            .expect("ticket must auto-expire");
        assert!(matches!(
            expired,
            GatekeeperVerdict::Deny {
                reason: DenyReason::TicketExpired,
                ..
            }
        ));
        assert_eq!(interceptor.engine().pending_ticket_count(), 0);
        assert_eq!(interceptor.denied_expired_count(), 1);
        assert_eq!(interceptor.avatar_mood(), AvatarMood::Idle);
        assert!(matches!(
            interceptor.modal_state(),
            ConfirmationModalState::Resolved {
                req_hash: ref h,
                resolved_at_ms: 35_001,
                ..
            } if h == &req_hash
        ));
    }

    // LEDGER-2.6-3
    #[test]
    fn test_anti_toctou_cryptographic_single_use_token_rejects_replay_and_argument_tampering() {
        let profile =
            UiGatekeeperInterceptor::default_production_profile("C:/Users/cmiranda/miku-workspace");
        let mut interceptor = UiGatekeeperInterceptor::new(profile).unwrap();
        let mut bridge = WebView2IpcBridge::new().unwrap();

        // Case A: Anti-TOCTOU argument tampering between visual approval and execution
        let ask_a = interceptor
            .intercept_pre_execution(
                &mut bridge,
                "req-toctou-a",
                "claude_code",
                Capability::Shell,
                "git commit",
                "-m 'fix typo'",
                &["src/lib.rs"],
                1_000,
            )
            .unwrap();
        let hash_a = match ask_a.verdict {
            GatekeeperVerdict::AskPending { ticket, .. } => ticket.req_hash,
            other => panic!("expected AskPending, got {:?}", other),
        };

        let allow_env_a = WebViewIpcEnvelopeV1::new(
            "dec-toctou-a",
            Some("req-toctou-a".to_string()),
            OVERVIEW_VIRTUAL_INDEX_URL,
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::PermissionDecision {
                req_hash: hash_a,
                decision: PermissionDecisionKind::Allow,
                biometric_verified: false,
            }),
        );
        // Attacker mutates args from `-m 'fix typo'` to `-m 'fix typo' && del /F /Q *`
        let tampered_out = interceptor
            .handle_ipc_permission_decision(
                &mut bridge,
                OVERVIEW_VIRTUAL_INDEX_URL,
                &allow_env_a.encode_post_message_json().unwrap(),
                "claude_code",
                Capability::Shell,
                "git commit",
                "-m 'fix typo' && del /F /Q *",
                false,
                1_100,
            )
            .unwrap();
        assert!(matches!(
            tampered_out.verdict,
            GatekeeperVerdict::Deny {
                reason: DenyReason::ToctouHashMismatch,
                ..
            }
        ));
        assert_eq!(interceptor.denied_toctou_count(), 1);

        // Case B: Valid approval followed by Replay attack using the same req_hash
        let ask_b = interceptor
            .intercept_pre_execution(
                &mut bridge,
                "req-replay-b",
                "claude_code",
                Capability::FsWrite,
                "src/lib.rs",
                "pub fn clean() {}",
                &["src/lib.rs"],
                2_000,
            )
            .unwrap();
        let hash_b = match ask_b.verdict {
            GatekeeperVerdict::AskPending { ticket, .. } => ticket.req_hash,
            other => panic!("expected AskPending, got {:?}", other),
        };
        let allow_env_b = WebViewIpcEnvelopeV1::new(
            "dec-replay-b",
            Some("req-replay-b".to_string()),
            OVERVIEW_VIRTUAL_INDEX_URL,
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::PermissionDecision {
                req_hash: hash_b,
                decision: PermissionDecisionKind::Allow,
                biometric_verified: false,
            }),
        );
        let raw_b = allow_env_b.encode_post_message_json().unwrap();

        // First use succeeds
        let first_use = interceptor
            .handle_ipc_permission_decision(
                &mut bridge,
                OVERVIEW_VIRTUAL_INDEX_URL,
                &raw_b,
                "claude_code",
                Capability::FsWrite,
                "src/lib.rs",
                "pub fn clean() {}",
                false,
                2_050,
            )
            .unwrap();
        assert!(matches!(first_use.verdict, GatekeeperVerdict::Allow { .. }));

        // Second use (Replay) is rejected immediately
        let second_use = interceptor
            .handle_ipc_permission_decision(
                &mut bridge,
                OVERVIEW_VIRTUAL_INDEX_URL,
                &raw_b,
                "claude_code",
                Capability::FsWrite,
                "src/lib.rs",
                "pub fn clean() {}",
                false,
                2_060,
            )
            .unwrap();
        assert!(matches!(
            second_use.verdict,
            GatekeeperVerdict::Deny {
                reason: DenyReason::TicketNotFoundOrAlreadyUsed,
                ..
            }
        ));
        assert_eq!(interceptor.denied_replay_count(), 1);

        // Case C: Mobile client attempting to approve a `pc_only = true` destructive action
        let ask_c = interceptor
            .intercept_pre_execution(
                &mut bridge,
                "req-pconly-c",
                "claude_code",
                Capability::Shell,
                "cargo publish",
                "--dry-run",
                &["Cargo.toml"],
                3_000,
            )
            .unwrap();
        let hash_c = match ask_c.verdict {
            GatekeeperVerdict::AskPending { ticket, .. } => ticket.req_hash,
            other => panic!("expected AskPending, got {:?}", other),
        };
        let allow_env_c = WebViewIpcEnvelopeV1::new(
            "dec-pconly-c",
            Some("req-pconly-c".to_string()),
            OVERVIEW_VIRTUAL_INDEX_URL,
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::PermissionDecision {
                req_hash: hash_c,
                decision: PermissionDecisionKind::Allow,
                biometric_verified: true,
            }),
        );
        let mobile_attempt = interceptor
            .handle_ipc_permission_decision(
                &mut bridge,
                OVERVIEW_VIRTUAL_INDEX_URL,
                &allow_env_c.encode_post_message_json().unwrap(),
                "claude_code",
                Capability::Shell,
                "cargo publish",
                "--dry-run",
                true, // from_mobile_client = true
                3_050,
            )
            .unwrap();
        assert!(matches!(
            mobile_attempt.verdict,
            GatekeeperVerdict::Deny {
                reason: DenyReason::PcOnlyFromMobileForbidden,
                ..
            }
        ));
    }

    // LEDGER-2.6-4
    #[test]
    fn test_ipc_permission_request_decision_channel_and_tamper_evident_sha256_audit_chain() {
        let report =
            run_ui_gatekeeper_interceptor_audit(5).expect("run 5-cycle UI Gatekeeper audit");
        assert_eq!(report.allowed_count, 10); // 5 direct Allow + 5 Ask->Allow
        assert_eq!(report.denied_default_count, 5);
        assert_eq!(report.denied_rule_count, 5);
        assert_eq!(report.denied_jail_count, 5);
        assert_eq!(report.denied_toctou_count, 5);
        assert_eq!(report.denied_expired_count, 5);
        assert_eq!(report.denied_replay_count, 5);
        assert_eq!(report.audit_chain_entries, 55);
        assert!(report.audit_chain_verified);
        assert!(report.tamper_simulation_detected);

        let limit_ms = if cfg!(debug_assertions) {
            15.0
        } else {
            MAX_GATEKEEPER_INTERCEPT_LATENCY_MS
        };
        assert!(
            report.max_intercept_latency_ms < limit_ms,
            "max intercept latency {:.4} ms exceeded limit {:.1} ms",
            report.max_intercept_latency_ms,
            limit_ms
        );
    }

