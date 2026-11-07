#![allow(unused_imports)]
use crate::prelude::*;

    // LEDGER-2.10-1
    #[test]
    fn test_idle_suspension_coordinator_transitions_to_deep_idle_after_3000ms_ttl_with_zero_timers_and_zero_fps(
    ) {
        let avatar_dir = resolve_avatar_assets_dir();
        let temp_dir = create_unique_isolated_audit_temp_dir("miku_ui_step210_1", 1).unwrap();
        let mon = MonitorDescriptor {
            device_name: "DISPLAY1".to_string(),
            is_primary: true,
            dpi: STANDARD_DPI,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
        };
        let policy = WebView2SecurityPolicy::release_hardened();
        let driver = IsolatedWebView2ProcessDriver::new(2.51, 62.50);
        let mut host = WebView2HostStateMachine::new(
            mon,
            TOP_MARGIN_DEFAULT_PX,
            Some(&temp_dir),
            policy,
            DEFAULT_IDLE_SUSPENSION_TTL_MS,
            driver,
        )
        .unwrap();

        let mut bridge = WebView2IpcBridge::new().unwrap();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).unwrap();
        let avatar = reloader.runtime_mut();
        let mut voice = VoiceInteractionController::new_default(avatar_dir.clone());
        let mut coord = IdleSuspensionCoordinator::default();
        assert_eq!(
            *coord.power_state(),
            IdlePowerState::DeepIdleSuspended { suspended_at_ms: 0 }
        );

        // Expand WebView2, enqueue an IPC message, and push PCM audio frames
        let exp = host.expand_island(1_000).unwrap();
        assert!(!host.active_child_pids().is_empty());
        assert!(exp.active_ram_mb > 30.0);

        let cell = sanitize_for_text_content("miku-test-cell", "active_payload").unwrap();
        bridge
            .enqueue_outbound(WebViewIpcEnvelopeV1::new(
                "msg-210-1",
                None,
                OVERVIEW_VIRTUAL_INDEX_URL,
                cell,
            ))
            .unwrap();
        assert!(bridge.retained_buffer_bytes() > 0);

        voice
            .start_ptt_session(
                DEFAULT_PTT_HOTKEY_COMBO,
                1_050,
                0x0011_00AA,
                0x0011_00AA,
                0x0099_00FF,
                avatar,
            )
            .unwrap();
        let samples = vec![8_000i16; 320];
        let _ = voice
            .ingest_pcm_frame(&samples, 1_070, avatar, Some(&mut bridge))
            .unwrap();
        assert!(voice.pcm_buffer().capacity_bytes() > 0);

        // Start 3_000 ms cooling TTL at t = 2_000 ms
        let cooling = coord.begin_cooling_ttl(2_000, &mut host).unwrap();
        assert_eq!(
            cooling,
            IdlePowerState::CoolingWarmTtl {
                collapsed_at_ms: 2_000,
                expires_at_ms: 2_000 + DEFAULT_IDLE_SUSPENSION_TTL_MS,
            }
        );

        // Poll at 4_999 ms (1 ms before TTL expiry) -> must remain in CoolingWarmTtl
        let early = coord
            .poll_inactivity_ttl(4_999, &mut host, &mut bridge, &mut voice, avatar, &avatar_dir)
            .unwrap();
        assert!(early.is_none());

        // Poll at 5_000 ms (exact TTL expiry) -> transitions to DeepIdleSuspended
        let receipt = coord
            .poll_inactivity_ttl(5_000, &mut host, &mut bridge, &mut voice, avatar, &avatar_dir)
            .unwrap()
            .expect("must transition to DeepIdleSuspended at 5_000 ms");

        assert_eq!(
            receipt.new_state,
            IdlePowerState::DeepIdleSuspended {
                suspended_at_ms: 5_000
            }
        );
        assert!(receipt.webview2_evicted);
        assert_eq!(receipt.residual_webview2_pids, 0);
        assert_eq!(receipt.active_timers_count, 0);
        assert_eq!(receipt.avatar_settled_fps, 0.0);
        assert_eq!(receipt.voice_wave_fps, 0.0);
        assert_eq!(receipt.retained_ipc_bytes, 0);
        assert_eq!(receipt.retained_pcm_bytes, 0);
        assert!(receipt.wake_word_frozen);
        assert!(receipt.post_suspension_ram_mb <= MAX_IDLE_POST_TTL_RAM_MB);
        assert_eq!(receipt.post_suspension_cpu_pct, 0.0);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    // LEDGER-2.10-2
    #[test]
    fn test_idle_suspension_freezes_wake_word_on_battery_or_fullscreen_gaming_and_defers_events() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).unwrap();
        let avatar = reloader.runtime_mut();
        let mut voice = VoiceInteractionController::new_default(avatar_dir.clone());
        let mut coord = IdleSuspensionCoordinator::default();

        // Activate fullscreen gaming mode + battery power (BUD-05)
        coord.apply_power_and_gaming_policy(true, true, false, &mut voice);
        assert!(coord.is_wake_word_suspended());
        assert!(coord.is_gaming_mode_active());
        assert!(coord.last_gaming_cpu_pct() < MAX_GAMING_CPU_BUDGET_PCT);

        // Wake-word stimulus must be blocked while in battery/gaming mode
        let ww_err = coord.wake_on_stimulus(
            WakeStimulusKind::WakeWordDetected,
            10_000,
            0x0022_00AA,
            0x0022_00AA,
            0x0099_00FF,
            &mut voice,
            avatar,
            &avatar_dir,
        );
        assert!(matches!(ww_err, Err(UiError::IdleSuspensionError(_))));

        // Enqueue 40 non-critical events -> bounded ring retains exactly MAX_DEFERRED_GAMING_EVENTS (32)
        for i in 0..40 {
            let delivered =
                coord.enqueue_event_respecting_gaming_mode(&format!("repo_push_{}", i), false);
            assert!(!delivered);
        }
        assert_eq!(
            coord.deferred_gaming_events().len(),
            MAX_DEFERRED_GAMING_EVENTS
        );
        assert_eq!(coord.deferred_gaming_events()[0], "repo_push_8");
        assert_eq!(coord.deferred_gaming_events()[31], "repo_push_39");

        // Critical security alert bypasses gaming retention immediately
        let crit = coord.enqueue_event_respecting_gaming_mode("gatekeeper_denied_shell", true);
        assert!(crit);
        assert_eq!(coord.delivered_critical_alerts().len(), 1);

        // Exit gaming mode and flush deferred events
        coord.apply_power_and_gaming_policy(false, false, false, &mut voice);
        let flushed = coord.flush_deferred_gaming_events();
        assert_eq!(flushed.len(), MAX_DEFERRED_GAMING_EVENTS);
        assert!(coord.deferred_gaming_events().is_empty());
    }

    // LEDGER-2.10-3
    #[test]
    fn test_idle_suspension_warm_wake_on_hover_or_ptt_restores_60fps_under_16ms_without_focus_steal(
    ) {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).unwrap();
        let avatar = reloader.runtime_mut();
        let mut voice = VoiceInteractionController::new_default(avatar_dir.clone());
        let mut coord = IdleSuspensionCoordinator::default();

        // 1. Valid wake on MouseHoverEnter preserves foreground focus and restores 60 fps in < 16 ms
        let wake_hover = coord
            .wake_on_stimulus(
                WakeStimulusKind::MouseHoverEnter,
                20_000,
                0x0055_00AA,
                0x0055_00AA,
                0x0099_00FF,
                &mut voice,
                avatar,
                &avatar_dir,
            )
            .unwrap();
        assert!(wake_hover.focus_preserved);
        assert!(wake_hover.wake_latency_ms < MAX_WARM_WAKE_LATENCY_MS);
        assert_eq!(wake_hover.island_state, IslandState::Expanded);
        assert_eq!(wake_hover.avatar_mood, AvatarMood::Happy);
        assert_eq!(wake_hover.target_fps, 60.0);

        // 2. Valid wake on PushToTalkHotkey
        let wake_ptt = coord
            .wake_on_stimulus(
                WakeStimulusKind::PushToTalkHotkey,
                21_000,
                0x0055_00AA,
                0x0055_00AA,
                0x0099_00FF,
                &mut voice,
                avatar,
                &avatar_dir,
            )
            .unwrap();
        assert!(wake_ptt.focus_preserved);
        assert!(wake_ptt.wake_latency_ms < MAX_WARM_WAKE_LATENCY_MS);
        assert_eq!(wake_ptt.island_state, IslandState::Listening);
        assert_eq!(wake_ptt.avatar_mood, AvatarMood::Listening);

        // 3. Simulated focus steal (`fg_after == pill_hwnd`) must be rejected with FocusStealDetected
        let steal_err = coord.wake_on_stimulus(
            WakeStimulusKind::MouseHoverEnter,
            22_000,
            0x0055_00AA,
            0x0099_00FF,
            0x0099_00FF,
            &mut voice,
            avatar,
            &avatar_dir,
        );
        assert!(matches!(steal_err, Err(UiError::FocusStealDetected { .. })));
    }

    // LEDGER-2.10-4
    #[test]
    fn test_ten_cycle_expand_collapse_deep_idle_stability_zero_ram_drift_and_budget_matrix() {
        let avatar_dir = resolve_avatar_assets_dir();
        let temp_dir = create_unique_isolated_audit_temp_dir("miku_ui_step210_4", 4).unwrap();
        let mon = MonitorDescriptor {
            device_name: "DISPLAY1".to_string(),
            is_primary: true,
            dpi: STANDARD_DPI,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
        };

        let report = run_idle_suspension_bank05_audit(
            mon,
            Some(&temp_dir),
            &avatar_dir,
            TEN_CYCLE_STABILITY_REPS,
        )
        .unwrap();

        assert_eq!(report.cycles_completed, TEN_CYCLE_STABILITY_REPS);
        assert_eq!(
            report.deep_suspensions_completed,
            TEN_CYCLE_STABILITY_REPS as u64
        );
        assert_eq!(report.warm_wakes_completed, TEN_CYCLE_STABILITY_REPS as u64);
        assert_eq!(report.gaming_events_deferred, 4);
        assert_eq!(report.gaming_critical_alerts_delivered, 1);
        assert_eq!(report.residual_webview2_pids, 0);
        assert_eq!(report.residual_idle_timers, 0);
        assert_eq!(report.residual_ipc_bytes, 0);
        assert_eq!(report.residual_pcm_bytes, 0);
        assert_eq!(report.idle_settled_fps, 0.0);
        assert_eq!(report.idle_cpu_pct, 0.0);
        assert!(report.gaming_cpu_pct < MAX_GAMING_CPU_BUDGET_PCT);
        assert!(report.max_post_ttl_ram_mb <= MAX_IDLE_POST_TTL_RAM_MB);
        assert!(report.max_expanded_ram_mb <= MAX_EXPANDED_WEBVIEW2_RAM_MB);
        assert!(report.focus_preserved_and_zero_drift);
        assert!(report.certification_matrix.all_budgets_certified);
        assert!(report.certification_matrix.bud_01_idle_ram.passed);
        assert!(report.certification_matrix.bud_02_expand_latency.passed);
        assert!(report.certification_matrix.bud_04_expanded_ram.passed);
        assert!(report.certification_matrix.bud_05_gaming_cpu.passed);
        assert!(report.certification_matrix.bud_09_binary_size.passed);

        let _ = fs::remove_dir_all(&temp_dir);
    }

