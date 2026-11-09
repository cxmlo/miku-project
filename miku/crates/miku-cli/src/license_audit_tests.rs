use super::*;

// LEDGER-1.9-1
#[test]
fn test_mit_license_file_integrity_copyright_holders_and_f30_asset_scope() {
    let root = resolve_repo_root();
    let license_text =
        fs::read_to_string(root.join("LICENSE")).expect("read root LICENSE file");
    let miku_license_text =
        fs::read_to_string(root.join("miku").join("LICENSE")).expect("read miku/LICENSE file");

    let hash_a = verify_mit_license_text(&license_text).expect("valid root MIT license");
    let hash_b = verify_mit_license_text(&miku_license_text).expect("valid miku MIT license");
    assert_eq!(hash_a, hash_b);
    assert_eq!(hash_a.len(), 64);

    // Tampering or removing the warranty disclaimer must fail
    let tampered = license_text.replace("WITHOUT WARRANTY OF ANY KIND", "WITH SOME WARRANTY");
    assert!(verify_mit_license_text(&tampered).is_err());
}

// LEDGER-1.9-2
#[test]
fn test_all_nine_workspace_crates_enforce_mit_spdx_in_cargo_toml() {
    let root = resolve_repo_root();
    let report =
        run_workspace_license_and_ci_audit(&root).expect("workspace license audit pass");
    assert_eq!(
        report.workspace_crates_mit_count,
        EXPECTED_WORKSPACE_CRATE_COUNT
    );
    for c in &report.crates {
        assert_eq!(c.spdx_license, "MIT");
    }

    // Verify a non-MIT crate manifest is rejected
    let bad_toml = r#"
[package]
name = "miku-bad"
version = "0.1.0"
license = "GPL-3.0-only"
"#;
    let err = verify_single_crate_cargo_toml(bad_toml, Some("MIT")).unwrap_err();
    assert!(matches!(
        err,
        LicenseAuditError::CrateLicenseViolation { .. }
    ));
}

// LEDGER-1.9-3
#[test]
fn test_dependency_graph_spdx_scanner_allows_permissive_and_blocks_copyleft() {
    let root = resolve_repo_root();
    let lock_raw =
        fs::read_to_string(root.join("miku").join("Cargo.lock")).expect("read Cargo.lock");

    let records = audit_cargo_lock_dependency_graph(&lock_raw, &BTreeMap::new())
        .expect("all 32 packages in Cargo.lock are permissive");
    assert!(records.len() >= 30);
    assert!(records
        .iter()
        .all(|r| r.approved_permissive && !r.is_copyleft));

    // Injecting GPL / AGPL / LGPL overrides must be blocked immediately
    for forbidden_spdx in ["GPL-3.0-only", "AGPL-3.0-or-later", "LGPL-2.1-only"] {
        let mut overrides = BTreeMap::new();
        overrides.insert("bytes".to_string(), forbidden_spdx.to_string());
        let res = audit_cargo_lock_dependency_graph(&lock_raw, &overrides);
        assert!(matches!(
            res,
            Err(LicenseAuditError::CopyleftForbidden { .. })
        ));
    }
}

// LEDGER-1.9-4
#[test]
fn test_github_actions_ci_workflow_and_cargo_deny_contracts_are_valid() {
    let root = resolve_repo_root();
    let ci_raw = fs::read_to_string(root.join(".github").join("workflows").join("ci.yml"))
        .expect("read ci.yml");
    let deny_raw =
        fs::read_to_string(root.join("miku").join("deny.toml")).expect("read deny.toml");

    verify_ci_workflow_and_deny_toml(&ci_raw, &deny_raw)
        .expect("CI workflow and deny.toml contracts valid");

    // Removing `-D warnings` from CI workflow must fail validation
    let weakened_ci = ci_raw.replace("-D warnings", "--allow-warnings");
    assert!(verify_ci_workflow_and_deny_toml(&weakened_ci, &deny_raw).is_err());

    // Adding GPL-3.0 to deny.toml allowlist must fail validation
    let polluted_deny = deny_raw.replace("\"MIT\",", "\"MIT\", \"GPL-3.0\",");
    assert!(verify_ci_workflow_and_deny_toml(&ci_raw, &polluted_deny).is_err());
}
