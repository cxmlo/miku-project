#![allow(unused_imports)]
use crate::prelude::*;

    // LEDGER-2.8-1
    #[test]
    fn test_voice_interaction_fsm_transitions_idle_listening_processing_speaking_and_reactive_pill_wave() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("init avatar reloader");
        let avatar = reloader.runtime_mut();
        let mut bridge = WebView2IpcBridge::new().expect("init IPC bridge");
        let mut ctrl = VoiceInteractionController::new_default(avatar_dir.clone());

        // 1. Initial state is Idle with 0.0 fps pill wave and 0 PCM samples
        assert_eq!(*ctrl.state(), VoiceInteractionState::Idle);
        assert_eq!(ctrl.island_state(), IslandState::Idle);
        assert_eq!(ctrl.soundwave().target_fps, 0.0);
        assert_eq!(ctrl.soundwave().bands, [0.0; SOUNDWAVE_BAND_COUNT]);
        assert!(ctrl.pcm_buffer().is_empty());

        // 2. Start Push-to-Talk -> transitions to Listening & AvatarMood::Listening
        ctrl.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            10_000,
            0x0010_00AA,
            0x0010_00AA,
            0x0099_00FF,
            avatar,
        )
        .expect("start PTT session");
        assert_eq!(*ctrl.state(), VoiceInteractionState::Listening);
        assert_eq!(ctrl.island_state(), IslandState::Listening);
        assert_eq!(avatar.current_mood, AvatarMood::Listening);

        // 3. Ingest active 20 ms 16 kHz PCM speech frame (320 samples) -> 5-band soundwave reacts in < 16 ms
        let speech_pcm: Vec<i16> = (0..320)
            .map(|i| (((i as f64) * 2.0 * std::f64::consts::PI / 32.0).sin() * 16_000.0) as i16)
            .collect();
        let wave = ctrl
            .ingest_pcm_frame(&speech_pcm, 10_020, avatar, Some(&mut bridge))
            .expect("ingest PCM frame");
        assert!(wave.voice_activity_detected);
        assert_eq!(wave.target_fps, 60.0);
        assert!(wave.bands[2] > wave.bands[0]); // Center band is dominant
        assert!(wave.bands.iter().all(|&b| b > 0.0 && b <= 1.0));
        assert!(wave.last_update_latency_ms < MAX_SOUNDWAVE_UPDATE_LATENCY_MS);

        // 4. Release PTT after 400 ms (>= 120 ms) -> transitions to Processing & AvatarMood::Thinking
        let proc_state = ctrl
            .release_ptt_and_begin_processing(10_400, avatar, Some(&mut bridge))
            .expect("release PTT into Processing");
        assert_eq!(proc_state, VoiceInteractionState::Processing);
        assert_eq!(avatar.current_mood, AvatarMood::Thinking);
        assert_eq!(ctrl.soundwave().target_fps, 0.0);

        // 5. Enter Speaking -> drives pill soundwave and AvatarMood::Talking
        let speak_wave = ctrl
            .begin_speaking_response(&speech_pcm, avatar)
            .expect("begin speaking");
        assert_eq!(*ctrl.state(), VoiceInteractionState::Speaking);
        assert_eq!(avatar.current_mood, AvatarMood::Talking);
        assert_eq!(speak_wave.target_fps, 60.0);
        assert!(speak_wave.bands[2] > 0.0);

        // 6. Complete utterance -> returns to Idle, 0.0 fps wave, AvatarMood::Idle
        let completion = ctrl
            .complete_utterance("run cargo clippy", "sess_01", avatar, &mut bridge)
            .expect("complete utterance");
        assert_eq!(*ctrl.state(), VoiceInteractionState::Idle);
        assert_eq!(completion.island_state, IslandState::Idle);
        assert_eq!(completion.avatar_mood, AvatarMood::Idle);
        assert_eq!(ctrl.soundwave().target_fps, 0.0);
        assert_eq!(ctrl.soundwave().bands, [0.0; SOUNDWAVE_BAND_COUNT]);
    }

    // LEDGER-2.8-2
    #[test]
    fn test_ptt_hotkey_and_wake_word_triggers_enforce_zero_focus_steal_and_battery_policy() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).unwrap();
        let avatar = reloader.runtime_mut();
        let mut ctrl = VoiceInteractionController::new_default(avatar_dir);

        // 1. Focus steal (fg_after == pill_hwnd) must be rejected immediately on both PTT and Wake-Word
        assert!(matches!(
            ctrl.start_ptt_session(
                DEFAULT_PTT_HOTKEY_COMBO,
                1_000,
                0x0010_00AA,
                0x0099_00FF,
                0x0099_00FF,
                avatar,
            ),
            Err(UiError::FocusStealDetected { .. })
        ));
        assert!(matches!(
            ctrl.trigger_wake_word(
                DEFAULT_WAKE_WORD_PHRASE,
                0.95,
                1_000,
                0x0010_00AA,
                0x0099_00FF,
                0x0099_00FF,
                avatar,
            ),
            Err(UiError::FocusStealDetected { .. })
        ));

        // 2. Wrong PTT hotkey or low-confidence Wake-Word must be rejected
        assert!(matches!(
            ctrl.start_ptt_session("Ctrl+Alt+Z", 1_000, 0x0010_00AA, 0x0010_00AA, 0x0099_00FF, avatar),
            Err(UiError::VoiceInteractionError(_))
        ));
        assert!(matches!(
            ctrl.trigger_wake_word("Hey Miku", 0.45, 1_000, 0x0010_00AA, 0x0010_00AA, 0x0099_00FF, avatar),
            Err(UiError::VoiceInteractionError(_))
        ));

        // 3. On battery power (or gaming mode), Wake-Word is automatically suspended (BUD-05),
        //    whereas explicit Push-to-Talk remains 100% functional!
        ctrl.set_power_policy(VoicePowerPolicy {
            on_battery_power: true,
            gaming_mode_active: false,
            allow_wake_word_on_battery: false,
        });
        assert!(!ctrl.power_policy().is_wake_word_permitted());
        assert!(matches!(
            ctrl.trigger_wake_word(
                DEFAULT_WAKE_WORD_PHRASE,
                0.98,
                2_000,
                0x0010_00AA,
                0x0010_00AA,
                0x0099_00FF,
                avatar,
            ),
            Err(UiError::VoiceInteractionError(_))
        ));
        assert_eq!(ctrl.wake_word_blocked_by_power_count(), 1);

        // PTT still works on battery power without stealing focus
        ctrl.start_ptt_session(
            "alt+space",
            2_100,
            0x0010_00AA,
            0x0010_00AA,
            0x0099_00FF,
            avatar,
        )
        .expect("PTT works on battery power");
        assert_eq!(*ctrl.state(), VoiceInteractionState::Listening);
    }

    // LEDGER-2.8-3
    #[test]
    fn test_cooperative_voice_cancellation_on_escape_key_ptt_release_or_silence_timeout() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).unwrap();
        let avatar = reloader.runtime_mut();
        let mut bridge = WebView2IpcBridge::new().unwrap();
        let mut ctrl = VoiceInteractionController::new_default(avatar_dir);

        let cancel_limit_ms = if cfg!(debug_assertions) {
            25.0
        } else {
            MAX_VOICE_CANCEL_LATENCY_MS
        };

        // Vector A: User presses Escape during Listening -> cancels in < 5.0 ms
        ctrl.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            5_000,
            0x0010_00AA,
            0x0010_00AA,
            0x0099_00FF,
            avatar,
        )
        .unwrap();
        let receipt_esc = ctrl
            .cancel_cooperatively(
                VoiceCancellationReason::UserPressedEscape,
                5_200,
                avatar,
                Some(&mut bridge),
            )
            .expect("escape cancellation");
        assert_eq!(receipt_esc.reason, VoiceCancellationReason::UserPressedEscape);
        assert_eq!(receipt_esc.previous_state, VoiceInteractionState::Listening);
        assert_eq!(receipt_esc.island_state, IslandState::Idle);
        assert_eq!(receipt_esc.avatar_mood, AvatarMood::Idle);
        assert!(receipt_esc.cancel_latency_ms < cancel_limit_ms);

        // Vector B: User releases PTT after only 45 ms (< MIN_PTT_HOLD_DURATION_MS = 120 ms) -> cancels
        ctrl.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            6_000,
            0x0010_00AA,
            0x0010_00AA,
            0x0099_00FF,
            avatar,
        )
        .unwrap();
        let early_res = ctrl
            .release_ptt_and_begin_processing(6_045, avatar, Some(&mut bridge))
            .expect("early PTT release cancels cooperatively");
        assert_eq!(
            early_res,
            VoiceInteractionState::Cancelled {
                reason: VoiceCancellationReason::PttReleasedTooEarly { held_ms: 45 }
            }
        );
        assert_eq!(*ctrl.state(), VoiceInteractionState::Idle);

        // Vector C: Silence exceeds DEFAULT_VOICE_SILENCE_TIMEOUT_MS (3,500 ms) during Listening
        ctrl.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            10_000,
            0x0010_00AA,
            0x0010_00AA,
            0x0099_00FF,
            avatar,
        )
        .unwrap();
        let silent_pcm = vec![0i16; 320];
        let _ = ctrl
            .ingest_pcm_frame(&silent_pcm, 11_000, avatar, Some(&mut bridge))
            .unwrap();
        assert_eq!(*ctrl.state(), VoiceInteractionState::Listening); // 1,000 ms < 3,500 ms

        let wave_after_timeout = ctrl
            .ingest_pcm_frame(&silent_pcm, 13_600, avatar, Some(&mut bridge))
            .unwrap();
        assert_eq!(*ctrl.state(), VoiceInteractionState::Idle); // 3,600 ms >= 3,500 ms -> auto-cancelled!
        assert_eq!(wave_after_timeout.target_fps, 0.0);
        assert_eq!(ctrl.cancelled_sessions_count(), 3);
    }

    // LEDGER-2.8-4
    #[test]
    fn test_voice_pcm_ring_buffer_zeroes_and_reclaims_memory_on_cancel_or_completion_and_routes_ipc() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).unwrap();
        let avatar = reloader.runtime_mut();
        let mut bridge = WebView2IpcBridge::new().unwrap();
        let mut ctrl = VoiceInteractionController::new_default(avatar_dir);

        // 1. Fill PCM ring buffer with non-zero speech samples
        ctrl.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            20_000,
            0x0010_00AA,
            0x0010_00AA,
            0x0099_00FF,
            avatar,
        )
        .unwrap();
        let speech_samples = vec![12_345i16; 4_800]; // 300 ms @ 16 kHz
        let _ = ctrl
            .ingest_pcm_frame(&speech_samples, 20_300, avatar, Some(&mut bridge))
            .unwrap();
        assert_eq!(ctrl.pcm_buffer().len(), 4_800);
        assert!(ctrl.pcm_buffer().capacity_bytes() >= 9_600);

        // 2. Cancel session -> must overwrite all 4,800 samples with 0i16 and reclaim 100% of capacity
        let cancel_receipt = ctrl
            .cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                20_350,
                avatar,
                Some(&mut bridge),
            )
            .unwrap();
        assert_eq!(cancel_receipt.zero_report.zeroed_samples, 4_800);
        assert_eq!(cancel_receipt.zero_report.residual_non_zero_samples, 0);
        assert_eq!(cancel_receipt.zero_report.retained_capacity_bytes, 0);
        assert_eq!(ctrl.pcm_buffer().len(), 0);
        assert_eq!(ctrl.pcm_buffer().capacity_bytes(), 0);
        let _ = bridge.drain_all_buffers();

        // 3. Complete a Wake-Word session with an XSS payload in the transcript ->
        //    must zero-fill PCM buffer AND sanitize the outbound IPC payloads for textContent
        ctrl.trigger_wake_word(
            DEFAULT_WAKE_WORD_PHRASE,
            0.96,
            30_000,
            0x0010_00AA,
            0x0010_00AA,
            0x0099_00FF,
            avatar,
        )
        .unwrap();
        let _ = ctrl
            .ingest_pcm_frame(&speech_samples, 30_200, avatar, Some(&mut bridge))
            .unwrap();
        let completion = ctrl
            .complete_utterance(
                "run test <img src=x onerror=alert('voice_xss')>",
                "sess_voice_99",
                avatar,
                &mut bridge,
            )
            .unwrap();
        assert_eq!(completion.zero_report.zeroed_samples, 4_800);
        assert_eq!(completion.zero_report.residual_non_zero_samples, 0);
        assert_eq!(completion.zero_report.retained_capacity_bytes, 0);

        let outbound_cmd_json = bridge
            .pop_next_outbound_json()
            .expect("outbound MikuLink Command");
        assert!(!outbound_cmd_json.contains("<img"));
        assert!(outbound_cmd_json.contains("&lt;img"));

        let outbound_dom_json = bridge
            .pop_next_outbound_json()
            .expect("outbound DomSafeTextRender");
        assert!(!outbound_dom_json.contains("<img"));
        assert!(outbound_dom_json.contains("&lt;img"));
    }

