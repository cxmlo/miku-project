use std::path::Path;

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, PolicyRule, ProjectProfile};
use miku_protocol::PermissionDecisionKind;

use super::*;
use crate::vision::types_and_codec::{
    DEFAULT_VISION_MAX_DIMENSION_PX, DEFAULT_VISION_MAX_OUTPUT_BYTES,
    DEFAULT_VISION_PREVIEW_MAX_BYTES, PNG_MAGIC_BYTES,
};
use crate::webview::{WebView2IpcBridge, WebViewIpcPayloadV1};
use crate::win32::UiError;

fn make_profile(rules: Vec<PolicyRule>) -> ProjectProfile {
    let mut profile = ProjectProfile::new("miku-vision-test", env!("CARGO_MANIFEST_DIR"), 30_000);
    for r in rules {
        profile = profile.with_rule(r.capability, &r.pattern, r.effect, r.pc_only);
    }
    profile
}

fn new_bridge() -> WebView2IpcBridge {
    WebView2IpcBridge::new().expect("bridge init")
}

// LEDGER-4.3-1
#[test]
fn test_vision_blocked_by_default_deny_ask_kill_switch_toctou_and_mobile_pc_only() {
    // 1. Default deny (no rules) and explicit deny never invoke capture backend
    for profile in [
        make_profile(vec![]),
        make_profile(vec![PolicyRule {
            capability: Capability::Vision,
            pattern: "*".to_string(),
            effect: PolicyEffect::Deny,
            pc_only: true,
        }]),
    ] {
        let mut gk = GatekeeperEngine::new(profile).expect("gatekeeper init");
        let mut bridge = new_bridge();
        let backend = DeterministicFixtureVisionBackend::new(640, 360, "VS Code");
        let mut ctrl =
            ScreenVisionController::new(backend, VisionCapturePolicy::default()).expect("ctrl");

        let outcome = ctrl
            .request_on_demand_capture(
                "claude-code",
                VisionCaptureTarget::ActiveWindow,
                1000,
                &mut gk,
                &mut bridge,
            )
            .expect("denied outcome");

        assert!(matches!(outcome, VisionCaptureOutcome::Denied { .. }));
        assert_eq!(ctrl.backend().capture_count(), 0);
        assert_eq!(ctrl.completed_captures(), 0);
    }

    // 2. Ask rule pauses with 0 captures; tampered req_hash (TOCTOU) and mobile pc_only are rejected
    let ask_profile = make_profile(vec![PolicyRule {
        capability: Capability::Vision,
        pattern: "*".to_string(),
        effect: PolicyEffect::Ask,
        pc_only: true,
    }]);
    let mut gk = GatekeeperEngine::new(ask_profile).expect("gatekeeper ask");
    let mut bridge = new_bridge();
    let backend = DeterministicFixtureVisionBackend::new(800, 600, "Terminal");
    let mut ctrl =
        ScreenVisionController::new(backend, VisionCapturePolicy::default()).expect("ctrl");

    let pending = ctrl
        .request_on_demand_capture(
            "cursor",
            VisionCaptureTarget::ActiveWindow,
            2000,
            &mut gk,
            &mut bridge,
        )
        .expect("ask outcome");

    let ticket = match pending {
        VisionCaptureOutcome::AwaitingApproval { ticket, .. } => ticket,
        other => panic!("expected AwaitingApproval, got {:?}", other),
    };
    assert_eq!(ctrl.backend().capture_count(), 0);

    // Tampered req_hash -> TOCTOU denial, 0 captures
    let toctou = ctrl
        .resolve_pending_capture(
            &ticket.req_hash,
            PermissionDecisionKind::Allow,
            "deadbeef_tampered_req_hash",
            false,
            true,
            2100,
            &mut gk,
            &mut bridge,
        )
        .expect("toctou outcome");
    assert!(
        matches!(toctou, VisionCaptureOutcome::Denied { ref reason_code, .. } if reason_code == "deny_toctou")
    );
    assert_eq!(ctrl.backend().capture_count(), 0);

    // Re-issue ticket and try approving from mobile when pc_only == true
    let pending2 = ctrl
        .request_on_demand_capture(
            "cursor",
            VisionCaptureTarget::ActiveWindow,
            2200,
            &mut gk,
            &mut bridge,
        )
        .expect("ask 2");
    let ticket2 = match pending2 {
        VisionCaptureOutcome::AwaitingApproval { ticket, .. } => ticket,
        other => panic!("expected AwaitingApproval, got {:?}", other),
    };
    let mobile_deny = ctrl
        .resolve_pending_capture(
            &ticket2.req_hash,
            PermissionDecisionKind::Allow,
            &ticket2.req_hash,
            true,
            true,
            2250,
            &mut gk,
            &mut bridge,
        )
        .expect("mobile pc_only deny");
    assert!(
        matches!(mobile_deny, VisionCaptureOutcome::Denied { ref reason_code, .. } if reason_code == "deny_pc_only_mobile")
    );
    assert_eq!(ctrl.backend().capture_count(), 0);

    // 3. Even with Allow rule, active Kill Switch immediately blocks all captures
    let allow_profile = make_profile(vec![PolicyRule {
        capability: Capability::Vision,
        pattern: "*".to_string(),
        effect: PolicyEffect::Allow,
        pc_only: true,
    }]);
    let mut gk_allow = GatekeeperEngine::new(allow_profile).expect("gatekeeper allow");
    let _revoked = gk_allow.trigger_kill_switch(3000);
    let ks_outcome = ctrl
        .request_on_demand_capture(
            "claude-code",
            VisionCaptureTarget::ActiveWindow,
            3005,
            &mut gk_allow,
            &mut bridge,
        )
        .expect("kill switch outcome");
    assert!(
        matches!(ks_outcome, VisionCaptureOutcome::Denied { ref reason_code, .. } if reason_code == "deny_kill_switch")
    );
    assert_eq!(ctrl.backend().capture_count(), 0);

    // 4. Even with Allow rule and no Kill Switch, sensitive password manager window is denied and zeroized
    let mut gk_sens = GatekeeperEngine::new(make_profile(vec![PolicyRule {
        capability: Capability::Vision,
        pattern: "*".to_string(),
        effect: PolicyEffect::Allow,
        pc_only: true,
    }]))
    .expect("gk_sens");
    let sens_backend = DeterministicFixtureVisionBackend::new(640, 480, "Bitwarden Vault - Personal");
    let mut sens_ctrl =
        ScreenVisionController::new(sens_backend, VisionCapturePolicy::default()).expect("sens");
    let sens_outcome = sens_ctrl
        .request_on_demand_capture(
            "claude-code",
            VisionCaptureTarget::ActiveWindow,
            3100,
            &mut gk_sens,
            &mut bridge,
        )
        .expect("sensitive deny");
    assert!(
        matches!(sens_outcome, VisionCaptureOutcome::Denied { ref reason_code, .. } if reason_code == "deny_sensitive_window")
    );
    assert_eq!(sens_ctrl.completed_captures(), 0);
}

// LEDGER-4.3-2
#[test]
fn test_vision_4k_buffer_resized_compressed_under_max_bytes_and_zeroed_after_turn() {
    let allow_profile = make_profile(vec![PolicyRule {
        capability: Capability::Vision,
        pattern: "screen_capture:*".to_string(),
        effect: PolicyEffect::Allow,
        pc_only: true,
    }]);
    let mut gk = GatekeeperEngine::new(allow_profile).expect("gk");
    let mut bridge = new_bridge();
    let backend = DeterministicFixtureVisionBackend::new(3840, 2160, "4K Editor");

    let policy = VisionCapturePolicy {
        max_dimension_px: 320,
        max_output_bytes: 96 * 1024,
        preview_max_width_px: 128,
        preview_max_height_px: 72,
        preview_max_bytes: 40 * 1024,
        continuous_recording_forbidden: true,
        zero_disk_temp_files: true,
    };
    let mut ctrl = ScreenVisionController::new(backend, policy.clone()).expect("ctrl");

    let outcome = ctrl
        .request_on_demand_capture(
            "claude-code",
            VisionCaptureTarget::ActiveWindow,
            4000,
            &mut gk,
            &mut bridge,
        )
        .expect("captured");

    let mut payload = match outcome {
        VisionCaptureOutcome::Captured(p) => p,
        other => panic!("expected Captured, got {:?}", other),
    };

    assert_eq!(DEFAULT_VISION_MAX_DIMENSION_PX, 1024);
    assert_eq!(DEFAULT_VISION_MAX_OUTPUT_BYTES, 250 * 1024);

    assert!(payload.zero_disk_temp_files);
    assert!(payload.width <= policy.max_dimension_px);
    assert!(payload.height <= policy.max_dimension_px);
    assert!(payload.downscale_passes >= 1);
    assert!(!payload.png_bytes.is_empty());
    assert!(payload.png_bytes.len() <= policy.max_output_bytes);
    assert_eq!(&payload.png_bytes[0..8], &PNG_MAGIC_BYTES);
    assert_eq!(
        decode_png_ihdr_dimensions(&payload.png_bytes).expect("valid IHDR"),
        (payload.width, payload.height)
    );
    assert!(payload.retained_bytes() > 0);

    // Verify CaptureTransform attached to TurnVisionPayload
    assert_eq!(payload.transform.origin_virtual_x, 0);
    assert_eq!(payload.transform.origin_virtual_y, 0);

    // Multimodal LLM payload serialization
    let mm_payload = payload.to_multimodal_llm_payload("Describe this screen");
    let parts = mm_payload.as_array().expect("must be array");
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["type"], "text");
    assert_eq!(parts[0]["text"], "Describe this screen");
    assert_eq!(parts[1]["type"], "image_url");
    let url = parts[1]["image_url"]["url"].as_str().expect("url str");
    assert!(url.starts_with("data:image/png;base64,"));

    // OCR extraction fallback
    let fixture_ocr = DeterministicFixtureOcrBackend::new();
    let ocr_res = payload.run_ocr(&fixture_ocr).expect("ocr run");
    assert_eq!(ocr_res.language, "es");
    assert!(!ocr_res.lines.is_empty());

    // End of turn releases and zero-fills all image bytes
    payload.clear_after_turn();
    assert!(payload.bytes_zeroed_after_turn);
    assert_eq!(payload.retained_bytes(), 0);
}

// LEDGER-4.3-3
#[test]
fn test_vision_ask_approval_emits_sanitized_confirmation_preview_and_blocks_xss() {
    let ask_profile = make_profile(vec![PolicyRule {
        capability: Capability::Vision,
        pattern: "screen_capture:region:*".to_string(),
        effect: PolicyEffect::Ask,
        pc_only: true,
    }]);
    let mut gk = GatekeeperEngine::new(ask_profile).expect("gk");
    let mut bridge = new_bridge();
    let hostile_title = "<script>alert(1)</script> <img src=x onerror=fetch('//evil')> Docs";
    let backend = DeterministicFixtureVisionBackend::new(1920, 1080, hostile_title);
    let mut ctrl = ScreenVisionController::new(backend, VisionCapturePolicy::default()).expect("c");

    let region = VisionCaptureTarget::SelectedRegion {
        x: 100,
        y: 80,
        width: 320,
        height: 180,
    };

    let ask_outcome = ctrl
        .request_on_demand_capture("cursor", region.clone(), 5000, &mut gk, &mut bridge)
        .expect("ask");
    let ticket = match ask_outcome {
        VisionCaptureOutcome::AwaitingApproval {
            ticket,
            prompt_render,
        } => {
            assert!(matches!(
                prompt_render,
                WebViewIpcPayloadV1::DomSafeTextRender {
                    csp_enforced: true,
                    ..
                }
            ));
            ticket
        }
        other => panic!("expected AwaitingApproval, got {:?}", other),
    };
    assert_eq!(ctrl.backend().capture_count(), 0);

    let approved = ctrl
        .resolve_pending_capture(
            &ticket.req_hash,
            PermissionDecisionKind::Allow,
            &ticket.req_hash,
            false,
            true,
            5100,
            &mut gk,
            &mut bridge,
        )
        .expect("approved capture");

    let mut payload = match approved {
        VisionCaptureOutcome::Captured(p) => p,
        other => panic!("expected Captured, got {:?}", other),
    };

    assert_eq!(ctrl.backend().capture_count(), 1);
    assert!(!payload.sanitized_window_title.contains('<'));
    assert!(!payload.sanitized_window_title.contains('>'));
    assert!(payload.sanitized_window_title.contains("&lt;script&gt;"));
    assert!(payload.confirmation_preview.preview_width <= 256);
    assert!(payload.confirmation_preview.preview_height <= 144);
    assert!(payload.confirmation_preview.preview_byte_len <= DEFAULT_VISION_PREVIEW_MAX_BYTES);
    assert_eq!(
        &payload.confirmation_preview.preview_png_bytes[0..8],
        &PNG_MAGIC_BYTES
    );
    assert!(bridge.pending_outbound_count() >= 3);

    payload.clear_after_turn();
    assert_eq!(payload.retained_bytes(), 0);
}

// LEDGER-4.3-4
#[test]
fn test_vision_enforces_zero_continuous_background_video_streams() {
    let backend = DeterministicFixtureVisionBackend::new(640, 480, "Safe");
    let ctrl = ScreenVisionController::new(backend, VisionCapturePolicy::default()).expect("ctrl");
    assert_eq!(ctrl.active_background_capture_streams(), 0);
    assert!(ctrl.policy().continuous_recording_forbidden);

    // Reject a policy trying to allow continuous background capture
    let bad_policy = VisionCapturePolicy {
        continuous_recording_forbidden: false,
        ..VisionCapturePolicy::default()
    };
    let backend2 = DeterministicFixtureVisionBackend::new(640, 480, "BadPolicy");
    assert!(ScreenVisionController::new(backend2, bad_policy).is_err());

    // Reject a backend reporting an active continuous background stream
    let bad_backend = DeterministicFixtureVisionBackend::new(640, 480, "Continuous");
    bad_backend
        .force_continuous_stream_flag
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(ScreenVisionController::new(bad_backend, VisionCapturePolicy::default()).is_err());
}

// LEDGER-4.3-5
#[test]
fn test_win32_gdi_screen_vision_backend_on_demand_probe_and_region_png() {
    let backend = Win32GdiScreenVisionBackend::new();
    assert!(!backend.is_continuous_stream_active());

    // 1. Always verify real GDI32 handle lifecycle (DC -> Bitmap -> BitBlt -> GetDIBits -> RAII cleanup)
    let mut offscreen_frame = backend
        .probe_offscreen_gdi_pipeline(32, 32)
        .expect("offscreen GDI probe must succeed on Windows");
    let mut offscreen_png = compress_rgba_to_bounded_png(
        &offscreen_frame,
        DEFAULT_VISION_MAX_DIMENSION_PX,
        DEFAULT_VISION_MAX_DIMENSION_PX,
        DEFAULT_VISION_MAX_OUTPUT_BYTES,
    )
    .expect("offscreen PNG encode");
    offscreen_frame.zeroize_and_clear();
    assert_eq!(&offscreen_png.png_bytes[0..8], &PNG_MAGIC_BYTES);
    assert_eq!(
        decode_png_ihdr_dimensions(&offscreen_png.png_bytes).expect("offscreen IHDR"),
        (32, 32)
    );
    offscreen_png.zeroize_and_clear();

    // 2. Also probe live desktop DC through Gatekeeper; in headless/restricted CI sessions where
    //    no interactive Window Station DC is attached (win32_err=5/6), verify typed fail-closed error.
    let allow_profile = make_profile(vec![PolicyRule {
        capability: Capability::Vision,
        pattern: "screen_capture:region:*".to_string(),
        effect: PolicyEffect::Allow,
        pc_only: true,
    }]);
    let mut gk = GatekeeperEngine::new(allow_profile).expect("gk");
    let mut bridge = new_bridge();
    let mut ctrl =
        ScreenVisionController::new(backend, VisionCapturePolicy::default()).expect("win32 ctrl");

    let target = VisionCaptureTarget::SelectedRegion {
        x: 0,
        y: 0,
        width: 32,
        height: 32,
    };
    match ctrl.request_on_demand_capture("miku-self-test", target, 6000, &mut gk, &mut bridge) {
        Ok(VisionCaptureOutcome::Captured(mut payload)) => {
            assert_eq!(&payload.png_bytes[0..8], &PNG_MAGIC_BYTES);
            assert_eq!(
                decode_png_ihdr_dimensions(&payload.png_bytes).expect("PNG IHDR"),
                (32, 32)
            );
            payload.clear_after_turn();
            assert_eq!(payload.retained_bytes(), 0);
        }
        Ok(other) => panic!("unexpected Gatekeeper outcome with Allow rule: {:?}", other),
        Err(UiError::VisionCaptureError(msg))
            if msg.contains("win32_err=5")
                || msg.contains("win32_err=6")
                || msg.contains("no active foreground window") =>
        {
            println!("WIN32_DESKTOP_DC_RESTRICTED_IN_HEADLESS_RUNNER: {}", msg);
        }
        Err(other) => panic!("unexpected Win32 desktop capture failure: {}", other),
    }
}

// LEDGER-4.3-6
#[test]
fn test_vision_modules_and_miku_ui_comply_with_sub_600_line_budget() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let vision_dir = manifest.join("src").join("vision");
    assert!(vision_dir.is_dir());

    let expected_files = [
        "mod.rs",
        "ocr.rs",
        "types_and_codec.rs",
        "win32_backend.rs",
        "orchestrator.rs",
        "tests.rs",
    ];
    let mut max_lines = 0usize;
    for name in expected_files {
        let p = vision_dir.join(name);
        assert!(p.is_file(), "missing vision module {}", name);
        let text = std::fs::read_to_string(&p).expect("read vision file");
        let lines = text.lines().count();
        assert!(
            lines > 0 && lines < 600,
            "vision/{} exceeded 600 lines: {}",
            name,
            lines
        );
        max_lines = max_lines.max(lines);
    }
    println!(
        "VISION_4_3_MODULAR_AUDIT: files={} max_lines={} (< 600)",
        expected_files.len(),
        max_lines
    );
}
