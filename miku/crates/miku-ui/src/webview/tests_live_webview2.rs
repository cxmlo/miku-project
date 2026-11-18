#![allow(unused_imports)]
use crate::prelude::*;
use crate::webview::{LiveWebView2Config, LiveWebView2Event, LiveWebView2Window};

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn temp_wv2_root(tag: &str) -> PathBuf {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let p = std::env::temp_dir().join(format!("miku_wv2_6_4_{}_{}_{}", tag, std::process::id(), ns));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).expect("create temp wv2 dir");
    p
}

fn spawn_single_shot_ui_server(ui_html: &'static str) -> (u16, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback ui server");
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).ok();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_srv = Arc::clone(&stop);

    let j = std::thread::spawn(move || {
        while !stop_srv.load(Ordering::Relaxed) {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    ui_html.len(),
                    ui_html
                );
                let _ = s.write_all(resp.as_bytes());
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    (port, stop, j)
}

#[test]
fn test_6_4_live_webview2_window_mounts_modular_ui_resizes_and_cleans_ttl_zero_orphans() {
    let _wv2_guard = super::WEBVIEW2_CHILD_PROC_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let temp = temp_wv2_root("mount");
    let (port, stop_srv, srv_join) = spawn_single_shot_ui_server(
        "<!doctype html><html><body><div id='isl'>Miku Live</div><script>window.chrome?.webview?.postMessage({k:'ready'});</script></body></html>",
    );

    let url = format!("http://127.0.0.1:{}/", port);
    let mut win = LiveWebView2Window::open(LiveWebView2Config {
        url,
        auth_token: Some("live-wv2-session-tok".to_string()),
        bounds: PillAnchorGeometry {
            x: 800,
            y: 8,
            width: IDLE_COMPACT_WIDTH_PX as i32,
            height: IDLE_COMPACT_HEIGHT_PX as i32,
            corner_radius_px: IDLE_CORNER_RADIUS_PX as i32,
            dpi: STANDARD_DPI,
            scale_pct: 100,
        },
        security_policy: WebView2SecurityPolicy::release_hardened(),
        user_data_dir_override: Some(temp.clone()),
        transparent_bg: true,
    })
    .expect("open LiveWebView2Window");

    assert!(win.hwnd_raw() != 0);
    assert!(
        win.user_data_dir()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("webview2"),
        "user data folder must be isolated under miku/webview2"
    );

    let deadline = Instant::now() + Duration::from_secs(6);
    while !win.is_controller_ready() && Instant::now() < deadline {
        let events = win.pump_events();
        if events
            .iter()
            .any(|e| matches!(e, LiveWebView2Event::ControllerError(_)))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }

    assert!(
        win.is_controller_ready(),
        "ICoreWebView2Controller must initialize cleanly on Windows WebView2 runtime"
    );

    let msg_deadline = Instant::now() + Duration::from_secs(4);
    let mut saw_ready_ipc = false;
    while !saw_ready_ipc && Instant::now() < msg_deadline {
        for ev in win.pump_events() {
            if let LiveWebView2Event::WebMessageJson(json) = ev {
                if json.contains("ready") {
                    saw_ready_ipc = true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(
        saw_ready_ipc,
        "WebView2 page must expose window.chrome.webview and deliver WebMessageJson"
    );

    // Fluid resize cycle: compact -> overview -> compact
    win.resize_bounds(PillAnchorGeometry {
        x: 580,
        y: 8,
        width: 760,
        height: 240,
        corner_radius_px: 28,
        dpi: STANDARD_DPI,
        scale_pct: 100,
    })
    .expect("resize expanded");
    win.pump_events();
    assert_eq!(win.bounds().width, 760);

    win.resize_bounds(PillAnchorGeometry {
        x: 800,
        y: 8,
        width: IDLE_COMPACT_WIDTH_PX as i32,
        height: IDLE_COMPACT_HEIGHT_PX as i32,
        corner_radius_px: IDLE_CORNER_RADIUS_PX as i32,
        dpi: STANDARD_DPI,
        scale_pct: 100,
    })
    .expect("resize compact");
    assert_eq!(win.bounds().width, IDLE_COMPACT_WIDTH_PX as i32);

    // Deterministic close + zero orphan child processes
    let orphans = win.close_and_drain();
    assert!(
        orphans.is_empty(),
        "expected 0 orphan msedgewebview2.exe processes after close, found {:?}",
        orphans
    );

    stop_srv.store(true, Ordering::Relaxed);
    let _ = srv_join.join();
    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_6_4_idle_pill_and_ttl_teardown_preserve_budgets() {
    let _wv2_guard = super::WEBVIEW2_CHILD_PROC_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let temp = temp_wv2_root("budgets");
    let bounds = PillAnchorGeometry {
        x: 836,
        y: 8,
        width: IDLE_COMPACT_WIDTH_PX as i32,
        height: IDLE_COMPACT_HEIGHT_PX as i32,
        corner_radius_px: IDLE_CORNER_RADIUS_PX as i32,
        dpi: STANDARD_DPI,
        scale_pct: 100,
    };
    let telemetry = run_win32_com_webview2_cycle(Some(&temp), bounds).expect("run win32 com cycle");
    assert!(telemetry.hwnd_raw != 0);
    assert!(telemetry.ex_style_verified);
    assert!(telemetry.ma_noactivate_verified);
    assert!(telemetry.foreground_focus_preserved);
    assert!(telemetry.live_webview2_child_pids.is_empty());
    assert!(telemetry.expand_latency_ms <= MAX_EXPAND_VISIBLE_LATENCY_MS);
    assert!(telemetry.process_private_mb > 0.5 && telemetry.process_private_mb < 64.0);
    let _ = fs::remove_dir_all(&temp);
}
