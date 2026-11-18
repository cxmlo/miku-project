use super::*;
use crate::audit::create_unique_isolated_audit_temp_dir;
use crate::UiError;
use std::fs;

// LEDGER-6.1-1
#[test]
fn test_6_1_custom_ui_tree_completeness_csp_and_zero_cdn_assets() {
    let base_dir = resolve_custom_ui_base_dir();
    let report = validate_custom_ui_tree(&base_dir).expect("custom UI tree validates");
    assert_eq!(report.file_count, REQUIRED_CUSTOM_UI_FILES.len());
    assert!(report.uses_strict_self_csp);
    assert!(!report.has_external_cdn_links);
    assert!(report.mascot_is_decoupled);
    assert!(report.json_files_validated >= 4);
}

// LEDGER-6.1-2
#[test]
fn test_6_1_user_override_resolution_and_path_traversal_jail() {
    let base_dir = resolve_custom_ui_base_dir();
    let override_dir = create_unique_isolated_audit_temp_dir("custom_ui_override_6_1", 1)
        .expect("override tempdir");
    let custom_mascot_dir = override_dir.join("mascot");
    fs::create_dir_all(&custom_mascot_dir).expect("mkdir custom mascot");
    let custom_cfg = r#"{"defaultOutfit":"crown","enableTwintails":false}"#;
    fs::write(custom_mascot_dir.join("mascot.config.json"), custom_cfg)
        .expect("write override mascot config");

    let resolver = CustomUiResolver::new(base_dir, Some(override_dir.clone()));

    let mascot_cfg = resolver
        .resolve_asset("mascot/mascot.config.json")
        .expect("override resolved");
    assert!(mascot_cfg.from_user_override);
    assert_eq!(mascot_cfg.content_type, "application/json; charset=utf-8");
    assert_eq!(String::from_utf8_lossy(&mascot_cfg.bytes), custom_cfg);

    let index_html = resolver.resolve_route("/").expect("route '/' resolves");
    assert!(!index_html.from_user_override);
    assert_eq!(index_html.relative_path, "index.html");
    assert_eq!(index_html.content_type, "text/html; charset=utf-8");

    let playground = resolver
        .resolve_route("/mascot")
        .expect("route '/mascot' resolves");
    assert_eq!(playground.relative_path, "mascot.html");

    for bad_path in [
        "../secrets.json",
        "..\\Windows\\System32\\drivers\\etc\\hosts",
        "/etc/passwd",
        "mascot/mascot.js:Zone.Identifier",
        "core/payload.exe",
        "%2e%2e/Cargo.toml",
    ] {
        let err = resolver
            .resolve_asset(bad_path)
            .expect_err("unsafe path must be rejected");
        assert!(
            matches!(err, UiError::UnsafeNavigationBlocked(_)),
            "expected UnsafeNavigationBlocked for '{}', got {:?}",
            bad_path,
            err
        );
    }

    let _ = fs::remove_dir_all(&override_dir);
}

// LEDGER-6.1-3
#[test]
fn test_6_1_mascot_module_decoupled_contract_and_no_hardcoded_python_demo() {
    let resolver = CustomUiResolver::from_defaults();

    let mascot_js = String::from_utf8(
        resolver
            .resolve_asset("mascot/mascot.js")
            .expect("mascot.js")
            .bytes,
    )
    .expect("utf8 mascot.js");
    for api_fn in [
        "mount",
        "setState",
        "emote",
        "setOutfit",
        "setColor",
        "lookAt",
        "setEatProgress",
        "renderFrame",
        "MikuMascot",
        "window.buddy",
    ] {
        assert!(
            mascot_js.contains(api_fn),
            "mascot.js missing public contract symbol '{}'",
            api_fn
        );
    }

    let bridge_js = String::from_utf8(
        resolver
            .resolve_asset("core/bridge.js")
            .expect("bridge.js")
            .bytes,
    )
    .expect("utf8 bridge.js");
    assert!(
        bridge_js.contains("47400"),
        "bridge.js must target Miku gateway port 47400"
    );
    assert!(
        bridge_js.contains("chrome.webview"),
        "bridge.js must support native WebView2 postMessage channel"
    );

    let theme_css = String::from_utf8(
        resolver
            .resolve_asset("theme.css")
            .expect("theme.css")
            .bytes,
    )
    .expect("utf8 theme.css");
    assert!(theme_css.contains("--ac:"));
    assert!(theme_css.contains("--island-bg:"));

    let es_json = String::from_utf8(
        resolver
            .resolve_asset("locales/es.json")
            .expect("es.json")
            .bytes,
    )
    .expect("utf8 es.json");
    assert!(!es_json.contains("Core 8000"));

    // Step 8.1 modular layout + backward-compat routes
    let settings_html = resolver
        .resolve_route("/settings")
        .expect("route '/settings' resolves");
    assert_eq!(settings_html.relative_path, "settings/miku_settings.html");
    let set_body = String::from_utf8_lossy(&settings_html.bytes);
    assert!(set_body.contains("Configuración · Miku"));
    assert!(set_body.contains("win-min") && set_body.contains("win-cls"));

    let island_route = resolver
        .resolve_route("/island")
        .expect("route '/island' resolves");
    assert_eq!(island_route.relative_path, "index.html");

    let shared_theme = resolver
        .resolve_asset("shared/theme.css")
        .expect("shared/theme.css resolves");
    assert!(!shared_theme.bytes.is_empty());
}

// LEDGER-7.1-1
#[test]
fn test_7_1_default_mascot_config_has_no_blue_twintails() {
    let resolver = CustomUiResolver::new(resolve_custom_ui_base_dir(), None);
    let cfg_bytes = resolver
        .resolve_asset("mascot/mascot.config.json")
        .expect("mascot.config.json")
        .bytes;
    let cfg_json: serde_json::Value =
        serde_json::from_slice(&cfg_bytes).expect("valid mascot.config.json");
    assert_eq!(cfg_json["enableTwintails"], false);
    assert_eq!(cfg_json["schemaVersion"], 2);
    let first_tail = cfg_json["twintailsColors"][0]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert_ne!(first_tail, "#62e8de");

    let mascot_js = String::from_utf8(
        resolver
            .resolve_asset("mascot/mascot.js")
            .expect("mascot.js")
            .bytes,
    )
    .expect("utf8 mascot.js");
    assert!(!mascot_js.contains("twintailsColors: ['#62e8de'"));
    assert!(mascot_js.contains("twintailsColors: ['#f4f6fb', '#cbd5e1']"));
    assert!(mascot_js.contains("enableTwintails: false"));
}
