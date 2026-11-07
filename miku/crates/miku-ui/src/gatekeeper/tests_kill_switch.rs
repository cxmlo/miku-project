#![allow(unused_imports)]
use crate::prelude::*;

    // LEDGER-2.9-1
    #[test]
    fn test_kill_switch_hotkey_registration_and_graceful_fallback_on_conflict() {
        let mut ks = UiKillSwitchManager::new_default();
        assert_eq!(ks.status(), &KillSwitchStatus::Armed);
        assert_eq!(ks.hotkey_id(), KILL_SWITCH_HOTKEY_ID);
        assert_eq!(ks.hotkey_combo(), DEFAULT_KILL_SWITCH_HOTKEY_COMBO);
        assert_eq!(ks.modifiers_mask(), 0x4007); // MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT
        assert_eq!(ks.vk_code(), 0x4B); // VK_K

        // 1. Normal registration (never panics even if OS shortcut is held externally)
        let normal_reg = ks.register_hotkey(false);
        assert_eq!(normal_reg.hotkey_id, 0x4D4B);
        assert_eq!(normal_reg.modifiers_mask, 0x4007);
        assert_eq!(normal_reg.vk_code, 0x4B);
        assert!(normal_reg.fallback_ui_button_active);

        // 2. Conflict simulation: another app already holds Ctrl+Alt+Shift+K -> graceful fallback, 0 panic
        let conflict_reg = ks.register_hotkey(true);
        assert!(!conflict_reg.os_registered);
        assert!(conflict_reg.fallback_ui_button_active);
        assert!(conflict_reg.diagnostic_note.contains("fallback"));
    }

    // LEDGER-2.9-2
    #[test]
    fn test_kill_switch_trip_revokes_tickets_aborts_voice_zeroes_pcm_and_sets_alert_mood() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).unwrap();
        let avatar = reloader.runtime_mut();
        let mut bridge = WebView2IpcBridge::new().unwrap();
        let mut voice = VoiceInteractionController::new_default(avatar_dir.clone());
        let mut ks = UiKillSwitchManager::new_default();

        let temp_dir = create_unique_isolated_audit_temp_dir("miku_test_ks_trip", 1).unwrap();
        let mut audit_store = PersistentJsonlAuditStore::open(&temp_dir, 512 * 1024, 3).unwrap();

        let profile = default_ui_gatekeeper_profile(r"C:\workspace\miku-project");
        let mut gatekeeper = GatekeeperEngine::new(profile).unwrap();

        // Issue 3 in-flight interactive AskPending tickets
        for idx in 0..3 {
            let verdict = gatekeeper.evaluate_request(
                "claude-code",
                Capability::Shell,
                &format!("cargo publish --pkg-{}", idx),
                "--allow-dirty",
                1_000 + idx * 10,
            );
            assert!(matches!(verdict, GatekeeperVerdict::AskPending { .. }));
        }
        assert_eq!(gatekeeper.pending_ticket_count(), 3);

        // Start active voice capture with 3,200 non-zero PCM samples
        voice
            .start_ptt_session(
                DEFAULT_PTT_HOTKEY_COMBO,
                1_500,
                0x0010_00AA,
                0x0010_00AA,
                0x0099_00FF,
                avatar,
            )
            .unwrap();
        let pcm_samples = vec![15_432i16; 3_200];
        let _ = voice
            .ingest_pcm_frame(&pcm_samples, 1_550, avatar, Some(&mut bridge))
            .unwrap();
        assert_eq!(voice.pcm_buffer().len(), 3_200);

        // Trip Global Kill Switch with an XSS payload in reason
        let report = ks
            .trip(
                2_000,
                "Ctrl+Alt+Shift+K <img src=x onerror=alert('ks_xss')>",
                &mut gatekeeper,
                &mut voice,
                avatar,
                &mut audit_store,
                &mut bridge,
            )
            .unwrap();

        let latency_limit = if cfg!(debug_assertions) {
            100.0
        } else {
            MAX_KILL_SWITCH_LATENCY_MS
        };
        assert!(
            report.reaction_latency_ms < latency_limit,
            "Kill Switch trip latency {:.4} ms exceeded {:.1} ms",
            report.reaction_latency_ms,
            latency_limit
        );
        assert_eq!(report.revoked_gatekeeper_tickets, 3);
        assert_eq!(gatekeeper.pending_ticket_count(), 0);
        assert!(gatekeeper.is_kill_switch_active());
        assert!(report.voice_aborted);
        assert_eq!(report.pcm_zero_report.zeroed_samples, 3_200);
        assert_eq!(report.pcm_zero_report.residual_non_zero_samples, 0);
        assert_eq!(report.pcm_zero_report.retained_capacity_bytes, 0);
        assert_eq!(*voice.state(), VoiceInteractionState::Idle);
        assert_eq!(report.avatar_mood, AvatarMood::Alert);
        assert_eq!(avatar.current_mood, AvatarMood::Alert);
        assert_eq!(report.island_state, IslandState::Alert);
        assert_eq!(report.pill_border_color_hex, "#F43F5E");

        if let WebViewIpcPayloadV1::DomSafeTextRender {
            ref safe_text,
            csp_enforced,
            ..
        } = report.ipc_alert_cell
        {
            assert!(csp_enforced);
            assert!(!safe_text.contains("<img"));
            assert!(
                safe_text.contains("&lt;img")
                    || safe_text.contains("&amp;lt;img")
                    || safe_text.contains("KILL SWITCH")
            );
        } else {
            panic!("expected DomSafeTextRender in KillSwitchTripReport");
        }

        let chain_ok = audit_store.verify_persisted_chain().unwrap();
        assert!(chain_ok.verified_entries >= 4);
        let _ = fs::remove_dir_all(&temp_dir);
    }

    // LEDGER-2.9-3
    #[test]
    fn test_kill_switch_enforces_total_lockdown_while_tripped() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).unwrap();
        let avatar = reloader.runtime_mut();
        let mut bridge = WebView2IpcBridge::new().unwrap();
        let mut voice = VoiceInteractionController::new_default(avatar_dir);
        let mut ks = UiKillSwitchManager::new_default();

        let temp_dir = create_unique_isolated_audit_temp_dir("miku_test_ks_lockdown", 2).unwrap();
        let mut audit_store = PersistentJsonlAuditStore::open(&temp_dir, 512 * 1024, 3).unwrap();

        let profile = default_ui_gatekeeper_profile(r"C:\workspace\miku-project");
        let mut gatekeeper = GatekeeperEngine::new(profile).unwrap();

        let _ = ks
            .trip(
                5_000,
                "emergency_lockdown",
                &mut gatekeeper,
                &mut voice,
                avatar,
                &mut audit_store,
                &mut bridge,
            )
            .unwrap();

        // 1. Even an explicitly allowed FsRead rule is denied with KillSwitchActive while tripped
        let verdict = gatekeeper.evaluate_request(
            "claude-code",
            Capability::FsRead,
            r"C:\workspace\miku-project\README.md",
            "read",
            5_100,
        );
        assert!(matches!(
            verdict,
            GatekeeperVerdict::Deny {
                reason: DenyReason::KillSwitchActive,
                ..
            }
        ));

        // 2. UI Kill Switch guard blocks any action and increments counter
        assert!(matches!(
            ks.guard_operation("dispatch_mcp_tool"),
            Err(UiError::KillSwitchError(_))
        ));
        assert!(matches!(
            ks.guard_operation("voice_start"),
            Err(UiError::KillSwitchError(_))
        ));
        assert_eq!(ks.blocked_while_tripped_count(), 2);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    // LEDGER-2.9-4
    #[test]
    fn test_kill_switch_rearm_constant_time_challenge_ttl_and_max_attempts_lockout() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).unwrap();
        let avatar = reloader.runtime_mut();
        let mut bridge = WebView2IpcBridge::new().unwrap();
        let mut voice = VoiceInteractionController::new_default(avatar_dir.clone());
        let mut ks = UiKillSwitchManager::new_default();

        let temp_dir = create_unique_isolated_audit_temp_dir("miku_test_ks_rearm", 3).unwrap();
        let mut audit_store = PersistentJsonlAuditStore::open(&temp_dir, 512 * 1024, 3).unwrap();

        let profile = default_ui_gatekeeper_profile(r"C:\workspace\miku-project");
        let mut gatekeeper = GatekeeperEngine::new(profile).unwrap();

        let _ = ks
            .trip(
                10_000,
                "manual_test_trip",
                &mut gatekeeper,
                &mut voice,
                avatar,
                &mut audit_store,
                &mut bridge,
            )
            .unwrap();

        // 1. Issue challenge #1 and exhaust MAX_REARM_ATTEMPTS (3 wrong tokens) -> locks back to Tripped
        let ch1 = ks.request_rearm(11_000).unwrap();
        assert_eq!(ch1.expires_at_ms, 11_000 + REARM_CHALLENGE_TTL_MS);
        assert_eq!(ch1.max_attempts, MAX_REARM_ATTEMPTS);

        for attempt in 1..=MAX_REARM_ATTEMPTS {
            let err = ks.confirm_rearm(
                "REARM-wrong-token-value",
                11_100 + (attempt as u64) * 10,
                &mut gatekeeper,
                avatar,
                &avatar_dir,
                &mut audit_store,
                &mut bridge,
            );
            assert!(matches!(err, Err(UiError::KillSwitchError(_))));
        }
        assert!(matches!(
            ks.status(),
            KillSwitchStatus::Tripped {
                reason,
                ..
            } if reason == "max_rearm_attempts_exceeded"
        ));

        // 2. Issue challenge #2 and let it expire past REARM_CHALLENGE_TTL_MS (30,000 ms)
        let ch2 = ks.request_rearm(20_000).unwrap();
        let expired_res = ks.confirm_rearm(
            &ch2.challenge_token,
            20_000 + REARM_CHALLENGE_TTL_MS + 1,
            &mut gatekeeper,
            avatar,
            &avatar_dir,
            &mut audit_store,
            &mut bridge,
        );
        assert!(matches!(expired_res, Err(UiError::KillSwitchError(_))));
        assert!(matches!(
            ks.status(),
            KillSwitchStatus::Tripped {
                reason,
                ..
            } if reason == "rearm_challenge_expired"
        ));

        // 3. Issue challenge #3 and confirm with exact token within TTL -> restores Armed & Idle
        let ch3 = ks.request_rearm(60_000).unwrap();
        let status_ok = ks
            .confirm_rearm(
                &ch3.challenge_token,
                60_500,
                &mut gatekeeper,
                avatar,
                &avatar_dir,
                &mut audit_store,
                &mut bridge,
            )
            .unwrap();
        assert_eq!(status_ok, KillSwitchStatus::Armed);
        assert!(!gatekeeper.is_kill_switch_active());
        assert_eq!(avatar.current_mood, AvatarMood::Idle);
        assert_eq!(ks.island_state(), IslandState::Idle);
        assert_eq!(ks.rearms_completed_count(), 1);
        assert_eq!(ks.rejected_rearm_attempts_count(), 4);

        let chain_ok = audit_store.verify_persisted_chain().unwrap();
        assert!(chain_ok.verified_entries >= 4);
        let _ = fs::remove_dir_all(&temp_dir);
    }

