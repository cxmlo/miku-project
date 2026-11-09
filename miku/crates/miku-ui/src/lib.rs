#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity,
    clippy::invisible_characters
)]

//! `miku-ui` — Native Win32 Island, Mode C WebView2 Host, 6-Spring Avatar Runtime,
//! Pre-Execution Gatekeeper Interceptor, Append-Only JSONL Audit Store, PTT/Wake-Word Voice Loop,
//! Global Kill Switch (`Ctrl+Alt+Shift+K`), Idle Suspension Coordinator, and Standalone Demo Mode.
//!
//! Micro-step 2.12 (Principle 8 · Modular Boundaries):
//! This crate root acts as an isomorphic public facade (`pub mod` + `pub use`) over 8 cohesive
//! domain modules (`win32`, `avatar`, `webview`, `gatekeeper`, `audit`, `voice`, `power`, `demo`),
//! where every source file in `miku/crates/miku-ui/src/` (including `main.rs`) is strictly `< 600` lines.

pub use miku_gatekeeper::{
    AuditEntry, CanonicalPathJail, Capability, DenyReason, GatekeeperEngine, GatekeeperVerdict,
    JailError, PermissionTicket, PolicyEffect, PolicyRule, ProjectProfile, TamperError,
    TamperEvidentAuditLog, GENESIS_AUDIT_HASH,
};

pub mod audit;
pub mod avatar;
pub mod demo;
pub mod gatekeeper;
pub mod power;
pub mod voice;
pub mod webview;
pub mod win32;

pub use crate::audit::*;
pub use crate::avatar::*;
pub use crate::demo::*;
pub use crate::gatekeeper::*;
pub use crate::power::*;
pub use crate::voice::*;
pub use crate::webview::*;
pub use crate::win32::*;

#[allow(unused_imports)]
pub(crate) mod prelude {
    pub(crate) use crate::{
        audit::*, avatar::*, demo::*, gatekeeper::*, power::*, voice::*, webview::*, win32::*,
    };
    pub(crate) use miku_gatekeeper::{
        AuditEntry, CanonicalPathJail, Capability, DenyReason, GatekeeperEngine, GatekeeperVerdict,
        JailError, PermissionTicket, PolicyEffect, PolicyRule, ProjectProfile, TamperError,
        TamperEvidentAuditLog, GENESIS_AUDIT_HASH,
    };
    pub(crate) use miku_protocol::{
        constant_time_token_eq, sha256_hex, IslandState, MikuLinkMessageV1, PermissionDecisionKind,
        MAX_AUTH_FRAME_BYTES, MIKU_PROTOCOL_VERSION_V1, SUPPORTED_PROTOCOL_VERSIONS,
    };
    pub(crate) use serde::{Deserialize, Serialize};
    pub(crate) use std::collections::{BTreeMap, VecDeque};
    pub(crate) use std::fs::{self, OpenOptions};
    pub(crate) use std::io::{BufRead, BufReader, Write};
    pub(crate) use std::path::{Path, PathBuf};
    pub(crate) use std::sync::atomic::{AtomicU64, Ordering};
    pub(crate) use std::time::{Instant, SystemTime, UNIX_EPOCH};
}

// ============================================================================
// Micro-step 2.12 · Modular Facade, Line Budget & Cross-Domain Integration Tests
// (LEDGER-2.12-1 .. LEDGER-2.12-3)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::TypeId;
    use std::fs;
    use std::path::Path;

    // LEDGER-2.12-1
    #[test]
    fn test_modular_facade_preserves_all_public_reexports_and_constants() {
        // 1. Verify constants across the public facade and win32 constants module
        assert_eq!(STANDARD_DPI, crate::win32::STANDARD_DPI);
        assert_eq!(REQUIRED_EX_STYLE, crate::win32::REQUIRED_EX_STYLE);
        assert_eq!(REQUIRED_EX_STYLE, 0x0808_0088);
        assert_eq!(HOVER_HYSTERESIS_PX, crate::win32::HOVER_HYSTERESIS_PX);
        assert_eq!(EXPECTED_AVATAR_SVG_COUNT, 22);
        assert_eq!(DEFAULT_WEBVIEW2_WARM_TTL_MS, 15_000);
        assert_eq!(DEFAULT_CONFIRMATION_MODAL_TTL_MS, 30_000);
        assert_eq!(MAX_KILL_SWITCH_LATENCY_MS, 20.0);
        assert_eq!(DEFAULT_MAX_AUDIT_FILE_BYTES, 5 * 1024 * 1024);
        assert_eq!(SOUNDWAVE_BAND_COUNT, 5);
        assert_eq!(DEFAULT_IDLE_SUSPENSION_TTL_MS, 3_000);
        assert_eq!(DEMO_BADGE_PREFIX, "[DEMO]");
        assert_eq!(DEMO_MAX_STARTUP_LATENCY_MS, 50.0);

        // 2. Verify exact TypeId isomorphism between flat crate root and all 8 domain paths
        assert_eq!(
            TypeId::of::<MonitorDescriptor>(),
            TypeId::of::<crate::win32::MonitorDescriptor>()
        );
        assert_eq!(
            TypeId::of::<PillHoverStateMachine>(),
            TypeId::of::<crate::win32::PillHoverStateMachine>()
        );
        assert_eq!(
            TypeId::of::<MultiSpringAvatarPhysics>(),
            TypeId::of::<crate::avatar::MultiSpringAvatarPhysics>()
        );
        assert_eq!(
            TypeId::of::<AvatarPackHotReloader>(),
            TypeId::of::<crate::avatar::AvatarPackHotReloader>()
        );
        assert_eq!(
            TypeId::of::<WebView2IpcBridge>(),
            TypeId::of::<crate::webview::WebView2IpcBridge>()
        );
        assert_eq!(
            TypeId::of::<OverviewSceneController>(),
            TypeId::of::<crate::webview::OverviewSceneController>()
        );
        assert_eq!(
            TypeId::of::<UiGatekeeperInterceptor>(),
            TypeId::of::<crate::gatekeeper::UiGatekeeperInterceptor>()
        );
        assert_eq!(
            TypeId::of::<UiKillSwitchManager>(),
            TypeId::of::<crate::gatekeeper::UiKillSwitchManager>()
        );
        assert_eq!(
            TypeId::of::<PersistentJsonlAuditStore>(),
            TypeId::of::<crate::audit::PersistentJsonlAuditStore>()
        );
        assert_eq!(
            TypeId::of::<VoiceInteractionController>(),
            TypeId::of::<crate::voice::VoiceInteractionController>()
        );
        assert_eq!(
            TypeId::of::<IdleSuspensionCoordinator>(),
            TypeId::of::<crate::power::IdleSuspensionCoordinator>()
        );
        assert_eq!(
            TypeId::of::<DemoInteractiveController>(),
            TypeId::of::<crate::demo::DemoInteractiveController>()
        );
    }

    // LEDGER-2.12-2
    #[test]
    fn test_every_miku_ui_source_module_enforces_sub_600_lines_and_clean_boundaries() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let src_dir = manifest_dir.join("src");
        let expected_domains = [
            "win32",
            "avatar",
            "webview",
            "gatekeeper",
            "audit",
            "voice",
            "power",
            "demo",
        ];

        // 1. Check all 8 domain directories exist with mod.rs
        for domain in expected_domains {
            let domain_dir = src_dir.join(domain);
            assert!(
                domain_dir.is_dir(),
                "Expected domain directory src/{} to exist",
                domain
            );
            assert!(
                domain_dir.join("mod.rs").is_file(),
                "Expected src/{}/mod.rs to exist",
                domain
            );
        }

        // 2. Recursively walk every .rs file under src/ (including lib.rs, main.rs, and all submodules)
        let mut stack = vec![src_dir.clone()];
        let mut total_checked_files = 0usize;
        let mut max_lines_observed = 0usize;
        let mut max_lines_file = String::new();

        while let Some(dir) = stack.pop() {
            for entry in fs::read_dir(&dir).expect("read src dir") {
                let entry = entry.expect("dir entry");
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    let text = fs::read_to_string(&path).expect("read .rs source file");
                    let count = text.lines().count();
                    let rel = path
                        .strip_prefix(&src_dir)
                        .unwrap_or(&path)
                        .display()
                        .to_string();
                    if count > max_lines_observed {
                        max_lines_observed = count;
                        max_lines_file = rel.clone();
                    }
                    assert!(
                        count > 0 && count < 600,
                        "Source file src/{} exceeded 600-line budget: {} lines",
                        rel,
                        count
                    );
                    total_checked_files += 1;
                }
            }
        }

        println!(
            "[MODULAR-2.12] checked_files={} max_lines={} ({})",
            total_checked_files, max_lines_observed, max_lines_file
        );

        assert!(
            total_checked_files >= 40,
            "Expected at least 40 modular .rs files across src/, checked {}",
            total_checked_files
        );
    }

    // LEDGER-2.12-3
    #[test]
    fn test_cross_module_integration_win32_webview_avatar_gatekeeper_audit_voice_power_demo() {
        let temp_audit_dir = create_unique_isolated_audit_temp_dir("modular_2_12", 1).unwrap();

        // 1. win32: primary monitor & top-center pill anchor (248x44 @ 836, 0)
        let monitor = MonitorDescriptor {
            device_name: "DISPLAY_MODULAR_TEST".to_string(),
            is_primary: true,
            dpi: STANDARD_DPI,
            monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
            work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
        };
        let mut pill_fsm =
            PillHoverStateMachine::new(monitor.clone(), TOP_MARGIN_DEFAULT_PX).unwrap();
        assert_eq!(pill_fsm.geometry().x, 836);
        assert_eq!(pill_fsm.geometry().width, 248);
        let hover_res = pill_fsm.on_cursor_move(960, 20).unwrap();
        assert_eq!(hover_res.new_mode, PillVisualMode::HoverPeek);

        // 2. avatar: 22 SVG catalog + 6-spring physics runtime settling to 0.00 fps
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).unwrap();
        let avatar_rt = reloader.runtime_mut();
        avatar_rt.physics.apply_mood_impulse(AvatarMood::Happy);
        let (_, settled_fps) = avatar_rt.step_to_rest(1.0 / 60.0, 240);
        assert_eq!(settled_fps, 0);

        // 3. webview: Mode C host FSM + JSON-RPC IPC bridge with textContent sanitization
        let driver = IsolatedWebView2ProcessDriver::new(2.51, 62.50);
        let mut host = WebView2HostStateMachine::new(
            monitor.clone(),
            TOP_MARGIN_DEFAULT_PX,
            Some(&temp_audit_dir),
            WebView2SecurityPolicy::release_hardened(),
            DEFAULT_IDLE_SUSPENSION_TTL_MS,
            driver,
        )
        .unwrap();
        let mut bridge = WebView2IpcBridge::new().unwrap();
        let clean_cell =
            sanitize_for_text_content("miku-mod-cell", "<img src=x onerror=alert(1)>Miku").unwrap();
        if let WebViewIpcPayloadV1::DomSafeTextRender {
            ref safe_text,
            csp_enforced,
            ..
        } = clean_cell
        {
            assert!(csp_enforced);
            assert!(!safe_text.contains("<img"));
        } else {
            panic!("expected DomSafeTextRender");
        }

        // 4. audit + gatekeeper: isolated JSONL store + Kill Switch + Voice
        let mut audit_store = PersistentJsonlAuditStore::open_default(&temp_audit_dir).unwrap();
        let mut gk =
            GatekeeperEngine::new(default_ui_gatekeeper_profile(&temp_audit_dir.to_string_lossy()))
                .unwrap();
        let mut voice = VoiceInteractionController::new_default(avatar_dir.clone());
        voice
            .start_ptt_session(
                DEFAULT_PTT_HOTKEY_COMBO,
                1_000,
                0x1111,
                0x1111,
                0x2222,
                avatar_rt,
            )
            .unwrap();
        let _ = voice
            .ingest_pcm_frame(&[8_000i16; 320], 1_020, avatar_rt, Some(&mut bridge))
            .unwrap();

        let mut ks = UiKillSwitchManager::new();
        let trip_report = ks
            .trip(
                1_050,
                "modular-cross-domain-trip",
                &mut gk,
                &mut voice,
                avatar_rt,
                &mut audit_store,
                &mut bridge,
            )
            .unwrap();
        assert!(trip_report.reaction_latency_ms < 100.0);
        assert_eq!(trip_report.pcm_zero_report.residual_non_zero_samples, 0);
        assert_eq!(voice.pcm_buffer().capacity_bytes(), 0);
        assert_eq!(avatar_rt.current_mood, AvatarMood::Alert);

        // 5. power: DeepIdleSuspended transition after 3000ms TTL & sub-16ms warm wake
        let _ = host.expand_island(1_900).unwrap();
        let mut power = IdleSuspensionCoordinator::new(DEFAULT_IDLE_SUSPENSION_TTL_MS);
        power.begin_cooling_ttl(2_000, &mut host).unwrap();
        let susp = power
            .poll_inactivity_ttl(
                5_100,
                &mut host,
                &mut bridge,
                &mut voice,
                avatar_rt,
                &avatar_dir,
            )
            .unwrap()
            .unwrap();
        assert_eq!(susp.residual_webview2_pids, 0);
        assert_eq!(susp.avatar_settled_fps, 0.0);
        let wake = power
            .wake_on_stimulus(
                WakeStimulusKind::MouseHoverEnter,
                5_200,
                0x1111,
                0x1111,
                0x2222,
                &mut voice,
                avatar_rt,
                &avatar_dir,
            )
            .unwrap();
        assert!(wake.wake_latency_ms < MAX_WARM_WAKE_LATENCY_MS);

        // 6. demo: CLI parser + DemoSandboxGuard in RAM (0 disk writes, 0 API keys)
        let args = vec![
            "miku".to_string(),
            "--demo".to_string(),
            "--duration-ms".to_string(),
            "25".to_string(),
        ];
        let opts = parse_demo_cli_args(&args).unwrap();
        assert!(opts.enabled);
        let mut demo_ctrl = DemoInteractiveController::new(avatar_dir, monitor).unwrap();
        let _ = demo_ctrl.step_escape_clean_exit(6_000).unwrap();
        let demo_rep = demo_ctrl.build_session_report(3.5, true).unwrap();
        assert_eq!(demo_rep.disk_files_mutated, 0);
        assert_eq!(demo_rep.api_keys_touched, 0);

        let _ = fs::remove_dir_all(&temp_audit_dir);
    }
}
