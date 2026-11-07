#![allow(unused_imports)]
use crate::prelude::*;

    // LEDGER-2.4-1
    #[test]
    fn test_overview_scene_virtual_host_bundle_under_150kb_csp_and_design_tokens() {
        let avatar_dir = resolve_avatar_assets_dir();
        let bundle = build_overview_scene_bundle(&avatar_dir).expect("build overview scene bundle");
        bundle
            .validate_security_and_budget()
            .expect("validate overview bundle security & budget");

        assert_eq!(bundle.virtual_url, OVERVIEW_VIRTUAL_INDEX_URL);
        assert!(bundle.byte_len > 512);
        assert!(
            bundle.byte_len < MAX_OVERVIEW_BUNDLE_BYTES,
            "bundle size {} bytes must be < 150 KB ({})",
            bundle.byte_len,
            MAX_OVERVIEW_BUNDLE_BYTES
        );
        assert_eq!(bundle.expanded_width_px, OVERVIEW_EXPANDED_WIDTH_PX);
        assert_eq!(bundle.expanded_height_px, OVERVIEW_EXPANDED_HEIGHT_PX);
        assert_eq!(bundle.corner_radius_px, OVERVIEW_CORNER_RADIUS_PX);
        assert!((bundle.avatar_scale_k - OVERVIEW_AVATAR_SCALE_K).abs() < 1e-6);
        assert!((bundle.avatar_body_px - OVERVIEW_AVATAR_BODY_PX).abs() < 1e-6);
        assert_eq!(bundle.viewbox, AVATAR_VIEWBOX_SIZE);
        assert_eq!(bundle.layer_count, 9);
        assert_eq!(bundle.accessory_count, 14);
        assert!(bundle.uses_only_text_content);
        assert!(!bundle.has_external_cdn_links);
        assert!(bundle.html_content.contains(MIKU_WEBVIEW_CSP_HEADER));
        assert!(bundle.html_content.contains("textContent"));
        assert!(!bundle.html_content.to_ascii_lowercase().contains("innerhtml"));
    }

    // LEDGER-2.4-2
    #[test]
    fn test_avatar_layer_stack_and_accessory_slot_anchor_transforms_in_128x128_viewbox() {
        let avatar_dir = resolve_avatar_assets_dir();
        let (layers, transforms) = compose_avatar_layer_stack(
            &avatar_dir,
            AvatarMood::Annoyed,
            &["crown", "witch", "bunny", "shades", "scarf"],
        )
        .expect("compose 9 layers and 14 accessory transforms");

        // 1. Verify strict Z-index ordering of the 9 layers
        let z_order: Vec<u32> = layers.iter().map(|l| l.z).collect();
        assert_eq!(z_order, vec![10, 20, 30, 40, 50, 55, 60, 70, 100]);

        // In Annoyed mood, both blush (z=40) and brows (z=55) must be visible
        let blush_node = layers.iter().find(|l| l.id == "blush").unwrap();
        let brows_node = layers.iter().find(|l| l.id == "brows").unwrap();
        assert!(blush_node.visible);
        assert!(brows_node.visible);

        // 2. Verify all 14 accessories resolve exact slot anchors and origin_in_avatar = [0, -shift_y]
        assert_eq!(transforms.len(), 14);
        let crown = transforms.get("crown").unwrap();
        assert_eq!(crown.slot, "head");
        assert_eq!(crown.slot_anchor, AVATAR_HEAD_TOP_ANCHOR);
        assert!(crown.mounts_back_layer_z10 && crown.mounts_front_layer_z70);
        assert_eq!(
            crown.back_svg.as_deref(),
            Some("avatar/accessories/crown_back.svg")
        );

        let bunny = transforms.get("bunny").unwrap();
        assert_eq!(bunny.slot, "head");
        assert!(bunny.mounts_back_layer_z10 && !bunny.mounts_front_layer_z70);
        assert_eq!(bunny.origin_in_avatar, [0.0, -14.0]);

        let shades = transforms.get("shades").unwrap();
        assert_eq!(shades.slot, "face");
        assert_eq!(shades.slot_anchor, AVATAR_EYES_CENTER_ANCHOR);
        assert!(!shades.follow_spring && shades.parallax_look);

        let scarf = transforms.get("scarf").unwrap();
        assert_eq!(scarf.slot, "neck");
        assert_eq!(scarf.slot_anchor, AVATAR_NECK_ANCHOR);
        assert!(scarf.follow_spring && !scarf.parallax_look);

        // 3. Verify z=10 (accessory_back) mounted crown, witch, and bunny (3 back layers)
        let back_layer = layers.iter().find(|l| l.id == "accessory_back").unwrap();
        assert_eq!(back_layer.mounted_accessories.len(), 3);
        // Verify z=70 (accessory_front) mounted crown, witch, shades, and scarf (4 front layers)
        let front_layer = layers.iter().find(|l| l.id == "accessory_front").unwrap();
        assert_eq!(front_layer.mounted_accessories.len(), 4);
    }

    // LEDGER-2.4-3
    #[test]
    fn test_reactive_overview_telemetry_and_interaction_updates_bounded_without_oom_or_xss() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut controller =
            OverviewSceneController::new(&avatar_dir, false).expect("init overview controller");
        let mut bridge = WebView2IpcBridge::new().expect("init IPC bridge");

        // Send 200 reactive events (>> MAX_OVERVIEW_EVENT_FEED = 32) containing XSS vectors
        for i in 0..200usize {
            let payload = if i % 3 == 0 {
                WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::AgentEvent {
                    agent: "claude_code".to_string(),
                    kind: "thinking".to_string(),
                    title: format!("<img src=x onerror=alert({})> building crate", i),
                })
            } else if i % 3 == 1 {
                WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::PermissionRequest {
                    req_hash: format!("sha256:{}", i),
                    agent: "cursor".to_string(),
                    capability: "shell_exec".to_string(),
                    target: "<script>steal()</script>".to_string(),
                    ttl_sec: 30,
                    pc_only: true,
                })
            } else {
                WebViewIpcPayloadV1::TelemetrySnapshot {
                    island_state: IslandState::Listening,
                    webview_state: "ActiveExpanded".to_string(),
                    ram_private_mb_x100: 378,
                    rtt_us: 14,
                    active_agents: 2,
                }
            };
            let env = WebViewIpcEnvelopeV1::new(
                format!("burst-2-4-{}", i),
                None,
                OVERVIEW_VIRTUAL_INDEX_URL,
                payload,
            );
            let raw = env.encode_post_message_json().unwrap();
            let safe_dom = controller
                .apply_ipc_envelope(&mut bridge, OVERVIEW_VIRTUAL_INDEX_URL, &raw)
                .expect("apply reactive IPC envelope");

            match safe_dom {
                WebViewIpcPayloadV1::DomSafeTextRender {
                    safe_text,
                    csp_enforced,
                    ..
                } => {
                    assert!(csp_enforced);
                    assert!(!safe_text.contains("<script"));
                    assert!(!safe_text.contains("<img"));
                }
                other => panic!("expected DomSafeTextRender, got {:?}", other),
            }
        }

        // Circular feed must never exceed MAX_OVERVIEW_EVENT_FEED (32) -> 0 OOM
        assert_eq!(controller.dom_updates_count(), 200);
        assert_eq!(controller.event_feed_len(), MAX_OVERVIEW_EVENT_FEED);
    }

    // LEDGER-2.4-4
    #[test]
    fn test_fluid_expand_transition_hover_to_overview_under_150ms_and_reduced_motion() {
        let avatar_dir = resolve_avatar_assets_dir();
        let monitor = MonitorDescriptor {
            device_name: "\\\\.\\DISPLAY1".to_string(),
            is_primary: true,
            dpi: 96,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
        };
        let mut hover_fsm = PillHoverStateMachine::new(monitor.clone(), 0).unwrap();
        let policy = WebView2SecurityPolicy::release_hardened();
        let driver = IsolatedWebView2ProcessDriver::new(2.51, 62.5);
        let mut wv2_host = WebView2HostStateMachine::new(
            monitor,
            0,
            Some(Path::new(r"C:\Users\cmiranda\AppData\Local")),
            policy,
            15_000,
            driver,
        )
        .unwrap();

        // 1. Standard motion: transitions in < 150.0 ms, settles springs to 0 fps
        let mut controller = OverviewSceneController::new(&avatar_dir, false).unwrap();
        let m1 = controller
            .transition_hover_to_overview(&mut hover_fsm, &mut wv2_host, 1_000)
            .expect("standard overview transition");
        assert_eq!(m1.geometry.width, OVERVIEW_EXPANDED_WIDTH_PX as i32);
        assert_eq!(m1.geometry.height, OVERVIEW_EXPANDED_HEIGHT_PX as i32);
        assert!((m1.avatar_scale_k - OVERVIEW_AVATAR_SCALE_K).abs() < 1e-6);
        assert!(m1.transition_latency_ms <= MAX_EXPAND_VISIBLE_LATENCY_MS);
        assert!(m1.expanded_ram_mb <= MAX_EXPANDED_WEBVIEW2_RAM_MB);
        assert_eq!(m1.duration_curve_ms, 550);
        assert!(m1.settled_steps > 0);
        assert_eq!(m1.settled_fps, 0);

        // 2. Windows Reduced Motion enabled: 0 ms duration curve and 0 spring steps
        controller.set_reduced_motion(true);
        let m2 = controller
            .transition_hover_to_overview(&mut hover_fsm, &mut wv2_host, 2_000)
            .expect("reduced motion overview transition");
        assert_eq!(m2.duration_curve_ms, 0);
        assert_eq!(m2.settled_steps, 0);
        assert_eq!(m2.settled_fps, 0);
    }

