use super::*;

// LEDGER-1.1-1
#[test]
fn test_base_contracts_rig_toml_and_design_tokens_loader_and_validation() {
    let bundle = BaseContractBundle::load_embedded_defaults()
        .expect("embedded rig.toml and design-tokens.json must validate cleanly");
    assert_eq!(bundle.rig.schema_version, 1);
    assert_eq!(bundle.rig.states.idle.fps_when_asleep, 0);
    assert_eq!(bundle.tokens.island.pill_width, 220);
    assert_eq!(bundle.tokens.island.pill_height, 36);
    assert_eq!(bundle.contract_sha256.len(), 64);

    // Reject non-zero fps_when_asleep
    let bad_rig = DEFAULT_RIG_TOML.replace("fps_when_asleep = 0", "fps_when_asleep = 60");
    let err = RigConfig::parse_and_validate(&bad_rig).unwrap_err();
    assert!(matches!(err, ProtocolError::ContractValidation(_)));

    // Reject malformed hex color using r##"..."## delimiter
    let bad_tokens = DEFAULT_DESIGN_TOKENS_JSON.replace(r##""#0B0F17""##, r##""#ZZZZZZ""##);
    let err_tokens = DesignTokens::parse_and_validate(&bad_tokens).unwrap_err();
    assert!(matches!(err_tokens, ProtocolError::ContractValidation(_)));

    // Reject unknown fields (deny_unknown_fields)
    let unknown_field_rig = format!("{}\nunexpected_root_key = 42\n", DEFAULT_RIG_TOML);
    assert!(matches!(
        RigConfig::parse_and_validate(&unknown_field_rig),
        Err(ProtocolError::MalformedToml(_))
    ));
}

// LEDGER-1.1-2
#[test]
fn test_protocol_v1_envelope_serialization_and_constant_time_token_auth() {
    let samples = vec![
        MikuLinkEnvelopeV1 {
            v: 1,
            id: "msg-01".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Hello {
                client: "miku_cli".to_string(),
                token: "tok_secret_123".to_string(),
                capabilities: vec!["agent_events".to_string()],
            },
        },
        MikuLinkEnvelopeV1 {
            v: 1,
            id: "msg-02".to_string(),
            reply_to: Some("msg-01".to_string()),
            msg: MikuLinkMessageV1::HelloAck {
                session_id: "sess_01".to_string(),
                island_state: IslandState::Idle,
            },
        },
        MikuLinkEnvelopeV1 {
            v: 1,
            id: "msg-03".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Ping { ts: 1_762_315_641 },
        },
        MikuLinkEnvelopeV1 {
            v: 1,
            id: "msg-04".to_string(),
            reply_to: Some("msg-03".to_string()),
            msg: MikuLinkMessageV1::Pong { ts: 1_762_315_641 },
        },
        MikuLinkEnvelopeV1 {
            v: 1,
            id: "msg-05".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::AgentEvent {
                agent: "claude_code".to_string(),
                kind: "needs_input".to_string(),
                title: "Edit src/lib.rs".to_string(),
            },
        },
        MikuLinkEnvelopeV1 {
            v: 1,
            id: "msg-06".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::PermissionRequest {
                req_hash: "sha256:abcdef".to_string(),
                agent: "claude_code".to_string(),
                capability: "fs_write".to_string(),
                target: "src/lib.rs".to_string(),
                ttl_sec: 60,
                pc_only: false,
            },
        },
        MikuLinkEnvelopeV1 {
            v: 1,
            id: "msg-07".to_string(),
            reply_to: Some("msg-06".to_string()),
            msg: MikuLinkMessageV1::PermissionDecision {
                req_hash: "sha256:abcdef".to_string(),
                decision: PermissionDecisionKind::Allow,
                biometric_verified: true,
            },
        },
        MikuLinkEnvelopeV1 {
            v: 1,
            id: "msg-08".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Command {
                session_id: "sess_01".to_string(),
                action: "enqueue_prompt".to_string(),
                text: "cargo test".to_string(),
            },
        },
        MikuLinkEnvelopeV1 {
            v: 1,
            id: "msg-09".to_string(),
            reply_to: Some("msg-01".to_string()),
            msg: MikuLinkMessageV1::Error {
                code: "unsupported_version".to_string(),
                supported: Some(vec![1]),
                detail: None,
            },
        },
    ];

    for env in samples {
        let encoded = env.encode_json().expect("must serialize");
        let decoded = MikuLinkEnvelopeV1::decode_and_validate(&encoded).expect("must decode");
        assert_eq!(env, decoded);
    }

    // Unsupported version v=99 must return UnsupportedVersion
    let bad_version = r##"{"v":99,"id":"msg-99","msg":{"type":"ping","data":{"ts":10}}}"##;
    assert_eq!(
        MikuLinkEnvelopeV1::decode_and_validate(bad_version),
        Err(ProtocolError::UnsupportedVersion {
            received: 99,
            supported: vec![1]
        })
    );

    // Oversized frame must be rejected immediately
    let huge = "x".repeat(MAX_FRAME_BYTES + 10);
    assert!(matches!(
        MikuLinkEnvelopeV1::decode_and_validate(&huge),
        Err(ProtocolError::FrameTooLarge { .. })
    ));

    // Constant-time token comparison checks
    assert!(constant_time_token_eq(
        "miku_tok_v1_99887766",
        "miku_tok_v1_99887766"
    ));
    assert!(!constant_time_token_eq(
        "miku_tok_v1_99887766",
        "miku_tok_v1_99887767"
    ));
    assert!(!constant_time_token_eq("miku_tok_v1_99887766", "short"));
    assert!(!constant_time_token_eq("", ""));
}
