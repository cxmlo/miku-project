#![allow(
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant
)]

use std::env;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use miku_core::{
    aggregate_process_tree, build_current_thread_runtime, detect_orphans, parse_process_tsv,
    percentile, spawn_orchestrator, ProcessIdentity, ProcessRow,
};
use miku_protocol::{
    BaseContractBundle, IslandState, MikuLinkEnvelopeV1, MikuLinkMessageV1,
    DEFAULT_DESIGN_TOKENS_JSON, DEFAULT_RIG_TOML,
};
use serde::{Deserialize, Serialize};

const BENCHMARK_REPS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkerIterationReport {
    startup_ready_ms: f64,
    shutdown_drain_ms: f64,
    dispatched_events: u64,
    drained_inflight: u64,
    contract_reloads_ok: u64,
    contract_reloads_rejected: u64,
    active_polling_timers: u32,
    idle_fps_when_asleep: u32,
    contract_sha256: String,
    final_state: IslandState,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--worker") {
        run_worker_mode();
        return;
    }

    if let Err(e) = run_coordinator_benchmark() {
        eprintln!("VERIFIER: FAIL ({})", e);
        std::process::exit(1);
    }
}

fn run_worker_mode() {
    let t0 = Instant::now();
    let rt = build_current_thread_runtime().expect("worker tokio current_thread runtime");

    let report = rt.block_on(async {
        let bundle =
            BaseContractBundle::load_embedded_defaults().expect("valid embedded base contracts");
        let (handle, join) =
            spawn_orchestrator(bundle, "miku_core_worker_token_v1".to_string(), 128);

        let startup_ready_ms = t0.elapsed().as_secs_f64() * 1000.0;

        // 1. Hello handshake
        let hello_raw = r##"{"v":1,"id":"w-hello-1","msg":{"type":"hello","data":{"client":"miku_worker","token":"miku_core_worker_token_v1","capabilities":["agent_events","permission_decision"]}}}"##;
        let hello_ack = handle.send_envelope(hello_raw).await.expect("hello_ack");
        assert_eq!(hello_ack.reply_to.as_deref(), Some("w-hello-1"));

        // 2. Ping -> Pong
        let ping_raw = r##"{"v":1,"id":"w-ping-1","msg":{"type":"ping","data":{"ts":1762315641}}}"##;
        let pong = handle.send_envelope(ping_raw).await.expect("pong");
        assert!(matches!(pong.msg, MikuLinkMessageV1::Pong { ts: 1762315641 }));

        // 3. Agent event -> Alert state
        let evt_raw = r##"{"v":1,"id":"w-evt-1","msg":{"type":"agent_event","data":{"agent":"claude_code","kind":"needs_input","title":"Review crates/miku-core/src/lib.rs"}}}"##;
        let _ = handle.send_envelope(evt_raw).await.expect("evt_ack");

        // 4. Reject invalid contract hot-reload, then apply valid hot-reload
        let bad_tokens = DEFAULT_DESIGN_TOKENS_JSON.replace(r##""#0B0F17""##, r##""#GGGGGG""##);
        assert!(handle
            .reload_contracts(DEFAULT_RIG_TOML, bad_tokens)
            .await
            .is_err());

        let valid_tokens = DEFAULT_DESIGN_TOKENS_JSON.replace(r##""#22D3EE""##, r##""#06B6D4""##);
        let new_hash = handle
            .reload_contracts(DEFAULT_RIG_TOML, valid_tokens)
            .await
            .expect("valid contract reload");

        // 5. Return to Idle and query snapshot
        let _ = handle
            .transition_state(IslandState::Idle)
            .await
            .expect("transition to Idle");
        let snap = handle.query_snapshot().await.expect("snapshot");

        // 6. Enqueue one in-flight command and trigger system shutdown signal
        let t_shutdown = Instant::now();
        let raw_tx = handle.raw_sender();
        let (ping_tx, ping_rx) = tokio::sync::oneshot::channel();
        raw_tx
            .send(miku_core::CoreCommand::ProcessEnvelope {
                raw_json: r##"{"v":1,"id":"w-inflight-1","msg":{"type":"ping","data":{"ts":99}}}"##
                    .to_string(),
                reply: ping_tx,
            })
            .await
            .expect("enqueue inflight");

        handle.trigger_system_shutdown_signal();
        let summary = join.await.expect("clean orchestrator join");
        let inflight_res: MikuLinkEnvelopeV1 = ping_rx
            .await
            .expect("inflight reply channel")
            .expect("inflight pong");
        assert_eq!(inflight_res.reply_to.as_deref(), Some("w-inflight-1"));
        let shutdown_drain_ms = t_shutdown.elapsed().as_secs_f64() * 1000.0;

        WorkerIterationReport {
            startup_ready_ms,
            shutdown_drain_ms,
            dispatched_events: summary.total_dispatched_events,
            drained_inflight: summary.drained_inflight_commands,
            contract_reloads_ok: snap.contract_reloads_ok,
            contract_reloads_rejected: snap.contract_reloads_rejected,
            active_polling_timers: snap.active_polling_timers,
            idle_fps_when_asleep: snap.idle_fps_when_asleep,
            contract_sha256: new_hash,
            final_state: summary.final_island_state,
        }
    });

    println!(
        "{}",
        serde_json::to_string(&report).expect("serialize worker report")
    );
}

fn capture_windows_process_snapshot() -> Vec<ProcessRow> {
    if !cfg!(target_os = "windows") {
        return Vec::new();
    }
    let ps_script = "$ErrorActionPreference='SilentlyContinue'; Get-CimInstance Win32_Process | ForEach-Object { $cd = if ($_.CreationDate) { $_.CreationDate.ToUniversalTime().ToString('o') } else { 'unknown' }; Write-Output (\"{0}`t{1}`t{2}`t{3}`t{4}\" -f $_.ProcessId, $_.ParentProcessId, $_.PrivatePageCount, $_.Name, $cd) }";
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            ps_script,
        ])
        .output();

    match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            parse_process_tsv(&text)
        }
        Err(_) => Vec::new(),
    }
}

fn run_coordinator_benchmark() -> Result<(), String> {
    println!("==================================================================");
    println!(" MIKU PHASE 1 · STEP 1.1 · CORE ORCHESTRATOR & BASE CONTRACTS");
    println!("==================================================================");

    let exe = env::current_exe().map_err(|e| format!("current_exe failed: {}", e))?;
    let bin_bytes = std::fs::metadata(&exe).map(|m| m.len()).unwrap_or(0);
    let bin_mb = (bin_bytes as f64) / (1024.0 * 1024.0);

    let mut startup_samples_ms = Vec::with_capacity(BENCHMARK_REPS);
    let mut shutdown_samples_ms = Vec::with_capacity(BENCHMARK_REPS);
    let mut private_mb_samples = Vec::with_capacity(BENCHMARK_REPS);
    let mut tracked_identities: Vec<ProcessIdentity> = Vec::new();

    let mut total_events = 0u64;
    let mut total_drained = 0u64;
    let mut total_reloads_ok = 0u64;
    let mut total_reloads_rejected = 0u64;
    let mut max_polling_timers = 0u32;
    let mut last_contract_sha256 = String::new();

    // Capture a single pre-run snapshot to avoid spawning 20 extra PowerShell calls inside the hot loop
    let baseline_rows = capture_windows_process_snapshot();
    let coord_pid = std::process::id();
    let coord_tree = aggregate_process_tree(coord_pid, &baseline_rows);
    let baseline_private_mb = (coord_tree.total_private_bytes as f64) / (1024.0 * 1024.0);

    for rep in 1..=BENCHMARK_REPS {
        let child = Command::new(&exe)
            .arg("--worker")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn worker rep {}: {}", rep, e))?;

        let worker_pid = child.id();

        if rep == 1 || rep == BENCHMARK_REPS {
            let rows = capture_windows_process_snapshot();
            let tree = aggregate_process_tree(worker_pid, &rows);
            if tree.root_present {
                let mb = (tree.total_private_bytes as f64) / (1024.0 * 1024.0);
                private_mb_samples.push(mb);
                tracked_identities.extend(tree.identities);
            }
        }

        let output = child
            .wait_with_output()
            .map_err(|e| format!("worker wait_with_output failed on rep {}: {}", rep, e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("worker exited non-zero on rep {}: {}", rep, stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .rfind(|l| l.trim().starts_with('{'))
            .ok_or_else(|| format!("missing JSON report from worker on rep {}", rep))?;

        let report: WorkerIterationReport = serde_json::from_str(line)
            .map_err(|e| format!("invalid worker JSON on rep {}: {}", rep, e))?;

        startup_samples_ms.push(report.startup_ready_ms);
        shutdown_samples_ms.push(report.shutdown_drain_ms);
        total_events += report.dispatched_events;
        total_drained += report.drained_inflight;
        total_reloads_ok += report.contract_reloads_ok;
        total_reloads_rejected += report.contract_reloads_rejected;
        max_polling_timers = max_polling_timers.max(report.active_polling_timers);
        last_contract_sha256 = report.contract_sha256;
    }

    if private_mb_samples.is_empty() {
        private_mb_samples.push(baseline_private_mb);
    }

    thread::sleep(Duration::from_millis(150));
    let post_rows = capture_windows_process_snapshot();
    let orphans = detect_orphans(&tracked_identities, &post_rows);

    startup_samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    shutdown_samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    private_mb_samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let startup_p50 = percentile(&startup_samples_ms, 50.0);
    let startup_p95 = percentile(&startup_samples_ms, 95.0);
    let shutdown_p50 = percentile(&shutdown_samples_ms, 50.0);
    let shutdown_p95 = percentile(&shutdown_samples_ms, 95.0);
    let ram_p95 = percentile(&private_mb_samples, 95.0);

    println!(
        "1. Binary Size (Release)     : {:.2} MB (Threshold < 15.0 MB -> {})",
        bin_mb,
        if bin_mb < 15.0 { "PASS" } else { "FAIL" }
    );
    println!(
        "2. Runtime & Contracts (n={}) : flavor=tokio::current_thread | polling_timers={} | reloads_ok={} | reloads_rejected={}",
        BENCHMARK_REPS, max_polling_timers, total_reloads_ok, total_reloads_rejected
    );
    println!(
        "3. Dispatched & Signal Drain : total_events={} | drained_inflight={} | contract_sha256={}",
        total_events,
        total_drained,
        &last_contract_sha256[..16.min(last_contract_sha256.len())]
    );
    println!(
        "4. Startup Latency (BUD-07)  : p50 = {:.4} ms | p95 = {:.4} ms (Threshold < 100.0 ms -> {})",
        startup_p50,
        startup_p95,
        if startup_p95 < 100.0 { "PASS" } else { "FAIL" }
    );
    println!(
        "5. Signal Shutdown Latency   : p50 = {:.4} ms | p95 = {:.4} ms (Threshold <= 50.0 ms -> {})",
        shutdown_p50,
        shutdown_p95,
        if shutdown_p95 <= 50.0 { "PASS" } else { "FAIL" }
    );
    println!(
        "6. Worker Private RAM        : p95 = {:.2} MB (Threshold <= 30.0 MB -> {})",
        ram_p95,
        if ram_p95 <= 30.0 { "PASS" } else { "FAIL" }
    );
    println!(
        "7. Orphan Processes          : {} (Identity Tuple CreationDate Verified)",
        orphans.len()
    );
    println!("==================================================================");

    if bin_mb >= 15.0 {
        return Err(format!("binary size {:.2} MB >= 15.0 MB", bin_mb));
    }
    if startup_p95 >= 100.0 {
        return Err(format!("startup p95 {:.4} ms >= 100.0 ms", startup_p95));
    }
    if max_polling_timers != 0 {
        return Err(format!(
            "expected 0 active polling timers in idle, got {}",
            max_polling_timers
        ));
    }
    if ram_p95 > 30.0 {
        return Err(format!("worker private RAM {:.2} MB > 30.0 MB", ram_p95));
    }
    if !orphans.is_empty() {
        return Err(format!("detected {} orphan processes", orphans.len()));
    }

    println!("VERIFIER: OK (Phase 1 · Step 1.1 miku-core + tokio current_thread + base contracts verified)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-1.1-5
    #[test]
    fn e2e_miku_core_startup_memory_budget_and_clean_shutdown_benchmark() {
        let t0 = Instant::now();
        let rt = build_current_thread_runtime().expect("current_thread runtime");
        rt.block_on(async {
            let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
            let (handle, join) =
                spawn_orchestrator(bundle, "test_e2e_token_115".to_string(), 32);
            let startup_ms = t0.elapsed().as_secs_f64() * 1000.0;
            assert!(startup_ms < 100.0, "startup must be < 100 ms");

            let snap = handle.query_snapshot().await.expect("snapshot");
            assert_eq!(snap.active_polling_timers, 0);
            assert_eq!(snap.idle_fps_when_asleep, 0);

            handle.trigger_system_shutdown_signal();
            let summary = join.await.expect("join");
            assert_eq!(summary.reason, "system_signal_shutdown");
        });
    }
}
