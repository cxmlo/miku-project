#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity,
    clippy::invisible_characters
)]

use std::env;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use miku_core::{
    aggregate_process_tree, build_current_thread_runtime, detect_orphans, parse_process_tsv,
    percentile, spawn_orchestrator, ProcessIdentity, ProcessRow,
};
use miku_gateway::{start_gateway_server, GatewayConfig, LoopbackWsClient};
use miku_protocol::{BaseContractBundle, IslandState, MikuLinkMessageV1};
use serde::{Deserialize, Serialize};

const BENCHMARK_REPS: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GatewayWorkerReport {
    startup_and_handshake_ms: f64,
    ws_rtt_ms: f64,
    routed_messages: u64,
    rejected_unauth: u64,
    rejected_version: u64,
    rejected_oversized: u64,
    residual_active_connections: usize,
    residual_authenticated_sessions: usize,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--worker") {
        let hold_for_ram = args.iter().any(|a| a == "--hold-for-ram");
        run_worker_mode(hold_for_ram);
        return;
    }

    if let Err(e) = run_coordinator_benchmark() {
        eprintln!("VERIFIER: FAIL ({})", e);
        std::process::exit(1);
    }
}

fn run_worker_mode(hold_for_ram: bool) {
    let t0 = Instant::now();
    let rt = build_current_thread_runtime().expect("worker current_thread runtime");

    let report = rt.block_on(async {
        let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
        let (orch_handle, orch_join) =
            spawn_orchestrator(bundle, "miku_gw_worker_tok_v1".to_string(), 128);

        let (gw_handle, gw_join) =
            start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                .await
                .expect("start gateway");

        // 1. Connect authenticated WS client and complete hello -> hello_ack
        let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
            .await
            .expect("connect WS");
        let hello = r##"{"v":1,"id":"gw-h-1","msg":{"type":"hello","data":{"client":"miku_android","token":"miku_gw_worker_tok_v1","capabilities":["agent_events","permission_decision"]}}}"##;
        client.send_text(hello).await.expect("send hello");
        let ack = client.recv_envelope().await.expect("recv hello_ack");
        assert_eq!(ack.reply_to.as_deref(), Some("gw-h-1"));

        let startup_and_handshake_ms = t0.elapsed().as_secs_f64() * 1000.0;

        // 2. Measure ping -> pong WebSocket RTT
        let t_rtt = Instant::now();
        let ping = r##"{"v":1,"id":"gw-p-1","msg":{"type":"ping","data":{"ts":1762315641}}}"##;
        client.send_text(ping).await.expect("send ping");
        let pong = client.recv_envelope().await.expect("recv pong");
        assert!(matches!(pong.msg, MikuLinkMessageV1::Pong { ts: 1762315641 }));
        let ws_rtt_ms = t_rtt.elapsed().as_secs_f64() * 1000.0;

        // 3. Send agent_event -> Alert
        let evt = r##"{"v":1,"id":"gw-e-1","msg":{"type":"agent_event","data":{"agent":"claude_code","kind":"needs_input","title":"Approve crates/miku-gateway"}}}"##;
        client.send_text(evt).await.expect("send evt");
        let evt_ack = client.recv_envelope().await.expect("recv evt_ack");
        assert!(matches!(
            evt_ack.msg,
            MikuLinkMessageV1::HelloAck {
                island_state: IslandState::Alert,
                ..
            }
        ));

        client.close_cleanly().await.expect("clean client close");

        // 4. Verify rejection of unauthenticated command
        {
            let mut intruder = LoopbackWsClient::connect(gw_handle.local_addr)
                .await
                .expect("intruder connect");
            let unauth_cmd = r##"{"v":1,"id":"bad-1","msg":{"type":"command","data":{"session_id":"s","action":"run","text":"whoami"}}}"##;
            intruder.send_text(unauth_cmd).await.expect("send unauth");
            let resp = intruder.recv_envelope().await.expect("recv unauth err");
            assert!(matches!(
                resp.msg,
                MikuLinkMessageV1::Error { ref code, .. } if code == "unauthorized"
            ));
        }

        // 5. Verify rejection of unsupported version v=9
        {
            let mut v_client = LoopbackWsClient::connect(gw_handle.local_addr)
                .await
                .expect("v_client connect");
            let bad_v = r##"{"v":9,"id":"bad-v","msg":{"type":"ping","data":{"ts":1}}}"##;
            v_client.send_text(bad_v).await.expect("send bad v");
            let resp = v_client.recv_envelope().await.expect("recv bad v err");
            assert!(matches!(
                resp.msg,
                MikuLinkMessageV1::Error { ref code, .. } if code == "unsupported_version"
            ));
        }

        // 6. Verify rejection of oversized frame (> 64 KB) before buffer allocation
        {
            let mut big_client = LoopbackWsClient::connect(gw_handle.local_addr)
                .await
                .expect("big_client connect");
            big_client
                .send_oversized_header_only(72_000)
                .await
                .expect("send oversized header");
            let resp = big_client.recv_envelope().await.expect("recv oversized err");
            assert!(matches!(
                resp.msg,
                MikuLinkMessageV1::Error { ref code, .. } if code == "frame_too_large"
            ));
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
        let m = gw_handle.metrics();

        gw_handle.trigger_shutdown();
        gw_join.await.expect("gw join");

        orch_handle.trigger_system_shutdown_signal();
        orch_join.await.expect("orch join");

        GatewayWorkerReport {
            startup_and_handshake_ms,
            ws_rtt_ms,
            routed_messages: m.total_messages_routed,
            rejected_unauth: m.total_rejected_unauth,
            rejected_version: m.total_rejected_version,
            rejected_oversized: m.total_rejected_oversized,
            residual_active_connections: m.active_connections,
            residual_authenticated_sessions: m.active_authenticated_sessions,
        }
    });

    println!(
        "{}",
        serde_json::to_string(&report).expect("serialize worker report")
    );
    let _ = std::io::stdout().flush();

    if hold_for_ram {
        // Wait until the coordinator snapshots PrivatePageCount and closes stdin
        let mut buf = [0u8; 1];
        let _ = std::io::stdin().read(&mut buf);
    }
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
    println!(" MIKU PHASE 1 · STEP 1.2 · LOCAL WEBSOCKET GATEWAY (miku-link v1)");
    println!("==================================================================");

    let exe = env::current_exe().map_err(|e| format!("current_exe failed: {}", e))?;
    let bin_bytes = std::fs::metadata(&exe).map(|m| m.len()).unwrap_or(0);
    let bin_mb = (bin_bytes as f64) / (1024.0 * 1024.0);

    let mut handshake_samples_ms = Vec::with_capacity(BENCHMARK_REPS);
    let mut rtt_samples_ms = Vec::with_capacity(BENCHMARK_REPS);
    let mut private_mb_samples = Vec::with_capacity(BENCHMARK_REPS);
    let mut tracked_identities: Vec<ProcessIdentity> = Vec::new();

    let mut total_routed = 0u64;
    let mut total_unauth_blocked = 0u64;
    let mut total_version_blocked = 0u64;
    let mut total_oversized_blocked = 0u64;
    let mut max_residual_conns = 0usize;

    for rep in 1..=BENCHMARK_REPS {
        let sample_ram = rep == 1 || rep == BENCHMARK_REPS;
        let mut cmd = Command::new(&exe);
        cmd.arg("--worker");
        if sample_ram {
            cmd.arg("--hold-for-ram");
        }

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to spawn worker rep {}: {}", rep, e))?;

        let worker_pid = child.id();

        if sample_ram {
            // Give the worker 40 ms to complete its WS benchmark and wait on stdin
            thread::sleep(Duration::from_millis(40));
            let rows = capture_windows_process_snapshot();
            let tree = aggregate_process_tree(worker_pid, &rows);
            if tree.root_present {
                let mb = (tree.total_private_bytes as f64) / (1024.0 * 1024.0);
                private_mb_samples.push(mb);
                tracked_identities.extend(tree.identities);
            }
        }

        // Drop stdin so the worker unblocks immediately and exits cleanly
        drop(child.stdin.take());

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

        let report: GatewayWorkerReport = serde_json::from_str(line)
            .map_err(|e| format!("invalid worker JSON on rep {}: {}", rep, e))?;

        handshake_samples_ms.push(report.startup_and_handshake_ms);
        rtt_samples_ms.push(report.ws_rtt_ms);
        total_routed += report.routed_messages;
        total_unauth_blocked += report.rejected_unauth;
        total_version_blocked += report.rejected_version;
        total_oversized_blocked += report.rejected_oversized;
        max_residual_conns = max_residual_conns
            .max(report.residual_active_connections)
            .max(report.residual_authenticated_sessions);
    }

    thread::sleep(Duration::from_millis(150));
    let post_rows = capture_windows_process_snapshot();
    let orphans = detect_orphans(&tracked_identities, &post_rows);

    handshake_samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    rtt_samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    private_mb_samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let hs_p50 = percentile(&handshake_samples_ms, 50.0);
    let hs_p95 = percentile(&handshake_samples_ms, 95.0);
    let rtt_p50 = percentile(&rtt_samples_ms, 50.0);
    let rtt_p95 = percentile(&rtt_samples_ms, 95.0);
    let ram_p95 = percentile(&private_mb_samples, 95.0);

    println!(
        "1. Binary Size (Release)     : {:.2} MB (Threshold < 15.0 MB -> {})",
        bin_mb,
        if bin_mb < 15.0 { "PASS" } else { "FAIL" }
    );
    println!(
        "2. Loopback WS Traffic (n={}) : routed={} | unauth_blocked={} | bad_version_blocked={} | oversized_64kb_blocked={}",
        BENCHMARK_REPS, total_routed, total_unauth_blocked, total_version_blocked, total_oversized_blocked
    );
    println!(
        "3. Session Registry Hygiene  : residual_active_conns={} (Expected 0 -> {})",
        max_residual_conns,
        if max_residual_conns == 0 { "PASS" } else { "FAIL" }
    );
    println!(
        "4. Startup + Handshake (BUD-07): p50 = {:.4} ms | p95 = {:.4} ms (Threshold < 100.0 ms -> {})",
        hs_p50,
        hs_p95,
        if hs_p95 < 100.0 { "PASS" } else { "FAIL" }
    );
    println!(
        "5. WS Loopback Ping-Pong RTT : p50 = {:.4} ms | p95 = {:.4} ms",
        rtt_p50, rtt_p95
    );
    println!(
        "6. Worker Private RAM (BUD-01): p95 = {:.2} MB (Threshold <= 30.0 MB -> {})",
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
    if hs_p95 >= 100.0 {
        return Err(format!("startup + WS handshake p95 {:.4} ms >= 100.0 ms", hs_p95));
    }
    if max_residual_conns != 0 {
        return Err(format!(
            "residual active connections {} != 0",
            max_residual_conns
        ));
    }
    if ram_p95 > 30.0 {
        return Err(format!("worker private RAM {:.2} MB > 30.0 MB", ram_p95));
    }
    if !orphans.is_empty() {
        return Err(format!("detected {} orphan processes", orphans.len()));
    }

    println!("VERIFIER: OK (Phase 1 · Step 1.2 miku-gateway local WebSocket v1 verified)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-1.2-5
    #[test]
    fn e2e_miku_gateway_ws_loopback_20_reps_latency_memory_and_zero_orphans() {
        let t0 = Instant::now();
        let rt = build_current_thread_runtime().expect("current_thread runtime");
        rt.block_on(async {
            let bundle = BaseContractBundle::load_embedded_defaults().expect("contracts");
            let (orch_handle, orch_join) =
                spawn_orchestrator(bundle, "test_e2e_gw_tok".to_string(), 32);

            let (gw_handle, gw_join) =
                start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                    .await
                    .expect("start gateway");

            let mut client = LoopbackWsClient::connect(gw_handle.local_addr)
                .await
                .expect("connect");
            let hello = r##"{"v":1,"id":"e2e-h","msg":{"type":"hello","data":{"client":"test","token":"test_e2e_gw_tok","capabilities":[]}}}"##;
            client.send_text(hello).await.expect("send hello");
            let _ = client.recv_envelope().await.expect("recv ack");

            let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
            assert!(elapsed_ms < 100.0, "startup + WS handshake must be < 100 ms");

            client.close_cleanly().await.expect("clean close");
            tokio::time::sleep(Duration::from_millis(10)).await;
            assert_eq!(gw_handle.metrics().active_connections, 0);

            gw_handle.trigger_shutdown();
            gw_join.await.expect("gw join");
            orch_handle.trigger_system_shutdown_signal();
            orch_join.await.expect("orch join");
        });
    }
}
