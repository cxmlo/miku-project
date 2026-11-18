#![allow(unused_imports)]
use crate::prelude::*;

// LEDGER-4.15-1
#[test]
fn test_4_15_full_overview_ui_bundle_sub_150kb_zero_cdn_all_panels_and_v7_assets() {
    let avatar_dir = resolve_avatar_assets_dir();
    let catalog = validate_avatar_asset_catalog(&avatar_dir).expect("v7 catalog on disk");
    assert_eq!(catalog.verified_svg_count, EXPECTED_AVATAR_SVG_COUNT);

    let bundle = build_overview_scene_bundle(&avatar_dir).expect("build full v7 overview bundle");
    bundle
        .validate_security_and_budget()
        .expect("overview bundle CSP/XSS/budget check");

    assert!(
        bundle.byte_len < MAX_OVERVIEW_BUNDLE_BYTES,
        "overview bundle {} bytes must be < {}",
        bundle.byte_len,
        MAX_OVERVIEW_BUNDLE_BYTES
    );
    assert!(!bundle.has_external_cdn_links);
    assert!(bundle.uses_only_text_content);
    assert_eq!(bundle.layer_count, 9);
    assert_eq!(bundle.accessory_count, 14);

    let html = &bundle.html_content;
    for marker in [
        "panel-permissions",
        "panel-chat",
        "panel-feed",
        "panel-keys",
        "panel-agents",
        "panel-media",
        "panel-link",
        "miku-kill-switch-btn",
        "miku-vision-thumb-badge",
        "miku-file-dropzone",
        "miku-link-qr",
        "prefers-reduced-motion: reduce",
        "feGaussianBlur",
    ] {
        assert!(html.contains(marker), "missing UI marker '{}'", marker);
    }

    // Must not inline bulky C2PA metadata or leak raw API secrets
    assert!(!html.contains("c2pa:manifest"));
    assert!(!html.contains("sk-proj-"));
    assert!(!html.contains("gsk_"));

    println!(
        "[UI-4.15-BUNDLE] bytes={} max={} svgs={} layers={} acc={}",
        bundle.byte_len,
        MAX_OVERVIEW_BUNDLE_BYTES,
        catalog.verified_svg_count,
        bundle.layer_count,
        bundle.accessory_count
    );
}

// LEDGER-4.15-2
#[test]
fn test_4_15_overview_ipc_dispatches_all_actions_15_moods_and_sanitizes_xss() {
    let avatar_dir = resolve_avatar_assets_dir();
    let mut controller = OverviewSceneController::new(&avatar_dir, false).expect("controller");
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");

    let action_expectations = [
        ("perm_allow", AvatarMood::Happy),
        ("perm_deny", AvatarMood::Annoyed),
        ("ptt_toggle", AvatarMood::Listening),
        ("kill_switch", AvatarMood::Alert),
        ("mood:idle", AvatarMood::Idle),
        ("mood:listening", AvatarMood::Listening),
        ("mood:thinking", AvatarMood::Thinking),
        ("mood:working", AvatarMood::Working),
        ("mood:talking", AvatarMood::Talking),
        ("mood:happy", AvatarMood::Happy),
        ("mood:love", AvatarMood::Love),
        ("mood:focused", AvatarMood::Focused),
        ("mood:sleepy", AvatarMood::Sleepy),
        ("mood:wink", AvatarMood::Wink),
        ("mood:surprised", AvatarMood::Surprised),
        ("mood:annoyed", AvatarMood::Annoyed),
        ("mood:sleeping", AvatarMood::Sleeping),
        ("mood:error", AvatarMood::Error),
        ("mood:alert", AvatarMood::Alert),
    ];

    for (idx, (target_id, expected_mood)) in action_expectations.iter().enumerate() {
        let env = WebViewIpcEnvelopeV1::new(
            format!("ui415-{}", idx),
            None,
            OVERVIEW_VIRTUAL_INDEX_URL,
            WebViewIpcPayloadV1::UiInteraction {
                event: UiInteractionKind::ActionClick,
                target_id: (*target_id).to_string(),
                x: 12,
                y: 12,
                timestamp_ms: 1000 + idx as u64,
            },
        );
        let raw = env.encode_post_message_json().expect("encode");
        let rendered = controller
            .apply_ipc_envelope(&mut bridge, OVERVIEW_VIRTUAL_INDEX_URL, &raw)
            .expect("apply action");
        assert_eq!(controller.current_mood(), *expected_mood);
        if let WebViewIpcPayloadV1::DomSafeTextRender { csp_enforced, .. } = rendered {
            assert!(csp_enforced);
        } else {
            panic!("expected DomSafeTextRender");
        }
    }

    // XSS vector inside UI target_id must be strictly escaped for textContent
    let xss_env = WebViewIpcEnvelopeV1::new(
        "ui415-xss",
        None,
        OVERVIEW_VIRTUAL_INDEX_URL,
        WebViewIpcPayloadV1::UiInteraction {
            event: UiInteractionKind::ActionClick,
            target_id: "<script>alert(1)</script>".to_string(),
            x: 0,
            y: 0,
            timestamp_ms: 9999,
        },
    );
    let xss_raw = xss_env.encode_post_message_json().expect("encode xss");
    let xss_rendered = controller
        .apply_ipc_envelope(&mut bridge, OVERVIEW_VIRTUAL_INDEX_URL, &xss_raw)
        .expect("neutralize xss");
    match xss_rendered {
        WebViewIpcPayloadV1::DomSafeTextRender { safe_text, .. } => {
            assert!(!safe_text.contains("<script>"));
            assert!(safe_text.contains("&lt;script&gt;"));
        }
        other => panic!("unexpected payload {:?}", other),
    }

    // Reduced motion check: 0 fps & 0 spring steps
    controller.set_reduced_motion(true);
    let _ = controller.step_springs_to_rest(1.0 / 60.0, 200);
    assert_eq!(controller.active_fps(), 0);
}

// LEDGER-4.15-3
#[test]
fn test_4_15_all_ui_submodules_strictly_under_600_lines_and_export_preview_html() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/webview");
    for file in [
        "overview_bundle.rs",
        "overview_controller.rs",
        "overview_svg_assets.rs",
        "overview_ui_styles.rs",
        "overview_ui_panels.rs",
        "overview_ui_script.rs",
        "tests_overview_ui_v7.rs",
    ] {
        let count = std::fs::read_to_string(dir.join(file))
            .expect("read webview submodule")
            .lines()
            .count();
        assert!(count < 600, "{} has {} lines (>= 600)", file, count);
    }

    if let Ok(export_path) = std::env::var("MIKU_EXPORT_OVERVIEW_HTML") {
        let avatar_dir = resolve_avatar_assets_dir();
        let bundle = build_overview_scene_bundle(&avatar_dir).expect("build overview bundle");
        let p = std::path::Path::new(&export_path);
        std::fs::write(p, &bundle.html_content).expect("export overview html");
        if let Some(parent) = p.parent() {
            std::fs::write(parent.join("app.js"), &bundle.script_content)
                .expect("export overview app.js");
        }
    }
}
