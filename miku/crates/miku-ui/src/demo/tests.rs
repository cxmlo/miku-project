#![allow(unused_imports)]
use crate::prelude::*;

    // LEDGER-2.11-1
    #[test]
    fn test_demo_cli_args_parser_supports_demo_subcommand_flag_and_bounded_duration() {
        let args_sub = vec![
            "miku-cli".to_string(),
            "demo".to_string(),
            "--duration-ms".to_string(),
            "5000".to_string(),
            "--json".to_string(),
        ];
        let parsed_sub = parse_demo_cli_args(&args_sub).unwrap();
        assert!(parsed_sub.enabled);
        assert_eq!(parsed_sub.duration_ms, Some(5000));
        assert!(!parsed_sub.auto_script);
        assert!(parsed_sub.json_output);

        let args_flag = vec![
            "miku.exe".to_string(),
            "--demo".to_string(),
            "--auto-script".to_string(),
            "--duration-ms=1200".to_string(),
        ];
        let parsed_flag = parse_demo_cli_args(&args_flag).unwrap();
        assert!(parsed_flag.enabled);
        assert!(parsed_flag.auto_script);
        assert_eq!(parsed_flag.duration_ms, Some(1200));

        // Zero duration must be rejected
        let bad_zero = vec![
            "miku".to_string(),
            "--demo".to_string(),
            "--duration-ms".to_string(),
            "0".to_string(),
        ];
        assert!(matches!(
            parse_demo_cli_args(&bad_zero),
            Err(UiError::DemoModeError(_))
        ));

        // Missing value for --duration-ms must be rejected
        let bad_missing = vec![
            "miku".to_string(),
            "--demo".to_string(),
            "--duration-ms".to_string(),
        ];
        assert!(matches!(
            parse_demo_cli_args(&bad_missing),
            Err(UiError::DemoModeError(_))
        ));
    }

    // LEDGER-2.11-2
    #[test]
    fn test_demo_sandbox_guard_enforces_zero_disk_writes_zero_api_keys_and_demo_badge_prefix() {
        let mut guard = DemoSandboxGuard::default();
        assert!(guard.in_memory_only);
        assert!(!guard.allow_disk_writes);
        assert!(!guard.allow_api_keys);

        let labeled_a = guard.format_demo_labeled_text("Claude Code requests fs_write");
        assert_eq!(labeled_a, "[DEMO] Claude Code requests fs_write");

        let labeled_b = guard.format_demo_labeled_text("[DEMO] Already prefixed card");
        assert_eq!(labeled_b, "[DEMO] Already prefixed card");
        assert_eq!(guard.labeled_cards_emitted, 2);

        // Any attempt to write to disk or read API keys in demo mode must be blocked
        assert!(matches!(
            guard.attempt_disk_write("C:/Users/Default/AppData/Local/miku/audit/audit.jsonl"),
            Err(UiError::DemoModeError(_))
        ));
        assert!(matches!(
            guard.attempt_api_key_access("anthropic-main"),
            Err(UiError::DemoModeError(_))
        ));

        assert_eq!(guard.disk_files_mutated, 0);
        assert_eq!(guard.api_keys_touched, 0);
        guard.verify_invariants().unwrap();
    }

    // LEDGER-2.11-3
    #[test]
    fn test_demo_interactive_session_steps_hover_click_ptt_and_kill_switch_with_6_spring_physics() {
        let mut ctrl = DemoInteractiveController::default_with_resolved_assets().unwrap();
        assert_eq!(*ctrl.phase(), DemoInteractionPhase::IdleCompact);
        assert!(ctrl.last_demo_banner().starts_with(DEMO_BADGE_PREFIX));

        let geom = ctrl.pill_geometry();
        assert_eq!(geom.width, IDLE_COMPACT_WIDTH_PX as i32);
        assert_eq!(geom.height, IDLE_COMPACT_HEIGHT_PX as i32);

        // 1. Hover enter wakes 6-spring physics and expands to HoverPeek
        let p_hover = ctrl
            .step_hover_enter(geom.center_x(), geom.y + 16, 1_000)
            .unwrap();
        assert!(matches!(
            p_hover,
            DemoInteractionPhase::HoverPeekExpanded { .. }
        ));
        assert_eq!(ctrl.avatar_runtime().current_mood, AvatarMood::Happy);
        assert!(ctrl.last_demo_banner().starts_with(DEMO_BADGE_PREFIX));

        // 2. Click opens [DEMO] Gatekeeper Y/N permission modal in-memory
        let p_modal = ctrl.step_click_permission_modal(1_100).unwrap();
        match p_modal {
            DemoInteractionPhase::PermissionModalActive { prompt_text, .. } => {
                assert!(prompt_text.starts_with(DEMO_BADGE_PREFIX));
                assert!(prompt_text.contains("fs_write"));
            }
            other => panic!("expected PermissionModalActive, got {:?}", other),
        }
        let _ = ctrl.step_approve_or_deny_modal(true, 1_150).unwrap();
        assert_eq!(ctrl.avatar_runtime().current_mood, AvatarMood::Working);

        // 3. Alt+Space triggers PTT voice listening with 5-band soundwave
        let p_ptt = ctrl.step_ptt_hotkey(1_200).unwrap();
        match p_ptt {
            DemoInteractionPhase::PttVoiceListening { band_levels } => {
                assert!(band_levels.iter().any(|&b| b > 0.05));
            }
            other => panic!("expected PttVoiceListening, got {:?}", other),
        }
        assert_eq!(ctrl.avatar_runtime().current_mood, AvatarMood::Listening);

        // 4. Ctrl+Alt+Shift+K trips Global Kill Switch (#F43F5E) and revokes tickets
        let p_ks = ctrl.step_kill_switch_hotkey(1_300).unwrap();
        match p_ks {
            DemoInteractionPhase::KillSwitchTrippedAlert { reason, color_hex } => {
                assert!(reason.starts_with(DEMO_BADGE_PREFIX));
                assert_eq!(color_hex, KILL_SWITCH_ALERT_COLOR_HEX);
            }
            other => panic!("expected KillSwitchTrippedAlert, got {:?}", other),
        }
        assert_eq!(ctrl.avatar_runtime().current_mood, AvatarMood::Alert);
        ctrl.sandbox().verify_invariants().unwrap();
    }

    // LEDGER-2.11-4
    #[test]
    fn test_demo_cooperative_escape_key_and_close_releases_hotkeys_window_and_zeroes_buffers() {
        let mut ctrl = DemoInteractiveController::default_with_resolved_assets().unwrap();
        let geom = ctrl.pill_geometry();
        let _ = ctrl
            .step_hover_enter(geom.center_x(), geom.y + 16, 2_000)
            .unwrap();
        let _ = ctrl.step_ptt_hotkey(2_100).unwrap();

        let p_exit = ctrl.step_escape_clean_exit(2_200).unwrap();
        assert_eq!(p_exit, DemoInteractionPhase::CleanExited);

        let report = ctrl.build_session_report(4.2, true).unwrap();
        assert!(report.escaped_cleanly);
        assert!(report.focus_preserved);
        assert!(report.hotkeys_unregistered);
        assert!(report.window_destroyed);
        assert_eq!(report.disk_files_mutated, 0);
        assert_eq!(report.api_keys_touched, 0);
        assert_eq!(report.residual_pcm_bytes, 0);
        assert_eq!(report.residual_ipc_bytes, 0);
        assert_eq!(report.settled_idle_fps, 0.0);
        assert!(report.last_demo_banner.starts_with(DEMO_BADGE_PREFIX));
    }

