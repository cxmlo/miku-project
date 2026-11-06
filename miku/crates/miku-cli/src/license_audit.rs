use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use miku_protocol::sha256_hex;
use serde::{Deserialize, Serialize};

pub const EXPECTED_WORKSPACE_CRATE_COUNT: usize = 10;

pub const ALLOWED_PERMISSIVE_SPDX_IDS: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "Unicode-DFS-2016",
    "Unicode-3.0",
    "CC0-1.0",
    "Zlib",
    "ISC",
    "Unlicense",
];

pub const FORBIDDEN_COPYLEFT_PREFIXES: &[&str] = &[
    "GPL",
    "AGPL",
    "LGPL",
    "MPL",
    "EUPL",
    "SSPL",
    "CC-BY-SA",
    "CPAL",
    "OSL",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LicenseAuditError {
    IoError(String),
    InvalidLicenseFile(String),
    LicenseMismatchBetweenRootAndMiku,
    ManifestParseError(String),
    MissingWorkspaceMitLicense,
    CrateLicenseViolation {
        crate_name: String,
        found: Option<String>,
    },
    UnexpectedWorkspaceCrateCount {
        expected: usize,
        found: usize,
    },
    CopyleftForbidden {
        package: String,
        version: String,
        spdx_expression: String,
    },
    UnapprovedSpdxLicense {
        package: String,
        version: String,
        spdx_expression: String,
    },
    CiWorkflowInvalid(String),
    CargoDenyConfigInvalid(String),
}

impl std::fmt::Display for LicenseAuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LicenseAuditError::IoError(msg) => write!(f, "I/O error: {}", msg),
            LicenseAuditError::InvalidLicenseFile(msg) => {
                write!(f, "invalid LICENSE file: {}", msg)
            }
            LicenseAuditError::LicenseMismatchBetweenRootAndMiku => {
                write!(f, "root LICENSE and miku/LICENSE do not match byte-for-byte")
            }
            LicenseAuditError::ManifestParseError(msg) => {
                write!(f, "TOML manifest parse error: {}", msg)
            }
            LicenseAuditError::MissingWorkspaceMitLicense => {
                write!(f, "[workspace.package] in miku/Cargo.toml must set license = \"MIT\"")
            }
            LicenseAuditError::CrateLicenseViolation { crate_name, found } => write!(
                f,
                "crate '{}' does not declare MIT license (found: {:?})",
                crate_name, found
            ),
            LicenseAuditError::UnexpectedWorkspaceCrateCount { expected, found } => write!(
                f,
                "expected {} workspace crates, found {}",
                expected, found
            ),
            LicenseAuditError::CopyleftForbidden {
                package,
                version,
                spdx_expression,
            } => write!(
                f,
                "restrictive copyleft license forbidden in '{}' v{}: '{}'",
                package, version, spdx_expression
            ),
            LicenseAuditError::UnapprovedSpdxLicense {
                package,
                version,
                spdx_expression,
            } => write!(
                f,
                "unapproved SPDX license in '{}' v{}: '{}'",
                package, version, spdx_expression
            ),
            LicenseAuditError::CiWorkflowInvalid(msg) => {
                write!(f, "invalid GitHub Actions CI workflow: {}", msg)
            }
            LicenseAuditError::CargoDenyConfigInvalid(msg) => {
                write!(f, "invalid miku/deny.toml configuration: {}", msg)
            }
        }
    }
}

impl std::error::Error for LicenseAuditError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrateLicenseRecord {
    pub crate_name: String,
    pub version: String,
    pub spdx_license: String,
    pub inherited_from_workspace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyLicenseRecord {
    pub name: String,
    pub version: String,
    pub is_workspace_member: bool,
    pub spdx_expression: String,
    pub approved_permissive: bool,
    pub is_copyleft: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LicenseGovernanceReport {
    pub license_sha256: String,
    pub workspace_crates_mit_count: usize,
    pub total_locked_packages_audited: usize,
    pub third_party_packages_audited: usize,
    pub copyleft_packages_found: usize,
    pub ci_workflow_verified: bool,
    pub cargo_deny_verified: bool,
    pub audit_duration_ms: f64,
    pub crates: Vec<CrateLicenseRecord>,
}

/// Validates that a raw `LICENSE` string contains the canonical OSI MIT License text,
/// the official Miku copyright holders, and the F-30 / ADR-010 unified asset scope.
pub fn verify_mit_license_text(raw_text: &str) -> Result<String, LicenseAuditError> {
    let trimmed = raw_text.trim();
    if !trimmed.starts_with("MIT License") {
        return Err(LicenseAuditError::InvalidLicenseFile(
            "must begin with 'MIT License' header".to_string(),
        ));
    }

    let required_phrases = [
        "Copyright (c) 2026 Miku Desktop Orchestrator Contributors",
        "ADR-010",
        "F-30",
        "rig.toml",
        "design-tokens.json",
        "Permission is hereby granted, free of charge, to any person obtaining a copy",
        "without limitation the rights\nto use, copy, modify, merge, publish, distribute, sublicense, and/or sell",
        "THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND",
    ];

    let normalized = trimmed.replace("\r\n", "\n");
    for phrase in required_phrases {
        if !normalized.contains(phrase) {
            return Err(LicenseAuditError::InvalidLicenseFile(format!(
                "missing required MIT/F-30 clause: '{}'",
                phrase
            )));
        }
    }

    Ok(sha256_hex(normalized.as_bytes()))
}

/// Validates a single crate's `Cargo.toml` content against the workspace default license.
pub fn verify_single_crate_cargo_toml(
    raw_cargo_toml: &str,
    workspace_license: Option<&str>,
) -> Result<CrateLicenseRecord, LicenseAuditError> {
    let val: toml::Value = toml::from_str(raw_cargo_toml)
        .map_err(|e| LicenseAuditError::ManifestParseError(e.to_string()))?;

    let pkg = val.get("package").ok_or_else(|| {
        LicenseAuditError::ManifestParseError("missing [package] section".to_string())
    })?;

    let crate_name = pkg
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| LicenseAuditError::ManifestParseError("missing package.name".to_string()))?
        .to_string();

    let version = pkg
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0.1.0")
        .to_string();

    let lic_field = pkg.get("license");
    match lic_field {
        Some(toml::Value::String(s)) => {
            if s.trim() == "MIT" {
                Ok(CrateLicenseRecord {
                    crate_name,
                    version,
                    spdx_license: "MIT".to_string(),
                    inherited_from_workspace: false,
                })
            } else {
                Err(LicenseAuditError::CrateLicenseViolation {
                    crate_name,
                    found: Some(s.clone()),
                })
            }
        }
        Some(toml::Value::Table(tbl)) => {
            let inherits_ws = tbl
                .get("workspace")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if inherits_ws && workspace_license == Some("MIT") {
                Ok(CrateLicenseRecord {
                    crate_name,
                    version,
                    spdx_license: "MIT".to_string(),
                    inherited_from_workspace: true,
                })
            } else {
                Err(LicenseAuditError::CrateLicenseViolation {
                    crate_name,
                    found: workspace_license.map(|s| s.to_string()),
                })
            }
        }
        _ => Err(LicenseAuditError::CrateLicenseViolation {
            crate_name,
            found: None,
        }),
    }
}

/// Returns the canonical SPDX license expression for known crates in `miku/Cargo.lock`.
pub fn canonical_crates_io_spdx_registry() -> BTreeMap<&'static str, &'static str> {
    let mut map = BTreeMap::new();
    map.insert("bytes", "MIT");
    map.insert("equivalent", "Apache-2.0 OR MIT");
    map.insert("errno", "MIT OR Apache-2.0");
    map.insert("hashbrown", "MIT OR Apache-2.0");
    map.insert("indexmap", "Apache-2.0 OR MIT");
    map.insert("itoa", "MIT OR Apache-2.0");
    map.insert("libc", "MIT OR Apache-2.0");
    map.insert("memchr", "Unlicense OR MIT");
    map.insert("mio", "MIT");
    map.insert("pin-project-lite", "Apache-2.0 OR MIT");
    map.insert("proc-macro2", "MIT OR Apache-2.0");
    map.insert("quote", "MIT OR Apache-2.0");
    map.insert("serde", "MIT OR Apache-2.0");
    map.insert("serde_core", "MIT OR Apache-2.0");
    map.insert("serde_derive", "MIT OR Apache-2.0");
    map.insert("serde_json", "MIT OR Apache-2.0");
    map.insert("serde_spanned", "MIT OR Apache-2.0");
    map.insert("signal-hook-registry", "Apache-2.0 OR MIT");
    map.insert("socket2", "MIT OR Apache-2.0");
    map.insert("syn", "MIT OR Apache-2.0");
    map.insert("tokio", "MIT");
    map.insert("tokio-macros", "MIT");
    map.insert("toml", "MIT OR Apache-2.0");
    map.insert("toml_datetime", "MIT OR Apache-2.0");
    map.insert("toml_edit", "MIT OR Apache-2.0");
    map.insert("toml_write", "MIT OR Apache-2.0");
    map.insert("unicode-ident", "(MIT OR Apache-2.0) AND Unicode-3.0");
    map.insert(
        "wasi",
        "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT",
    );
    map.insert("windows-link", "MIT OR Apache-2.0");
    map.insert("windows-sys", "MIT OR Apache-2.0");
    map.insert("winnow", "MIT");
    map.insert("zmij", "MIT");
    map
}

pub fn contains_forbidden_copyleft_token(spdx_expr: &str) -> bool {
    let cleaned = spdx_expr.replace(['(', ')', '/'], " ");
    for token in cleaned.split_whitespace() {
        let upper = token.trim().to_ascii_uppercase();
        if upper == "OR" || upper == "AND" || upper == "WITH" {
            continue;
        }
        if FORBIDDEN_COPYLEFT_PREFIXES
            .iter()
            .any(|prefix| upper.starts_with(prefix))
        {
            return true;
        }
    }
    false
}

pub fn is_spdx_expression_permissive_approved(spdx_expr: &str) -> bool {
    if contains_forbidden_copyleft_token(spdx_expr) {
        return false;
    }
    let normalized = spdx_expr.replace(['(', ')'], " ");
    let mut found_any_license = false;

    for and_part in normalized.split("AND") {
        let mut or_branch_satisfied = false;
        for or_choice in and_part.split("OR") {
            let candidate = or_choice.trim();
            if candidate.is_empty() {
                continue;
            }
            if ALLOWED_PERMISSIVE_SPDX_IDS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(candidate))
            {
                or_branch_satisfied = true;
                found_any_license = true;
            }
        }
        if !or_branch_satisfied {
            return false;
        }
    }

    found_any_license
}

/// Parses `Cargo.lock` and audits every locked package (workspace + third-party)
/// against the permissive SPDX allowlist and zero-copyleft policy.
pub fn audit_cargo_lock_dependency_graph(
    raw_cargo_lock: &str,
    custom_spdx_overrides: &BTreeMap<String, String>,
) -> Result<Vec<DependencyLicenseRecord>, LicenseAuditError> {
    let parsed: toml::Value = toml::from_str(raw_cargo_lock)
        .map_err(|e| LicenseAuditError::ManifestParseError(e.to_string()))?;

    let packages = parsed
        .get("package")
        .and_then(|p| p.as_array())
        .ok_or_else(|| {
            LicenseAuditError::ManifestParseError(
                "Cargo.lock missing [[package]] array".to_string(),
            )
        })?;

    let registry = canonical_crates_io_spdx_registry();
    let mut records = Vec::with_capacity(packages.len());

    for pkg in packages {
        let name = pkg
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                LicenseAuditError::ManifestParseError("package missing name".to_string())
            })?
            .to_string();
        let version = pkg
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0.0.0")
            .to_string();
        let is_workspace_member = pkg.get("source").is_none();

        let spdx_expression = if let Some(overridden) = custom_spdx_overrides.get(&name) {
            overridden.clone()
        } else if is_workspace_member && name.starts_with("miku-") {
            "MIT".to_string()
        } else if let Some(known) = registry.get(name.as_str()) {
            (*known).to_string()
        } else {
            return Err(LicenseAuditError::UnapprovedSpdxLicense {
                package: name,
                version,
                spdx_expression: "UNKNOWN".to_string(),
            });
        };

        let is_copyleft = contains_forbidden_copyleft_token(&spdx_expression);
        if is_copyleft {
            return Err(LicenseAuditError::CopyleftForbidden {
                package: name,
                version,
                spdx_expression,
            });
        }

        let approved_permissive = is_spdx_expression_permissive_approved(&spdx_expression);
        if !approved_permissive {
            return Err(LicenseAuditError::UnapprovedSpdxLicense {
                package: name,
                version,
                spdx_expression,
            });
        }

        records.push(DependencyLicenseRecord {
            name,
            version,
            is_workspace_member,
            spdx_expression,
            approved_permissive,
            is_copyleft,
        });
    }

    Ok(records)
}

/// Validates `.github/workflows/ci.yml` and `miku/deny.toml` governance contracts.
pub fn verify_ci_workflow_and_deny_toml(
    raw_ci_yml: &str,
    raw_deny_toml: &str,
) -> Result<(), LicenseAuditError> {
    let required_ci_tokens = [
        "windows-latest",
        "cargo clippy --manifest-path miku/Cargo.toml --all-targets -- -D warnings",
        "cargo test --manifest-path miku/Cargo.toml --workspace",
        "cargo audit",
        "cargo deny",
        "miku-license-audit",
    ];

    for token in required_ci_tokens {
        if !raw_ci_yml.contains(token) {
            return Err(LicenseAuditError::CiWorkflowInvalid(format!(
                "missing required CI step or directive: '{}'",
                token
            )));
        }
    }

    let deny_val: toml::Value = toml::from_str(raw_deny_toml)
        .map_err(|e| LicenseAuditError::CargoDenyConfigInvalid(e.to_string()))?;

    let allow_list = deny_val
        .get("licenses")
        .and_then(|l| l.get("allow"))
        .and_then(|a| a.as_array())
        .ok_or_else(|| {
            LicenseAuditError::CargoDenyConfigInvalid(
                "missing [licenses].allow array in deny.toml".to_string(),
            )
        })?;

    let allowed_strs: Vec<&str> = allow_list.iter().filter_map(|v| v.as_str()).collect();
    if !allowed_strs.contains(&"MIT") || !allowed_strs.contains(&"Apache-2.0") {
        return Err(LicenseAuditError::CargoDenyConfigInvalid(
            "deny.toml [licenses].allow must include MIT and Apache-2.0".to_string(),
        ));
    }

    for s in allowed_strs {
        if contains_forbidden_copyleft_token(s) {
            return Err(LicenseAuditError::CargoDenyConfigInvalid(format!(
                "deny.toml cannot allow copyleft license '{}'",
                s
            )));
        }
    }

    Ok(())
}

pub fn resolve_repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/miku-cli -> crates -> miku -> repo root
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Runs the full end-to-end license & CI governance audit against the live repository on disk.
pub fn run_workspace_license_and_ci_audit(
    repo_root: &Path,
) -> Result<LicenseGovernanceReport, LicenseAuditError> {
    let t0 = Instant::now();

    let root_license_path = repo_root.join("LICENSE");
    let miku_license_path = repo_root.join("miku").join("LICENSE");

    let root_license_text = fs::read_to_string(&root_license_path)
        .map_err(|e| LicenseAuditError::IoError(format!("read LICENSE: {}", e)))?;
    let miku_license_text = fs::read_to_string(&miku_license_path)
        .map_err(|e| LicenseAuditError::IoError(format!("read miku/LICENSE: {}", e)))?;

    let sha_root = verify_mit_license_text(&root_license_text)?;
    let sha_miku = verify_mit_license_text(&miku_license_text)?;
    if sha_root != sha_miku {
        return Err(LicenseAuditError::LicenseMismatchBetweenRootAndMiku);
    }

    let ws_cargo_path = repo_root.join("miku").join("Cargo.toml");
    let ws_cargo_raw = fs::read_to_string(&ws_cargo_path)
        .map_err(|e| LicenseAuditError::IoError(format!("read miku/Cargo.toml: {}", e)))?;
    let ws_val: toml::Value = toml::from_str(&ws_cargo_raw)
        .map_err(|e| LicenseAuditError::ManifestParseError(e.to_string()))?;

    let ws_license = ws_val
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("license"))
        .and_then(|l| l.as_str());

    if ws_license != Some("MIT") {
        return Err(LicenseAuditError::MissingWorkspaceMitLicense);
    }

    let members = ws_val
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .ok_or_else(|| {
            LicenseAuditError::ManifestParseError("missing workspace.members".to_string())
        })?;

    if members.len() != EXPECTED_WORKSPACE_CRATE_COUNT {
        return Err(LicenseAuditError::UnexpectedWorkspaceCrateCount {
            expected: EXPECTED_WORKSPACE_CRATE_COUNT,
            found: members.len(),
        });
    }

    let mut crate_records = Vec::with_capacity(members.len());
    for member in members {
        let rel = member.as_str().ok_or_else(|| {
            LicenseAuditError::ManifestParseError("non-string member in workspace".to_string())
        })?;
        let member_toml_path = repo_root.join("miku").join(rel).join("Cargo.toml");
        let member_raw = fs::read_to_string(&member_toml_path).map_err(|e| {
            LicenseAuditError::IoError(format!("read {}: {}", member_toml_path.display(), e))
        })?;
        let record = verify_single_crate_cargo_toml(&member_raw, ws_license)?;
        crate_records.push(record);
    }

    let lock_path = repo_root.join("miku").join("Cargo.lock");
    let lock_raw = fs::read_to_string(&lock_path)
        .map_err(|e| LicenseAuditError::IoError(format!("read miku/Cargo.lock: {}", e)))?;
    let dep_records = audit_cargo_lock_dependency_graph(&lock_raw, &BTreeMap::new())?;

    let third_party_count = dep_records
        .iter()
        .filter(|r| !r.is_workspace_member)
        .count();
    let copyleft_count = dep_records.iter().filter(|r| r.is_copyleft).count();

    let ci_path = repo_root.join(".github").join("workflows").join("ci.yml");
    let ci_raw = fs::read_to_string(&ci_path)
        .map_err(|e| LicenseAuditError::IoError(format!("read .github/workflows/ci.yml: {}", e)))?;
    let deny_path = repo_root.join("miku").join("deny.toml");
    let deny_raw = fs::read_to_string(&deny_path)
        .map_err(|e| LicenseAuditError::IoError(format!("read miku/deny.toml: {}", e)))?;

    verify_ci_workflow_and_deny_toml(&ci_raw, &deny_raw)?;

    let duration_ms = t0.elapsed().as_secs_f64() * 1_000.0;
    Ok(LicenseGovernanceReport {
        license_sha256: sha_root,
        workspace_crates_mit_count: crate_records.len(),
        total_locked_packages_audited: dep_records.len(),
        third_party_packages_audited: third_party_count,
        copyleft_packages_found: copyleft_count,
        ci_workflow_verified: true,
        cargo_deny_verified: true,
        audit_duration_ms: duration_ms,
        crates: crate_records,
    })
}

#[cfg(test)]
mod tests {
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
}
