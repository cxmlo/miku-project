use std::cell::Cell;
use std::env;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use miku_protocol::sha256_hex;

use super::*;

struct CountingFetcher {
    calls: Cell<usize>,
    payload: Vec<u8>,
}

impl CountingFetcher {
    fn new(payload: Vec<u8>) -> Self {
        Self {
            calls: Cell::new(0),
            payload,
        }
    }
}

impl PayloadFetcher for CountingFetcher {
    fn fetch_bytes(&self, _url: &str) -> Result<Vec<u8>, String> {
        self.calls.set(self.calls.get() + 1);
        Ok(self.payload.clone())
    }
}

fn temp_layout(tag: &str) -> (PathBuf, InstallLayout) {
    let root = env::temp_dir().join(format!(
        "miku_installer_test_{}_{}",
        tag,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let install_dir = root.join("Programs").join("Miku");
    let data_dir = root.join("Miku");
    (root, InstallLayout::from_roots(&install_dir, &data_dir))
}

fn sample_manifest(payload: &[u8]) -> ReleaseManifestV1 {
    ReleaseManifestV1 {
        version: "1.0.0".to_string(),
        channel: "stable".to_string(),
        artifact_name: "miku.exe".to_string(),
        download_url: "https://github.com/miku-project/releases/download/v1.0.0/miku.exe".to_string(),
        sha256_hex: sha256_hex(payload),
        size_bytes: payload.len() as u64,
        release_notes_summary: "Initial verified release".to_string(),
    }
}

// LEDGER-5.4-1: Updater MUST abort and delete downloaded file on SHA-256 mismatch
#[test]
fn test_5_4_updater_aborts_and_deletes_staging_on_sha256_mismatch() {
    let (root, layout) = temp_layout("sha_mismatch");
    fs::create_dir_all(&layout.install_dir).expect("create install dir");
    let original_binary = b"MIKU_ORIGINAL_GOOD_BINARY_V0";
    fs::write(&layout.target_exe, original_binary).expect("seed installed exe");

    let expected_good = b"MIKU_SIGNED_RELEASE_BINARY_V1";
    let manifest = sample_manifest(expected_good);
    let tampered_download = b"MIKU_TAMPERED_PAYLOAD_V1_XX!!";
    assert_eq!(tampered_download.len(), expected_good.len());

    let fetcher = CountingFetcher::new(tampered_download.to_vec());
    let err = apply_verified_update(&manifest, &layout, &fetcher, true)
        .expect_err("tampered SHA-256 must fail update");

    match err {
        UpdateError::IntegrityMismatch {
            staging_cleaned, ..
        } => assert!(staging_cleaned, "staging artifact must be deleted on mismatch"),
        other => panic!("unexpected error variant: {:?}", other),
    }

    let expected_staging = layout.staging_dir.join("miku.exe.staging");
    assert!(
        !expected_staging.exists(),
        "staging file '{}' must NOT linger after SHA-256 mismatch",
        expected_staging.display()
    );
    assert_eq!(
        fs::read(&layout.target_exe).expect("read original binary"),
        original_binary,
        "original binary must remain untouched on hash mismatch"
    );

    let _ = fs::remove_dir_all(&root);
}

// LEDGER-5.4-2: Updater MUST NOT download or stage without explicit user consent
#[test]
fn test_5_4_updater_never_downloads_without_explicit_consent() {
    let (root, layout) = temp_layout("no_consent");
    let payload = b"MIKU_RELEASE_V1_CONSENT_CHECK";
    let manifest = sample_manifest(payload);
    let fetcher = CountingFetcher::new(payload.to_vec());

    let mut out = Vec::new();
    let prompt_consent =
        prompt_for_update_consent(&manifest, false, Cursor::new(b"y\n".to_vec()), &mut out)
            .expect("prompt eval");
    assert!(!prompt_consent, "non-TTY stdin must fail closed");

    let err = apply_verified_update(&manifest, &layout, &fetcher, prompt_consent)
        .expect_err("update without consent must abort");
    assert_eq!(err, UpdateError::ConsentDenied);
    assert_eq!(fetcher.calls.get(), 0, "fetcher must never be called without consent");
    assert!(!layout.staging_dir.exists(), "staging dir must never be created without consent");

    let _ = fs::remove_dir_all(&root);
}

// LEDGER-5.4-3: Full install, verified update swap with backup, and consent-aware uninstall
#[test]
fn test_5_4_install_verified_update_and_uninstall_preserve_or_purge_data() {
    let (root, layout) = temp_layout("full_lifecycle");
    let src_exe = root.join("src_miku.exe");
    fs::create_dir_all(&root).expect("create root");
    let v0_bytes = b"MIKU_V0_BINARY_BYTES";
    fs::write(&src_exe, v0_bytes).expect("write src exe");

    let install_rep = install_local_binary(&src_exe, &layout, false, false).expect("install ok");
    assert_eq!(install_rep.sha256_hex, sha256_hex(v0_bytes));
    assert!(layout.target_exe.exists());

    let vault_file = layout.data_dir.join("vault.db");
    fs::write(&vault_file, b"user_encrypted_vault").expect("write vault");

    let mut bad_url = sample_manifest(b"abc");
    bad_url.download_url = "http://insecure.example.com/miku.exe".to_string();
    assert!(matches!(bad_url.validate(), Err(UpdateError::InvalidManifest(_))));

    let v1_bytes = b"MIKU_V1_0_0_VERIFIED_BYTES";
    let v1_manifest = sample_manifest(v1_bytes);
    let fetcher = CountingFetcher::new(v1_bytes.to_vec());
    let upd_rep =
        apply_verified_update(&v1_manifest, &layout, &fetcher, true).expect("verified update");
    assert_eq!(upd_rep.target_version, "1.0.0");
    assert!(upd_rep.previous_backup_created);
    assert_eq!(fs::read(&layout.target_exe).expect("v1 exe"), v1_bytes);
    assert_eq!(fs::read(&layout.previous_backup_exe).expect("v0 backup"), v0_bytes);
    assert!(!layout.staging_dir.join("miku.exe.staging").exists());

    let mut out_keep = Vec::new();
    let uninst_keep = uninstall_local_binary(
        &layout,
        None,
        false,
        false,
        true,
        Cursor::new(b"n\n".to_vec()),
        &mut out_keep,
    )
    .expect("uninstall keep");
    assert!(uninst_keep.removed_exe);
    assert!(!uninst_keep.data_purged);
    assert!(vault_file.exists(), "user data must survive when user declines purge");

    let mut out_purge = Vec::new();
    let uninst_purge = uninstall_local_binary(
        &layout,
        None,
        false,
        false,
        true,
        Cursor::new(b"y\n".to_vec()),
        &mut out_purge,
    )
    .expect("uninstall purge");
    assert!(uninst_purge.data_purged);
    assert!(!layout.data_dir.exists(), "user data dir must be removed when confirmed");

    let _ = fs::remove_dir_all(&root);
}
