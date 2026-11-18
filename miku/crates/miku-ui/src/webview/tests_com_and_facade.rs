#![allow(unused_imports)]
use crate::prelude::*;

use std::fs;
use std::path::Path;

// LEDGER-4.13-1
#[test]
fn test_4_13_miku_ui_lib_has_zero_wildcard_pub_use_and_respects_600_lines() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let lib_rs = manifest_dir.join("src").join("lib.rs");
    let lib_text = fs::read_to_string(&lib_rs).expect("read miku-ui/src/lib.rs");

    for (idx, line) in lib_text.lines().enumerate() {
        let trimmed = line.trim();
        assert!(
            !(trimmed.contains("pub use") && trimmed.contains("::*")),
            "miku-ui/src/lib.rs line {} contains forbidden wildcard `pub use *`: {}",
            idx + 1,
            trimmed
        );
        assert!(
            !(trimmed.contains("pub(crate) use") && trimmed.contains("::*")),
            "miku-ui/src/lib.rs line {} contains wildcard `pub(crate) use *`: {}",
            idx + 1,
            trimmed
        );
    }

    let line_count = lib_text.lines().count();
    assert!(
        line_count < 600,
        "miku-ui/src/lib.rs exceeded 600-line modular budget: {} lines",
        line_count
    );
}

// LEDGER-4.13-2
#[test]
fn test_4_13_win32_com_driver_probes_edge_webview2_runtime_and_enforces_isolated_dir() {
    let probe = probe_edge_webview2_runtime();
    #[cfg(target_os = "windows")]
    {
        assert!(
            probe.installed,
            "Edge WebView2 Runtime must be discoverable on Windows host"
        );
        assert!(
            probe.com_sta_ready,
            "COM STA initialization via ole32.dll must succeed"
        );
        println!(
            "[UI-4.13-PROBE] webview2_version={:?} loader_dll={} com_sta={}",
            probe.runtime_version, probe.webview2_loader_available, probe.com_sta_ready
        );
    }

    let mut driver = Win32WebView2ComDriver::new();
    let bad_dir = Path::new(r"C:\Users\ci\EBWebView");
    let err = driver
        .spawn_isolated_controller(
            bad_dir,
            &WebView2SecurityPolicy::release_hardened(),
            PillAnchorGeometry {
                x: 520,
                y: 0,
                width: 880,
                height: 520,
                corner_radius_px: 28,
                dpi: STANDARD_DPI,
                scale_pct: 100,
            },
        )
        .expect_err("unisolated EBWebView directory must be rejected");
    assert!(matches!(err, UiError::InvalidUserDataFolder(_)));
}

// LEDGER-4.13-3
#[test]
fn test_4_13_real_createwindowexw_and_mode_c_ttl_evicts_cleanly_within_budgets() {
    let _wv2_guard = super::WEBVIEW2_CHILD_PROC_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let temp_root = std::env::temp_dir().join(format!("miku_4_13_com_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_root);
    fs::create_dir_all(&temp_root).unwrap();

    let monitor = MonitorDescriptor {
        device_name: "DISPLAY_4_13".to_string(),
        is_primary: true,
        dpi: STANDARD_DPI,
        monitor_rect: VirtualRect::new(0, 0, 1920, 1080).unwrap(),
        work_area: VirtualRect::new(0, 0, 1920, 1040).unwrap(),
    };
    let mut host = WebView2HostStateMachine::new(
        monitor,
        TOP_MARGIN_DEFAULT_PX,
        Some(&temp_root),
        WebView2SecurityPolicy::release_hardened(),
        DEFAULT_WEBVIEW2_WARM_TTL_MS,
        Win32WebView2ComDriver::new(),
    )
    .unwrap();

    let mut expand_latencies = Vec::new();
    for cycle in 0..3u64 {
        let t_base = 10_000 + cycle * 30_000;
        let rep = host.expand_island(t_base).unwrap();
        expand_latencies.push(rep.expand_latency_ms);
        assert!(host.driver().hwnd_raw() != 0);
        let tel = host.driver().telemetry();
        assert!(tel.ex_style_verified);
        assert!(tel.ma_noactivate_verified);
        assert!(tel.foreground_focus_preserved);

        // Warm hide + quick reopen
        let _ = host.collapse_island(t_base + 100).unwrap();
        let warm = host.expand_island(t_base + 200).unwrap();
        assert!(warm.reused_warm);
        expand_latencies.push(warm.expand_latency_ms);

        // Collapse & expire TTL -> zero children, clean controller eviction
        let _ = host.collapse_island(t_base + 300).unwrap();
        let evicted = host
            .poll_ttl(t_base + 300 + DEFAULT_WEBVIEW2_WARM_TTL_MS)
            .unwrap();
        assert!(evicted);
        assert!(!host.driver().has_active_controller());
        assert_eq!(host.driver().teardown_cycles(), cycle + 1);
        assert!(host.driver().active_child_webview2_pids().is_empty());
        assert!(live_child_msedgewebview2_pids().is_empty());
    }

    expand_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = expand_latencies[((expand_latencies.len() as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(expand_latencies.len() - 1)];
    let post_ttl_ram = query_current_process_private_ram_mb();
    assert!(
        p95 <= MAX_EXPAND_VISIBLE_LATENCY_MS,
        "p95 expand latency {:.3} ms exceeded {:.1} ms (samples={:?})",
        p95,
        MAX_EXPAND_VISIBLE_LATENCY_MS,
        expand_latencies
    );
    assert!(post_ttl_ram > 0.5 && post_ttl_ram < 128.0);
    println!(
        "[UI-4.13-LIFECYCLE] cycles=3 p95_expand_ms={:.3} post_ttl_ram_mb={:.2} orphan_pids=0",
        p95, post_ttl_ram
    );

    let _ = fs::remove_dir_all(&temp_root);
}

// LEDGER-4.13-4
#[test]
fn test_4_13_run_win32_com_webview2_cycle_helper_succeeds() {
    let _wv2_guard = super::WEBVIEW2_CHILD_PROC_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let temp_root = std::env::temp_dir().join(format!("miku_4_13_helper_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_root);
    fs::create_dir_all(&temp_root).unwrap();

    let bounds = PillAnchorGeometry {
        x: 520,
        y: 8,
        width: OVERVIEW_EXPANDED_WIDTH_PX as i32,
        height: OVERVIEW_EXPANDED_HEIGHT_PX as i32,
        corner_radius_px: OVERVIEW_CORNER_RADIUS_PX as i32,
        dpi: STANDARD_DPI,
        scale_pct: 100,
    };
    let tel = run_win32_com_webview2_cycle(Some(&temp_root), bounds).unwrap();
    assert!(tel.hwnd_raw != 0);
    assert!(tel.ex_style_verified);
    assert!(tel.ma_noactivate_verified);
    assert!(tel.live_webview2_child_pids.is_empty());
    assert!(
        tel.expand_latency_ms <= MAX_EXPAND_VISIBLE_LATENCY_MS,
        "expand latency {:.3} ms exceeded {:.1} ms",
        tel.expand_latency_ms,
        MAX_EXPAND_VISIBLE_LATENCY_MS
    );

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn test_webview2_microphone_permission_auto_approve_handler() {
    #[cfg(target_os = "windows")]
    {
        use crate::webview::com_abi::win_com;
        use std::ffi::c_void;
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::mpsc;

        assert_eq!(win_com::COREWEBVIEW2_PERMISSION_KIND_MICROPHONE, 2);
        assert_eq!(win_com::COREWEBVIEW2_PERMISSION_STATE_ALLOW, 1);
        assert!(win_com::IID_PERMISSION_REQUESTED.eq(&win_com::Guid::from_bytes([
            0xA3, 0xC6, 0xE1, 0x15, 0x2A, 0xC7, 0x40, 0x4D, 0x91, 0xAE, 0x36, 0x93, 0x4B, 0x67, 0x23, 0xD5
        ])));

        // Mock EventArgs with vtable matching ICoreWebView2PermissionRequestedEventArgs
        #[repr(C)]
        struct MockEventArgsVtbl {
            qi: unsafe extern "system" fn(*mut c_void, *const win_com::Guid, *mut *mut c_void) -> i32,
            add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
            release: unsafe extern "system" fn(*mut c_void) -> u32,
            get_uri: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32,
            get_permission_kind: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
            get_is_user_initiated: unsafe extern "system" fn(*mut c_void, *mut i32) -> i32,
            get_state: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
            put_state: unsafe extern "system" fn(*mut c_void, u32) -> i32,
        }

        #[repr(C)]
        struct MockEventArgs {
            vtbl: *const MockEventArgsVtbl,
            kind: u32,
            state: AtomicU32,
        }

        unsafe extern "system" fn mock_qi(_: *mut c_void, _: *const win_com::Guid, _: *mut *mut c_void) -> i32 { 0 }
        unsafe extern "system" fn mock_add_ref(_: *mut c_void) -> u32 { 1 }
        unsafe extern "system" fn mock_release(_: *mut c_void) -> u32 { 1 }
        unsafe extern "system" fn mock_get_uri(_: *mut c_void, _: *mut *mut u16) -> i32 { 0 }
        unsafe extern "system" fn mock_get_kind(this: *mut c_void, out: *mut u32) -> i32 {
            let obj = &*(this as *const MockEventArgs);
            *out = obj.kind;
            0
        }
        unsafe extern "system" fn mock_get_user_init(_: *mut c_void, _: *mut i32) -> i32 { 0 }
        unsafe extern "system" fn mock_get_state(this: *mut c_void, out: *mut u32) -> i32 {
            let obj = &*(this as *const MockEventArgs);
            *out = obj.state.load(Ordering::SeqCst);
            0
        }
        unsafe extern "system" fn mock_put_state(this: *mut c_void, state: u32) -> i32 {
            let obj = &*(this as *const MockEventArgs);
            obj.state.store(state, Ordering::SeqCst);
            0
        }

        static MOCK_VTBL: MockEventArgsVtbl = MockEventArgsVtbl {
            qi: mock_qi,
            add_ref: mock_add_ref,
            release: mock_release,
            get_uri: mock_get_uri,
            get_permission_kind: mock_get_kind,
            get_is_user_initiated: mock_get_user_init,
            get_state: mock_get_state,
            put_state: mock_put_state,
        };

        let mock_args = MockEventArgs {
            vtbl: &MOCK_VTBL,
            kind: win_com::COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
            state: AtomicU32::new(0),
        };

        let (tx, rx) = std::sync::mpsc::channel();
        let invoke_with = |allowed: bool| {
            win_com::set_mic_permission_allowed(allowed);
            mock_args.state.store(0, Ordering::SeqCst);
            let cb = win_com::alloc_com_callback(
                win_com::CallbackKind::PermissionRequested,
                win_com::IID_PERMISSION_REQUESTED,
                tx.clone(),
            );
            unsafe {
                let vtbl = *(cb as *const *const *const c_void);
                let invoke_fn: unsafe extern "system" fn(*mut c_void, isize, *mut c_void) -> i32 =
                    std::mem::transmute(*vtbl.add(3));
                let hr = invoke_fn(cb, 0, &mock_args as *const MockEventArgs as *mut c_void);
                assert_eq!(hr, 0);
                win_com::com_release(cb);
            }
        };

        // Opt-In default (false): must DENY microphone (2)
        invoke_with(false);
        assert_eq!(
            mock_args.state.load(Ordering::SeqCst),
            win_com::COREWEBVIEW2_PERMISSION_STATE_DENY
        );
        let denied_sig = rx.recv().expect("must receive denied signal");
        assert!(matches!(denied_sig, win_com::ComCallbackSignal::PermissionRequested(2, 2)));

        // Opt-In enabled (true): must ALLOW microphone (1)
        invoke_with(true);
        assert_eq!(
            mock_args.state.load(Ordering::SeqCst),
            win_com::COREWEBVIEW2_PERMISSION_STATE_ALLOW
        );
        let allowed_sig = rx.recv().expect("must receive allowed signal");
        assert!(matches!(allowed_sig, win_com::ComCallbackSignal::PermissionRequested(2, 1)));
        win_com::set_mic_permission_allowed(false);
    }
}
