use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use miku_agent::claude_tracker_and_telemetry::execute_hook_with_fail_open;
use miku_cli::hook_cmd::{install_miku_hook, HookInstallError, HookTargetAgent, TerminalHookPrompter};
use miku_cli::license_audit::{resolve_repo_root, run_workspace_license_and_ci_audit};
use miku_cli::secrets_cmd::{
    execute_cli_secret_command_with_prompter_and_backend, SecretPrompter,
};
use miku_core::build_current_thread_runtime;
use miku_gateway::GatewayConfig;
use miku_gatekeeper::{
    CanonicalPathJail, Capability, DenyReason, GatekeeperEngine, GatekeeperVerdict, PolicyEffect,
    ProjectProfile,
};
use miku_memory::island_search_ipc::redact_sensitive_text;
use miku_protocol::{
    constant_time_token_eq, ClaudeBashInput, ClaudeHookDecisionKind, ClaudeHookResult,
    ClaudePreToolUseParams, JsonRpcId, JsonRpcRequest, JsonRpcResponse, PermissionDecisionKind,
    CLAUDE_HOOK_FAIL_OPEN_TIMEOUT_MS,
};
use miku_ui::{
    build_overview_scene_bundle, redact_sensitive_voice_diagnostic_with_secrets,
    resolve_avatar_assets_dir, sanitize_for_text_content, validate_ipc_sender_origin,
    validate_webview_csp_policy, WebView2LifecycleDriver, WebView2SecurityPolicy,
    WebViewIpcPayloadV1, Win32WebView2ComDriver,
};

struct NonInteractiveDummyPrompter;

impl SecretPrompter for NonInteractiveDummyPrompter {
    fn is_terminal(&self) -> bool {
        false
    }

    fn prompt_secret(&self, _prompt_text: &str) -> Result<miku_agent::secrets::Zeroizing<Vec<u8>>, String> {
        Ok(miku_agent::secrets::Zeroizing::new(b"should-never-be-called".to_vec()))
    }
}

fn temp_sec_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("miku_sec_{}_{}_{}", tag, std::process::id(), nanos));
    let _ = fs::create_dir_all(&dir);
    dir
}

// LEDGER-5.3-1: Constant-time auth, loopback bind guard & secret/log redaction
#[test]
fn test_5_3_sec_tokens_loopback_dpapi_and_log_redaction() {
    assert!(!constant_time_token_eq("", ""));
    assert!(!constant_time_token_eq("miku-tok-alpha", "miku-tok-alphb"));
    assert!(!constant_time_token_eq("miku-tok-alpha", "miku-tok-alph"));
    assert!(constant_time_token_eq("miku-tok-alpha", "miku-tok-alpha"));

    // Non-loopback gateway bind without LAN pairing must fail closed
    let non_loopback = GatewayConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 47400),
        allow_lan_pairing: false,
        handshake_timeout: Duration::from_millis(2_000),
    };
    assert!(non_loopback.validate_bind_policy().is_err());
    assert!(GatewayConfig::loopback(47400).validate_bind_policy().is_ok());

    // CLI secret subcommands must never accept plaintext `--value` or non-TTY pipes
    let value_flag_args = vec![
        "miku".to_string(),
        "secret".to_string(),
        "set".to_string(),
        "openai_key".to_string(),
        "--value".to_string(),
        "sk-plaintext-leak-test".to_string(),
    ];
    let err = execute_cli_secret_command_with_prompter_and_backend(
        &value_flag_args,
        &NonInteractiveDummyPrompter,
        Some(Box::new(miku_agent::secrets::EncryptedMemoryBackend::new())),
    )
    .expect_err("must reject --value secret leaks on argv");
    assert!(!err.contains("sk-plaintext-leak-test"));

    // Log redaction across memory search + voice diagnostics
    let scrubbed = redact_sensitive_text(
        "token sk-test9999 ghp_secret123 path C:\\Windows\\System32\\config\\SAM",
    );
    assert!(!scrubbed.contains("sk-test9999"));
    assert!(!scrubbed.contains("ghp_secret123"));
    assert!(!scrubbed.contains("System32"));

    let active_secret = b"gsk_live_voice_987654";
    let voice_scrubbed = redact_sensitive_voice_diagnostic_with_secrets(
        "auth gsk_live_voice_987654 transcript=\"open vault password\" pcm=[120, -45, 900]",
        &[active_secret],
    );
    assert!(!voice_scrubbed.contains("gsk_live_voice_987654"));
    assert!(!voice_scrubbed.contains("open vault password"));
    assert!(!voice_scrubbed.contains("120, -45"));
}

// LEDGER-5.3-2: Strict WebView2 CSP, XSS neutralization, origin isolation & EBWebView isolation
#[test]
fn test_5_3_sec_webview2_csp_origin_isolation_and_xss_sanitization() {
    let avatar_dir = resolve_avatar_assets_dir();
    let bundle = build_overview_scene_bundle(&avatar_dir).expect("build overview bundle");
    validate_webview_csp_policy(&bundle.csp_header).expect("bundle CSP must satisfy Section 8");

    assert!(
        !bundle.has_external_cdn_links,
        "bundle must flag zero external CDN links"
    );
    let lower_html = bundle.html_content.to_ascii_lowercase();
    assert!(!lower_html.contains("http://"), "bundle HTML must have zero insecure HTTP links");
    for seg in lower_html.split("https://").skip(1) {
        let rest = seg
            .strip_prefix("miku.local")
            .expect("every https:// URL inside bundle HTML must stay on miku.local");
        assert!(
            rest.is_empty()
                || matches!(rest.as_bytes()[0], b'/' | b'"' | b'\'' | b'<' | b' ' | b';'),
            "disallowed host suffix after miku.local in bundle HTML: {seg:?}"
        );
    }
    assert!(!bundle.script_content.contains("eval("), "script.js must never call eval()");
    assert!(
        !bundle.script_content.contains("innerHTML"),
        "script.js must render untrusted values via textContent, never innerHTML"
    );

    // Unsafe CSPs rejected
    assert!(validate_webview_csp_policy("default-src *; script-src 'unsafe-eval'").is_err());
    assert!(validate_webview_csp_policy("default-src 'self'; script-src *").is_err());

    // Untrusted IPC origins rejected
    assert!(validate_ipc_sender_origin("https://miku.local/index.html").is_ok());
    assert!(validate_ipc_sender_origin("https://evil.example/payload").is_err());
    assert!(validate_ipc_sender_origin("http://miku.local/index.html").is_err());
    assert!(validate_ipc_sender_origin("file:///C:/Users/evil.html").is_err());

    // XSS payload neutralized via DomSafeTextRender (`textContent`)
    let rendered = sanitize_for_text_content("miku-sec-xss", "<img src=x onerror=alert(1)>\0")
        .expect("XSS payload sanitized");
    match rendered {
        WebViewIpcPayloadV1::DomSafeTextRender {
            safe_text,
            csp_enforced,
            ..
        } => {
            assert!(csp_enforced);
            assert!(!safe_text.contains('<'));
            assert!(!safe_text.contains('>'));
            assert!(!safe_text.contains('\0'));
        }
        other => panic!("unexpected payload: {:?}", other),
    }

    // Release hardening flags & isolated `%LOCALAPPDATA%/miku/webview2` path
    let policy = WebView2SecurityPolicy::release_hardened();
    policy.validate_release_hardening().expect("release hardened");
    let mut insecure_policy = policy.clone();
    insecure_policy.are_devtools_enabled = true;
    assert!(insecure_policy.validate_release_hardening().is_err());

    let mut driver = Win32WebView2ComDriver::new();
    let bad_dir = Path::new(r"C:\Users\miku\EBWebView");
    assert!(driver
        .spawn_isolated_controller(
            bad_dir,
            &policy,
            miku_ui::PillAnchorGeometry {
                x: 0,
                y: 0,
                width: 640,
                height: 380,
                corner_radius_px: 28,
                dpi: 96,
                scale_pct: 100,
            }
        )
        .is_err());
}

// LEDGER-5.3-3: Gatekeeper mobile biometrics, pc_only guard, TOCTOU, jail, Kill Switch & hash-chain
#[test]
fn test_5_3_sec_gatekeeper_mobile_biometrics_toctou_jail_killswitch_and_audit_chain() {
    let profile = ProjectProfile::new("miku-sec", "C:/Users/miku/workspace", 30_000)
        .with_rule(Capability::FsWrite, "src/**", PolicyEffect::Ask, false)
        .with_rule(Capability::Shell, "cargo publish*", PolicyEffect::Ask, true)
        .with_rule(Capability::Shell, "*rm -rf*", PolicyEffect::Deny, true);
    let mut gk = GatekeeperEngine::new(profile).expect("gatekeeper");

    // 1. Mobile Allow without biometric verification -> BiometricRequiredOnMobile
    let pending = gk.evaluate_request("miku-mobile", Capability::FsWrite, "src/main.rs", "write", 1_000);
    let ticket_bio = match pending {
        GatekeeperVerdict::AskPending { ticket, .. } => ticket,
        other => panic!("expected AskPending, got {:?}", other),
    };
    let no_bio = gk.resolve_permission_decision(
        &ticket_bio.req_hash,
        PermissionDecisionKind::Allow,
        true,
        false,
        "miku-mobile",
        Capability::FsWrite,
        "src/main.rs",
        "write",
        1_100,
    );
    assert!(matches!(
        no_bio,
        GatekeeperVerdict::Deny {
            reason: DenyReason::BiometricRequiredOnMobile,
            ..
        }
    ));

    // 2. Mobile Allow WITH biometrics on pc_only rule -> PcOnlyFromMobileForbidden
    let pc_only_req =
        gk.evaluate_request("miku-mobile", Capability::Shell, "cargo publish --dry-run", "", 2_000);
    let ticket_pc = match pc_only_req {
        GatekeeperVerdict::AskPending { ticket, .. } => ticket,
        other => panic!("expected AskPending, got {:?}", other),
    };
    let mobile_pc = gk.resolve_permission_decision(
        &ticket_pc.req_hash,
        PermissionDecisionKind::Allow,
        true,
        true,
        "miku-mobile",
        Capability::Shell,
        "cargo publish --dry-run",
        "",
        2_100,
    );
    assert!(matches!(
        mobile_pc,
        GatekeeperVerdict::Deny {
            reason: DenyReason::PcOnlyFromMobileForbidden,
            ..
        }
    ));

    // 3. TOCTOU argument mutation after approval ticket issued -> ToctouHashMismatch
    let toctou_req = gk.evaluate_request("agent", Capability::FsWrite, "src/lib.rs", "v1", 3_000);
    let ticket_toctou = match toctou_req {
        GatekeeperVerdict::AskPending { ticket, .. } => ticket,
        other => panic!("expected AskPending, got {:?}", other),
    };
    let toctou_verdict = gk.resolve_permission_decision(
        &ticket_toctou.req_hash,
        PermissionDecisionKind::Allow,
        false,
        false,
        "agent",
        Capability::FsWrite,
        "src/../Cargo.toml",
        "v1",
        3_100,
    );
    assert!(matches!(
        toctou_verdict,
        GatekeeperVerdict::Deny {
            reason: DenyReason::ToctouHashMismatch,
            ..
        }
    ));

    // 4. Jail escape & ADS rejection
    let jail = CanonicalPathJail::new("C:/Users/miku/workspace").expect("jail");
    assert!(jail.resolve_and_confine("../../etc/passwd").is_err());
    assert!(jail.resolve_and_confine("src/lib.rs:hidden").is_err());
    assert!(CanonicalPathJail::new("C:/Users/miku/workspace:ads").is_err());

    // 5. Global Kill Switch revokes pending & blocks execution, and Audit chain detects tampering
    let pending_ks = gk.evaluate_request("agent", Capability::FsWrite, "src/app.rs", "w", 4_000);
    assert!(matches!(pending_ks, GatekeeperVerdict::AskPending { .. }));
    let revoked = gk.trigger_kill_switch(4_100);
    assert_eq!(revoked, 1);
    let after_ks = gk.evaluate_request("agent", Capability::FsWrite, "src/app.rs", "w", 4_200);
    assert!(matches!(
        after_ks,
        GatekeeperVerdict::Deny {
            reason: DenyReason::KillSwitchActive,
            ..
        }
    ));

    assert!(gk.audit_log().verify_chain().is_ok());
    gk.audit_log_mut_for_tamper_test().entries_mut_for_tamper_simulation()[0].target =
        "tampered.rs".to_string();
    assert!(gk.audit_log().verify_chain().is_err());
}

// LEDGER-5.3-4: IDE hook fail-open under 15ms, non-interactive confirmation rejection, SECURITY.md & deps
#[test]
fn test_5_3_sec_hook_fail_open_backup_safety_security_md_and_supply_chain() {
    // 1. Hanging daemon fails open safely under 15 ms so IDE never freezes
    let rt = build_current_thread_runtime().expect("tokio runtime");
    rt.block_on(async {
        let req = JsonRpcRequest::new(
            Some(JsonRpcId::Number(7)),
            "pre_tool_use",
            ClaudePreToolUseParams {
                session_id: "sec-5-3".to_string(),
                tool_name: "Bash".to_string(),
                tool_input: serde_json::to_value(ClaudeBashInput {
                    command: "echo ok".to_string(),
                    timeout_ms: Some(1_000),
                    description: None,
                })
                .unwrap(),
                cwd: None,
                timestamp_ms: 9_000,
            },
        );
        let raw = req.encode_line().unwrap();
        let t0 = std::time::Instant::now();
        let out = execute_hook_with_fail_open(
            &raw,
            Duration::from_millis(4),
            |_r| async move {
                tokio::time::sleep(Duration::from_millis(60)).await;
                Ok(JsonRpcResponse::ok(
                    Some(JsonRpcId::Number(7)),
                    ClaudeHookResult::allow(1, "h".to_string()),
                ))
            },
        )
        .await;
        let ms = t0.elapsed().as_secs_f64() * 1_000.0;
        assert!(ms < (CLAUDE_HOOK_FAIL_OPEN_TIMEOUT_MS as f64));
        let parsed = JsonRpcResponse::<ClaudeHookResult>::decode_line(&out).unwrap();
        assert_eq!(parsed.result.unwrap().decision, ClaudeHookDecisionKind::FailOpen);
    });

    // 2. Hook installer rejects corrupt settings.json without modifying file on disk
    let tmp = temp_sec_dir("hook_corrupt");
    let settings = tmp.join("settings.json");
    let invalid_body = "{\n  \"hooks\": [corrupt\n}";
    fs::write(&settings, invalid_body).unwrap();
    let root_str = tmp.to_string_lossy().to_string();
    let profile = ProjectProfile::new("hook-sec", &root_str, 60_000)
        .with_rule(Capability::FsRead, "*", PolicyEffect::Allow, false)
        .with_rule(Capability::FsWrite, "*", PolicyEffect::Allow, false);
    let mut gk = GatekeeperEngine::new(profile).unwrap();
    let err = install_miku_hook(
        &tmp,
        &settings,
        HookTargetAgent::ClaudeCode,
        None,
        &TerminalHookPrompter,
        &mut gk,
        1_700_000_000_000,
    )
    .expect_err("corrupt JSON must fail safely");
    assert!(matches!(err, HookInstallError::InvalidJsonSyntax { .. }));
    assert_eq!(fs::read_to_string(&settings).unwrap(), invalid_body);
    let _ = fs::remove_dir_all(&tmp);

    // 3. SECURITY.md & full workspace license / CI / deny.toml supply-chain audit
    let root = resolve_repo_root();
    let sec_md = fs::read_to_string(root.join("SECURITY.md")).expect("SECURITY.md must exist");
    assert!(sec_md.contains("Section 8"));
    assert!(sec_md.contains("constant_time_token_eq"));
    assert!(sec_md.contains("BiometricRequiredOnMobile"));
    assert!(sec_md.contains("cargo audit"));

    let audit = run_workspace_license_and_ci_audit(&root).expect("workspace license/deny/ci audit");
    assert_eq!(audit.copyleft_packages_found, 0);
    assert!(audit.ci_workflow_verified && audit.cargo_deny_verified);
}
