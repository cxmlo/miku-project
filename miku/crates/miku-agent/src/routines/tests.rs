use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use miku_browser::{
    BrowserController, BrowserPageSnapshot, BrowserProtocolKind, DownloadJailPolicy,
    DownloadReceipt,
};
use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, PolicyRule, ProjectProfile};
use miku_platform::{PcControlController, SandboxPcControlBackend, WindowsNativePcControlBackend};

use super::{
    load_routine_toml, save_routine_toml, RoutineExecutor, RoutineHaltReason, RoutineManifest,
    RoutinePrecondition, RoutineStep, RoutineStepAction,
};

struct NoOpBrowserBackend;

impl miku_browser::BrowserBackend for NoOpBrowserBackend {
    fn navigate_and_read_dom<'a>(
        &'a self,
        url: &'a str,
        host: &'a str,
        isolated_profile_used: bool,
        _abort_flag: &'a std::sync::atomic::AtomicBool,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<BrowserPageSnapshot, miku_browser::BrowserError>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            Ok(BrowserPageSnapshot {
                url: url.to_string(),
                host: host.to_string(),
                title: "Routine Mock Page".to_string(),
                dom_text: "zero-token deterministic browser step".to_string(),
                protocol_used: BrowserProtocolKind::GeckoBiDi,
                isolated_profile_used,
            })
        })
    }

    fn download_artifact<'a>(
        &'a self,
        url: &'a str,
        suggested_filename: &'a str,
        jail: &'a DownloadJailPolicy,
        _abort_flag: &'a std::sync::atomic::AtomicBool,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<DownloadReceipt, miku_browser::BrowserError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let p = jail.canonical_jail_dir.join(suggested_filename);
            Ok(DownloadReceipt {
                file_name: suggested_filename.to_string(),
                canonical_path: p.to_string_lossy().to_string(),
                bytes_written: 12,
                source_url: url.to_string(),
            })
        })
    }

    fn active_processes(&self) -> usize {
        0
    }
}

fn fresh_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("miku_routine_{label}_{nanos}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn build_profile(root: &Path, rules: Vec<(Capability, &str, PolicyEffect, bool)>) -> ProjectProfile {
    let mut profile = ProjectProfile::new("routine-test", &root.to_string_lossy(), 60_000);
    for (cap, pat, eff, pc_only) in rules {
        profile.rules.push(PolicyRule {
            capability: cap,
            pattern: pat.to_string(),
            effect: eff,
            pc_only,
        });
    }
    profile
}

#[tokio::test]
async fn test_three_step_routine_replays_deterministically_with_zero_llm_tokens() {
    let root = fresh_temp_dir("replay");
    let dl_jail = DownloadJailPolicy::new(root.join("dl")).unwrap();
    let routine_file = root.join("deploy-check.routine.toml");

    let mut defaults = BTreeMap::new();
    defaults.insert("target_host".to_string(), "docs.rs".to_string());
    defaults.insert("greeting".to_string(), "miku-routine-ok".to_string());

    let manifest = RoutineManifest {
        name: "daily-workspace-check".to_string(),
        version: 1,
        parameters: defaults,
        steps: vec![
            RoutineStep {
                id: "step-1".to_string(),
                description: "Write marker report".to_string(),
                preconditions: vec![RoutinePrecondition::ParameterPresent {
                    key: "greeting".to_string(),
                }],
                action: RoutineStepAction::FsWrite {
                    path: "notes/status.txt".to_string(),
                    content: "{{greeting}}".to_string(),
                    create_dirs: true,
                },
            },
            RoutineStep {
                id: "step-2".to_string(),
                description: "Run safe echo check".to_string(),
                preconditions: vec![],
                action: RoutineStepAction::ShellCommand {
                    program: "echo".to_string(),
                    args: vec!["{{greeting}}".to_string()],
                    cwd: None,
                    timeout_ms: 5_000,
                },
            },
            RoutineStep {
                id: "step-3".to_string(),
                description: "Open docs page".to_string(),
                preconditions: vec![],
                action: RoutineStepAction::BrowserNavigate {
                    url: "https://{{target_host}}/miku".to_string(),
                    download_filename: None,
                },
            },
        ],
    };

    save_routine_toml(&routine_file, &manifest).unwrap();
    let loaded = load_routine_toml(&routine_file).unwrap();

    let sandbox = SandboxPcControlBackend::default();
    let mut pc = PcControlController::new(sandbox);
    let mut browser = BrowserController::new(NoOpBrowserBackend, dl_jail, true);
    let profile = build_profile(
        &root,
        vec![
            (Capability::FsWrite, "*", PolicyEffect::Allow, false),
            (Capability::Shell, "echo*", PolicyEffect::Allow, false),
            (Capability::Browser, "docs.rs", PolicyEffect::Allow, false),
        ],
    );
    let mut gk = GatekeeperEngine::new(profile).unwrap();

    let report = RoutineExecutor::run_deterministic(
        &loaded,
        "routine-runner",
        &BTreeMap::new(),
        &mut pc,
        &mut browser,
        &mut gk,
        10_000,
    )
    .await;

    assert!(report.succeeded);
    assert_eq!(report.steps_executed, 3);
    assert_eq!(report.telemetry.llm_calls_made, 0);
    assert_eq!(report.telemetry.prompt_tokens_used, 0);
    assert_eq!(report.telemetry.completion_tokens_used, 0);
    assert_eq!(report.step_outputs.len(), 3);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn test_routine_always_passes_every_step_through_gatekeeper_and_blocks_destructive_shell() {
    let root = fresh_temp_dir("gatekeeper");
    let dl_jail = DownloadJailPolicy::new(root.join("dl")).unwrap();
    let manifest = RoutineManifest {
        name: "unsafe-routine".to_string(),
        version: 1,
        parameters: BTreeMap::new(),
        steps: vec![RoutineStep {
            id: "step-nuke".to_string(),
            description: "Blocked destructive shell".to_string(),
            preconditions: vec![],
            action: RoutineStepAction::ShellCommand {
                program: "rm".to_string(),
                args: vec!["-rf".to_string(), "/".to_string()],
                cwd: None,
                timeout_ms: 5_000,
            },
        }],
    };

    let mut pc = PcControlController::new(SandboxPcControlBackend::default());
    let mut browser = BrowserController::new(NoOpBrowserBackend, dl_jail, true);
    let profile = build_profile(&root, vec![(Capability::Shell, "*", PolicyEffect::Allow, false)]);
    let mut gk = GatekeeperEngine::new(profile).unwrap();

    let report = RoutineExecutor::run_deterministic(
        &manifest,
        "routine-runner",
        &BTreeMap::new(),
        &mut pc,
        &mut browser,
        &mut gk,
        20_000,
    )
    .await;

    assert!(!report.succeeded);
    assert_eq!(report.steps_executed, 0);
    assert!(matches!(
        report.halt_reason,
        RoutineHaltReason::GatekeeperBlocked { .. }
    ));
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn test_routine_halts_safely_when_environment_precondition_changes() {
    let root = fresh_temp_dir("precond");
    let dl_jail = DownloadJailPolicy::new(root.join("dl")).unwrap();
    let missing = root.join("missing-config.toml").to_string_lossy().to_string();
    let manifest = RoutineManifest {
        name: "env-sensitive".to_string(),
        version: 1,
        parameters: BTreeMap::new(),
        steps: vec![RoutineStep {
            id: "step-read".to_string(),
            description: "Requires file present".to_string(),
            preconditions: vec![RoutinePrecondition::FileExists { path: missing }],
            action: RoutineStepAction::FsRead {
                path: "missing-config.toml".to_string(),
            },
        }],
    };

    let mut pc = PcControlController::new(SandboxPcControlBackend::default());
    let mut browser = BrowserController::new(NoOpBrowserBackend, dl_jail, true);
    let mut gk = GatekeeperEngine::new(build_profile(
        &root,
        vec![(Capability::FsRead, "*", PolicyEffect::Allow, false)],
    ))
    .unwrap();

    let report = RoutineExecutor::run_deterministic(
        &manifest,
        "routine-runner",
        &BTreeMap::new(),
        &mut pc,
        &mut browser,
        &mut gk,
        30_000,
    )
    .await;

    assert!(!report.succeeded);
    assert!(matches!(
        report.halt_reason,
        RoutineHaltReason::PreconditionFailed { .. }
    ));
    assert_eq!(report.telemetry.llm_calls_made, 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn test_real_windows_native_routine_roundtrip_on_disk_zero_tokens() {
    let root = fresh_temp_dir("win_native");
    let dl_jail = DownloadJailPolicy::new(root.join("dl")).unwrap();
    let manifest = RoutineManifest {
        name: "native-write-read-echo".to_string(),
        version: 1,
        parameters: BTreeMap::new(),
        steps: vec![
            RoutineStep {
                id: "s1".to_string(),
                description: "Write inside jail".to_string(),
                preconditions: vec![],
                action: RoutineStepAction::FsWrite {
                    path: "out/hello.txt".to_string(),
                    content: "routine-native-4.7".to_string(),
                    create_dirs: true,
                },
            },
            RoutineStep {
                id: "s2".to_string(),
                description: "Read back inside jail".to_string(),
                preconditions: vec![],
                action: RoutineStepAction::FsRead {
                    path: "out/hello.txt".to_string(),
                },
            },
            RoutineStep {
                id: "s3".to_string(),
                description: "Safe echo shell".to_string(),
                preconditions: vec![],
                action: RoutineStepAction::ShellCommand {
                    program: "cmd".to_string(),
                    args: vec!["/c".to_string(), "echo".to_string(), "miku-4.7-ok".to_string()],
                    cwd: None,
                    timeout_ms: 5_000,
                },
            },
        ],
    };

    let mut pc = PcControlController::new(WindowsNativePcControlBackend);
    let mut browser = BrowserController::new(NoOpBrowserBackend, dl_jail, true);
    let mut gk = GatekeeperEngine::new(build_profile(
        &root,
        vec![
            (Capability::FsWrite, "*", PolicyEffect::Allow, false),
            (Capability::FsRead, "*", PolicyEffect::Allow, false),
            (Capability::Shell, "cmd /c echo*", PolicyEffect::Allow, false),
        ],
    ))
    .unwrap();

    let report = RoutineExecutor::run_deterministic(
        &manifest,
        "routine-runner",
        &BTreeMap::new(),
        &mut pc,
        &mut browser,
        &mut gk,
        40_000,
    )
    .await;

    assert!(report.succeeded, "expected native routine success, got {:?}", report.halt_reason);
    assert_eq!(report.steps_executed, 3);
    assert!(report.step_outputs[1].contains("routine-native-4.7"));
    assert!(report.step_outputs[2].contains("miku-4.7-ok"));
    assert_eq!(report.telemetry.llm_calls_made, 0);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_routine_modules_comply_with_sub_600_line_modular_budget() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("routines");
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
    println!("ROUTINES_4_7_MODULAR_AUDIT: files={files} max_lines={max_lines} (< 600)");
    assert_eq!(files, 4);
}
