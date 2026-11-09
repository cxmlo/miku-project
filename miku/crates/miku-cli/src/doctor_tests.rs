use std::io::{Read, Write};
use std::net::TcpListener as StdTcpListener;
use std::thread;
use std::time::Duration;

use miku_core::{build_current_thread_runtime, spawn_orchestrator};
use miku_gateway::{start_gateway_server, GatewayConfig};
use miku_protocol::BaseContractBundle;

use crate::{
    probe_loopback_port, probe_miku_toml_and_contracts, probe_orphan_processes,
    probe_performance_budgets, probe_webview2_runtime, run_doctor_suite, BudgetTelemetryInput,
    DoctorReportV0, DoctorRunConfig, LoopbackPortCondition, ProbeStatus, ProcessSnapshotEntry,
    WebView2ProbeSource, DEFAULT_SAMPLE_MIKU_TOML,
};

// LEDGER-1.8-1
#[test]
fn probe_loopback_port_distinguishes_available_miku_daemon_and_foreign_collision() {
    let temp_listener = StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let free_port = temp_listener.local_addr().unwrap().port();
    drop(temp_listener);

    let (res_free, cond_free) = probe_loopback_port(free_port, "tok-18", 150);
    assert_eq!(res_free.status, ProbeStatus::Pass);
    assert_eq!(
        cond_free,
        LoopbackPortCondition::PortAvailable { port: free_port }
    );

    let server_listener =
        StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral port for gateway");
    let miku_port = server_listener.local_addr().unwrap().port();
    drop(server_listener);

    let server_thread = thread::spawn(move || {
        let rt = build_current_thread_runtime().expect("server runtime");
        rt.block_on(async move {
            let bundle =
                BaseContractBundle::load_embedded_defaults().expect("load embedded contracts");
            let (orch_handle, orch_join) =
                spawn_orchestrator(bundle, "doctor-secret-18".to_string(), 16);
            let cfg = GatewayConfig::loopback(miku_port);
            let (gw_handle, gw_join) = start_gateway_server(cfg, orch_handle.clone())
                .await
                .expect("start gateway");
            tokio::time::sleep(Duration::from_millis(120)).await;
            gw_handle.trigger_shutdown();
            let _ = gw_join.await;
            orch_handle.trigger_system_shutdown_signal();
            let _ = orch_join.await;
        });
    });

    thread::sleep(Duration::from_millis(25));
    let (res_miku, cond_miku) = probe_loopback_port(miku_port, "doctor-secret-18", 250);
    server_thread.join().expect("join server thread");

    assert_eq!(res_miku.status, ProbeStatus::Pass);
    assert!(matches!(
        cond_miku,
        LoopbackPortCondition::MikuDaemonResponding { port, .. } if port == miku_port
    ));

    let foreign_listener =
        StdTcpListener::bind("127.0.0.1:0").expect("bind foreign listener");
    let foreign_port = foreign_listener.local_addr().unwrap().port();
    let foreign_thread = thread::spawn(move || {
        if let Ok((mut stream, _)) = foreign_listener.accept() {
            let mut buf = [0u8; 128];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
        }
    });

    let (res_foreign, cond_foreign) = probe_loopback_port(foreign_port, "doctor-secret-18", 150);
    foreign_thread.join().expect("join foreign thread");

    assert_eq!(res_foreign.status, ProbeStatus::Fail);
    assert!(matches!(
        cond_foreign,
        LoopbackPortCondition::ForeignProcessOccupied { port, .. } if port == foreign_port
    ));
    assert!(res_foreign.remediation.is_some());
}

// LEDGER-1.8-2
#[test]
fn probe_webview2_and_orphan_detector_flag_missing_runtime_and_zombie_pids() {
    let wv_ok = probe_webview2_runtime(&WebView2ProbeSource::SimulatedVersion(Some(
        "131.0.2903.86".to_string(),
    )));
    assert_eq!(wv_ok.status, ProbeStatus::Pass);
    assert!(wv_ok.summary.contains("131.0.2903.86"));

    let wv_missing = probe_webview2_runtime(&WebView2ProbeSource::SimulatedVersion(None));
    assert_eq!(wv_missing.status, ProbeStatus::Fail);
    assert!(wv_missing.remediation.is_some());

    let wv_zero = probe_webview2_runtime(&WebView2ProbeSource::SimulatedVersion(Some(
        "0.0.0.0".to_string(),
    )));
    assert_eq!(wv_zero.status, ProbeStatus::Fail);

    let clean_snapshot = vec![
        ProcessSnapshotEntry {
            pid: 500,
            parent_pid: 4,
            name: "miku.exe".to_string(),
        },
        ProcessSnapshotEntry {
            pid: 501,
            parent_pid: 500,
            name: "msedgewebview2.exe".to_string(),
        },
    ];
    let (orp_ok, pids_ok) = probe_orphan_processes(&clean_snapshot);
    assert_eq!(orp_ok.status, ProbeStatus::Pass);
    assert!(pids_ok.is_empty());

    let dirty_snapshot = vec![
        ProcessSnapshotEntry {
            pid: 500,
            parent_pid: 4,
            name: "miku.exe".to_string(),
        },
        ProcessSnapshotEntry {
            pid: 7788,
            parent_pid: 9999,
            name: "msedgewebview2.exe".to_string(),
        },
    ];
    let (orp_fail, pids_fail) = probe_orphan_processes(&dirty_snapshot);
    assert_eq!(orp_fail.status, ProbeStatus::Fail);
    assert_eq!(pids_fail, vec![7788]);
    assert!(orp_fail
        .remediation
        .as_deref()
        .unwrap_or("")
        .contains("7788"));
}

// LEDGER-1.8-3
#[test]
fn probe_miku_toml_validates_gatekeeper_rules_and_rejects_traversal_or_bad_syntax() {
    let valid_res = probe_miku_toml_and_contracts(DEFAULT_SAMPLE_MIKU_TOML);
    assert_eq!(valid_res.status, ProbeStatus::Pass);
    assert!(valid_res.summary.contains("miku-desktop-orchestrator"));

    let unisolated_toml = DEFAULT_SAMPLE_MIKU_TOML.replace(
        "profile_isolation = true",
        "profile_isolation = false",
    );
    let unisolated_res = probe_miku_toml_and_contracts(&unisolated_toml);
    assert_eq!(unisolated_res.status, ProbeStatus::Fail);
    assert!(unisolated_res.summary.contains("profile_isolation"));

    let traversal_toml = DEFAULT_SAMPLE_MIKU_TOML.replace(
        r#"fs_write = ["src/**", "crates/**"]"#,
        r#"fs_write = ["../../Windows/System32/**"]"#,
    );
    let traversal_res = probe_miku_toml_and_contracts(&traversal_toml);
    assert_eq!(traversal_res.status, ProbeStatus::Fail);
    assert!(traversal_res.summary.contains("CanonicalPathJail"));

    let broken_res = probe_miku_toml_and_contracts("[project\nname = unclosed");
    assert_eq!(broken_res.status, ProbeStatus::Fail);
}

// LEDGER-1.8-4
#[test]
fn probe_budgets_and_doctor_report_v0_ascii_and_json_roundtrip_exit_codes() {
    let healthy_budgets = BudgetTelemetryInput::default();
    let (b_pass, v_pass) = probe_performance_budgets(&healthy_budgets);
    assert_eq!(b_pass.status, ProbeStatus::Pass);
    assert!(v_pass.is_empty());

    let over_budget = BudgetTelemetryInput {
        worker_private_ram_mb: 42.5,
        release_binary_mb: 18.2,
        cold_start_ws_ms: 145.0,
        idle_cpu_pct: 1.5,
        free_disk_mb: 50.0,
    };
    let (b_fail, v_fail) = probe_performance_budgets(&over_budget);
    assert_eq!(b_fail.status, ProbeStatus::Fail);
    assert_eq!(v_fail.len(), 5);

    let temp_listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    let free_port = temp_listener.local_addr().unwrap().port();
    drop(temp_listener);

    let pass_cfg = DoctorRunConfig {
        port: free_port,
        webview2_source: WebView2ProbeSource::SimulatedVersion(Some(
            "131.0.2903.86".to_string(),
        )),
        ..DoctorRunConfig::default()
    };
    let report_ok = run_doctor_suite(&pass_cfg);
    assert_eq!(report_ok.exit_code, 0);
    assert_eq!(report_ok.passed_count, 5);
    assert_eq!(report_ok.failed_count, 0);

    let ascii = report_ok.render_ascii_table();
    assert!(ascii.contains("OVERALL: [PASS]"));
    assert!(ascii.contains("exit_code=0"));

    let json_str = report_ok.to_json_pretty().expect("serialize json");
    let parsed = DoctorReportV0::from_json(&json_str).expect("deserialize json");
    assert_eq!(parsed.schema_version, report_ok.schema_version);
    assert_eq!(parsed.overall_status, report_ok.overall_status);
    assert_eq!(parsed.exit_code, report_ok.exit_code);
    assert_eq!(parsed.passed_count, report_ok.passed_count);
    assert_eq!(parsed.probes.len(), report_ok.probes.len());
    assert!((parsed.total_duration_ms - report_ok.total_duration_ms).abs() < 1e-6);

    let fail_cfg = DoctorRunConfig {
        port: free_port,
        webview2_source: WebView2ProbeSource::SimulatedVersion(None),
        budget_telemetry: over_budget,
        ..DoctorRunConfig::default()
    };
    let report_fail = run_doctor_suite(&fail_cfg);
    assert_eq!(report_fail.exit_code, 1);
    assert_eq!(report_fail.overall_status, ProbeStatus::Fail);
    assert_eq!(report_fail.failed_count, 2);
}
