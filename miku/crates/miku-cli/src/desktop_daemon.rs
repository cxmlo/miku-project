//! Step 6.5 resident Windows desktop daemon:
//! - First-run customizable `%LOCALAPPDATA%\Miku\ui\` scaffold without overwriting user edits
//! - Integration between System Tray (`win32_tray::DesktopTrayHost`), global hotkeys, and WebView2 pill
//! - Zero-orphan `LiveWebView2Window` + SQLite WAL clean shutdown

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use miku_gatekeeper::CanonicalPathJail;
use miku_gateway::http_loopback::resolve_default_ui_dir;
use miku_ui::{
    GlobalCursorTracker, LiveWebView2Config, LiveWebView2Window, WebView2SecurityPolicy,
    IDLE_COMPACT_HEIGHT_PX, IDLE_COMPACT_WIDTH_PX,
};
use serde_json::json;

use crate::live_gateway::{LiveGatewayConfig, LiveGatewayRunner};
use crate::live_ui_host::{
    build_island_ui_url, compute_centered_top_pill_bounds, handle_live_ui_events_with_tracker,
    open_path_or_url_in_shell, spawn_ctrl_c_flag, LiveIslandUiConfig,
};
use crate::{
    query_live_windows_webview2_version, run_doctor_suite, DoctorRunConfig, WebView2ProbeSource,
};
pub use crate::win32_tray::{TrayDaemonAction, TrayRegistrationReport};

#[cfg(target_os = "windows")]
pub use crate::win32_tray::DesktopTrayHost;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstRunScaffoldReport {
    pub ui_override_dir: PathBuf,
    pub agents_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub created_files: Vec<PathBuf>,
    pub preserved_existing_files: Vec<PathBuf>,
}

const DEFAULT_README_CUSTOMIZE_ES: &str = "\
Miku Desktop — Carpeta Personalizable de Usuario\r\n\
=================================================\r\n\
\r\n\
Estructura modular: shared/ (comun), island/ (pildora), settings/ (ventana 1080x780).\r\n\
Edita o reemplaza solo los archivos que quieras personalizar (sin recompilar miku.exe):\r\n\
\r\n\
- shared/theme.css                 -> Colores, bordes, brillos, tipografia y radios.\r\n\
- shared/mascot/mascot.config.json -> Paleta, ojo, coletas, accesorios y presets del muneco.\r\n\
- shared/mascot/mascot.js          -> Reemplazo completo del avatar (window.MikuMascot).\r\n\
- shared/audio/sounds/manifest.json-> Mapea sonidos propios (.wav/.mp3/.ogg) por evento.\r\n\
- shared/locales/es.json           -> Sobrescribe textos de la interfaz.\r\n\
\r\n\
Atajos globales por defecto:\r\n\
- Alt + Space           -> Abrir / activar dictado de voz (PTT)\r\n\
- Ctrl + Alt + Shift + K-> Kill Switch de emergencia (pausa o reanuda agentes)\r\n";

const DEFAULT_THEME_CSS_FALLBACK: &str = "\
:root {\n  --ac: #f5f5f7;\n  --acx: #111111;\n  --sky: #e4e6eb;\n  --coral: #FF7A70;\n  --island-bg: #050609;\n  --card: #0e0f11;\n  --card-alt: #17181b;\n}\n";

const DEFAULT_MASCOT_CONFIG_FALLBACK: &str = "\
{\n  \"version\": 1,\n  \"schemaVersion\": 2,\n  \"name\": \"Miku User Override\",\n  \"defaultColor\": \"#ffffff\",\n  \"twintailsColors\": [\"#f4f6fb\", \"#cbd5e1\"],\n  \"enableTwintails\": false,\n  \"enableEyeTracking\": true,\n  \"enableIdleSleep\": true,\n  \"defaultOutfit\": \"none\"\n}\n";

pub fn default_user_custom_ui_dir() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("Miku")
        .join("ui")
}

fn read_bundled_asset_or_default(
    bundled_root: Option<&Path>,
    candidates: &[&[&str]],
    fallback: &str,
) -> String {
    if let Some(root) = bundled_root {
        for rel_parts in candidates {
            let mut p = root.to_path_buf();
            for seg in *rel_parts {
                p.push(seg);
            }
            if let Ok(content) = fs::read_to_string(&p) {
                return content;
            }
        }
    }
    fallback.to_string()
}

fn materialize_if_missing(
    jail: &CanonicalPathJail,
    rel_path: &str,
    content: &str,
    report: &mut FirstRunScaffoldReport,
) -> Result<(), String> {
    let resolved = jail
        .resolve_and_confine(rel_path)
        .map_err(|e| format!("unsafe scaffold path '{}': {:?}", rel_path, e))?;
    let target = PathBuf::from(resolved);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("create parent '{}': {}", parent.display(), e))?;
    }
    if target.exists() {
        report.preserved_existing_files.push(target);
        return Ok(());
    }
    fs::write(&target, content.as_bytes())
        .map_err(|e| format!("write scaffold file '{}': {}", target.display(), e))?;
    report.created_files.push(target);
    Ok(())
}

/// Upgrades an untouched Phase 6 `mascot.config.json` override that shipped with
/// Vocaloid blue twintails (`#62e8de`/`#25b1ab`) without overwriting custom user edits.
fn migrate_phase6_mascot_override_if_untouched(
    jail: &CanonicalPathJail,
    rel_path: &str,
) -> Result<(), String> {
    let resolved = jail
        .resolve_and_confine(rel_path)
        .map_err(|e| format!("unsafe mascot migration path: {:?}", e))?;
    let target = PathBuf::from(resolved);
    let Ok(existing) = fs::read_to_string(&target) else {
        return Ok(());
    };
    let mut parsed: serde_json::Value = match serde_json::from_str(&existing) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };
    let Some(obj) = parsed.as_object_mut() else {
        return Ok(());
    };
    if obj.contains_key("schemaVersion") {
        return Ok(());
    }
    let is_legacy_blue_tails = obj
        .get("enableTwintails")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        && obj
            .get("twintailsColors")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .map(|hex| hex.eq_ignore_ascii_case("#62e8de"))
            .unwrap_or(false);
    if !is_legacy_blue_tails {
        return Ok(());
    }
    obj.insert("schemaVersion".to_string(), serde_json::json!(2));
    obj.insert("enableTwintails".to_string(), serde_json::json!(false));
    obj.insert(
        "twintailsColors".to_string(),
        serde_json::json!(["#f4f6fb", "#cbd5e1"]),
    );
    let updated = serde_json::to_string_pretty(&parsed)
        .map_err(|e| format!("serialize migrated mascot.config.json: {}", e))?;
    fs::write(&target, updated.as_bytes())
        .map_err(|e| format!("write migrated mascot.config.json: {}", e))?;
    Ok(())
}

/// Upgrades an untouched Phase 6 `theme.css` override (`--ac: #3B9EFF; --sky: #80C4FF;`)
/// to the monochrome `cxmlo/miku` palette without overwriting custom user themes.
fn migrate_phase6_theme_override_if_untouched(
    jail: &CanonicalPathJail,
    rel_path: &str,
    default_theme: &str,
) -> Result<(), String> {
    let resolved = jail
        .resolve_and_confine(rel_path)
        .map_err(|e| format!("unsafe theme migration path: {:?}", e))?;
    let target = PathBuf::from(resolved);
    let Ok(existing) = fs::read_to_string(&target) else {
        return Ok(());
    };
    let untouched_phase6 = (existing.contains("--ac: #3B9EFF;")
        || (existing.contains("Paleta principal monocromática") && !existing.contains("--ac-soft:")))
        && !existing.contains("custom");
    if !untouched_phase6 {
        return Ok(());
    }
    fs::write(&target, default_theme.as_bytes())
        .map_err(|e| format!("write migrated theme.css: {}", e))?;
    Ok(())
}

fn migrate_legacy_flat_file_to_shared(
    jail: &CanonicalPathJail,
    legacy_rel: &str,
    shared_rel: &str,
) -> Result<(), String> {
    let old_path = PathBuf::from(
        jail.resolve_and_confine(legacy_rel)
            .map_err(|e| format!("unsafe legacy path '{}': {:?}", legacy_rel, e))?,
    );
    let new_path = PathBuf::from(
        jail.resolve_and_confine(shared_rel)
            .map_err(|e| format!("unsafe shared path '{}': {:?}", shared_rel, e))?,
    );
    if old_path.is_file() && !new_path.exists() {
        if let Some(parent) = new_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create shared parent '{}': {}", parent.display(), e))?;
        }
        let _ = fs::copy(&old_path, &new_path);
    }
    Ok(())
}

/// Ensures `%LOCALAPPDATA%\Miku\ui\` modular folders (`shared/`, `island/`, `settings/`) and
/// starter templates exist safely without ever overwriting existing user modifications.
pub fn ensure_user_custom_ui_and_config(
    ui_override_dir: &Path,
    data_dir: &Path,
    bundled_ui_dir: Option<&Path>,
) -> Result<FirstRunScaffoldReport, String> {
    fs::create_dir_all(ui_override_dir)
        .map_err(|e| format!("create custom UI dir '{}': {}", ui_override_dir.display(), e))?;
    let agents_dir = data_dir.join("agents");
    let skills_dir = data_dir.join("skills");
    fs::create_dir_all(&agents_dir)
        .map_err(|e| format!("create agents dir '{}': {}", agents_dir.display(), e))?;
    fs::create_dir_all(&skills_dir)
        .map_err(|e| format!("create skills dir '{}': {}", skills_dir.display(), e))?;
    for sub in [
        ui_override_dir.join("shared").join("mascot"),
        ui_override_dir.join("shared").join("audio").join("sounds"),
        ui_override_dir.join("island"),
        ui_override_dir.join("settings"),
    ] {
        fs::create_dir_all(&sub)
            .map_err(|e| format!("create modular custom UI dir '{}': {}", sub.display(), e))?;
    }

    let jail = CanonicalPathJail::new(&ui_override_dir.to_string_lossy())
        .map_err(|e| format!("canonical jail for '{}': {:?}", ui_override_dir.display(), e))?;

    let mut report = FirstRunScaffoldReport {
        ui_override_dir: ui_override_dir.to_path_buf(),
        agents_dir,
        skills_dir,
        created_files: Vec::new(),
        preserved_existing_files: Vec::new(),
    };

    migrate_legacy_flat_file_to_shared(&jail, "theme.css", "shared/theme.css")?;
    migrate_phase6_mascot_override_if_untouched(&jail, "mascot/mascot.config.json")?;
    migrate_legacy_flat_file_to_shared(
        &jail,
        "mascot/mascot.config.json",
        "shared/mascot/mascot.config.json",
    )?;
    migrate_legacy_flat_file_to_shared(
        &jail,
        "audio/sounds/manifest.json",
        "shared/audio/sounds/manifest.json",
    )?;

    let default_theme = read_bundled_asset_or_default(
        bundled_ui_dir,
        &[&["shared", "theme.css"], &["theme.css"]],
        DEFAULT_THEME_CSS_FALLBACK,
    );
    let default_mascot_cfg = read_bundled_asset_or_default(
        bundled_ui_dir,
        &[
            &["shared", "mascot", "mascot.config.json"],
            &["mascot", "mascot.config.json"],
        ],
        DEFAULT_MASCOT_CONFIG_FALLBACK,
    );
    let default_sounds_manifest = read_bundled_asset_or_default(
        bundled_ui_dir,
        &[
            &["shared", "audio", "sounds", "manifest.json"],
            &["audio", "sounds", "manifest.json"],
        ],
        "{}\n",
    );

    materialize_if_missing(
        &jail,
        "README-PERSONALIZAR.txt",
        DEFAULT_README_CUSTOMIZE_ES,
        &mut report,
    )?;
    materialize_if_missing(&jail, "shared/theme.css", &default_theme, &mut report)?;
    migrate_phase6_theme_override_if_untouched(&jail, "shared/theme.css", &default_theme)?;
    materialize_if_missing(
        &jail,
        "shared/mascot/mascot.config.json",
        &default_mascot_cfg,
        &mut report,
    )?;
    migrate_phase6_mascot_override_if_untouched(&jail, "shared/mascot/mascot.config.json")?;
    materialize_if_missing(
        &jail,
        "shared/audio/sounds/manifest.json",
        &default_sounds_manifest,
        &mut report,
    )?;

    Ok(report)
}

/// Dispatches a tray/hotkey daemon command against the live WebView2 pill and gateway.
/// Returns `Ok(false)` when exiting cleanly.
pub fn dispatch_tray_action(
    action: TrayDaemonAction,
    win: &mut LiveWebView2Window,
    gateway: &LiveGatewayRunner,
    island_visible: &mut bool,
    ui_override_dir: &Path,
) -> Result<bool, String> {
    match action {
        TrayDaemonAction::ToggleIsland => {
            *island_visible = !*island_visible;
            win.set_visible(*island_visible);
        }
        TrayDaemonAction::OpenCustomUiFolder => {
            open_path_or_url_in_shell(&ui_override_dir.to_string_lossy());
        }
        TrayDaemonAction::OpenSetup => {
            gateway
                .api_handler()
                .request_open_settings(Some("keys".to_string()));
        }
        TrayDaemonAction::HotkeyPttWake => {
            *island_visible = true;
            win.set_visible(true);
            let ev = json!({ "type": "wake_ptt", "source": "hotkey" });
            let _ = win.post_web_message_json(&ev.to_string());
            gateway.api_handler().push_event(ev);
        }
        TrayDaemonAction::ToggleKillSwitch => {
            let active = gateway.api_handler().toggle_kill_switch();
            let ev = if active {
                json!({
                    "type": "alert",
                    "title": "Kill Switch Activado",
                    "body": "Agentes y herramientas detenidos. Usa el System Tray o Ctrl+Alt+Shift+K para reanudar."
                })
            } else {
                json!({
                    "type": "notify",
                    "title": "Kill Switch Reanudado",
                    "body": "Orquestador local operativo nuevamente."
                })
            };
            let _ = win.post_web_message_json(&ev.to_string());
            gateway.api_handler().push_event(ev);
        }
        TrayDaemonAction::RunDoctor => {
            let live_wv2 = query_live_windows_webview2_version();
            let cfg = DoctorRunConfig {
                webview2_source: WebView2ProbeSource::SimulatedVersion(
                    live_wv2.or_else(|| Some("131.0.2903.86".to_string())),
                ),
                ..DoctorRunConfig::default()
            };
            let rep = run_doctor_suite(&cfg);
            let ev = json!({
                "type": "notify",
                "title": "Miku Doctor",
                "body": format!("Estado={:?} (pass={}, warn={}, exit={})", rep.overall_status, rep.passed_count, rep.warn_count, rep.exit_code)
            });
            *island_visible = true;
            win.set_visible(true);
            let _ = win.post_web_message_json(&ev.to_string());
            gateway.api_handler().push_event(ev);
        }
        TrayDaemonAction::Exit => return Ok(false),
    }
    Ok(true)
}

/// Runs Miku as a full desktop daemon: first-run customizable UI scaffold, loopback gateway,
/// live rounded WebView2 Dynamic Island pill, System Tray icon + menu, and global hotkeys.
pub fn run_resident_desktop_daemon(mut config: LiveIslandUiConfig) -> Result<(), String> {
    let ui_override_dir = config
        .user_override_dir
        .clone()
        .unwrap_or_else(default_user_custom_ui_dir);
    let bundled_probe = config
        .default_ui_dir
        .clone()
        .unwrap_or_else(resolve_default_ui_dir);
    let bundled_opt = if bundled_probe.is_dir() {
        Some(bundled_probe.as_path())
    } else {
        None
    };

    let scaffold =
        ensure_user_custom_ui_and_config(&ui_override_dir, &config.data_dir, bundled_opt)?;
    config.user_override_dir = Some(scaffold.ui_override_dir.clone());

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

    let needs_onboarding = !gateway.api_handler().has_any_api_key();
    let mut url = build_island_ui_url(&gateway);
    if needs_onboarding {
        url.push_str("&onboarding=1");
    }

    let bounds = compute_centered_top_pill_bounds(
        IDLE_COMPACT_WIDTH_PX as i32,
        IDLE_COMPACT_HEIGHT_PX as i32,
    );
    let mut win = LiveWebView2Window::open(LiveWebView2Config {
        url,
        auth_token: Some(gateway.session_token.clone()),
        bounds,
        security_policy: WebView2SecurityPolicy::release_hardened(),
        user_data_dir_override: config.user_data_dir_override.clone(),
        transparent_bg: true,
    })
    .map_err(|e| format!("open WebView2 daemon island: {}", e))?;

    #[cfg(target_os = "windows")]
    let mut tray = DesktopTrayHost::start()?;
    #[cfg(target_os = "windows")]
    {
        let stt_enabled = gateway.api_handler().is_voice_stt_enabled();
        miku_ui::webview::set_webview_mic_permission_allowed(stt_enabled);
        tray.sync_ptt_hotkey(stt_enabled);
    }
    #[cfg(target_os = "windows")]
    let tray_info = tray.report().clone();
    #[cfg(not(target_os = "windows"))]
    let tray_info = TrayRegistrationReport {
        tray_icon_active: false,
        ptt_hotkey_registered: false,
        kill_switch_hotkey_registered: false,
        warnings: Vec::new(),
    };

    println!(
        "MIKU_DAEMON_READY|addr={}|ui_dir={}|onboarding={}|tray={}|ptt_hotkey={}|ks_hotkey={}",
        gateway.local_addr,
        scaffold.ui_override_dir.display(),
        needs_onboarding,
        tray_info.tray_icon_active,
        tray_info.ptt_hotkey_registered,
        tray_info.kill_switch_hotkey_registered
    );
    for w in &tray_info.warnings {
        eprintln!("MIKU_TRAY_WARN: {}", w);
    }

    let mut settings_coord = crate::settings_window::SettingsWindowCoordinator::new(
        &gateway,
        config.user_data_dir_override.clone(),
    );
    let mut cursor_tracker = GlobalCursorTracker::new();
    let mut island_visible = true;
    let stop = spawn_ctrl_c_flag();
    while !stop.load(Ordering::Relaxed) {
        let ui_events = win.pump_events();
        if !handle_live_ui_events_with_tracker(
            &mut win,
            &ui_events,
            &gateway,
            Some(&mut cursor_tracker),
        )? {
            break;
        }
        let _ = cursor_tracker.poll_and_dispatch(&mut win);
        settings_coord.pump(&gateway)?;
        #[cfg(target_os = "windows")]
        {
            let stt_enabled = gateway.api_handler().is_voice_stt_enabled();
            miku_ui::webview::set_webview_mic_permission_allowed(stt_enabled);
            tray.sync_ptt_hotkey(stt_enabled);
            let tray_actions = tray.poll_actions();
            let mut should_exit = false;
            for act in tray_actions {
                if !dispatch_tray_action(
                    act,
                    &mut win,
                    &gateway,
                    &mut island_visible,
                    &scaffold.ui_override_dir,
                )? {
                    should_exit = true;
                    break;
                }
            }
            if should_exit {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(12));
    }

    #[cfg(target_os = "windows")]
    tray.shutdown();
    settings_coord.release_window();
    let orphans = win.close_and_drain();
    gateway.shutdown_cleanly()?;
    if !orphans.is_empty() {
        return Err(format!("orphan msedgewebview2 processes detected: {:?}", orphans));
    }
    Ok(())
}
