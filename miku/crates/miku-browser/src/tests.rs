use std::path::{Path, PathBuf};

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, PolicyRule, ProjectProfile};
use miku_protocol::PermissionDecisionKind;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::{
    build_isolated_launch_spec, discover_default_windows_browser, is_personal_profile_protected,
    resolve_isolated_profile_dir, validate_download_target, BrowserApprovalOrigin,
    BrowserController, BrowserEngineKind, BrowserError, BrowserProtocolKind,
    BrowserWorkflowOutcome, DownloadJailPolicy, DownloadRequest, LiveLoopbackDualProtocolBackend,
};

fn build_profile(rules: Vec<(Capability, &str, PolicyEffect, bool)>) -> ProjectProfile {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let mut profile = ProjectProfile::new("miku-browser-test", manifest_dir, 60_000);
    for (cap, pat, eff, pc_only) in rules {
        profile.rules.push(PolicyRule {
            capability: cap,
            pattern: pat.to_string(),
            effect: eff,
            pc_only,
        });
    }
    profile
}

fn fresh_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("miku_browser_{label}_{nanos}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

async fn spawn_mock_dual_ws_and_http_server(
    protocol: BrowserProtocolKind,
    download_payload: Vec<u8>,
) -> (String, String, tokio::task::JoinHandle<()>) {
    let ws_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ws_addr = ws_listener.local_addr().unwrap().to_string();

    let http_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let http_addr = http_listener.local_addr().unwrap().to_string();

    let handle = tokio::spawn(async move {
        let ws_task = tokio::spawn(async move {
            if let Ok((mut s, _)) = ws_listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
                    )
                    .await;
                loop {
                    let mut hdr = [0u8; 2];
                    if s.read_exact(&mut hdr).await.is_err() {
                        break;
                    }
                    let opcode = hdr[0] & 0x0f;
                    if opcode == 0x8 {
                        break;
                    }
                    let masked = (hdr[1] & 0x80) != 0;
                    let mut len = (hdr[1] & 0x7f) as usize;
                    if len == 126 {
                        let mut ext = [0u8; 2];
                        if s.read_exact(&mut ext).await.is_err() {
                            break;
                        }
                        len = u16::from_be_bytes(ext) as usize;
                    }
                    let mut mask = [0u8; 4];
                    if masked && s.read_exact(&mut mask).await.is_err() {
                        break;
                    }
                    let mut body = vec![0u8; len];
                    if s.read_exact(&mut body).await.is_err() {
                        break;
                    }
                    if masked {
                        for (i, b) in body.iter_mut().enumerate() {
                            *b ^= mask[i % 4];
                        }
                    }
                    let req: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("");
                    let reply = match (protocol, method) {
                        (BrowserProtocolKind::ChromiumCdp, "Runtime.evaluate") => json!({
                            "id": req["id"],
                            "result": { "title": "GitHub Repo Search (Edge CDP)", "value": "rust miku orchestrator DOM text" }
                        }),
                        (BrowserProtocolKind::GeckoBiDi, "script.evaluate") => json!({
                            "id": req["id"],
                            "result": { "title": "GitHub Repo Search (Zen BiDi)", "text": "zen browser isolated gecko DOM text" }
                        }),
                        _ => json!({ "id": req["id"], "result": {} }),
                    };
                    let bytes = serde_json::to_vec(&reply).unwrap();
                    let mut out = Vec::with_capacity(bytes.len() + 4);
                    out.push(0x81);
                    if bytes.len() < 126 {
                        out.push(bytes.len() as u8);
                    } else {
                        out.push(126);
                        out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
                    }
                    out.extend_from_slice(&bytes);
                    if s.write_all(&out).await.is_err() {
                        break;
                    }
                }
            }
        });

        let http_task = tokio::spawn(async move {
            if let Ok((mut s, _)) = http_listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    download_payload.len()
                );
                let _ = s.write_all(hdr.as_bytes()).await;
                let _ = s.write_all(&download_payload).await;
            }
        });

        let _ = tokio::join!(ws_task, http_task);
    });

    (ws_addr, http_addr, handle)
}

#[test]
fn test_windows_browser_auto_detection_and_personal_profile_isolation() {
    let report = discover_default_windows_browser();
    println!("BROWSER_4_5_DISCOVERY: prog_id={:?} candidates={}", report.windows_prog_id, report.installed_candidates.len());

    // On this Windows developer workstation at least Edge or Zen should be detected
    assert!(
        !report.installed_candidates.is_empty(),
        "expected at least one installed browser candidate on Windows"
    );
    let selected = report.selected.expect("selected browser");

    let sandbox_profiles = fresh_temp_dir("profiles");
    let isolated = resolve_isolated_profile_dir(Some(&sandbox_profiles), selected.engine).unwrap();
    assert!(isolated.exists());
    assert!(is_personal_profile_protected(&isolated));

    // Explicit guard against user's personal Zen / Firefox / Edge profiles
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let personal_zen = Path::new(&appdata).join("zen").join("Profiles");
        assert!(!is_personal_profile_protected(&personal_zen));
        let err = resolve_isolated_profile_dir(Some(Path::new(&appdata)), BrowserEngineKind::Zen)
            .expect_err("must reject overlapping personal zen profile");
        assert!(matches!(err, BrowserError::PersonalProfileViolation(_)));
    }

    let spec = build_isolated_launch_spec(&selected, Some(&sandbox_profiles), 0, true).unwrap();
    assert!(spec.args.iter().any(|a| a.contains(&isolated.to_string_lossy().to_string())));
    let _ = std::fs::remove_dir_all(&sandbox_profiles);
}

#[tokio::test]
async fn test_non_allowlisted_domain_triggers_gatekeeper_ask_and_blocks_ssrf_and_deny() {
    let jail_dir = fresh_temp_dir("gatekeeper");
    let jail = DownloadJailPolicy::new(&jail_dir).unwrap();
    let (ws_addr, http_addr, _srv) =
        spawn_mock_dual_ws_and_http_server(BrowserProtocolKind::GeckoBiDi, b"ok".to_vec()).await;
    let backend = LiveLoopbackDualProtocolBackend::new(
        BrowserProtocolKind::GeckoBiDi,
        ws_addr,
        "/session",
        Some(http_addr),
    );
    let mut controller = BrowserController::new(backend, jail, true);

    let profile = build_profile(vec![
        (Capability::Browser, "github.com", PolicyEffect::Allow, false),
        (Capability::Browser, "127.0.0.1", PolicyEffect::Ask, false),
        (Capability::Browser, "evil.example", PolicyEffect::Deny, false),
    ]);
    let mut gk = GatekeeperEngine::new(profile).unwrap();

    // 1) SSRF / metadata IP & file:// scheme must fail closed before reaching Gatekeeper
    let ssrf_err = controller
        .browse_and_optional_download("researcher", "https://169.254.169.254/latest", None, &mut gk, 1000)
        .await
        .expect_err("metadata SSRF must fail");
    assert!(matches!(ssrf_err, BrowserError::BlockedSsrfOrScheme(_)));

    let file_err = controller
        .browse_and_optional_download("researcher", "file:///C:/Windows/win.ini", None, &mut gk, 1001)
        .await
        .expect_err("file:// must fail");
    assert!(matches!(file_err, BrowserError::BlockedSsrfOrScheme(_)));

    // 2) Explicitly denied domain -> Denied
    let denied = controller
        .browse_and_optional_download("researcher", "https://evil.example/payload", None, &mut gk, 1002)
        .await
        .unwrap();
    assert!(matches!(denied, BrowserWorkflowOutcome::Denied { .. }));

    // 3) Unlisted / ask domain -> Ask -> User approves exact req_hash -> Completed via Zen BiDi
    let ask = controller
        .browse_and_optional_download("researcher", "http://127.0.0.1:8080/search?q=miku", None, &mut gk, 1003)
        .await
        .unwrap();
    let ticket = match ask {
        BrowserWorkflowOutcome::AwaitingApproval { ticket } => ticket,
        other => panic!("expected Ask ticket, got {other:?}"),
    };

    // Wrong req_hash rejected (anti-TOCTOU) and valid req_hash still resolves cleanly after rejection
    let bad_hash = controller
        .resolve_and_execute(
            &ticket.req_hash,
            PermissionDecisionKind::Allow,
            "tampered-hash",
            BrowserApprovalOrigin::desktop(),
            &mut gk,
            1004,
        )
        .await
        .unwrap();
    assert!(matches!(bad_hash, BrowserWorkflowOutcome::Denied { .. }));

    let approved = controller
        .resolve_and_execute(
            &ticket.req_hash,
            PermissionDecisionKind::Allow,
            &ticket.req_hash,
            BrowserApprovalOrigin::desktop(),
            &mut gk,
            1005,
        )
        .await
        .unwrap();
    assert!(matches!(approved, BrowserWorkflowOutcome::Completed(_)));

    let _ = std::fs::remove_dir_all(&jail_dir);
}

#[test]
fn test_download_jail_blocks_exe_scripts_and_path_traversal_outside_sandbox() {
    let jail_dir = fresh_temp_dir("dl_jail");
    let jail = DownloadJailPolicy::new(&jail_dir).unwrap();

    for dangerous in ["installer.exe", "dropper.bat", "script.ps1", "report.pdf.exe", "evil.dll"] {
        let err = validate_download_target(&jail, dangerous, 128)
            .expect_err("dangerous extension must be blocked");
        assert!(matches!(err, BrowserError::DangerousDownloadBlocked(_)));
    }

    for escape in ["../outside.txt", "..\\outside.md", "C:\\Windows\\system.ini"] {
        let err = validate_download_target(&jail, escape, 64)
            .expect_err("path escape must be blocked");
        assert!(matches!(err, BrowserError::DownloadOutsideJail(_)));
    }

    let safe = validate_download_target(&jail, "notes/miku-summary.md", 256).unwrap();
    assert!(safe.starts_with(&jail.canonical_jail_dir));
    let _ = std::fs::remove_dir_all(&jail_dir);
}

#[tokio::test]
async fn test_dual_protocol_e2e_cdp_and_zen_bidi_download_to_jail_and_return_island_card() {
    for (proto, allow_host) in [
        (BrowserProtocolKind::GeckoBiDi, "127.0.0.1"),
        (BrowserProtocolKind::ChromiumCdp, "127.0.0.1"),
    ] {
        let jail_dir = fresh_temp_dir("e2e_dual");
        let jail = DownloadJailPolicy::new(&jail_dir).unwrap();
        let artifact = b"Miku E2E DOM & Artifact Payload".to_vec();
        let (ws_addr, http_addr, _srv) =
            spawn_mock_dual_ws_and_http_server(proto, artifact.clone()).await;

        let backend = LiveLoopbackDualProtocolBackend::new(proto, ws_addr, "/devtools", Some(http_addr));
        let mut controller = BrowserController::new(backend, jail, true);

        let profile = build_profile(vec![(
            Capability::Browser,
            allow_host,
            PolicyEffect::Allow,
            false,
        )]);
        let mut gk = GatekeeperEngine::new(profile).unwrap();

        let outcome = controller
            .browse_and_optional_download(
                "researcher",
                "http://127.0.0.1:9222/repos/miku",
                Some(DownloadRequest {
                    suggested_filename: "repo-readme.md".to_string(),
                }),
                &mut gk,
                2000,
            )
            .await
            .unwrap();

        let card = match outcome {
            BrowserWorkflowOutcome::Completed(c) => c,
            other => panic!("expected Completed IslandBrowserCard, got {other:?}"),
        };
        assert_eq!(card.protocol_used, proto);
        assert!(card.isolated_profile_used);
        assert!(!card.dom_excerpt.is_empty());
        let receipt = card.downloaded_file.expect("download receipt");
        assert_eq!(receipt.bytes_written, artifact.len());
        assert_eq!(std::fs::read(&receipt.canonical_path).unwrap(), artifact);
        assert_eq!(controller.active_browser_processes(), 0);

        let _ = std::fs::remove_dir_all(&jail_dir);
    }
}

#[tokio::test]
async fn test_kill_switch_zero_idle_processes_and_visible_login_spec_uses_isolated_profile() {
    let jail_dir = fresh_temp_dir("kill");
    let jail = DownloadJailPolicy::new(&jail_dir).unwrap();
    let backend = LiveLoopbackDualProtocolBackend::new(
        BrowserProtocolKind::GeckoBiDi,
        "127.0.0.1:1",
        "/bidi",
        None,
    );
    let mut controller = BrowserController::new(backend, jail, true);
    let profile = build_profile(vec![(Capability::Browser, "*", PolicyEffect::Allow, false)]);
    let mut gk = GatekeeperEngine::new(profile).unwrap();

    // Visible login spec uses isolated profile and headless=false
    let sandbox_profiles = fresh_temp_dir("login_profile");
    let login_spec = controller
        .prepare_visible_login_profile(None, Some(&sandbox_profiles))
        .unwrap();
    assert!(!login_spec.headless);
    assert!(is_personal_profile_protected(Path::new(
        &login_spec.isolated_profile_dir
    )));

    // Kill switch aborts further runs immediately and leaves 0 processes
    controller.trip_kill_switch(&mut gk, 3000);
    assert!(controller.is_abort_active());
    let err = controller
        .browse_and_optional_download("researcher", "http://127.0.0.1/test", None, &mut gk, 3001)
        .await
        .expect_err("kill switch must block");
    assert!(matches!(err, BrowserError::KillSwitchAborted));
    assert_eq!(controller.active_browser_processes(), 0);

    let _ = std::fs::remove_dir_all(&sandbox_profiles);
    let _ = std::fs::remove_dir_all(&jail_dir);
}

#[test]
fn test_miku_browser_modules_comply_with_sub_600_line_modular_budget() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut count = 0usize;
    let mut max_lines = 0usize;
    for entry in std::fs::read_dir(&src_dir).expect("read miku-browser/src") {
        let p = entry.unwrap().path();
        if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            let lines = std::fs::read_to_string(&p).unwrap().lines().count();
            count += 1;
            max_lines = max_lines.max(lines);
            assert!(
                lines < 600,
                "module {} exceeded 600 lines ({lines})",
                p.display()
            );
        }
    }
    println!("BROWSER_4_5_MODULAR_AUDIT: files={count} max_lines={max_lines} (< 600)");
    assert!(count >= 5);
}
