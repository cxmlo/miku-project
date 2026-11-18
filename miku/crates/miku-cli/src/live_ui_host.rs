//! Step 6.4 live desktop island UI host connecting `LiveGatewayRunner` (`127.0.0.1:47400`)
//! to `miku_ui::LiveWebView2Window` with bidirectional IPC resizing, safe URL handoff, and
//! deterministic zero-orphan WebView2 + SQLite WAL teardown.

use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use miku_gateway::DEFAULT_LOOPBACK_PORT;
use miku_ui::{
    GlobalCursorTracker, IslandHitRect, LiveWebView2Config, LiveWebView2Event,
    LiveWebView2Window, PillAnchorGeometry, WebView2SecurityPolicy, IDLE_COMPACT_HEIGHT_PX,
    IDLE_COMPACT_WIDTH_PX, IDLE_CORNER_RADIUS_PX, STANDARD_DPI,
};
use serde_json::Value;

use crate::live_gateway::{LiveGatewayConfig, LiveGatewayRunner};

#[cfg(target_os = "windows")]
#[link(name = "user32")]
extern "system" {
    fn GetSystemMetrics(n_index: i32) -> i32;
}

#[cfg(target_os = "windows")]
#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut std::ffi::c_void,
        lp_operation: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show_cmd: i32,
    ) -> isize;
}

#[derive(Debug, Clone)]
pub struct LiveIslandUiConfig {
    pub port: u16,
    pub data_dir: PathBuf,
    pub user_override_dir: Option<PathBuf>,
    pub default_ui_dir: Option<PathBuf>,
    pub user_data_dir_override: Option<PathBuf>,
    pub use_memory_secrets: bool,
    pub initial_width: i32,
    pub initial_height: i32,
}

impl Default for LiveIslandUiConfig {
    fn default() -> Self {
        let local_app = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        Self {
            port: DEFAULT_LOOPBACK_PORT,
            data_dir: local_app.join("Miku").join("data"),
            user_override_dir: None,
            default_ui_dir: None,
            user_data_dir_override: None,
            use_memory_secrets: false,
            initial_width: IDLE_COMPACT_WIDTH_PX as i32,
            initial_height: IDLE_COMPACT_HEIGHT_PX as i32,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiveUiBridgeCommand {
    ResizeWindow {
        width: i32,
        height: i32,
    },
    CursorRegion {
        outside: bool,
        compact: Option<bool>,
        rect: Option<IslandHitRect>,
    },
    MascotPower {
        awake: bool,
    },
    OpenExternal { url: String },
    OpenSettings { section: Option<String> },
    WindowControl { action: String },
    StartDrag,
    ToggleKillSwitch,
    Shutdown,
}

/// Parses `--port`, `--data-dir`, and `--ui-dir` flags for `serve`, `ui`, and `daemon` modes.
pub fn parse_ui_cli_args(args: &[String]) -> LiveIslandUiConfig {
    let mut cfg = LiveIslandUiConfig::default();
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--port" if i + 1 < args.len() => {
                if let Ok(p) = args[i + 1].parse::<u16>() {
                    cfg.port = p;
                }
                i += 2;
                continue;
            }
            "--data-dir" if i + 1 < args.len() => {
                cfg.data_dir = PathBuf::from(&args[i + 1]);
                i += 2;
                continue;
            }
            "--ui-dir" if i + 1 < args.len() => {
                cfg.user_override_dir = Some(PathBuf::from(&args[i + 1]));
                i += 2;
                continue;
            }
            _ => i += 1,
        }
    }
    cfg
}

/// Computes centered top-pill bounds using primary Windows display width or `1920` fallback.
pub fn compute_centered_top_pill_bounds(width: i32, height: i32) -> PillAnchorGeometry {
    let w = width.clamp(180, 1280);
    let h = height.clamp(36, 920);
    #[cfg(target_os = "windows")]
    let screen_w = unsafe {
        let sx = GetSystemMetrics(0);
        if sx >= 640 {
            sx
        } else {
            1920
        }
    };
    #[cfg(not(target_os = "windows"))]
    let screen_w = 1920;

    let corner = if h <= 60 {
        IDLE_CORNER_RADIUS_PX as i32
    } else {
        28
    };
    PillAnchorGeometry {
        x: ((screen_w - w) / 2).max(0),
        y: 0,
        width: w,
        height: h,
        corner_radius_px: corner,
        dpi: STANDARD_DPI,
        scale_pct: 100,
    }
}

/// Validates external URLs strictly before OS shell handoff (no quotes, spaces, or `file:`/`javascript:`).
pub fn is_safe_external_url(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty()
        || t.contains('"')
        || t.contains('\'')
        || t.contains('\\')
        || t.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://127.0.0.1:")
        || lower.starts_with("http://localhost:")
}

/// Parses incoming `window.chrome.webview.postMessage(...)` JSON envelopes from the modular JS bridge.
pub fn parse_live_ui_bridge_command(raw_json: &str) -> Option<LiveUiBridgeCommand> {
    let parsed: Value = serde_json::from_str(raw_json).ok()?;
    let kind = parsed.get("k")?.as_str()?;
    match kind {
        "resize_window" => {
            let width = parsed.get("width")?.as_i64()? as i32;
            let height = parsed.get("height")?.as_i64()? as i32;
            Some(LiveUiBridgeCommand::ResizeWindow {
                width: width.clamp(180, 1280),
                height: height.clamp(36, 920),
            })
        }
        "cursor_region" => {
            let outside = parsed.get("outside")?.as_bool()?;
            let compact = parsed.get("compact").and_then(|v| v.as_bool());
            let rect = parsed.get("rect").and_then(|r| {
                Some(IslandHitRect {
                    left: r.get("left")?.as_f64()?,
                    top: r.get("top")?.as_f64()?,
                    right: r.get("right")?.as_f64()?,
                    bottom: r.get("bottom")?.as_f64()?,
                })
            });
            Some(LiveUiBridgeCommand::CursorRegion {
                outside,
                compact,
                rect,
            })
        }
        "mascot_power" => {
            let awake = parsed.get("awake")?.as_bool()?;
            Some(LiveUiBridgeCommand::MascotPower { awake })
        }
        "open_external" => {
            let url = parsed.get("url")?.as_str()?.trim().to_string();
            if is_safe_external_url(&url) {
                Some(LiveUiBridgeCommand::OpenExternal { url })
            } else {
                None
            }
        }
        "open_settings" => {
            let section = parsed
                .get("section")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Some(LiveUiBridgeCommand::OpenSettings { section })
        }
        "window_control" => {
            let action = parsed.get("action")?.as_str()?.trim().to_ascii_lowercase();
            Some(LiveUiBridgeCommand::WindowControl { action })
        }
        "start_drag" => Some(LiveUiBridgeCommand::StartDrag),
        "kill_switch" => Some(LiveUiBridgeCommand::ToggleKillSwitch),
        "shutdown" => Some(LiveUiBridgeCommand::Shutdown),
        _ => None,
    }
}

/// Launches a pre-validated URL or local directory with Win32 `ShellExecuteW` (`SW_SHOWNORMAL = 1`).
pub fn open_path_or_url_in_shell(target: &str) {
    #[cfg(target_os = "windows")]
    unsafe {
        let op: Vec<u16> = "open\0".encode_utf16().collect();
        let file: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
        let _ = ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
        );
    }
    #[cfg(not(target_os = "windows"))]
    let _ = target;
}

/// Dispatches WebView2 events against the live window and gateway. Returns `Ok(false)` on shutdown.
pub fn handle_live_ui_events(
    win: &mut LiveWebView2Window,
    events: &[LiveWebView2Event],
    gateway: &LiveGatewayRunner,
) -> Result<bool, String> {
    handle_live_ui_events_with_tracker(win, events, gateway, None)
}

/// Dispatches WebView2 events while keeping `GlobalCursorTracker` region/sleep state synchronized.
pub fn handle_live_ui_events_with_tracker(
    win: &mut LiveWebView2Window,
    events: &[LiveWebView2Event],
    gateway: &LiveGatewayRunner,
    mut tracker: Option<&mut GlobalCursorTracker>,
) -> Result<bool, String> {
    for ev in events {
        match ev {
            LiveWebView2Event::ControllerError(err) => {
                return Err(format!("WebView2 controller error: {}", err));
            }
            LiveWebView2Event::WebMessageJson(raw) => {
                if let Some(cmd) = parse_live_ui_bridge_command(raw) {
                    match cmd {
                        LiveUiBridgeCommand::ResizeWindow { width, height } => {
                            let next = compute_centered_top_pill_bounds(width, height);
                            win.resize_bounds(next).map_err(|e| e.to_string())?;
                        }
                        LiveUiBridgeCommand::CursorRegion {
                            outside,
                            compact,
                            rect,
                        } => {
                            if let Some(t) = tracker.as_deref_mut() {
                                t.update_region(compact, rect);
                            }
                            win.set_cursor_pass_through(outside);
                        }
                        LiveUiBridgeCommand::MascotPower { awake } => {
                            if let Some(t) = tracker.as_deref_mut() {
                                t.set_mascot_awake(awake);
                            }
                        }
                        LiveUiBridgeCommand::OpenExternal { url } => {
                            if is_safe_external_url(&url) {
                                open_path_or_url_in_shell(&url);
                            }
                        }
                        LiveUiBridgeCommand::OpenSettings { section } => {
                            gateway.api_handler().request_open_settings(section);
                        }
                        LiveUiBridgeCommand::WindowControl { action } => match action.as_str() {
                            "minimize" => {
                                win.set_interactive_focus(false, false);
                                win.minimize();
                            }
                            "hide" | "close" => {
                                win.set_interactive_focus(false, false);
                                win.hide_and_trim();
                            }
                            "focus" | "show" => win.restore_and_focus(),
                            "focus_input" => win.set_interactive_focus(true, false),
                            "activate_input" => win.set_interactive_focus(true, true),
                            "blur_input" => win.set_interactive_focus(false, false),
                            _ => {}
                        },
                        LiveUiBridgeCommand::StartDrag => {
                            win.start_drag();
                        }
                        LiveUiBridgeCommand::ToggleKillSwitch => {
                            let _ = gateway.api_handler().toggle_kill_switch();
                        }
                        LiveUiBridgeCommand::Shutdown => return Ok(false),
                    }
                }
            }
            LiveWebView2Event::ControllerReady => {}
            LiveWebView2Event::PermissionRequested { .. } => {}
        }
    }
    Ok(true)
}

/// Spawns a non-blocking Ctrl-C listener thread that flips `stop_flag` without blocking Win32 pump.
pub fn spawn_ctrl_c_flag() -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_tx = Arc::clone(&stop);
    let _ = std::thread::Builder::new()
        .name("miku-ctrlc-watcher".to_string())
        .spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                rt.block_on(async {
                    let _ = tokio::signal::ctrl_c().await;
                    stop_tx.store(true, Ordering::SeqCst);
                });
            }
        });
    stop
}

pub fn build_island_ui_url(gateway: &LiveGatewayRunner) -> String {
    format!(
        "http://{}/?core=1&token={}",
        gateway.local_addr, gateway.session_token
    )
}

/// Starts the live loopback gateway, opens the real Win32 WebView2 Dynamic Island window,
/// pumps bidirectional IPC events, and drains cleanly with 0 orphan child processes.
pub fn run_live_island_ui_mode(config: LiveIslandUiConfig) -> Result<(), String> {
    let gateway = LiveGatewayRunner::start_with_lan(
        LiveGatewayConfig {
            data_dir: config.data_dir.clone(),
            port: config.port,
            default_ui_dir: config.default_ui_dir.clone(),
            user_override_dir: config.user_override_dir.clone(),
            use_memory_secrets: config.use_memory_secrets,
        },
        true,
    )?;

    let url = build_island_ui_url(&gateway);
    let bounds = compute_centered_top_pill_bounds(config.initial_width, config.initial_height);
    let mut win = LiveWebView2Window::open(LiveWebView2Config {
        url: url.clone(),
        auth_token: Some(gateway.session_token.clone()),
        bounds,
        security_policy: WebView2SecurityPolicy::release_hardened(),
        user_data_dir_override: config.user_data_dir_override.clone(),
        transparent_bg: true,
    })
    .map_err(|e| format!("open WebView2 island: {}", e))?;

    println!(
        "MIKU_UI_READY|hwnd={}|addr={}|url=http://{}/",
        win.hwnd_raw(),
        gateway.local_addr,
        gateway.local_addr
    );

    miku_ui::webview::set_webview_mic_permission_allowed(
        gateway.api_handler().is_voice_stt_enabled(),
    );

    let mut settings_coord = crate::settings_window::SettingsWindowCoordinator::new(
        &gateway,
        config.user_data_dir_override.clone(),
    );
    let mut cursor_tracker = GlobalCursorTracker::new();
    let stop = spawn_ctrl_c_flag();
    while !stop.load(Ordering::Relaxed) {
        miku_ui::webview::set_webview_mic_permission_allowed(
            gateway.api_handler().is_voice_stt_enabled(),
        );
        let events = win.pump_events();
        if !handle_live_ui_events_with_tracker(&mut win, &events, &gateway, Some(&mut cursor_tracker))? {
            break;
        }
        let _ = cursor_tracker.poll_and_dispatch(&mut win);
        settings_coord.pump(&gateway)?;
        std::thread::sleep(Duration::from_millis(12));
    }

    settings_coord.release_window();
    let orphans = win.close_and_drain();
    gateway.shutdown_cleanly()?;
    if !orphans.is_empty() {
        return Err(format!("orphan msedgewebview2 processes detected: {:?}", orphans));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) static CLI_LIVE_WV2_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    fn temp_ui_test_dir(tag: &str) -> PathBuf {
        let ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = env::temp_dir().join(format!("miku_ui_host_{}_{}_{}", tag, std::process::id(), ns));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("create temp dir");
        p
    }

    #[test]
    fn test_6_4_live_ui_bridge_parses_resize_clamps_bounds_and_validates_external_urls() {
        assert_eq!(
            parse_live_ui_bridge_command(r#"{"k":"resize_window","width":760,"height":240}"#),
            Some(LiveUiBridgeCommand::ResizeWindow {
                width: 760,
                height: 240
            })
        );
        assert_eq!(
            parse_live_ui_bridge_command(r#"{"k":"resize_window","width":9999,"height":5}"#),
            Some(LiveUiBridgeCommand::ResizeWindow {
                width: 1280,
                height: 36
            })
        );
        assert_eq!(
            parse_live_ui_bridge_command(r#"{"k":"open_external","url":"https://docs.rs"}"#),
            Some(LiveUiBridgeCommand::OpenExternal {
                url: "https://docs.rs".to_string()
            })
        );
        assert_eq!(
            parse_live_ui_bridge_command(r#"{"k":"open_external","url":"javascript:alert(1)"}"#),
            None
        );
        assert_eq!(
            parse_live_ui_bridge_command(r#"{"k":"open_external","url":"file:///C:/Windows/win.ini"}"#),
            None
        );
        assert_eq!(
            parse_live_ui_bridge_command(
                r#"{"k":"cursor_region","outside":true,"compact":true,"rect":{"left":17,"top":0,"right":285,"bottom":44}}"#
            ),
            Some(LiveUiBridgeCommand::CursorRegion {
                outside: true,
                compact: Some(true),
                rect: Some(IslandHitRect {
                    left: 17.0,
                    top: 0.0,
                    right: 285.0,
                    bottom: 44.0,
                }),
            })
        );
        assert_eq!(
            parse_live_ui_bridge_command(r#"{"k":"mascot_power","awake":false}"#),
            Some(LiveUiBridgeCommand::MascotPower { awake: false })
        );
    }

    #[test]
    fn test_6_4_miku_ui_host_connects_gateway_resizes_and_drains_zero_orphans() {
        let _guard = CLI_LIVE_WV2_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = temp_ui_test_dir("connect");
        let data_dir = root.join("data");
        let wv2_dir = root.join("wv2");
        fs::create_dir_all(&data_dir).unwrap();
        fs::create_dir_all(&wv2_dir).unwrap();

        let gw = LiveGatewayRunner::start(LiveGatewayConfig {
            data_dir,
            port: 0,
            default_ui_dir: None,
            user_override_dir: None,
            use_memory_secrets: true,
        })
        .expect("start gateway");

        let url = build_island_ui_url(&gw);
        assert!(url.contains(&format!("http://{}/?core=1&token=", gw.local_addr)));

        let mut win = LiveWebView2Window::open(LiveWebView2Config {
            url,
            auth_token: Some(gw.session_token.clone()),
            bounds: compute_centered_top_pill_bounds(
                IDLE_COMPACT_WIDTH_PX as i32,
                IDLE_COMPACT_HEIGHT_PX as i32,
            ),
            security_policy: WebView2SecurityPolicy::release_hardened(),
            user_data_dir_override: Some(wv2_dir),
            transparent_bg: true,
        })
        .expect("open live webview2");

        let deadline = Instant::now() + Duration::from_secs(6);
        while !win.is_controller_ready() && Instant::now() < deadline {
            let events = win.pump_events();
            assert!(handle_live_ui_events(&mut win, &events, &gw).unwrap());
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(win.is_controller_ready(), "live WebView2 host must become ready");

        let keep_running = handle_live_ui_events(
            &mut win,
            &[LiveWebView2Event::WebMessageJson(
                r#"{"k":"resize_window","width":760,"height":240}"#.to_string(),
            )],
            &gw,
        )
        .expect("resize ipc");
        assert!(keep_running);
        assert_eq!(win.bounds().width, 760);
        assert_eq!(win.bounds().height, 240);

        let orphans = win.close_and_drain();
        assert!(orphans.is_empty(), "expected 0 orphans, got {:?}", orphans);
        gw.shutdown_cleanly().expect("shutdown gateway");
        let _ = fs::remove_dir_all(&root);
    }
}
