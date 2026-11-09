use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use miku_gatekeeper::{
    Capability, DenyReason, GatekeeperEngine, PolicyEffect, PolicyRule, ProjectProfile,
};

use crate::{
    normalize_os_path_for_jail, SkillEngine, SkillError, SkillManifest, SkillReloadOutcome,
    MAX_HOT_RELOAD_LATENCY_MS,
};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(crate) fn make_temp_test_dir(prefix: &str) -> PathBuf {
    let n = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "miku_skills_3_10_{}_{}_{}",
        prefix,
        std::process::id(),
        n
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

pub(crate) fn build_test_gatekeeper(project_root: &str) -> GatekeeperEngine {
    let mut profile = ProjectProfile::new("miku-skills-test", project_root, 30_000);
    profile.rules = vec![
        PolicyRule {
            capability: Capability::FsRead,
            pattern: "*".to_string(),
            effect: PolicyEffect::Allow,
            pc_only: false,
        },
        PolicyRule {
            capability: Capability::Shell,
            pattern: "cargo test*".to_string(),
            effect: PolicyEffect::Allow,
            pc_only: false,
        },
        PolicyRule {
            capability: Capability::Shell,
            pattern: "cargo check*".to_string(),
            effect: PolicyEffect::Allow,
            pc_only: false,
        },
        PolicyRule {
            capability: Capability::Shell,
            pattern: "rm -rf*".to_string(),
            effect: PolicyEffect::Deny,
            pc_only: false,
        },
        PolicyRule {
            capability: Capability::Network,
            pattern: "https://api.github.com/*".to_string(),
            effect: PolicyEffect::Allow,
            pc_only: false,
        },
        PolicyRule {
            capability: Capability::Network,
            pattern: "https://evil.example.com/*".to_string(),
            effect: PolicyEffect::Deny,
            pc_only: false,
        },
    ];
    GatekeeperEngine::new(profile).expect("valid GatekeeperEngine")
}

pub(crate) fn build_network_denied_gatekeeper(project_root: &str) -> GatekeeperEngine {
    let mut profile = ProjectProfile::new("miku-skills-net-denied", project_root, 30_000);
    profile.rules = vec![
        PolicyRule {
            capability: Capability::FsRead,
            pattern: "*".to_string(),
            effect: PolicyEffect::Allow,
            pc_only: false,
        },
        PolicyRule {
            capability: Capability::Network,
            pattern: "*".to_string(),
            effect: PolicyEffect::Deny,
            pc_only: false,
        },
    ];
    GatekeeperEngine::new(profile).expect("valid GatekeeperEngine with Network Deny rule")
}

// ============================================================================
// LEDGER-3.10-1: Carga de manifiesto válido y rechazo de manifiestos malformados,
//                campos desconocidos, versiones inválidas, capacidades desconocidas
//                y rutas fuera de raíz autorizada.
// ============================================================================
#[test]
fn test_skill_manifest_valid_load_and_rejects_malformed_unknown_fields_bad_semver_and_path_escape()
{
    let temp = make_temp_test_dir("ledger_3_10_1");
    let skill_dir = temp.join("alpha-inspector");
    fs::create_dir_all(&skill_dir).expect("create skill_dir");
    fs::write(skill_dir.join("README.md"), "# Alpha Inspector\n").expect("write entrypoint");

    let valid_toml = r#"
schema_version = 1

[skill]
id = "alpha-inspector"
name = "Alpha Inspector"
version = "1.2.3"
description = "Inspects workspace files safely."
entrypoint = "README.md"
capabilities = ["fs_read", "shell"]
allowed_targets = ["Cargo.toml", "crates/**"]

[limits]
timeout_ms = 2500
max_output_bytes = 16384
isolated_process = true
"#;
    fs::write(skill_dir.join("skill.toml"), valid_toml).expect("write skill.toml");

    let loaded = SkillManifest::load_from_dir(&skill_dir).expect("valid skill.toml must load");
    assert_eq!(loaded.id, "alpha-inspector");
    assert_eq!(loaded.version.to_string(), "1.2.3");
    assert_eq!(loaded.entrypoint_rel, "README.md");
    assert_eq!(
        loaded.capabilities,
        vec![Capability::FsRead, Capability::Shell]
    );
    assert_eq!(loaded.limits.timeout_ms, 2500);
    assert_eq!(loaded.limits.max_output_bytes, 16384);
    assert_eq!(loaded.manifest_sha256.len(), 64);

    let jail_root = normalize_os_path_for_jail(&skill_dir);

    // 1. Malformed TOML syntax
    let malformed = "schema_version = [unclosed";
    assert!(matches!(
        SkillManifest::parse_and_validate(malformed, &jail_root),
        Err(SkillError::ManifestParseError(_))
    ));

    // 2. Unknown fields rejected by #[serde(deny_unknown_fields)]
    let unknown_field_toml = r#"
schema_version = 1
unexpected_root_key = "injected"

[skill]
id = "alpha-inspector"
name = "Alpha Inspector"
version = "1.0.0"
description = "Test"
entrypoint = "README.md"
capabilities = ["fs_read"]
"#;
    assert!(matches!(
        SkillManifest::parse_and_validate(unknown_field_toml, &jail_root),
        Err(SkillError::ManifestParseError(_))
    ));

    // 3. Invalid SemVer rejected
    for bad_ver in ["1.0", "v1.2.3", "01.2.3", "1.2.beta", ""] {
        let bad_ver_toml = valid_toml.replace("1.2.3", bad_ver);
        assert!(
            matches!(
                SkillManifest::parse_and_validate(&bad_ver_toml, &jail_root),
                Err(SkillError::InvalidSemVer(_))
            ),
            "expected InvalidSemVer for '{}'",
            bad_ver
        );
    }

    // 4. Unknown capability strings (including `process_kill`) rejected at manifest parse time
    for unknown_cap in ["process_kill", "kernel_ring0_driver"] {
        let unknown_cap_toml = valid_toml.replace("\"shell\"", &format!("\"{}\"", unknown_cap));
        assert!(
            matches!(
                SkillManifest::parse_and_validate(&unknown_cap_toml, &jail_root),
                Err(SkillError::UnknownCapability(ref cap)) if cap == unknown_cap
            ),
            "expected UnknownCapability('{}') at manifest parse",
            unknown_cap
        );
    }

    // 5. Path escape outside skill directory rejected by CanonicalPathJail
    for escape_ep in [
        "../outside.sh",
        "../../Windows/System32/cmd.exe",
        "C:/Windows/System32/cmd.exe",
        "/etc/passwd",
        "sub/../../escape.md",
        "CON.txt",
    ] {
        let escape_toml = valid_toml.replace("README.md", escape_ep);
        assert!(
            matches!(
                SkillManifest::parse_and_validate(&escape_toml, &jail_root),
                Err(SkillError::PathJailViolation(_))
            ),
            "expected PathJailViolation for entrypoint '{}'",
            escape_ep
        );
    }

    let _ = fs::remove_dir_all(&temp);
}

// ============================================================================
// LEDGER-3.10-2: Seguridad de capacidades y Gatekeeper: una Skill sin `network`
//                no puede realizar acciones de red; una Skill con capacidad
//                declarada pero no permitida por Gatekeeper también queda bloqueada.
// ============================================================================
#[test]
fn test_skill_capability_ceiling_blocks_undeclared_network_and_gatekeeper_blocks_declared_unpermitted_capability(
) {
    let temp = make_temp_test_dir("ledger_3_10_2");
    let root_jail = normalize_os_path_for_jail(&temp);
    let mut gatekeeper = build_test_gatekeeper(&root_jail);
    let mut engine = SkillEngine::new();

    // Skill A declares ONLY ["fs_read"] (no "network", no "shell")
    let skill_a_dir = temp.join("read-only-skill");
    fs::create_dir_all(&skill_a_dir).expect("create skill_a_dir");
    fs::write(skill_a_dir.join("README.md"), "# Read Only\n").expect("write README");
    let skill_a_toml = r#"
schema_version = 1

[skill]
id = "read-only-skill"
name = "Read Only Skill"
version = "1.0.0"
description = "Only declares fs_read."
entrypoint = "README.md"
capabilities = ["fs_read"]
"#;
    fs::write(skill_a_dir.join("skill.toml"), skill_a_toml).expect("write skill.toml");

    engine
        .load_or_reload_from_dir(&skill_a_dir, &gatekeeper, 1_000)
        .expect("load read-only-skill");

    // 1. Skill without `network` attempting a network action MUST be blocked with UndeclaredSkillCapability
    let net_attempt = engine.authorize_skill_action(
        "read-only-skill",
        Capability::Network,
        "https://api.github.com/repos",
        "GET",
        &mut gatekeeper,
        1_010,
    );
    assert_eq!(
        net_attempt,
        Err(SkillError::UndeclaredSkillCapability {
            skill_id: "read-only-skill".to_string(),
            capability: Capability::Network,
        })
    );

    // 2. Skill B declares `["network", "shell"]` in skill.toml.
    let skill_b_dir = temp.join("net-shell-skill");
    fs::create_dir_all(&skill_b_dir).expect("create skill_b_dir");
    fs::write(skill_b_dir.join("README.md"), "# Net & Shell\n").expect("write README");
    let skill_b_toml = r#"
schema_version = 1

[skill]
id = "net-shell-skill"
name = "Net And Shell Skill"
version = "1.0.0"
description = "Declares network and shell."
entrypoint = "README.md"
capabilities = ["network", "shell"]
"#;
    fs::write(skill_b_dir.join("skill.toml"), skill_b_toml).expect("write skill.toml");

    engine
        .load_or_reload_from_dir(&skill_b_dir, &gatekeeper, 1_020)
        .expect("load net-shell-skill");

    // 2a. Target explicitly denied by Gatekeeper -> GatekeeperDenied(ExplicitDenyRule)
    let denied_net = engine.authorize_skill_action(
        "net-shell-skill",
        Capability::Network,
        "https://evil.example.com/exfil",
        "POST",
        &mut gatekeeper,
        1_030,
    );
    assert!(matches!(
        denied_net,
        Err(SkillError::GatekeeperDenied {
            reason: DenyReason::ExplicitDenyRule,
            ..
        })
    ));

    // 2b. Target not matching any Allow rule -> GatekeeperDenied(DefaultDeny)
    let default_deny_net = engine.authorize_skill_action(
        "net-shell-skill",
        Capability::Network,
        "https://unlisted.example.org",
        "GET",
        &mut gatekeeper,
        1_040,
    );
    assert!(matches!(
        default_deny_net,
        Err(SkillError::GatekeeperDenied {
            reason: DenyReason::DefaultDeny,
            ..
        })
    ));

    // 2c. Target allowed by both skill.toml and Gatekeeper -> Allow
    let allowed_net = engine
        .authorize_skill_action(
            "net-shell-skill",
            Capability::Network,
            "https://api.github.com/repos",
            "GET",
            &mut gatekeeper,
            1_050,
        )
        .expect("allowed by both skill manifest and Gatekeeper");
    assert_eq!(allowed_net.skill_id, "net-shell-skill");
    assert_eq!(allowed_net.capability, Capability::Network);

    // 3. Skill C requesting valid capability `network` against a Gatekeeper profile with a `Deny` rule on `Network`
    let net_denied_gatekeeper = build_network_denied_gatekeeper(&root_jail);
    let skill_c_dir = temp.join("forbidden-cap-skill");
    fs::create_dir_all(&skill_c_dir).expect("create skill_c_dir");
    fs::write(skill_c_dir.join("README.md"), "# Forbidden\n").expect("write README");
    let skill_c_toml = r#"
schema_version = 1

[skill]
id = "forbidden-cap-skill"
name = "Forbidden Capability Skill"
version = "1.0.0"
description = "Requests network which is explicitly denied by profile."
entrypoint = "README.md"
capabilities = ["network"]
"#;
    fs::write(skill_c_dir.join("skill.toml"), skill_c_toml).expect("write skill.toml");

    let load_c = engine.load_or_reload_from_dir(&skill_c_dir, &net_denied_gatekeeper, 1_060);
    assert!(matches!(
        load_c,
        Err(SkillError::CapabilityForbiddenByGatekeeper {
            capability: Capability::Network,
            ..
        })
    ));

    gatekeeper
        .audit_log()
        .verify_chain()
        .expect("audit log chain must remain valid");
    let _ = fs::remove_dir_all(&temp);
}

// ============================================================================
// LEDGER-3.10-3: Hot-reload transaccional: una edición válida reemplaza la versión
//                activa; una edición inválida o con capacidades prohibidas falla
//                y mantiene intacta la versión anterior.
// ============================================================================
#[test]
fn test_transactional_hot_reload_swaps_valid_update_and_preserves_active_version_on_invalid_or_forbidden_manifest(
) {
    let temp = make_temp_test_dir("ledger_3_10_3");
    let root_jail = normalize_os_path_for_jail(&temp);
    let mut gatekeeper = build_network_denied_gatekeeper(&root_jail);
    let mut engine = SkillEngine::new();

    let skill_dir = temp.join("hot-reload-skill");
    fs::create_dir_all(&skill_dir).expect("create skill_dir");
    fs::write(skill_dir.join("README.md"), "# Hot Reload Skill\n").expect("write entrypoint");

    let v1_toml = r#"
schema_version = 1

[skill]
id = "hot-reload-skill"
name = "Hot Reload Skill"
version = "1.0.0"
description = "Initial valid v1.0.0 skill."
entrypoint = "README.md"
capabilities = ["fs_read"]
"#;
    fs::write(skill_dir.join("skill.toml"), v1_toml).expect("write v1 skill.toml");

    let initial = engine
        .load_or_reload_from_dir(&skill_dir, &gatekeeper, 2_000)
        .expect("initial load v1.0.0");
    assert!(matches!(
        initial,
        SkillReloadOutcome::Loaded {
            ref version,
            generation: 1,
            ..
        } if version == "1.0.0"
    ));

    // Step 1: Valid hot-reload to v1.1.0 replaces the active version atomically
    let v1_1_toml = v1_toml
        .replace("1.0.0", "1.1.0")
        .replace("Initial valid v1.0.0 skill.", "Updated valid v1.1.0 skill.");
    fs::write(skill_dir.join("skill.toml"), &v1_1_toml).expect("write v1.1.0 skill.toml");

    let reload_ok = engine
        .load_or_reload_from_dir(&skill_dir, &gatekeeper, 2_100)
        .expect("valid reload to v1.1.0");
    match reload_ok {
        SkillReloadOutcome::Reloaded {
            previous_version,
            new_version,
            generation,
            latency_ms,
            ..
        } => {
            assert_eq!(previous_version, "1.0.0");
            assert_eq!(new_version, "1.1.0");
            assert_eq!(generation, 2);
            assert!(
                latency_ms < MAX_HOT_RELOAD_LATENCY_MS,
                "hot-reload latency {:.3} ms exceeded budget {:.1} ms",
                latency_ms,
                MAX_HOT_RELOAD_LATENCY_MS
            );
        }
        other => panic!("expected Reloaded outcome, got {:?}", other),
    }

    // Step 2: Corrupt TOML edit is rejected and v1.1.0 remains active and operational
    let corrupt_toml = "schema_version = 1\n[skill\nid = broken_syntax";
    fs::write(skill_dir.join("skill.toml"), corrupt_toml).expect("write corrupt skill.toml");

    let reload_corrupt = engine.load_or_reload_from_dir(&skill_dir, &gatekeeper, 2_200);
    assert!(matches!(
        reload_corrupt,
        Err(SkillError::ManifestParseError(_))
    ));

    let active_after_corrupt = engine
        .get_skill("hot-reload-skill")
        .expect("v1.1.0 must remain active after corrupt reload");
    assert_eq!(active_after_corrupt.manifest.version.to_string(), "1.1.0");
    assert_eq!(active_after_corrupt.reload_generation, 2);

    // Step 3: Edit requesting capability forbidden by Gatekeeper Deny rule (`network`) is rejected and v1.1.0 remains active
    let forbidden_update_toml = v1_1_toml
        .replace("1.1.0", "2.0.0")
        .replace("[\"fs_read\"]", "[\"fs_read\", \"network\"]");
    fs::write(skill_dir.join("skill.toml"), &forbidden_update_toml)
        .expect("write forbidden capability skill.toml");

    let reload_forbidden = engine.load_or_reload_from_dir(&skill_dir, &gatekeeper, 2_300);
    assert!(matches!(
        reload_forbidden,
        Err(SkillError::CapabilityForbiddenByGatekeeper {
            capability: Capability::Network,
            ..
        })
    ));

    let active_after_forbidden = engine
        .get_skill("hot-reload-skill")
        .expect("v1.1.0 must remain active after forbidden capability reload");
    assert_eq!(active_after_forbidden.manifest.version.to_string(), "1.1.0");
    assert_eq!(active_after_forbidden.reload_generation, 2);
    assert_eq!(engine.rejected_reload_count(), 2);
    assert_eq!(
        engine.reload_diagnostics()[1].preserved_active_version,
        Some("1.1.0".to_string())
    );

    // Step 4: Edit with unknown capability string (`process_kill`) is rejected at manifest parse and v1.1.0 remains active
    let unknown_cap_update_toml = v1_1_toml
        .replace("1.1.0", "2.1.0")
        .replace("[\"fs_read\"]", "[\"fs_read\", \"process_kill\"]");
    fs::write(skill_dir.join("skill.toml"), &unknown_cap_update_toml)
        .expect("write unknown capability skill.toml");

    let reload_unknown_cap = engine.load_or_reload_from_dir(&skill_dir, &gatekeeper, 2_350);
    assert!(matches!(
        reload_unknown_cap,
        Err(SkillError::UnknownCapability(ref cap)) if cap == "process_kill"
    ));
    assert_eq!(engine.rejected_reload_count(), 3);

    let auth = engine
        .authorize_skill_action(
            "hot-reload-skill",
            Capability::FsRead,
            "README.md",
            "",
            &mut gatekeeper,
            2_400,
        )
        .expect("active v1.1.0 skill still authorizes fs_read");
    assert_eq!(auth.skill_version, "1.1.0");

    let _ = fs::remove_dir_all(&temp);
}
