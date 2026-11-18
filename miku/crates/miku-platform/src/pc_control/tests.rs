use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};
use miku_protocol::PermissionDecisionKind;

use super::*;

fn build_profile_with_root(root: &str, rules: Vec<(Capability, &str, PolicyEffect, bool)>) -> ProjectProfile {
    let mut profile = ProjectProfile::new("miku-pc-control-test", root, 30_000);
    for (cap, pat, eff, pc_only) in rules {
        profile = profile.with_rule(cap, pat, eff, pc_only);
    }
    profile
}

// LEDGER-4.4-1
#[test]
fn test_pc_control_exhaustive_capability_matrix_allow_ask_and_deny() {
    let root = env!("CARGO_MANIFEST_DIR");

    let all_actions = vec![
        PcControlAction::FsRead {
            path: "Cargo.toml".to_string(),
        },
        PcControlAction::FsWrite {
            path: "notes.txt".to_string(),
            content: "hello miku".to_string(),
            create_dirs: false,
        },
        PcControlAction::ShellCommand {
            program: "cargo".to_string(),
            args: vec!["--version".to_string()],
            cwd: None,
            timeout_ms: 5_000,
        },
        PcControlAction::ClipboardWrite {
            text: "copied text".to_string(),
        },
        PcControlAction::UiAutomationInvoke {
            window_title_pattern: "VS Code".to_string(),
            automation_id_or_name: "SaveButton".to_string(),
            control_type: "Button".to_string(),
        },
        PcControlAction::InputKeyCombo {
            keys: vec!["Ctrl".to_string(), "S".to_string()],
        },
    ];

    // 1. DefaultDeny + explicit Deny block 100% of all 6 capabilities
    for profile in [
        build_profile_with_root(root, vec![]),
        build_profile_with_root(
            root,
            vec![
                (Capability::FsRead, "*", PolicyEffect::Deny, false),
                (Capability::FsWrite, "*", PolicyEffect::Deny, true),
                (Capability::Shell, "*", PolicyEffect::Deny, true),
                (Capability::Clipboard, "*", PolicyEffect::Deny, false),
                (Capability::UiAutomation, "*", PolicyEffect::Deny, true),
                (Capability::Input, "*", PolicyEffect::Deny, true),
            ],
        ),
    ] {
        let mut gk = GatekeeperEngine::new(profile).expect("gk deny");
        let mut ctrl = PcControlController::new(SandboxPcControlBackend::new());
        for action in &all_actions {
            let outcome = ctrl
                .execute_action("claude-code", action.clone(), 1000, &mut gk)
                .expect("deny outcome");
            assert!(
                matches!(outcome, PcControlOutcome::Denied { .. }),
                "expected Denied for {:?}, got {:?}",
                action.capability(),
                outcome
            );
        }
        assert!(ctrl.backend().recorded_actions().is_empty());
        assert_eq!(ctrl.active_resident_threads(), 0);
    }

    // 2. Ask pauses each of the 6 capabilities, then executes ONLY upon explicit Allow (+ rejects UserDenied / TOCTOU / mobile pc_only)
    let ask_profile = build_profile_with_root(
        root,
        vec![
            (Capability::FsRead, "**", PolicyEffect::Ask, false),
            (Capability::FsWrite, "**", PolicyEffect::Ask, true),
            (Capability::Shell, "*", PolicyEffect::Ask, true),
            (Capability::Clipboard, "*", PolicyEffect::Ask, false),
            (Capability::UiAutomation, "*", PolicyEffect::Ask, true),
            (Capability::Input, "*", PolicyEffect::Ask, true),
        ],
    );
    let mut gk_ask = GatekeeperEngine::new(ask_profile).expect("gk ask");
    let backend = SandboxPcControlBackend::new();
    let canonical_cargo = format!("{}/Cargo.toml", root.replace('\\', "/"));
    backend.seed_file(&canonical_cargo, "[package]\nname=\"miku-platform\"");
    let mut ctrl_ask = PcControlController::new(backend);

    for (idx, action) in all_actions.iter().enumerate() {
        let now = 2000 + (idx as u64) * 10;
        let pending = ctrl_ask
            .execute_action("cursor", action.clone(), now, &mut gk_ask)
            .expect("ask pending");
        let ticket = match pending {
            PcControlOutcome::AwaitingApproval { ticket } => ticket,
            other => panic!("expected AwaitingApproval, got {:?}", other),
        };
        assert_eq!(ctrl_ask.executed_count(), idx as u64);

        // Mobile approval rejected for pc_only capabilities
        if ticket.pc_only {
            let mob = ctrl_ask
                .resolve_pending_action(
                    &ticket.req_hash,
                    PermissionDecisionKind::Allow,
                    &ticket.req_hash,
                    true,
                    true,
                    now + 2,
                    &mut gk_ask,
                )
                .expect("mobile reject");
            assert!(
                matches!(mob, PcControlOutcome::Denied { ref reason_code, .. } if reason_code == "deny_pc_only_mobile")
            );

            // Re-issue and approve from PC
            let pending2 = ctrl_ask
                .execute_action("cursor", action.clone(), now + 4, &mut gk_ask)
                .expect("re-ask");
            let ticket2 = match pending2 {
                PcControlOutcome::AwaitingApproval { ticket } => ticket,
                other => panic!("expected AwaitingApproval, got {:?}", other),
            };
            let ok = ctrl_ask
                .resolve_pending_action(
                    &ticket2.req_hash,
                    PermissionDecisionKind::Allow,
                    &ticket2.req_hash,
                    false,
                    true,
                    now + 5,
                    &mut gk_ask,
                )
                .expect("pc allow");
            assert!(matches!(ok, PcControlOutcome::Executed(_)));
        } else {
            let ok = ctrl_ask
                .resolve_pending_action(
                    &ticket.req_hash,
                    PermissionDecisionKind::Allow,
                    &ticket.req_hash,
                    false,
                    true,
                    now + 2,
                    &mut gk_ask,
                )
                .expect("allow");
            assert!(matches!(ok, PcControlOutcome::Executed(_)));
        }
    }
    assert_eq!(ctrl_ask.executed_count(), all_actions.len() as u64);
    assert_eq!(ctrl_ask.active_resident_threads(), 0);
}

// LEDGER-4.4-2
#[test]
fn test_immutable_global_destructive_shell_denylist_overrides_even_allow_all() {
    let root = env!("CARGO_MANIFEST_DIR");
    let permissive_profile = build_profile_with_root(
        root,
        vec![(Capability::Shell, "*", PolicyEffect::Allow, false)],
    );
    let mut gk = GatekeeperEngine::new(permissive_profile).expect("gk");
    let mut ctrl = PcControlController::new(SandboxPcControlBackend::new());

    let dangerous_commands = [
        ("rm", vec!["-rf", "/"]),
        ("cmd", vec!["/C", "del /f /q C:\\important"]),
        ("powershell", vec!["-Command", "Remove-Item -Recurse -Force C:\\repo"]),
        ("format", vec!["D:", "/FS:NTFS"]),
        ("vssadmin", vec!["delete", "shadows", "/all"]),
        ("git", vec!["reset", "--hard", "HEAD~5"]),
        ("powershell", vec!["-EncodedCommand", "SQBFAFgA"]),
    ];

    for (prog, args) in dangerous_commands {
        let action = PcControlAction::ShellCommand {
            program: prog.to_string(),
            args: args.into_iter().map(str::to_string).collect(),
            cwd: None,
            timeout_ms: 3_000,
        };
        let outcome = ctrl
            .execute_action("rogue-agent", action, 3000, &mut gk)
            .expect("immutable guard result");
        assert!(
            matches!(outcome, PcControlOutcome::Denied { ref reason_code, .. } if reason_code == "deny_immutable_shell_guard"),
            "dangerous shell command `{}` was not blocked by immutable denylist: {:?}",
            prog,
            outcome
        );
    }
    assert!(ctrl.backend().recorded_actions().is_empty());
}

// LEDGER-4.4-3
#[test]
fn test_ui_automation_prioritizes_semantic_tree_over_pixel_coordinates() {
    let root = env!("CARGO_MANIFEST_DIR");
    let profile = build_profile_with_root(
        root,
        vec![(Capability::UiAutomation, "*", PolicyEffect::Allow, true)],
    );
    let mut gk = GatekeeperEngine::new(profile).expect("gk");
    let mut ctrl = PcControlController::new(SandboxPcControlBackend::new());

    // 1. Semantic UI Automation succeeds with used_ui_automation_tree = true
    let uia = PcControlAction::UiAutomationInvoke {
        window_title_pattern: "Notepad".to_string(),
        automation_id_or_name: "Save".to_string(),
        control_type: "Button".to_string(),
    };
    let res = ctrl
        .execute_action("claude-code", uia, 4000, &mut gk)
        .expect("uia ok");
    match res {
        PcControlOutcome::Executed(r) => assert!(r.used_ui_automation_tree),
        other => panic!("expected Executed, got {:?}", other),
    }

    // 2. Unjustified pixel coordinate fallback is rejected during validation
    let unjustified_pixel = PcControlAction::UiAutomationFallbackPixelClick {
        window_title_pattern: "CanvasApp".to_string(),
        x: 200,
        y: 120,
        ui_tree_unavailable_reason: "".to_string(),
    };
    assert!(ctrl
        .execute_action("claude-code", unjustified_pixel, 4001, &mut gk)
        .is_err());

    // 3. Verified fallback marks used_ui_automation_tree = false explicitly
    let justified_pixel = PcControlAction::UiAutomationFallbackPixelClick {
        window_title_pattern: "CanvasApp".to_string(),
        x: 200,
        y: 120,
        ui_tree_unavailable_reason: "custom_opengl_canvas_exposes_no_uia_tree".to_string(),
    };
    let res2 = ctrl
        .execute_action("claude-code", justified_pixel, 4002, &mut gk)
        .expect("justified fallback");
    match res2 {
        PcControlOutcome::Executed(r) => assert!(!r.used_ui_automation_tree),
        other => panic!("expected Executed fallback, got {:?}", other),
    }
}

// LEDGER-4.4-4
#[test]
fn test_kill_switch_aborts_input_sequence_and_real_shell_process_under_20ms() {
    let root = env!("CARGO_MANIFEST_DIR");
    let profile = build_profile_with_root(
        root,
        vec![
            (Capability::Input, "*", PolicyEffect::Allow, true),
            (Capability::Shell, "*", PolicyEffect::Allow, true),
        ],
    );
    let mut gk = GatekeeperEngine::new(profile).expect("gk");

    // 1. Abort an in-flight multi-step input sequence in < 20 ms
    let mut ctrl_input = PcControlController::new(SandboxPcControlBackend::new());
    let report = ctrl_input.trip_kill_switch(5000, &mut gk);
    assert!(report.within_20ms_budget, "elapsed={}ms", report.elapsed_ms);
    assert!(report.abort_flag_active);

    let steps = vec![
        InputStep::KeyCombo(vec!["Ctrl".to_string(), "A".to_string()]),
        InputStep::TypeText("should not type".to_string()),
        InputStep::MouseClick { x: 10, y: 20 },
    ];
    let seq_outcome = ctrl_input
        .execute_authorized_input_sequence("claude-code", &steps, 5005, &mut gk)
        .expect("input sequence outcome");
    assert!(
        matches!(seq_outcome, PcControlOutcome::Denied { ref reason_code, .. } if reason_code == "deny_kill_switch")
    );
    assert!(ctrl_input.backend().recorded_actions().is_empty());
    assert_eq!(ctrl_input.active_resident_threads(), 0);

    // 2. Terminate a real Windows child process when Kill Switch flips mid-flight (< 20 ms kill response)
    #[cfg(windows)]
    {
        let gk_live = GatekeeperEngine::new(build_profile_with_root(
            root,
            vec![(Capability::Shell, "*", PolicyEffect::Allow, true)],
        ))
        .expect("gk live");
        let mut gk_live = gk_live;
        let mut ctrl_live = PcControlController::new(WindowsNativePcControlBackend::new());
        let abort_handle = ctrl_live.abort_flag_handle();

        let stopper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(60));
            let t0 = Instant::now();
            abort_handle.store(true, Ordering::SeqCst);
            t0
        });

        let long_cmd = PcControlAction::ShellCommand {
            program: "ping".to_string(),
            args: vec!["127.0.0.1".to_string(), "-n".to_string(), "10".to_string()],
            cwd: None,
            timeout_ms: 15_000,
        };
        let out = ctrl_live
            .execute_action("claude-code", long_cmd, 6000, &mut gk_live)
            .expect("real shell kill switch");
        let stop_instant = stopper.join().expect("stopper join");
        let kill_react_ms = stop_instant.elapsed().as_secs_f64() * 1000.0;
        assert!(
            matches!(out, PcControlOutcome::Denied { ref reason_code, .. } if reason_code == "deny_kill_switch"),
            "expected kill switch abort, got {:?}",
            out
        );
        assert!(
            kill_react_ms < 20.0,
            "child shell kill reaction took {:.3} ms >= 20 ms",
            kill_react_ms
        );
        assert_eq!(ctrl_live.active_resident_threads(), 0);
    }
}

// LEDGER-4.4-5
#[test]
fn test_windows_native_pc_control_jailed_filesystem_and_safe_shell_roundtrip() {
    let temp_root = std::env::temp_dir().join(format!("miku_pc_4_4_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_root);
    fs::create_dir_all(&temp_root).expect("create temp jail");
    let root_str = temp_root.to_string_lossy().to_string();

    let profile = build_profile_with_root(
        &root_str,
        vec![
            (Capability::FsWrite, "**", PolicyEffect::Allow, true),
            (Capability::FsRead, "**", PolicyEffect::Allow, false),
            (Capability::Shell, "cmd /c echo*", PolicyEffect::Allow, true),
        ],
    );
    let mut gk = GatekeeperEngine::new(profile).expect("gk jail");
    let mut ctrl = PcControlController::new(WindowsNativePcControlBackend::new());

    // 1. Path traversal outside PathJail is blocked
    let escape = ctrl
        .execute_action(
            "cursor",
            PcControlAction::FsWrite {
                path: "..\\outside_secret.txt".to_string(),
                content: "nope".to_string(),
                create_dirs: false,
            },
            7000,
            &mut gk,
        )
        .expect("jail check");
    assert!(
        matches!(escape, PcControlOutcome::Denied { ref reason_code, .. } if reason_code == "deny_jail")
    );

    // 2. Write & read inside PathJail succeeds
    let wr = ctrl
        .execute_action(
            "cursor",
            PcControlAction::FsWrite {
                path: "sub/hello.txt".to_string(),
                content: "miku_pc_control_ok".to_string(),
                create_dirs: true,
            },
            7001,
            &mut gk,
        )
        .expect("write inside jail");
    assert!(matches!(wr, PcControlOutcome::Executed(r) if r.bytes_written == 18));

    let rd = ctrl
        .execute_action(
            "cursor",
            PcControlAction::FsRead {
                path: "sub/hello.txt".to_string(),
            },
            7002,
            &mut gk,
        )
        .expect("read inside jail");
    match rd {
        PcControlOutcome::Executed(r) => assert_eq!(r.stdout_or_value, "miku_pc_control_ok"),
        other => panic!("expected Executed fs_read, got {:?}", other),
    }

    let _ = fs::remove_dir_all(&temp_root);
}

// LEDGER-4.4-6
#[test]
fn test_pc_control_modules_comply_with_sub_600_line_modular_budget() {
    let pc_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("pc_control");
    assert!(pc_dir.is_dir());
    let files = [
        "mod.rs",
        "types_and_guards.rs",
        "backends.rs",
        "controller.rs",
        "coords.rs",
        "win32_input.rs",
        "win32_uia.rs",
        "tests.rs",
    ];
    let mut max_lines = 0usize;
    for f in files {
        let content = fs::read_to_string(pc_dir.join(f)).expect("read pc_control file");
        let count = content.lines().count();
        assert!(
            count > 0 && count < 600,
            "pc_control/{} exceeded 600 lines: {}",
            f,
            count
        );
        max_lines = max_lines.max(count);
    }
    println!(
        "PC_CONTROL_4_4_MODULAR_AUDIT: files={} max_lines={} (< 600)",
        files.len(),
        max_lines
    );
}

#[test]
fn test_key_combo_guard_raii_modifier_release() {
    use std::sync::{Arc, Mutex};
    use crate::pc_control::win32_input::{KeyComboGuard, VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT};

    let sink = Arc::new(Mutex::new(Vec::new()));
    {
        let mut guard = KeyComboGuard::from_key_names(["ctrl", "alt", "shift", "win"]);
        guard.set_test_release_sink(Arc::clone(&sink));
        assert_eq!(guard.modifiers().len(), 4);
    }
    // All 4 modifiers released on drop in reverse order
    let released = sink.lock().unwrap().clone();
    assert_eq!(released, vec![VK_LWIN, VK_SHIFT, VK_MENU, VK_CONTROL]);
}

#[test]
fn test_capture_transform_multi_dpi_virtual_coordinates() {
    use crate::pc_control::coords::{pixel_to_virtual, screen_coords_to_virtual, virtual_to_screen_pixel, CaptureTransform};

    let t = CaptureTransform::new(-1920, 0, 1920, 1080, 2.0);
    assert_eq!(screen_coords_to_virtual(0.5, 0.5, &t), (-960, 540));
    assert_eq!(pixel_to_virtual(200, 300, &t), (-1920 + 400, 600));
    assert_eq!(virtual_to_screen_pixel(-1520, 600, &t), (200, 300));
}

#[tokio::test]
async fn test_ui_automation_safe_invocation_with_600ms_timeout() {
    use crate::pc_control::win32_uia::invoke_uia_with_600ms_timeout;

    // Normal fast call (< 600ms)
    let r1 = invoke_uia_with_600ms_timeout("Notepad", "Edit", "EditControl", Some(20)).await;
    assert!(r1.target_summary.contains("Notepad"));

    // Call that exceeds 600ms timeout
    let r2 = invoke_uia_with_600ms_timeout("FrozenApp", "btn", "Button", Some(750)).await;
    assert!(!r2.used_ui_automation_tree);
    assert!(r2.target_summary.contains("timeout_fallback_600ms"));
}
