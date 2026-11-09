use std::fs;
use std::path::Path;
use std::time::Duration;

use miku_core::build_current_thread_runtime;
use miku_gatekeeper::Capability;

use crate::tests::{build_test_gatekeeper, make_temp_test_dir};
use crate::{
    normalize_os_path_for_jail, resolve_example_skills_dir, SkillEngine, SkillError, SkillFsWatcher,
};

// ============================================================================
// LEDGER-3.10-4: Watcher por eventos del sistema operativo (0 polling): un cambio
//                en disco dispara recarga sin polling; eventos repetidos no dejan
//                estado parcial; el apagado limpia el watcher.
// ============================================================================
#[test]
fn test_os_event_driven_watcher_zero_polling_coalesces_events_and_shuts_down_cleanly() {
    let temp = make_temp_test_dir("ledger_3_10_4");
    let root_jail = normalize_os_path_for_jail(&temp);
    let gatekeeper = build_test_gatekeeper(&root_jail);
    let mut engine = SkillEngine::new();

    let skill_dir = temp.join("watched-skill");
    fs::create_dir_all(&skill_dir).expect("create skill_dir");
    fs::write(skill_dir.join("README.md"), "# Watched Skill\n").expect("write entrypoint");

    let base_toml = r#"
schema_version = 1

[skill]
id = "watched-skill"
name = "Watched Skill"
version = "1.0.0"
description = "Watched by OS event notification."
entrypoint = "README.md"
capabilities = ["fs_read"]
"#;
    fs::write(skill_dir.join("skill.toml"), base_toml).expect("write initial skill.toml");
    engine
        .load_or_reload_from_dir(&skill_dir, &gatekeeper, 3_000)
        .expect("initial load");

    let mut watcher = SkillFsWatcher::start(&temp).expect("start zero-polling OS watcher");
    assert!(watcher.is_running());

    let rt = build_current_thread_runtime().expect("build current_thread runtime");
    rt.block_on(async {
        let updated_toml = base_toml.replace("1.0.0", "1.4.0");
        fs::write(skill_dir.join("skill.toml"), &updated_toml).expect("write v1.4.0 skill.toml");

        for i in 0..5 {
            watcher
                .notify_skill_dir_changed(&skill_dir, 3_100 + i)
                .expect("notify watcher event");
        }

        let summary = tokio::time::timeout(
            Duration::from_secs(2),
            watcher.recv_and_apply_coalesced(&mut engine, &gatekeeper),
        )
        .await
        .expect("watcher event received within timeout")
        .expect("watcher batch summary");

        assert!(summary.raw_events_received >= 5);
        assert_eq!(summary.coalesced_dirs_processed, 1);
        assert_eq!(summary.reloads_applied, 1);
        assert_eq!(summary.reloads_rejected, 0);

        let loaded = engine.get_skill("watched-skill").expect("watched-skill active");
        assert_eq!(loaded.manifest.version.to_string(), "1.4.0");
        assert_eq!(loaded.reload_generation, 2);
    });

    watcher.shutdown();
    assert!(!watcher.is_running());
    assert!(watcher.notify_skill_dir_changed(&skill_dir, 3_500).is_err());

    let _ = fs::remove_dir_all(&temp);
}

// ============================================================================
// LEDGER-3.10-5: Las dos Skills de ejemplo en `skills/examples/` cargan, validan
//                y ejecutan dentro del sandbox.
// ============================================================================
#[test]
fn test_example_skills_in_skills_examples_load_validate_and_execute_sandboxed() {
    let examples_dir = resolve_example_skills_dir();
    assert!(
        examples_dir.is_dir(),
        "skills/examples directory must exist at '{}'",
        examples_dir.display()
    );

    let root_jail = normalize_os_path_for_jail(Path::new(&examples_dir));
    let mut gatekeeper = build_test_gatekeeper(&root_jail);
    let mut engine = SkillEngine::new();

    let outcomes = engine
        .load_all_from_parent_dir(&examples_dir, &gatekeeper, 4_000)
        .expect("load all example skills in skills/examples/");
    assert_eq!(outcomes.len(), 2, "expected exactly 2 example skills");

    let inspector = engine
        .get_skill("repo-status-inspector")
        .expect("repo-status-inspector must be loaded");
    assert_eq!(inspector.manifest.version.to_string(), "1.0.0");
    assert_eq!(inspector.manifest.capabilities, vec![Capability::FsRead]);

    let runner = engine
        .get_skill("cargo-test-runner")
        .expect("cargo-test-runner must be loaded");
    assert_eq!(runner.manifest.version.to_string(), "1.2.0");
    assert_eq!(
        runner.manifest.capabilities,
        vec![Capability::FsRead, Capability::Shell]
    );

    let rt = build_current_thread_runtime().expect("build runtime");
    rt.block_on(async {
        let rep1 = engine
            .execute_skill_isolated(
                "repo-status-inspector",
                Capability::FsRead,
                "Cargo.toml",
                "",
                &mut gatekeeper,
                4_100,
                |auth| async move {
                    Ok(format!(
                        "INSPECTED_OK|skill={}|target={}",
                        auth.skill_id, auth.canonical_target
                    ))
                },
            )
            .await
            .expect("repo-status-inspector sandboxed execution");
        assert!(rep1.output.starts_with("INSPECTED_OK|skill=repo-status-inspector|"));

        let blocked_shell = engine
            .execute_skill_isolated(
                "repo-status-inspector",
                Capability::Shell,
                "cargo test",
                "",
                &mut gatekeeper,
                4_110,
                |_| async move { Ok("should not run".to_string()) },
            )
            .await;
        assert!(matches!(
            blocked_shell,
            Err(SkillError::UndeclaredSkillCapability {
                capability: Capability::Shell,
                ..
            })
        ));

        let rep2 = engine
            .execute_skill_isolated(
                "cargo-test-runner",
                Capability::Shell,
                "cargo test --workspace",
                "--test-threads=1",
                &mut gatekeeper,
                4_120,
                |auth| async move {
                    Ok(format!(
                        "RUNNER_OK|skill={}|cmd={}",
                        auth.skill_id, auth.canonical_target
                    ))
                },
            )
            .await
            .expect("cargo-test-runner sandboxed execution");
        assert_eq!(
            rep2.output,
            "RUNNER_OK|skill=cargo-test-runner|cmd=cargo test --workspace"
        );
    });
}

// ============================================================================
// LEDGER-3.10-6: Aislamiento de ejecución, timeout y límite de salida (`max_output_bytes`)
//                para que un proceso colgado o ruidoso no bloquee el motor.
// ============================================================================
#[test]
fn test_isolated_skill_runner_enforces_timeout_and_max_output_bytes_without_hanging_engine() {
    let temp = make_temp_test_dir("ledger_3_10_6");
    let root_jail = normalize_os_path_for_jail(&temp);
    let mut gatekeeper = build_test_gatekeeper(&root_jail);
    let mut engine = SkillEngine::new();

    let skill_dir = temp.join("bounded-runner-skill");
    fs::create_dir_all(&skill_dir).expect("create skill_dir");
    fs::write(skill_dir.join("README.md"), "# Bounded Skill\n").expect("write entrypoint");

    let bounded_toml = r#"
schema_version = 1

[skill]
id = "bounded-runner-skill"
name = "Bounded Runner Skill"
version = "1.0.0"
description = "Enforces strict 40ms timeout and 256-byte output cap."
entrypoint = "README.md"
capabilities = ["fs_read"]

[limits]
timeout_ms = 40
max_output_bytes = 256
isolated_process = true
"#;
    fs::write(skill_dir.join("skill.toml"), bounded_toml).expect("write skill.toml");
    engine
        .load_or_reload_from_dir(&skill_dir, &gatekeeper, 5_000)
        .expect("load bounded-runner-skill");

    let rt = build_current_thread_runtime().expect("build runtime");
    rt.block_on(async {
        let hung_res = engine
            .execute_skill_isolated(
                "bounded-runner-skill",
                Capability::FsRead,
                "README.md",
                "",
                &mut gatekeeper,
                5_010,
                |_| async move {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    Ok("never reached".to_string())
                },
            )
            .await;
        assert_eq!(
            hung_res,
            Err(SkillError::ExecutionTimeout {
                skill_id: "bounded-runner-skill".to_string(),
                timeout_ms: 40,
            })
        );

        let noisy_res = engine
            .execute_skill_isolated(
                "bounded-runner-skill",
                Capability::FsRead,
                "README.md",
                "",
                &mut gatekeeper,
                5_020,
                |_| async move { Ok("X".repeat(1024)) },
            )
            .await;
        assert_eq!(
            noisy_res,
            Err(SkillError::OutputLimitExceeded {
                skill_id: "bounded-runner-skill".to_string(),
                produced_bytes: 1024,
                max_output_bytes: 256,
            })
        );

        let ok_res = engine
            .execute_skill_isolated(
                "bounded-runner-skill",
                Capability::FsRead,
                "README.md",
                "",
                &mut gatekeeper,
                5_030,
                |_| async move { Ok("healthy output within limits".to_string()) },
            )
            .await
            .expect("healthy invocation after timeout and output-limit rejections");
        assert_eq!(ok_res.output, "healthy output within limits");
    });

    let _ = fs::remove_dir_all(&temp);
}
