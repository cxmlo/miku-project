#![allow(unused_imports)]
use crate::prelude::*;

    // LEDGER-2.1-1
    #[test]
    fn test_multi_monitor_top_center_anchor_coordinates_across_dpi_and_negative_bounds() {
        // 1. Primary monitor 1920x1080 @ 100% DPI (96)
        let mon_100 = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY1".to_string(),
            is_primary: true,
            dpi: 96,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
        };
        let anc_100 = compute_top_center_pill_anchor(
            &mon_100,
            IDLE_COMPACT_WIDTH_PX,
            IDLE_COMPACT_HEIGHT_PX,
            IDLE_CORNER_RADIUS_PX,
            0,
        )
        .expect("100% DPI anchor");
        assert_eq!(anc_100.width, 248);
        assert_eq!(anc_100.height, 44);
        assert_eq!(anc_100.x, (1920 - 248) / 2); // 836
        assert_eq!(anc_100.y, 0);
        assert_eq!(anc_100.scale_pct, 100);

        // 2. Laptop display 1920x1080 @ 125% DPI (120)
        let mon_125 = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY1".to_string(),
            is_primary: true,
            dpi: 120,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1020).unwrap(),
        };
        let anc_125 = compute_top_center_pill_anchor(
            &mon_125,
            IDLE_COMPACT_WIDTH_PX,
            IDLE_COMPACT_HEIGHT_PX,
            IDLE_CORNER_RADIUS_PX,
            0,
        )
        .expect("125% DPI anchor");
        assert_eq!(anc_125.width, 310); // 248 * 1.25
        assert_eq!(anc_125.height, 55); // 44 * 1.25
        assert_eq!(anc_125.x, (1920 - 310) / 2); // 805
        assert_eq!(anc_125.scale_pct, 125);

        // 3. Secondary monitor positioned to the LEFT and ABOVE primary with negative coordinates
        //    e.g., [-2560, -140]..[0, 1300] @ 150% DPI (144)
        let mon_neg_150 = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY2".to_string(),
            is_primary: false,
            dpi: 144,
            monitor_rect: VirtualRect::new(-2560, -140, 0, 1300).unwrap(),
            work_area: VirtualRect::new(-2560, -140, 0, 1240).unwrap(),
        };
        let anc_neg = compute_top_center_pill_anchor(
            &mon_neg_150,
            IDLE_COMPACT_WIDTH_PX,
            IDLE_COMPACT_HEIGHT_PX,
            IDLE_CORNER_RADIUS_PX,
            8,
        )
        .expect("negative secondary monitor anchor");
        assert_eq!(anc_neg.width, 372); // 248 * 1.5
        assert_eq!(anc_neg.height, 66); // 44 * 1.5
        // Center X inside [-2560, 0]: -2560 + (2560 - 372) / 2 = -2560 + 1094 = -1466
        assert_eq!(anc_neg.x, -1466);
        // Top Y with 8px logical margin @ 150% (12px): -140 + 12 = -128
        assert_eq!(anc_neg.y, -128);
        assert_eq!(anc_neg.scale_pct, 150);

        // 4. Multi-monitor selector picks negative monitor when cursor is at (-500, 100)
        let monitors = vec![mon_100.clone(), mon_neg_150.clone()];
        let picked = select_target_monitor(&monitors, Some((-500, 100))).unwrap();
        assert_eq!(picked.device_name, "\\\\.\\DISPLAY2");
    }

    // LEDGER-2.1-2
    #[test]
    fn test_win32_layered_topmost_toolwindow_noactivate_style_flags_and_asset_catalog() {
        let cfg = Win32PillStyleConfig::default();
        cfg.validate().expect("default Win32 pill flags must be valid");
        assert_eq!(cfg.base_style, 0x9000_0000);
        assert_eq!(cfg.ex_style, 0x0808_0088);
        assert_eq!(cfg.show_cmd, SW_SHOWNOACTIVATE_CMD);

        // Removing WS_EX_NOACTIVATE must fail validation
        let bad_noactivate = Win32PillStyleConfig {
            ex_style: cfg.ex_style & !WS_EX_NOACTIVATE_STYLE,
            ..cfg
        };
        assert!(bad_noactivate.validate().is_err());

        // Adding WS_EX_APPWINDOW must fail validation
        let bad_appwindow = Win32PillStyleConfig {
            ex_style: cfg.ex_style | WS_EX_APPWINDOW_FORBIDDEN,
            ..cfg
        };
        assert!(bad_appwindow.validate().is_err());

        // Validate the mounted avatar catalog (rig.toml v7, design-tokens.json, 22 SVGs)
        let avatar_dir = resolve_avatar_assets_dir();
        let report =
            validate_avatar_asset_catalog(&avatar_dir).expect("mounted avatar catalog valid");
        assert_eq!(report.rig_version, 1);
        assert_eq!(report.viewbox, [128, 128]);
        assert_eq!(report.idle_fps, 0);
        assert_eq!(report.layer_count, 9);
        assert_eq!(report.accessory_item_count, 14);
        assert_eq!(report.verified_svg_count, EXPECTED_AVATAR_SVG_COUNT);
        assert_eq!(report.compact_width_px, 248);
        assert_eq!(report.compact_height_px, 44);
        assert_eq!(report.catalog_sha256.len(), 64);
    }

    // LEDGER-2.1-3
    #[test]
    fn test_zero_focus_stealing_foreground_window_invariant_and_ma_noactivate() {
        assert_eq!(
            handle_wm_mouseactivate(WM_MOUSEACTIVATE_MSG),
            Some(MA_NOACTIVATE_RESULT)
        );
        assert_eq!(handle_wm_mouseactivate(0x0001), None);

        // Focus preserved when fg_before == fg_after != pill_hwnd
        assert!(verify_foreground_focus_invariant(
            0x0012_00AA,
            0x0012_00AA,
            0x0099_00FF,
            MA_NOACTIVATE_RESULT
        )
        .unwrap());

        // Headless CI fallback when fg_before == 0 and fg_after == 0
        assert!(verify_foreground_focus_invariant(0, 0, 0x0099_00FF, MA_NOACTIVATE_RESULT).unwrap());

        // Stealing focus (fg_after == pill_hwnd) must fail immediately
        assert!(matches!(
            verify_foreground_focus_invariant(
                0x0012_00AA,
                0x0099_00FF,
                0x0099_00FF,
                MA_NOACTIVATE_RESULT
            ),
            Err(UiError::FocusStealDetected { .. })
        ));

        // Real Win32 verification cycle on Windows host
        let avatar_dir = resolve_avatar_assets_dir();
        let cycle =
            run_native_pill_verification_cycle(1, &avatar_dir).expect("native Win32 pill cycle");
        assert!(cycle.ex_style_verified);
        assert!(cycle.ma_noactivate_verified);
        assert!(cycle.focus_preserved);
        assert_eq!(cycle.webview2_instances, 0);
    }

    // LEDGER-2.1-4
    #[test]
    fn test_idle_to_hover_dimension_transition_and_hit_test_hysteresis_under_16ms() {
        let monitor = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY1".to_string(),
            is_primary: true,
            dpi: 96,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
        };
        let mut fsm = PillHoverStateMachine::new(monitor, 0).expect("init hover FSM");
        assert_eq!(fsm.mode(), PillVisualMode::IdleCompact);
        assert_eq!(fsm.geometry().width, 248);
        assert_eq!(fsm.geometry().height, 44);
        assert_eq!(fsm.active_timers(), 0);

        // 1. Move cursor into the pill center -> transitions to HoverPeek (284x52, k=0.571)
        let enter = fsm.on_cursor_move(960, 20).expect("enter hover");
        assert!(enter.transitioned);
        assert_eq!(enter.new_mode, PillVisualMode::HoverPeek);
        assert_eq!(enter.geometry.width, 284);
        assert_eq!(enter.geometry.height, 52);
        assert!((enter.avatar_scale_k - HOVER_AVATAR_SCALE_K).abs() < 1e-6);
        assert!((enter.avatar_body_px - HOVER_AVATAR_BODY_PX).abs() < 1e-6);
        assert!(enter.elapsed_ms < MAX_HOVER_RESPONSE_MS);

        // 2. Move cursor 4px below the HoverPeek bottom edge (y = 52 + 4 = 56):
        //    Because hysteresis is 6px, it MUST remain in HoverPeek without flickering!
        let edge = fsm.on_cursor_move(960, 56).expect("within hysteresis margin");
        assert!(!edge.transitioned);
        assert_eq!(edge.new_mode, PillVisualMode::HoverPeek);

        // 3. Move cursor outside hysteresis margin (y = 70) -> settles back to IdleCompact
        let leave = fsm.on_cursor_move(960, 70).expect("leave hover");
        assert!(leave.transitioned);
        assert_eq!(leave.new_mode, PillVisualMode::IdleCompact);
        assert_eq!(leave.geometry.width, 248);
        assert_eq!(leave.geometry.height, 44);
        assert_eq!(leave.active_timers, 0);
        assert_eq!(leave.webview2_instances, 0);
        assert!(leave.elapsed_ms < MAX_HOVER_RESPONSE_MS);
    }

