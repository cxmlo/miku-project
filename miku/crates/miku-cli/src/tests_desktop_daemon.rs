use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use miku_ui::{
    LiveWebView2Config, LiveWebView2Window, WebView2SecurityPolicy, IDLE_COMPACT_HEIGHT_PX,
    IDLE_COMPACT_WIDTH_PX,
};

use crate::desktop_daemon::{
    dispatch_tray_action, ensure_user_custom_ui_and_config, DesktopTrayHost, TrayDaemonAction,
};
use crate::live_gateway::{LiveGatewayConfig, LiveGatewayRunner};
use crate::live_ui_host::{
    build_island_ui_url, compute_centered_top_pill_bounds, handle_live_ui_events, CLI_LIVE_WV2_LOCK,
};

fn temp_daemon_dir(tag: &str) -> PathBuf {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let p = env::temp_dir().join(format!("miku_6_5_{}_{}_{}", tag, std::process::id(), ns));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).expect("create temp daemon dir");
    p
}

fn bundled_ui_dir_for_test() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("ui")
}

fn http_get_raw(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect loopback");
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    );
    stream.write_all(req.as_bytes()).expect("write request");
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
}

#[test]
fn test_6_5_first_run_scaffold_materializes_custom_ui_and_default_config_safely() {
    let root = temp_daemon_dir("scaffold");
    let ui_dir = root.join("ui");
    let data_dir = root.join("data");
    let bundled = bundled_ui_dir_for_test();

    let first =
        ensure_user_custom_ui_and_config(&ui_dir, &data_dir, Some(&bundled)).expect("scaffold 1");
    assert!(first.ui_override_dir.join("shared").join("theme.css").is_file());
    assert!(first
        .ui_override_dir
        .join("shared")
        .join("mascot")
        .join("mascot.config.json")
        .is_file());
    assert!(first
        .ui_override_dir
        .join("shared")
        .join("audio")
        .join("sounds")
        .join("manifest.json")
        .is_file());
    assert!(first.ui_override_dir.join("island").is_dir());
    assert!(first.ui_override_dir.join("settings").is_dir());
    assert!(first.ui_override_dir.join("README-PERSONALIZAR.txt").is_file());
    assert!(first.agents_dir.is_dir());
    assert!(first.skills_dir.is_dir());
    assert!(first.created_files.len() >= 4);
    assert!(first.preserved_existing_files.is_empty());

    // User customizes shared/theme.css; re-running scaffold MUST preserve user edits verbatim.
    let custom_css = "/* user-custom-neon */\n:root { --ac: #FF2E93; }\n";
    fs::write(ui_dir.join("shared").join("theme.css"), custom_css)
        .expect("write user theme override");

    let second =
        ensure_user_custom_ui_and_config(&ui_dir, &data_dir, Some(&bundled)).expect("scaffold 2");
    assert!(second.created_files.is_empty());
    assert_eq!(second.preserved_existing_files.len(), 4);
    assert_eq!(
        fs::read_to_string(ui_dir.join("shared").join("theme.css")).unwrap(),
        custom_css
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_6_5_daemon_tray_hotkeys_and_kill_switch_clean_shutdown() {
    let _guard = CLI_LIVE_WV2_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_daemon_dir("daemon");
    let ui_dir = root.join("ui");
    let data_dir = root.join("data");
    let wv2_dir = root.join("wv2");
    let bundled = bundled_ui_dir_for_test();
    fs::create_dir_all(&wv2_dir).unwrap();

    let scaffold =
        ensure_user_custom_ui_and_config(&ui_dir, &data_dir, Some(&bundled)).expect("scaffold");
    fs::write(
        ui_dir.join("shared").join("theme.css"),
        "/* custom-6-5 */\n:root{--ac:#00ffa6;}\n",
    )
    .unwrap();

    let gw = LiveGatewayRunner::start(LiveGatewayConfig {
        data_dir,
        port: 0,
        default_ui_dir: Some(bundled),
        user_override_dir: Some(scaffold.ui_override_dir.clone()),
        use_memory_secrets: true,
    })
    .expect("start gateway");

    assert!(!gw.api_handler().has_any_api_key());
    let shared_theme_resp = http_get_raw(gw.local_addr.port(), "/shared/theme.css");
    assert!(shared_theme_resp.contains("X-Miku-Ui-Source: user-override"));
    assert!(shared_theme_resp.contains("--ac:#00ffa6"));
    let legacy_theme_resp = http_get_raw(gw.local_addr.port(), "/theme.css");
    assert!(legacy_theme_resp.contains("X-Miku-Ui-Source: user-override"));
    assert!(legacy_theme_resp.contains("--ac:#00ffa6"));

    let mut url = build_island_ui_url(&gw);
    url.push_str("&onboarding=1");

    let wv2_dir_for_settings = wv2_dir.clone();
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
    .expect("open daemon pill");
    let mut settings_coord = crate::settings_window::SettingsWindowCoordinator::new(
        &gw,
        Some(wv2_dir_for_settings),
    );

    #[cfg(target_os = "windows")]
    let mut tray = DesktopTrayHost::start().expect("start tray daemon host");

    let deadline = Instant::now() + Duration::from_secs(6);
    while !win.is_controller_ready() && Instant::now() < deadline {
        let evs = win.pump_events();
        assert!(handle_live_ui_events(&mut win, &evs, &gw).unwrap());
        #[cfg(target_os = "windows")]
        let _ = tray.poll_actions();
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(win.is_controller_ready(), "daemon island must become ready");

    let mut visible = true;
    assert!(dispatch_tray_action(
        TrayDaemonAction::HotkeyPttWake,
        &mut win,
        &gw,
        &mut visible,
        &ui_dir
    )
    .unwrap());
    assert!(visible);

    assert!(dispatch_tray_action(
        TrayDaemonAction::ToggleKillSwitch,
        &mut win,
        &gw,
        &mut visible,
        &ui_dir
    )
    .unwrap());
    assert!(gw.api_handler().is_kill_switch_active());

    assert!(dispatch_tray_action(
        TrayDaemonAction::ToggleKillSwitch,
        &mut win,
        &gw,
        &mut visible,
        &ui_dir
    )
    .unwrap());
    assert!(!gw.api_handler().is_kill_switch_active());

    assert!(dispatch_tray_action(
        TrayDaemonAction::ToggleIsland,
        &mut win,
        &gw,
        &mut visible,
        &ui_dir
    )
    .unwrap());
    assert!(!visible);

    assert!(dispatch_tray_action(
        TrayDaemonAction::OpenSetup,
        &mut win,
        &gw,
        &mut visible,
        &ui_dir
    )
    .unwrap());
    settings_coord.pump(&gw).expect("pump opens settings window");
    assert!(settings_coord.is_open_in_memory());
    assert!(settings_coord.is_visible());
    assert_eq!(settings_coord.bounds().unwrap().width, 1080);
    assert_eq!(settings_coord.bounds().unwrap().height, 780);

    let set_deadline = Instant::now() + Duration::from_secs(6);
    while !settings_coord.is_controller_ready() && Instant::now() < set_deadline {
        let _ = win.pump_events();
        settings_coord.pump(&gw).expect("pump settings ready");
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(
        settings_coord.is_controller_ready(),
        "independent Settings WebView2 controller must initialize cleanly alongside Island"
    );

    // Hide on close -> memory trimmed -> Reopen second time without blank window
    assert!(handle_live_ui_events(
        &mut win,
        &[miku_ui::LiveWebView2Event::WebMessageJson(
            r#"{"k":"open_settings","section":"system"}"#.to_string(),
        )],
        &gw,
    )
    .unwrap());
    settings_coord.pump(&gw).expect("focus settings via open_settings");
    assert!(settings_coord.is_visible());

    assert!(!dispatch_tray_action(
        TrayDaemonAction::Exit,
        &mut win,
        &gw,
        &mut visible,
        &ui_dir
    )
    .unwrap());

    #[cfg(target_os = "windows")]
    tray.shutdown();

    settings_coord.release_window();
    let orphans = win.close_and_drain();
    assert!(
        orphans.is_empty(),
        "expected 0 orphan msedgewebview2 processes, found {:?}",
        orphans
    );
    gw.shutdown_cleanly().expect("shutdown gateway");
    let _ = fs::remove_dir_all(&root);
}

// LEDGER-7.1-1
#[test]
fn test_7_1_migrates_untouched_phase6_mascot_override_without_clobbering_custom() {
    let root = temp_daemon_dir("migrate_mascot_7_1");
    let ui_dir = root.join("ui");
    let data_dir = root.join("data");
    let bundled = bundled_ui_dir_for_test();
    fs::create_dir_all(ui_dir.join("mascot")).expect("mkdir mascot");
    let legacy_cfg = r##"{
  "version": 1,
  "defaultColor": "#ffffff",
  "twintailsColors": ["#62e8de", "#25b1ab"],
  "enableTwintails": true,
  "defaultOutfit": "none"
}"##;
    fs::write(ui_dir.join("mascot").join("mascot.config.json"), legacy_cfg)
        .expect("write legacy cfg");

    ensure_user_custom_ui_and_config(&ui_dir, &data_dir, Some(&bundled))
        .expect("scaffold + migration");
    let after: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ui_dir.join("mascot").join("mascot.config.json")).expect("read after"),
    )
    .expect("json after");
    assert_eq!(after["schemaVersion"], 2);
    assert_eq!(after["enableTwintails"], false);
    assert_eq!(after["twintailsColors"][0], "#f4f6fb");

    let custom_cfg = r##"{
  "version": 1,
  "schemaVersion": 2,
  "defaultColor": "#ffd1dc",
  "twintailsColors": ["#ff8fb1", "#f43f5e"],
  "enableTwintails": true,
  "defaultOutfit": "crown"
}"##;
    fs::write(ui_dir.join("mascot").join("mascot.config.json"), custom_cfg)
        .expect("write custom cfg");
    ensure_user_custom_ui_and_config(&ui_dir, &data_dir, Some(&bundled))
        .expect("preserve custom cfg");
    let preserved: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(ui_dir.join("mascot").join("mascot.config.json")).expect("read custom"),
    )
    .expect("json custom");
    assert_eq!(preserved["enableTwintails"], true);
    assert_eq!(preserved["defaultOutfit"], "crown");
    assert_eq!(preserved["twintailsColors"][0], "#ff8fb1");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_8_2_migrates_untouched_phase6_theme_css_without_clobbering_custom() {
    let root = temp_daemon_dir("theme_migrate_8_2");
    let ui_dir = root.join("ui");
    let data_dir = root.join("data");
    let bundled = bundled_ui_dir_for_test();
    fs::create_dir_all(ui_dir.join("shared")).expect("mkdir shared");
    let untouched_phase6_css = "\
/* Miku Dynamic Island — Archivo de Tema Personalizable (theme.css) */\n\
:root {\n  --ac: #3B9EFF;\n  --sky: #80C4FF;\n  --card: #08090d;\n}\n";
    let theme_path = ui_dir.join("shared").join("theme.css");
    fs::write(&theme_path, untouched_phase6_css).expect("write untouched phase6 css");

    ensure_user_custom_ui_and_config(&ui_dir, &data_dir, Some(&bundled))
        .expect("scaffold + theme migration");
    let migrated = fs::read_to_string(&theme_path).expect("read migrated theme");
    assert!(migrated.contains("--ac: #f5f5f7;"), "migrated css: {}", migrated);
    assert!(!migrated.contains("--ac: #3B9EFF;"));

    let custom_user_css = "/* custom user palette */\n:root {\n  --ac: #00ffa6;\n  --sky: #80C4FF;\n}\n";
    fs::write(&theme_path, custom_user_css).expect("write custom user css");
    ensure_user_custom_ui_and_config(&ui_dir, &data_dir, Some(&bundled))
        .expect("preserve custom theme");
    let preserved = fs::read_to_string(&theme_path).expect("read preserved custom");
    assert_eq!(preserved, custom_user_css);

    let _ = fs::remove_dir_all(&root);
}
