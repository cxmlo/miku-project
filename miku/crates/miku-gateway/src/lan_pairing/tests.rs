use std::path::Path;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::{
    compute_cert_spki_pin_sha256, render_pairing_qr_svg, LanPairingConfig, LanPairingError,
    LanPairingManager, DEFAULT_QR_TTL_MS,
};

fn test_pin() -> String {
    compute_cert_spki_pin_sha256(b"MIKU-LAN-SELF-SIGNED-SPKI-DER-FIXTURE-V1")
}

#[test]
fn test_lan_pairing_disabled_by_default_with_zero_listeners_and_svg_qr_card() {
    let default_cfg = LanPairingConfig::default();
    let mgr = LanPairingManager::new(default_cfg);
    assert!(!mgr.is_lan_enabled());
    assert_eq!(mgr.active_lan_overhead_listeners(), 0);

    let pin = test_pin();
    let uri = format!("miku-link://pair?v=1&host=192.168.1.50&port=47400&nonce=abc&pin=sha256:{pin}&exp=60000");
    let svg = render_pairing_qr_svg(&uri);
    assert!(svg.starts_with("<svg "));
    assert!(svg.contains("Miku-Link Pairing QR"));
}

#[test]
fn test_expired_or_reused_or_bruteforced_qr_code_is_rejected_and_tls_pin_checked() {
    let pin = test_pin();
    let mut mgr = LanPairingManager::new(LanPairingConfig {
        enabled_by_user: true,
        lan_bind_host: "192.168.1.20".to_string(),
        port: 47400,
        pinned_cert_sha256: pin.clone(),
        qr_ttl_ms: DEFAULT_QR_TTL_MS,
    });

    // 1) Expired (> 60s) rejected
    let qr_exp = mgr.generate_single_use_qr(1_000).unwrap();
    let exp_err = mgr
        .redeem_qr_pairing("pixel-8", &qr_exp.pairing_nonce, &pin, 62_000)
        .expect_err("expired QR must fail");
    assert_eq!(exp_err, LanPairingError::QrExpired);

    // 2) TLS pinning mismatch rejected
    let qr = mgr.generate_single_use_qr(100_000).unwrap();
    let pin_err = mgr
        .redeem_qr_pairing("pixel-8", &qr.pairing_nonce, "wrong-pin", 101_000)
        .expect_err("wrong TLS pin must fail");
    assert_eq!(pin_err, LanPairingError::TlsPinMismatch);

    // 3) Valid redemption succeeds once, second scan of same QR rejected
    let session = mgr
        .redeem_qr_pairing("pixel-8", &qr.pairing_nonce, &pin, 102_000)
        .expect("valid pairing succeeds");
    assert_eq!(session.device_id, "pixel-8");
    let reuse_err = mgr
        .redeem_qr_pairing("other-phone", &qr.pairing_nonce, &pin, 103_000)
        .expect_err("second use of QR must be rejected");
    assert_eq!(reuse_err, LanPairingError::QrAlreadyUsed);

    // 4) Rate limit: 3 bad attempts invalidate active QR immediately
    let qr_bf = mgr.generate_single_use_qr(200_000).unwrap();
    for i in 0..2 {
        let e = mgr
            .redeem_qr_pairing("attacker", "bad-nonce", &pin, 200_100 + i)
            .unwrap_err();
        assert_eq!(e, LanPairingError::InvalidPairingNonce);
    }
    let locked = mgr
        .redeem_qr_pairing("attacker", "bad-nonce", &pin, 200_103)
        .unwrap_err();
    assert_eq!(locked, LanPairingError::QrInvalidatedByRateLimit);
    let after = mgr
        .redeem_qr_pairing("pixel-8", &qr_bf.pairing_nonce, &pin, 200_104)
        .unwrap_err();
    assert_eq!(after, LanPairingError::QrInvalidatedByRateLimit);
}

#[tokio::test]
async fn test_pair_again_immediately_closes_active_mobile_sessions_and_revokes_all_tokens() {
    let pin = test_pin();
    let mut mgr = LanPairingManager::new(LanPairingConfig {
        enabled_by_user: true,
        lan_bind_host: "127.0.0.1".to_string(),
        port: 47400,
        pinned_cert_sha256: pin.clone(),
        qr_ttl_ms: DEFAULT_QR_TTL_MS,
    });

    let qr = mgr.generate_single_use_qr(10_000).unwrap();
    let sess = mgr
        .redeem_qr_pairing("android-1", &qr.pairing_nonce, &pin, 10_500)
        .unwrap();
    assert!(mgr.authenticate_mobile_session(&sess.session_token, &pin).is_ok());

    // Live TCP session representing active mobile WebSocket subscribed to epoch watch
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut rx_epoch = mgr.subscribe_epoch_revocation();
    let initial_epoch = *rx_epoch.borrow();

    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let _ = socket.write_all(b"AUTH_OK\n").await;
        loop {
            tokio::select! {
                _ = rx_epoch.changed() => {
                    if *rx_epoch.borrow() != initial_epoch {
                        let _ = socket.write_all(b"REVOKED_CLOSE\n").await;
                        let _ = socket.shutdown().await;
                        break;
                    }
                }
            }
        }
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    let mut first = [0u8; 32];
    let n = client.read(&mut first).await.unwrap();
    assert_eq!(&first[..n], b"AUTH_OK\n");

    // Trigger "Pair again" -> invalidates all tokens and closes open TCP/WS stream
    let revoked = mgr.pair_again_revoke_all();
    assert_eq!(revoked, 1);
    assert_eq!(mgr.active_paired_device_count(), 0);
    assert_eq!(
        mgr.authenticate_mobile_session(&sess.session_token, &pin)
            .unwrap_err(),
        LanPairingError::UnknownOrRevokedSessionToken
    );

    let mut closed_msg = [0u8; 32];
    let m = tokio::time::timeout(Duration::from_secs(2), client.read(&mut closed_msg))
        .await
        .expect("revoke close notification timeout")
        .unwrap();
    assert_eq!(&closed_msg[..m], b"REVOKED_CLOSE\n");
    server.await.unwrap();
}

#[test]
fn test_lan_pairing_modules_comply_with_sub_600_line_modular_budget() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("lan_pairing");
    let mut files = 0usize;
    let mut max_lines = 0usize;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let p = entry.unwrap().path();
        if p.extension().and_then(|e| e.to_str()) == Some("rs") {
            let lines = std::fs::read_to_string(&p).unwrap().lines().count();
            files += 1;
            max_lines = max_lines.max(lines);
            assert!(lines < 600, "module {} exceeded 600 lines ({lines})", p.display());
        }
    }
    println!("LAN_PAIRING_4_8_MODULAR_AUDIT: files={files} max_lines={max_lines} (< 600)");
    assert_eq!(files, 4);
}
