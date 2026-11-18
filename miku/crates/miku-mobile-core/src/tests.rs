use std::ffi::{CStr, CString};
use std::fs;
use std::path::Path;

use miku_gatekeeper::{
    Capability, DenyReason, GatekeeperEngine, GatekeeperVerdict, PolicyEffect, PolicyRule,
    ProjectProfile,
};
use miku_protocol::{MikuLinkEnvelopeV1, MikuLinkMessageV1, PermissionDecisionKind};

use crate::mobile_client::{
    build_island_mobile_parity_matrix, compute_req_hash_v1,
    enforce_daemon_mobile_permission_decision, validate_mobile_config_no_provider_keys,
    BiometricProof, DisplayedPermissionCard, MobileAgentFeedEntry, MobileCoreClient,
    MobileCoreError, MobileDecisionInput,
};
use crate::uniffi_ffi::{
    miku_mobile_compute_req_hash, miku_mobile_decide_permission_json,
    miku_mobile_parity_matrix_json, miku_mobile_string_free, miku_mobile_validate_config_json,
    UNIFFI_UDL_SPEC,
};

#[test]
fn test_server_rejects_pc_only_permission_decision_from_mobile_even_with_biometrics() {
    let profile = ProjectProfile {
        project_name: "miku-project".to_string(),
        project_root: "C:/workspace/miku-project".to_string(),
        default_ttl_ms: 60_000,
        rules: vec![PolicyRule {
            capability: Capability::Shell,
            pattern: "cargo test".to_string(),
            effect: PolicyEffect::Ask,
            pc_only: true,
        }],
    };
    let mut gatekeeper = GatekeeperEngine::new(profile).expect("profile");
    let verdict = gatekeeper.evaluate_request(
        "claude-code",
        Capability::Shell,
        "cargo test",
        "",
        1_000,
    );
    let req_hash = match verdict {
        GatekeeperVerdict::AskPending { ticket, .. } => {
            assert!(
                ticket.pc_only,
                "Shell rule with pc_only=true must mark ticket pc_only"
            );
            ticket.req_hash
        }
        other => panic!("expected AskPending, got {:?}", other),
    };

    let client = MobileCoreClient::new("sess-1", "pixel-9");
    let card = DisplayedPermissionCard {
        req_hash: req_hash.clone(),
        agent: "claude-code".to_string(),
        capability: "shell".to_string(),
        target: "cargo test".to_string(),
        args: String::new(),
        ttl_sec: 60,
        received_at_ms: 1_000,
        pc_only: true,
    };

    // 1) Client kernel blocks Allow on pc_only cards before network transmit.
    let client_err = client
        .prepare_permission_decision(&MobileDecisionInput {
            message_id: "m-1".to_string(),
            card,
            requested_decision: PermissionDecisionKind::Allow,
            biometric: Some(BiometricProof {
                verified: true,
                auth_method: "fingerprint".to_string(),
                bound_req_hash: req_hash.clone(),
                verified_at_ms: 1_200,
            }),
            now_ms: 1_250,
        })
        .expect_err("mobile client must reject pc_only Allow");
    assert!(matches!(
        client_err,
        MobileCoreError::PcOnlyApprovalForbiddenOnMobile { .. }
    ));

    // 2) Even if a compromised mobile client crafts an Allow frame, daemon Gatekeeper MUST reject it.
    let forged_msg = MikuLinkMessageV1::PermissionDecision {
        req_hash,
        decision: PermissionDecisionKind::Allow,
        biometric_verified: true,
    };
    let daemon_verdict = enforce_daemon_mobile_permission_decision(
        &mut gatekeeper,
        &forged_msg,
        "claude-code",
        Capability::Shell,
        "cargo test",
        "",
        1_300,
    )
    .expect("daemon enforcement runs");
    assert!(matches!(
        daemon_verdict,
        GatekeeperVerdict::Deny {
            reason: DenyReason::PcOnlyFromMobileForbidden,
            ..
        }
    ));
}

#[test]
fn test_mobile_client_verifies_req_hash_matches_displayed_params_before_allow() {
    let client = MobileCoreClient::new("sess-2", "pixel-9");
    let true_hash = compute_req_hash_v1("cursor", "filesystem", "src/main.rs", "write");

    let tampered_card = DisplayedPermissionCard {
        req_hash: true_hash.clone(),
        agent: "cursor".to_string(),
        capability: "filesystem".to_string(),
        // Display tampered to mislead mobile user:
        target: "README.md".to_string(),
        args: "write".to_string(),
        ttl_sec: 60,
        received_at_ms: 2_000,
        pc_only: false,
    };

    let err = client
        .prepare_permission_decision(&MobileDecisionInput {
            message_id: "m-toctou".to_string(),
            card: tampered_card,
            requested_decision: PermissionDecisionKind::Allow,
            biometric: Some(BiometricProof {
                verified: true,
                auth_method: "biometric_strong".to_string(),
                bound_req_hash: true_hash,
                verified_at_ms: 2_100,
            }),
            now_ms: 2_200,
        })
        .expect_err("tampered UI parameters must fail req_hash verification");

    assert!(matches!(err, MobileCoreError::ReqHashMismatch { .. }));
}

#[test]
fn test_allow_requires_biometric_bound_to_req_hash_while_deny_requires_no_biometrics() {
    let client = MobileCoreClient::new("sess-3", "pixel-9");
    let valid_hash = compute_req_hash_v1("antigravity", "filesystem", "docs/plan.md", "edit");
    let card = DisplayedPermissionCard {
        req_hash: valid_hash.clone(),
        agent: "antigravity".to_string(),
        capability: "filesystem".to_string(),
        target: "docs/plan.md".to_string(),
        args: "edit".to_string(),
        ttl_sec: 60,
        received_at_ms: 5_000,
        pc_only: false,
    };

    // Allow without biometrics -> rejected
    let missing_bio = client
        .prepare_permission_decision(&MobileDecisionInput {
            message_id: "m-no-bio".to_string(),
            card: card.clone(),
            requested_decision: PermissionDecisionKind::Allow,
            biometric: None,
            now_ms: 5_100,
        })
        .expect_err("Allow without biometrics must fail");
    assert_eq!(missing_bio, MobileCoreError::BiometricRequiredForAllow);

    // Allow with biometric proof bound to a DIFFERENT req_hash -> rejected
    let wrong_binding = client
        .prepare_permission_decision(&MobileDecisionInput {
            message_id: "m-wrong-bio".to_string(),
            card: card.clone(),
            requested_decision: PermissionDecisionKind::Allow,
            biometric: Some(BiometricProof {
                verified: true,
                auth_method: "fingerprint".to_string(),
                bound_req_hash: "sha256:0000".to_string(),
                verified_at_ms: 5_150,
            }),
            now_ms: 5_200,
        })
        .expect_err("Biometric proof bound to another req_hash must fail");
    assert_eq!(wrong_binding, MobileCoreError::BiometricBoundHashMismatch);

    // Deny works in 1 tap WITHOUT biometrics
    let deny_res = client
        .prepare_permission_decision(&MobileDecisionInput {
            message_id: "m-deny".to_string(),
            card: card.clone(),
            requested_decision: PermissionDecisionKind::Deny,
            biometric: None,
            now_ms: 5_250,
        })
        .expect("1-tap Deny succeeds without biometrics");
    assert_eq!(deny_res.final_decision, PermissionDecisionKind::Deny);
    let parsed_deny = MikuLinkEnvelopeV1::decode_and_validate(&deny_res.envelope_json).unwrap();
    assert!(matches!(
        parsed_deny.msg,
        MikuLinkMessageV1::PermissionDecision {
            decision: PermissionDecisionKind::Deny,
            biometric_verified: false,
            ..
        }
    ));

    // Allow with valid biometric bound to exact req_hash -> succeeds
    let allow_res = client
        .prepare_permission_decision(&MobileDecisionInput {
            message_id: "m-allow".to_string(),
            card,
            requested_decision: PermissionDecisionKind::Allow,
            biometric: Some(BiometricProof {
                verified: true,
                auth_method: "fingerprint".to_string(),
                bound_req_hash: valid_hash,
                verified_at_ms: 5_300,
            }),
            now_ms: 5_350,
        })
        .expect("Biometric-bound Allow succeeds");
    assert_eq!(allow_res.final_decision, PermissionDecisionKind::Allow);
}

#[test]
fn test_mobile_config_never_stores_provider_api_keys_and_ttl_expiry_fails_safe_deny() {
    let bad_err = validate_mobile_config_no_provider_keys(
        r#"{"pairing_token":"tok_abc","groq":"gsk_secret_value_123"}"#,
    )
    .expect_err("provider API key must never be persisted on mobile");
    assert!(matches!(
        bad_err,
        MobileCoreError::ProviderApiKeyStorageForbidden { .. }
    ));

    validate_mobile_config_no_provider_keys(
        r#"{"pairing_token":"tok_abc","tls_cert_sha256":"abcd1234"}"#,
    )
    .expect("pairing metadata without provider keys allowed");

    let client = MobileCoreClient::new("sess-4", "pixel-9");
    let hash = compute_req_hash_v1("claude-code", "network", "https://docs.rs", "GET");
    let expired_res = client
        .prepare_permission_decision(&MobileDecisionInput {
            message_id: "m-exp".to_string(),
            card: DisplayedPermissionCard {
                req_hash: hash.clone(),
                agent: "claude-code".to_string(),
                capability: "network".to_string(),
                target: "https://docs.rs".to_string(),
                args: "GET".to_string(),
                ttl_sec: 15,
                received_at_ms: 10_000,
                pc_only: false,
            },
            requested_decision: PermissionDecisionKind::Allow,
            biometric: Some(BiometricProof {
                verified: true,
                auth_method: "face".to_string(),
                bound_req_hash: hash,
                verified_at_ms: 30_000,
            }),
            now_ms: 30_000, // 20s > 15s TTL
        })
        .expect("expired card resolves deterministically to safe Deny");
    assert_eq!(expired_res.final_decision, PermissionDecisionKind::Deny);
    assert!(expired_res.auto_expired_to_deny);
}

#[test]
fn test_uniffi_c_abi_exports_feed_chat_queue_ptt_kill_switch_and_parity_matrix() {
    assert!(UNIFFI_UDL_SPEC.contains("interface MobileCoreClient"));

    let mut client = MobileCoreClient::new("sess-5", "android-pixel");
    client.push_agent_event(MobileAgentFeedEntry {
        agent: "claude-code".to_string(),
        kind: "tool_call".to_string(),
        title: "Running cargo test".to_string(),
        received_at_ms: 100,
    });
    assert_eq!(client.agent_feed.len(), 1);

    // Offline chat prompt queues cleanly, then flushes on reconnect.
    assert!(client
        .enqueue_or_dispatch_prompt("chat-1", "Run lint check")
        .unwrap()
        .is_none());
    client.set_connected(true);
    let flushed = client.flush_offline_queue().unwrap();
    assert_eq!(flushed.len(), 1);
    MikuLinkEnvelopeV1::decode_and_validate(&flushed[0]).unwrap();

    let ptt = client
        .build_remote_ptt_command("ptt-1", "Approve docs update")
        .unwrap();
    assert!(ptt.contains("remote_ptt"));
    let kill = client.build_kill_switch_frame("kill-1").unwrap();
    assert!(kill.contains("kill_switch"));

    // Verify C-ABI exports cleanly across FFI boundary.
    let c_agent = CString::new("cursor").unwrap();
    let c_cap = CString::new("filesystem").unwrap();
    let c_target = CString::new("src/lib.rs").unwrap();
    let c_args = CString::new("read").unwrap();
    // SAFETY: Valid CString pointers freed immediately below.
    unsafe {
        let h_ptr = miku_mobile_compute_req_hash(
            c_agent.as_ptr(),
            c_cap.as_ptr(),
            c_target.as_ptr(),
            c_args.as_ptr(),
        );
        let h_str = CStr::from_ptr(h_ptr).to_str().unwrap().to_string();
        assert_eq!(
            h_str,
            compute_req_hash_v1("cursor", "filesystem", "src/lib.rs", "read")
        );
        miku_mobile_string_free(h_ptr);

        let good_cfg = CString::new(r#"{"device":"pixel"}"#).unwrap();
        let res_ptr = miku_mobile_validate_config_json(good_cfg.as_ptr());
        assert!(CStr::from_ptr(res_ptr).to_str().unwrap().contains(r#""ok":true"#));
        miku_mobile_string_free(res_ptr);

        let dummy_decision = CString::new("{}").unwrap();
        let err_ptr = miku_mobile_decide_permission_json(dummy_decision.as_ptr());
        assert!(CStr::from_ptr(err_ptr).to_str().unwrap().contains(r#""ok":false"#));
        miku_mobile_string_free(err_ptr);

        let mat_ptr = miku_mobile_parity_matrix_json();
        let mat_str = CStr::from_ptr(mat_ptr).to_str().unwrap();
        assert!(mat_str.contains("Global Kill Switch"));
        miku_mobile_string_free(mat_ptr);
    }

    assert_eq!(build_island_mobile_parity_matrix().len(), 7);

    // Modular line budget check (< 600 lines on all files)
    let crate_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for name in ["lib.rs", "mobile_client.rs", "uniffi_ffi.rs", "tests.rs"] {
        let content = fs::read_to_string(crate_src.join(name)).unwrap();
        let lines = content.lines().count();
        assert!(
            lines < 600,
            "{} has {} lines (must be < 600)",
            name,
            lines
        );
    }
}
