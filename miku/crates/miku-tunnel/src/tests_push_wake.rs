use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread;

use crate::push_wake::{
    build_zero_pii_wake_payload, serialize_verified_zero_pii_json, PushWakeClient,
    PushWakeClientConfig, PushWakeError, WakeAgentState, ZeroPiiWakePayload,
};

#[test]
fn test_wake_payload_strictly_contains_only_agent_name_state_and_ts_with_zero_pii() {
    // Reject file paths, prompts, code snippets, or secret prefixes.
    for bad_input in [
        "C:/Users/alice/secret.rs",
        "claude-code/../../etc/passwd",
        "Run rm -rf on workspace",
        "gsk_live_secret_token",
        "fix fn main() { panic!() }",
    ] {
        let err = build_zero_pii_wake_payload(bad_input, WakeAgentState::NeedsPermission, 100)
            .expect_err("PII/path/prompt/secret input must be rejected");
        assert!(matches!(
            err,
            PushWakeError::InvalidAgentIdentifier(_)
                | PushWakeError::PotentialPiiOrSecretDetected(_)
        ));
    }

    let ok = build_zero_pii_wake_payload("claude-code", WakeAgentState::NeedsPermission, 1_763_000_000)
        .expect("clean identifier allowed");
    let raw = serialize_verified_zero_pii_json(&ok).expect("serializes strictly");
    let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let obj = val.as_object().unwrap();
    assert_eq!(obj.len(), 3);
    assert_eq!(obj.get("agent_name").unwrap(), "claude-code");
    assert_eq!(obj.get("state").unwrap(), "needs_permission");
    assert_eq!(obj.get("event_ts").unwrap(), 1_763_000_000u64);
}

#[test]
fn test_optional_push_wake_roundtrip_over_loopback_relay_and_disabled_by_default() {
    let default_client = PushWakeClient::new(PushWakeClientConfig::default());
    assert!(!default_client.is_enabled());
    assert_eq!(
        default_client.dispatch_wake_signal("cursor", WakeAgentState::Completed, 123),
        Err(PushWakeError::DisabledByDefault)
    );

    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral relay port");
    let addr = listener.local_addr().unwrap();

    let server_handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).unwrap();
        let req = String::from_utf8_lossy(&buf[..n]).to_string();
        let body = req.split("\r\n\r\n").nth(1).unwrap().trim();
        let parsed: ZeroPiiWakePayload = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.agent_name, "antigravity");
        assert_eq!(parsed.state, WakeAgentState::Completed);

        let resp = "HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\n\r\nok";
        stream.write_all(resp.as_bytes()).unwrap();
    });

    let client = PushWakeClient::new(PushWakeClientConfig {
        enabled: true,
        relay_host_port: Some(addr.to_string()),
        relay_path: "/wake".to_string(),
        require_tailnet_or_loopback: true,
    });
    let code = client
        .dispatch_wake_signal("antigravity", WakeAgentState::Completed, 9_999)
        .expect("loopback wake relay succeeds");
    assert_eq!(code, 202);
    server_handle.join().unwrap();

    // Non-tailnet public IP rejected when require_tailnet_or_loopback = true
    let public_client = PushWakeClient::new(PushWakeClientConfig {
        enabled: true,
        relay_host_port: Some("8.8.8.8:8080".to_string()),
        relay_path: "/wake".to_string(),
        require_tailnet_or_loopback: true,
    });
    let err = public_client
        .dispatch_wake_signal("cursor", WakeAgentState::Error, 1)
        .expect_err("non-tailnet endpoint blocked");
    assert!(matches!(err, PushWakeError::UntrustedEndpoint(_)));

    // Modular budget check (< 600 lines on all crate source files)
    let crate_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for name in ["lib.rs", "tailscale.rs", "push_wake.rs", "tests.rs", "tests_push_wake.rs"] {
        let lines = fs::read_to_string(crate_src.join(name))
            .unwrap()
            .lines()
            .count();
        assert!(lines < 600, "{} has {} lines (< 600)", name, lines);
    }
}
