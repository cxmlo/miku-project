use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use miku_protocol::{MikuLinkEnvelopeV1, MikuLinkMessageV1, MIKU_PROTOCOL_VERSION_V1};

use crate::tailscale::{
    is_tailnet_ip, TailscaleBackend, TailscaleTunnelError, TailscaleTunnelManager,
    WindowsTailscaleCliBackend,
};

struct MockTailscaleBackend {
    status_json: String,
    reset_calls: Arc<AtomicUsize>,
}

impl TailscaleBackend for MockTailscaleBackend {
    fn query_status_json(&self) -> Result<String, TailscaleTunnelError> {
        Ok(self.status_json.clone())
    }

    fn reset_serve_route(&self) -> Result<(), TailscaleTunnelError> {
        self.reset_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

const SAMPLE_TS_STATUS_JSON: &str = r#"{
  "BackendState": "Running",
  "Self": {
    "DNSName": "miku-laptop.tailnet-abc.ts.net.",
    "TailscaleIPs": ["100.91.42.10", "fd7a:115c:a1e0::10"]
  },
  "Peer": {
    "nodekey:1": { "DNSName": "pixel-9.tailnet-abc.ts.net.", "Online": true }
  }
}"#;

#[test]
fn test_tailscale_status_url_detection_token_rotation_and_immediate_kill_switch_cutoff() {
    let reset_calls = Arc::new(AtomicUsize::new(0));
    let backend = MockTailscaleBackend {
        status_json: SAMPLE_TS_STATUS_JSON.to_string(),
        reset_calls: Arc::clone(&reset_calls),
    };

    let mut mgr = TailscaleTunnelManager::new(47400);
    assert!(
        !mgr.is_away_mode_enabled(),
        "Away mode must start disabled by default"
    );

    let session = mgr
        .enable_away_mode(&backend, "seed-a", 1_000)
        .expect("away mode starts cleanly when Tailscale Running");
    assert_eq!(
        session.wss_endpoint,
        "wss://miku-laptop.tailnet-abc.ts.net:47400/ws"
    );
    assert_eq!(session.tailnet_ip, "100.91.42.10");

    // Authenticate mobile peer over Tailnet IP with issued token
    let hello = serde_json::to_string(&MikuLinkEnvelopeV1 {
        v: MIKU_PROTOCOL_VERSION_V1,
        id: "h-1".to_string(),
        reply_to: None,
        msg: MikuLinkMessageV1::Hello {
            client: "pixel-9".to_string(),
            token: session.session_token.clone(),
            capabilities: vec!["permission_decision".to_string()],
        },
    })
    .unwrap();
    let authed_peer = mgr
        .authenticate_first_frame_fail_closed("100.91.42.99", true, &hello)
        .expect("valid Tailnet peer and token allowed");
    assert_eq!(authed_peer, "pixel-9@100.91.42.99");
    assert_eq!(mgr.active_peer_count(), 1);

    // Token rotation disconnects previous active peers and invalidates old token immediately
    let new_token = mgr.rotate_remote_session_token("seed-b", 2_000).unwrap();
    assert_ne!(new_token, session.session_token);
    assert_eq!(mgr.active_peer_count(), 0);
    let stale_err = mgr
        .authenticate_first_frame_fail_closed("100.91.42.99", true, &hello)
        .expect_err("stale rotated token must be rejected");
    assert!(matches!(
        stale_err,
        TailscaleTunnelError::FirstFrameRejectedBeforeEventBus(_)
    ));

    // Kill Switch immediately closes tunnel, clears sessions, resets route and stays blocked
    mgr.disable_away_mode_or_kill_switch(&backend, true);
    assert!(!mgr.is_away_mode_enabled());
    assert_eq!(reset_calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        mgr.enable_away_mode(&backend, "seed-c", 3_000),
        Err(TailscaleTunnelError::KillSwitchActive)
    ));
}

#[test]
fn test_remote_frame_without_pairing_token_or_non_tailnet_ip_rejected_at_first_frame_before_event_bus() {
    let backend = MockTailscaleBackend {
        status_json: SAMPLE_TS_STATUS_JSON.to_string(),
        reset_calls: Arc::new(AtomicUsize::new(0)),
    };
    let mut mgr = TailscaleTunnelManager::new(47400);
    let _ = mgr.enable_away_mode(&backend, "seed-sec", 5_000).unwrap();

    // 1) Non-Tailnet public/rogue IP rejected immediately before EventBus
    assert!(is_tailnet_ip("100.64.0.1"));
    assert!(!is_tailnet_ip("8.8.8.8"));
    let bad_ip_hello = serde_json::to_string(&MikuLinkEnvelopeV1 {
        v: MIKU_PROTOCOL_VERSION_V1,
        id: "h-rogue".to_string(),
        reply_to: None,
        msg: MikuLinkMessageV1::Hello {
            client: "attacker".to_string(),
            token: mgr.active_session().unwrap().session_token.clone(),
            capabilities: vec![],
        },
    })
    .unwrap();
    let err_ip = mgr
        .authenticate_first_frame_fail_closed("203.0.113.5", true, &bad_ip_hello)
        .expect_err("non-Tailnet IP rejected before EventBus");
    assert!(matches!(err_ip, TailscaleTunnelError::UntrustedPeerIp(_)));
    assert_eq!(mgr.event_bus_dispatches, 0);

    // 2) First frame is Command/Ping instead of Hello -> rejected before EventBus
    let non_hello = serde_json::to_string(&MikuLinkEnvelopeV1 {
        v: MIKU_PROTOCOL_VERSION_V1,
        id: "c-unauth".to_string(),
        reply_to: None,
        msg: MikuLinkMessageV1::Command {
            session_id: "s-1".to_string(),
            action: "prompt".to_string(),
            text: "ls".to_string(),
        },
    })
    .unwrap();
    let err_first = mgr
        .authenticate_first_frame_fail_closed("100.91.42.99", true, &non_hello)
        .expect_err("non-Hello first frame rejected before EventBus");
    assert!(matches!(
        err_first,
        TailscaleTunnelError::FirstFrameRejectedBeforeEventBus(_)
    ));
    assert_eq!(mgr.event_bus_dispatches, 0);

    // 3) Hello with wrong pairing token -> rejected before EventBus
    let bad_tok_hello = serde_json::to_string(&MikuLinkEnvelopeV1 {
        v: MIKU_PROTOCOL_VERSION_V1,
        id: "h-badtok".to_string(),
        reply_to: None,
        msg: MikuLinkMessageV1::Hello {
            client: "pixel-9".to_string(),
            token: "wrong_token".to_string(),
            capabilities: vec![],
        },
    })
    .unwrap();
    let err_tok = mgr
        .authenticate_first_frame_fail_closed("100.91.42.99", true, &bad_tok_hello)
        .expect_err("invalid token must never touch EventBus");
    assert!(matches!(
        err_tok,
        TailscaleTunnelError::FirstFrameRejectedBeforeEventBus(_)
    ));
    assert_eq!(mgr.event_bus_dispatches, 0);
}

#[test]
fn test_tailscale_doctor_check_is_fail_closed_and_probes_windows_cli() {
    let backend_ok = MockTailscaleBackend {
        status_json: SAMPLE_TS_STATUS_JSON.to_string(),
        reset_calls: Arc::new(AtomicUsize::new(0)),
    };
    let mgr = TailscaleTunnelManager::new(47400);
    let doc_ok = mgr.doctor_check(&backend_ok);
    assert!(doc_ok.healthy);
    assert!(doc_ok.backend_running);
    assert_eq!(
        doc_ok.assigned_endpoint.as_deref(),
        Some("wss://miku-laptop.tailnet-abc.ts.net:47400/ws")
    );

    // Also run real Windows probe cleanly (must degrade fail-closed if Tailscale CLI is not installed yet)
    let live_doc = mgr.doctor_check(&WindowsTailscaleCliBackend::new());
    if !live_doc.cli_available {
        assert!(!live_doc.healthy);
        assert!(live_doc.remediation.is_some());
    }
}

#[test]
fn test_tailscale_modules_comply_with_sub_600_line_modular_budget() {
    let crate_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for name in ["lib.rs", "tailscale.rs", "tests.rs"] {
        let lines = fs::read_to_string(crate_src.join(name))
            .unwrap()
            .lines()
            .count();
        assert!(lines < 600, "{} has {} lines (must be < 600)", name, lines);
    }
}
