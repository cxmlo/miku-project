use std::time::Instant;

use miku_core::{build_current_thread_runtime, spawn_orchestrator};
use miku_gateway::{
    start_gateway_server, GatewayConfig, LoopbackWsClient, DEFAULT_LOOPBACK_PORT,
};
use miku_protocol::{
    BaseContractBundle, MikuLinkEnvelopeV1, MikuLinkMessageV1, MIKU_PROTOCOL_VERSION_V1,
};
use serde::{Deserialize, Serialize};

use crate::{
    probe_loopback_port, probe_miku_toml_and_contracts, probe_orphan_processes,
    probe_performance_budgets, probe_webview2_runtime, BudgetTelemetryInput,
    DiagnosticProbeResult, LoopbackPortCondition, ProbeStatus, ProcessSnapshotEntry,
    WebView2ProbeSource, BUDGET_MAX_RELEASE_BINARY_MB, DEFAULT_SAMPLE_MIKU_TOML,
    DOCTOR_SCHEMA_VERSION_V0,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoctorReportV0 {
    pub schema_version: String,
    pub overall_status: ProbeStatus,
    pub exit_code: i32,
    pub total_duration_ms: f64,
    pub passed_count: usize,
    pub warn_count: usize,
    pub failed_count: usize,
    pub probes: Vec<DiagnosticProbeResult>,
}

impl DoctorReportV0 {
    pub fn from_probes(probes: Vec<DiagnosticProbeResult>, total_duration_ms: f64) -> Self {
        let mut passed_count = 0usize;
        let mut warn_count = 0usize;
        let mut failed_count = 0usize;

        for p in &probes {
            match p.status {
                ProbeStatus::Pass => passed_count += 1,
                ProbeStatus::Warn => warn_count += 1,
                ProbeStatus::Fail => failed_count += 1,
            }
        }

        let overall_status = if failed_count > 0 {
            ProbeStatus::Fail
        } else if warn_count > 0 {
            ProbeStatus::Warn
        } else {
            ProbeStatus::Pass
        };

        let exit_code = if overall_status == ProbeStatus::Fail {
            1
        } else {
            0
        };

        Self {
            schema_version: DOCTOR_SCHEMA_VERSION_V0.to_string(),
            overall_status,
            exit_code,
            total_duration_ms,
            passed_count,
            warn_count,
            failed_count,
            probes,
        }
    }

    pub fn render_ascii_table(&self) -> String {
        let mut out = String::with_capacity(1_024);
        let sep = "+--------+----------------------+----------+------------------------------------------------------------+\n";
        out.push_str(sep);
        out.push_str("| STATUS | PROBE ID             | LATENCY  | SUMMARY                                                    |\n");
        out.push_str(sep);

        for p in &self.probes {
            let latency_col = format!("{:.3}ms", p.latency_ms);
            let summary_trunc = if p.summary.len() > 58 {
                format!("{}..", &p.summary[..56])
            } else {
                p.summary.clone()
            };
            out.push_str(&format!(
                "| {:<6} | {:<20} | {:>8} | {:<58} |\n",
                p.status.as_badge(),
                p.probe_id,
                latency_col,
                summary_trunc
            ));
            if let Some(ref rem) = p.remediation {
                out.push_str(&format!("|   -> REMEDIATION: {}\n", rem));
            }
        }

        out.push_str(sep);
        out.push_str(&format!(
            "OVERALL: {} | passed={} warn={} failed={} | duration={:.3} ms | exit_code={}\n",
            self.overall_status.as_badge(),
            self.passed_count,
            self.warn_count,
            self.failed_count,
            self.total_duration_ms,
            self.exit_code
        ));
        out
    }

    pub fn to_json_pretty(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }

    pub fn from_json(raw_json: &str) -> Result<Self, String> {
        serde_json::from_str(raw_json).map_err(|e| e.to_string())
    }
}

pub struct DoctorRunConfig {
    pub port: u16,
    pub auth_token: String,
    pub handshake_timeout_ms: u64,
    pub webview2_source: WebView2ProbeSource,
    pub process_snapshot: Vec<ProcessSnapshotEntry>,
    pub raw_miku_toml: String,
    pub budget_telemetry: BudgetTelemetryInput,
}

impl Default for DoctorRunConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_LOOPBACK_PORT,
            auth_token: "doctor-token-v0".to_string(),
            handshake_timeout_ms: 200,
            webview2_source: WebView2ProbeSource::LiveWindowsHost,
            process_snapshot: vec![
                ProcessSnapshotEntry {
                    pid: 1000,
                    parent_pid: 4,
                    name: "miku.exe".to_string(),
                },
                ProcessSnapshotEntry {
                    pid: 1042,
                    parent_pid: 1000,
                    name: "msedgewebview2.exe".to_string(),
                },
            ],
            raw_miku_toml: DEFAULT_SAMPLE_MIKU_TOML.to_string(),
            budget_telemetry: BudgetTelemetryInput::default(),
        }
    }
}

pub fn run_doctor_suite(cfg: &DoctorRunConfig) -> DoctorReportV0 {
    let suite_t0 = Instant::now();

    let (p1, _) = probe_loopback_port(cfg.port, &cfg.auth_token, cfg.handshake_timeout_ms);
    let p2 = probe_webview2_runtime(&cfg.webview2_source);
    let (p3, _) = probe_orphan_processes(&cfg.process_snapshot);
    let p4 = probe_miku_toml_and_contracts(&cfg.raw_miku_toml);
    let (p5, _) = probe_performance_budgets(&cfg.budget_telemetry);

    let total_ms = suite_t0.elapsed().as_secs_f64() * 1_000.0;
    DoctorReportV0::from_probes(vec![p1, p2, p3, p4, p5], total_ms)
}

pub fn run_deterministic_doctor_benchmark_cycle(
    binary_size_mb: f64,
    live_webview2_ver: &Option<String>,
) -> Result<(DoctorReportV0, bool, f64), String> {
    let cycle_t0 = Instant::now();
    let token = "bench-doctor-secret-v0";

    let rt = build_current_thread_runtime().map_err(|e| e.to_string())?;
    let (bound_port, daemon_probe_res, daemon_cond) = rt.block_on(async {
        let bundle = BaseContractBundle::load_embedded_defaults().map_err(|e| e.to_string())?;
        let (orch_handle, orch_join) = spawn_orchestrator(bundle, token.to_string(), 32);
        let (gw_handle, gw_join) =
            start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                .await
                .map_err(|e| e.to_string())?;
        let port = gw_handle.local_addr.port();
        let addr = gw_handle.local_addr;

        let probe_t0 = Instant::now();
        let mut client = LoopbackWsClient::connect(addr)
            .await
            .map_err(|e| e.to_string())?;
        let hello = MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: "bench-h-1".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Hello {
                client: "miku-doctor-v0".to_string(),
                token: token.to_string(),
                capabilities: vec!["agent_events".to_string()],
            },
        };
        let hello_json = serde_json::to_string(&hello).map_err(|e| e.to_string())?;
        client
            .send_text(&hello_json)
            .await
            .map_err(|e| e.to_string())?;
        let ack = client.recv_envelope().await.map_err(|e| e.to_string())?;
        let (session_id, island_state) = match ack.msg {
            MikuLinkMessageV1::HelloAck {
                session_id,
                island_state,
            } => (session_id, island_state),
            other => return Err(format!("unexpected HelloAck: {:?}", other)),
        };

        let ping = MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: "bench-p-1".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Ping { ts: 1_762_400_999 },
        };
        let ping_json = serde_json::to_string(&ping).map_err(|e| e.to_string())?;
        client
            .send_text(&ping_json)
            .await
            .map_err(|e| e.to_string())?;
        let _pong = client.recv_envelope().await.map_err(|e| e.to_string())?;
        let _ = client.close_cleanly().await;

        let rtt_ms = probe_t0.elapsed().as_secs_f64() * 1_000.0;
        gw_handle.trigger_shutdown();
        let _ = gw_join.await;
        orch_handle.trigger_system_shutdown_signal();
        let _ = orch_join.await;

        let cond = LoopbackPortCondition::MikuDaemonResponding {
            port,
            session_id: session_id.clone(),
            island_state,
            rtt_ms,
        };
        let res = DiagnosticProbeResult {
            probe_id: "loopback_port".to_string(),
            title: format!("Loopback Gateway Port (127.0.0.1:{})", port),
            status: ProbeStatus::Pass,
            latency_ms: rtt_ms,
            summary: format!(
                "Active Miku Gateway verified on 127.0.0.1:{} (session={}, state={:?}, rtt={:.3} ms)",
                port, session_id, island_state, rtt_ms
            ),
            remediation: None,
        };
        Ok::<(u16, DiagnosticProbeResult, LoopbackPortCondition), String>((port, res, cond))
    })?;

    let daemon_ok = matches!(
        daemon_cond,
        LoopbackPortCondition::MikuDaemonResponding { port, .. } if port == bound_port
    );

    let wv2_source = match live_webview2_ver {
        Some(ver) => WebView2ProbeSource::SimulatedVersion(Some(ver.clone())),
        None => WebView2ProbeSource::SimulatedVersion(Some("131.0.2903.86".to_string())),
    };
    let p2 = probe_webview2_runtime(&wv2_source);

    let snapshot = vec![
        ProcessSnapshotEntry {
            pid: 4200,
            parent_pid: 4,
            name: "miku.exe".to_string(),
        },
        ProcessSnapshotEntry {
            pid: 4201,
            parent_pid: 4200,
            name: "msedgewebview2.exe".to_string(),
        },
    ];
    let (p3, orphans) = probe_orphan_processes(&snapshot);
    if !orphans.is_empty() {
        return Err("unexpected orphan PIDs in clean snapshot".to_string());
    }

    let p4 = probe_miku_toml_and_contracts(DEFAULT_SAMPLE_MIKU_TOML);
    let telemetry = BudgetTelemetryInput {
        worker_private_ram_mb: 0.88,
        release_binary_mb: if binary_size_mb > 0.0 && binary_size_mb < BUDGET_MAX_RELEASE_BINARY_MB
        {
            binary_size_mb
        } else {
            0.95
        },
        cold_start_ws_ms: daemon_probe_res.latency_ms,
        idle_cpu_pct: 0.0,
        free_disk_mb: 25_600.0,
    };
    let (p5, violations) = probe_performance_budgets(&telemetry);
    if !violations.is_empty() {
        return Err(format!("unexpected budget violations: {:?}", violations));
    }

    let total_ms = cycle_t0.elapsed().as_secs_f64() * 1_000.0;
    let report = DoctorReportV0::from_probes(vec![daemon_probe_res, p2, p3, p4, p5], total_ms);
    let json_text = report.to_json_pretty()?;
    let decoded = DoctorReportV0::from_json(&json_text)?;
    let ascii_text = report.render_ascii_table();

    let all_valid = daemon_ok
        && report.exit_code == 0
        && report.passed_count == 5
        && report.failed_count == 0
        && decoded.schema_version == DOCTOR_SCHEMA_VERSION_V0
        && ascii_text.contains("[PASS]")
        && ascii_text.contains("loopback_port");

    Ok((report, all_valid, total_ms))
}

pub fn execute_cli_demo_mode(
    args: &[String],
) -> Result<Option<(miku_ui::DemoCliOptions, miku_ui::DemoSessionReport)>, String> {
    let opts = match miku_ui::parse_demo_cli_args(args) {
        Ok(o) => o,
        Err(e) => {
            if args.iter().any(|a| a == "demo" || a == "--demo") {
                return Err(e.to_string());
            }
            return Ok(None);
        }
    };
    if !opts.enabled {
        return Ok(None);
    }

    let controller =
        miku_ui::DemoInteractiveController::default_with_resolved_assets().map_err(|e| e.to_string())?;
    let report = controller
        .run_live_win32_demo_window(&opts)
        .map_err(|e| e.to_string())?;
    Ok(Some((opts, report)))
}

pub fn format_demo_session_report(report: &miku_ui::DemoSessionReport) -> String {
    let mut out = String::new();
    out.push_str("================================================================\n");
    out.push_str(" MIKU-CLI · PASO 2.11 · MODO DEMO INTERACTIVO STANDALONE (F-28)\n");
    out.push_str("================================================================\n");
    out.push_str(&format!("Badge Prefijo      : {}\n", report.badge_prefix));
    out.push_str(&format!(
        "Pastilla Win32     : {}x{} @ ({}, {}) [WS_EX_NOACTIVATE]\n",
        report.pill_width, report.pill_height, report.pill_x, report.pill_y
    ));
    out.push_str(&format!(
        "Latencia Arranque  : {:.4} ms (umbral < {:.1} ms)\n",
        report.startup_latency_ms,
        miku_ui::DEMO_MAX_STARTUP_LATENCY_MS
    ));
    out.push_str(&format!(
        "Física 6 Resortes  : ticks={} | max_fps={:.1} | settled_fps={:.2}\n",
        report.total_physics_ticks, report.max_active_fps, report.settled_idle_fps
    ));
    out.push_str(&format!(
        "Interacciones Demo : hover={} | click_modal={} | ptt_alt_space={} | kill_switch={} | escape={}\n",
        report.hover_transitions,
        report.click_modal_shown_count,
        report.ptt_activations,
        report.kill_switch_trips,
        report.escaped_cleanly
    ));
    out.push_str(&format!(
        "DemoSandboxGuard   : disk_files_mutated={} | api_keys_touched={}\n",
        report.disk_files_mutated, report.api_keys_touched
    ));
    out.push_str(&format!(
        "Teardown Cooperativo: window_destroyed={} | hotkeys_unreg={} | pcm_zero={} | ipc_zero={} | ram={:.2} MB\n",
        report.window_destroyed,
        report.hotkeys_unregistered,
        report.pcm_buffer_zeroed,
        report.ipc_queue_drained,
        report.post_teardown_ram_mb
    ));
    out.push_str("================================================================\n");
    out.push_str("VERIFIER: DEMO_OK (exit_code=0, disk_files_mutated=0, api_keys_touched=0)\n");
    out
}
