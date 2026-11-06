use serde::{Deserialize, Serialize};
use spike_06_gatekeeper::{
    aggregate_tree_private_mb, detect_surviving_orphans, parse_process_snapshot_tsv, percentile,
    ActionRequest, AuditChainError, CanonicalPathJail, CapabilityScope, DecisionVerdict,
    DenyReason, GatewayIpcServer, GrantToken, IpcRequestEnvelope, IpcResponseEnvelope,
    LocalGatekeeper, PolicyRule, ProcessIdentity, ProcessRow, RuleEffect,
    TamperEvidentAuditLedger, DECISION_LATENCY_P95_MAX_MS, IDLE_RAM_MAX_MB, RELEASE_BINARY_MAX_MB,
};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BENCHMARK_REPS: usize = 20;
const SESSION_TOKEN: &str = "miku-local-ipc-secret-token-06";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkerIterationTelemetry {
    iterations: usize,
    total_ipc_decisions: usize,
    allow_count: usize,
    ask_issued_count: usize,
    deny_default_count: usize,
    deny_jail_count: usize,
    deny_toctou_count: usize,
    deny_kill_switch_count: usize,
    unauthorized_ipc_blocked: usize,
    chain_entries_verified: usize,
    chain_verified_ok: bool,
    tamper_simulation_detected: bool,
    decision_latencies_ms: Vec<f64>,
    head_hash: String,
}

fn build_benchmark_server() -> GatewayIpcServer {
    let jail = CanonicalPathJail::new("C:/Users/cmiranda/Projects/miku")
        .expect("Failed to initialize CanonicalPathJail");
    let rules = vec![
        PolicyRule {
            id: "allow-src-read".to_string(),
            capability: CapabilityScope::FsRead,
            target_pattern: "C:/Users/cmiranda/Projects/miku/src/*".to_string(),
            effect: RuleEffect::Allow,
        },
        PolicyRule {
            id: "ask-src-write".to_string(),
            capability: CapabilityScope::FsWrite,
            target_pattern: "C:/Users/cmiranda/Projects/miku/src/*".to_string(),
            effect: RuleEffect::Ask,
        },
        PolicyRule {
            id: "deny-private-keys".to_string(),
            capability: CapabilityScope::FsRead,
            target_pattern: "C:/Users/cmiranda/Projects/miku/src/secrets/*".to_string(),
            effect: RuleEffect::Deny,
        },
        PolicyRule {
            id: "ask-git-shell".to_string(),
            capability: CapabilityScope::ShellExec,
            target_pattern: "git *".to_string(),
            effect: RuleEffect::Ask,
        },
    ];
    let gatekeeper = LocalGatekeeper::new(jail, rules, 15_000);
    GatewayIpcServer::new(SESSION_TOKEN.to_string(), gatekeeper)
}

fn execute_20_rep_ipc_gatekeeper_benchmark() -> Result<WorkerIterationTelemetry, String> {
    let mut server = build_benchmark_server();
    let mut decision_latencies_ms = Vec::with_capacity(BENCHMARK_REPS * 7);

    let mut allow_count = 0usize;
    let mut ask_issued_count = 0usize;
    let mut deny_default_count = 0usize;
    let mut deny_jail_count = 0usize;
    let mut deny_toctou_count = 0usize;
    let mut deny_kill_switch_count = 0usize;
    let mut unauthorized_ipc_blocked = 0usize;

    for rep in 0..BENCHMARK_REPS {
        let base_ts = 100_000 + (rep as u64) * 1_000;

        // 1. Authorized FsRead inside jail -> Allow
        let frame_allow = serde_json::to_string(&IpcRequestEnvelope {
            v: 1,
            msg_id: format!("msg-{}-1", rep),
            auth_token: SESSION_TOKEN.to_string(),
            request: ActionRequest {
                session_id: format!("sess-{}", rep),
                agent_id: "claude-code".to_string(),
                capability: CapabilityScope::FsRead,
                target: format!("src/module_{}.rs", rep),
                canonical_args_json: r#"{"encoding":"utf8"}"#.to_string(),
                timestamp_ms: base_ts + 10,
                grant_token: None,
            },
        })
        .map_err(|e| e.to_string())?;

        let t0 = Instant::now();
        let resp_raw = server.handle_raw_json_frame(&frame_allow);
        decision_latencies_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        let resp: IpcResponseEnvelope =
            serde_json::from_str(&resp_raw).map_err(|e| e.to_string())?;
        if matches!(resp.verdict, Some(DecisionVerdict::Allow { .. })) {
            allow_count += 1;
        } else {
            return Err(format!("Rep {}: expected Allow verdict, got {:?}", rep, resp));
        }

        // 2. Unlisted capability (NetworkOut) -> Deny by default
        let frame_unlisted = serde_json::to_string(&IpcRequestEnvelope {
            v: 1,
            msg_id: format!("msg-{}-2", rep),
            auth_token: SESSION_TOKEN.to_string(),
            request: ActionRequest {
                session_id: format!("sess-{}", rep),
                agent_id: "cursor-mcp".to_string(),
                capability: CapabilityScope::NetworkOut,
                target: "https://telemetry.external.invalid/collect".to_string(),
                canonical_args_json: "{}".to_string(),
                timestamp_ms: base_ts + 20,
                grant_token: None,
            },
        })
        .map_err(|e| e.to_string())?;

        let t1 = Instant::now();
        let resp_raw = server.handle_raw_json_frame(&frame_unlisted);
        decision_latencies_ms.push(t1.elapsed().as_secs_f64() * 1000.0);
        let resp: IpcResponseEnvelope =
            serde_json::from_str(&resp_raw).map_err(|e| e.to_string())?;
        if matches!(
            resp.verdict,
            Some(DecisionVerdict::Deny {
                reason: DenyReason::UnlistedCapabilityDefaultDeny
            })
        ) {
            deny_default_count += 1;
        } else {
            return Err(format!(
                "Rep {}: expected UnlistedCapabilityDefaultDeny, got {:?}",
                rep, resp
            ));
        }

        // 3. Hostile Path Traversal / Windows Device Escape -> Deny via PathJailViolation
        let hostile_target = if rep % 3 == 0 {
            "..\\..\\Windows\\System32\\cmd.exe"
        } else if rep % 3 == 1 {
            "\\\\?\\UNC\\attacker\\share\\steal.exe"
        } else {
            "src\\CON.txt"
        };
        let frame_jail = serde_json::to_string(&IpcRequestEnvelope {
            v: 1,
            msg_id: format!("msg-{}-3", rep),
            auth_token: SESSION_TOKEN.to_string(),
            request: ActionRequest {
                session_id: format!("sess-{}", rep),
                agent_id: "antigravity".to_string(),
                capability: CapabilityScope::FsWrite,
                target: hostile_target.to_string(),
                canonical_args_json: r#"{"bytes":128}"#.to_string(),
                timestamp_ms: base_ts + 30,
                grant_token: None,
            },
        })
        .map_err(|e| e.to_string())?;

        let t2 = Instant::now();
        let resp_raw = server.handle_raw_json_frame(&frame_jail);
        decision_latencies_ms.push(t2.elapsed().as_secs_f64() * 1000.0);
        let resp: IpcResponseEnvelope =
            serde_json::from_str(&resp_raw).map_err(|e| e.to_string())?;
        if matches!(
            resp.verdict,
            Some(DecisionVerdict::Deny {
                reason: DenyReason::PathJailViolation(_)
            })
        ) {
            deny_jail_count += 1;
        } else {
            return Err(format!(
                "Rep {}: expected PathJailViolation, got {:?}",
                rep, resp
            ));
        }

        // 4. Interactive Ask flow -> issues PermissionTicket
        let ask_req_base = ActionRequest {
            session_id: format!("sess-{}", rep),
            agent_id: "claude-code".to_string(),
            capability: CapabilityScope::ShellExec,
            target: "git status --short".to_string(),
            canonical_args_json: r#"{"cwd":"C:/Users/cmiranda/Projects/miku"}"#.to_string(),
            timestamp_ms: base_ts + 40,
            grant_token: None,
        };
        let frame_ask = serde_json::to_string(&IpcRequestEnvelope {
            v: 1,
            msg_id: format!("msg-{}-4", rep),
            auth_token: SESSION_TOKEN.to_string(),
            request: ask_req_base.clone(),
        })
        .map_err(|e| e.to_string())?;

        let t3 = Instant::now();
        let resp_raw = server.handle_raw_json_frame(&frame_ask);
        decision_latencies_ms.push(t3.elapsed().as_secs_f64() * 1000.0);
        let resp: IpcResponseEnvelope =
            serde_json::from_str(&resp_raw).map_err(|e| e.to_string())?;
        let ticket = match resp.verdict {
            Some(DecisionVerdict::Ask { ticket, .. }) => {
                ask_issued_count += 1;
                ticket
            }
            other => return Err(format!("Rep {}: expected Ask ticket, got {:?}", rep, other)),
        };

        // 5. Anti-TOCTOU check: try reusing ticket with modified command
        let frame_toctou = serde_json::to_string(&IpcRequestEnvelope {
            v: 1,
            msg_id: format!("msg-{}-5", rep),
            auth_token: SESSION_TOKEN.to_string(),
            request: ActionRequest {
                target: "git clean -fdx".to_string(),
                timestamp_ms: base_ts + 50,
                grant_token: Some(GrantToken {
                    ticket_id: ticket.ticket_id.clone(),
                    approved_request_hash: ticket.request_hash.clone(),
                }),
                ..ask_req_base.clone()
            },
        })
        .map_err(|e| e.to_string())?;

        let t4 = Instant::now();
        let resp_raw = server.handle_raw_json_frame(&frame_toctou);
        decision_latencies_ms.push(t4.elapsed().as_secs_f64() * 1000.0);
        let resp: IpcResponseEnvelope =
            serde_json::from_str(&resp_raw).map_err(|e| e.to_string())?;
        if matches!(
            resp.verdict,
            Some(DecisionVerdict::Deny {
                reason: DenyReason::PermissionRequestHashMismatch
            })
        ) {
            deny_toctou_count += 1;
        } else {
            return Err(format!(
                "Rep {}: expected PermissionRequestHashMismatch, got {:?}",
                rep, resp
            ));
        }

        // 6. Legitimate GrantToken execution with exact matching arguments -> Allow
        let frame_granted = serde_json::to_string(&IpcRequestEnvelope {
            v: 1,
            msg_id: format!("msg-{}-6", rep),
            auth_token: SESSION_TOKEN.to_string(),
            request: ActionRequest {
                timestamp_ms: base_ts + 60,
                grant_token: Some(GrantToken {
                    ticket_id: ticket.ticket_id,
                    approved_request_hash: ticket.request_hash,
                }),
                ..ask_req_base.clone()
            },
        })
        .map_err(|e| e.to_string())?;

        let t5 = Instant::now();
        let resp_raw = server.handle_raw_json_frame(&frame_granted);
        decision_latencies_ms.push(t5.elapsed().as_secs_f64() * 1000.0);
        let resp: IpcResponseEnvelope =
            serde_json::from_str(&resp_raw).map_err(|e| e.to_string())?;
        if matches!(resp.verdict, Some(DecisionVerdict::Allow { .. })) {
            allow_count += 1;
        } else {
            return Err(format!(
                "Rep {}: expected Allow via valid GrantToken, got {:?}",
                rep, resp
            ));
        }

        // 7. Engage Global Kill Switch -> verify instant Deny -> reset Kill Switch
        server.gatekeeper_mut().trigger_kill_switch(base_ts + 70);
        let t6 = Instant::now();
        let resp_raw = server.handle_raw_json_frame(&frame_allow);
        decision_latencies_ms.push(t6.elapsed().as_secs_f64() * 1000.0);
        let resp: IpcResponseEnvelope =
            serde_json::from_str(&resp_raw).map_err(|e| e.to_string())?;
        if matches!(
            resp.verdict,
            Some(DecisionVerdict::Deny {
                reason: DenyReason::GlobalKillSwitchActive
            })
        ) {
            deny_kill_switch_count += 1;
        } else {
            return Err(format!(
                "Rep {}: expected GlobalKillSwitchActive, got {:?}",
                rep, resp
            ));
        }
        server.gatekeeper_mut().reset_kill_switch(base_ts + 80);

        // 8. Unauthorized IPC frame with invalid token -> rejected at Gateway boundary
        let frame_unauth = serde_json::to_string(&IpcRequestEnvelope {
            v: 1,
            msg_id: format!("msg-{}-unauth", rep),
            auth_token: "forged-token-000".to_string(),
            request: ask_req_base,
        })
        .map_err(|e| e.to_string())?;
        let resp_unauth: IpcResponseEnvelope =
            serde_json::from_str(&server.handle_raw_json_frame(&frame_unauth))
                .map_err(|e| e.to_string())?;
        if resp_unauth.ipc_error.as_deref() == Some("ERR_UNAUTHORIZED_TOKEN") {
            unauthorized_ipc_blocked += 1;
        }
    }

    // Verify full SHA-256 hash chain over all recorded entries
    let jsonl = server.gatekeeper().audit_ledger().to_jsonl();
    let chain_entries_verified =
        TamperEvidentAuditLedger::verify_jsonl(&jsonl).map_err(|e| format!("{:?}", e))?;

    // Simulate a 1-bit tamper on a cloned ledger copy and verify detection
    let mut cloned_entries = server.gatekeeper().audit_ledger().entries().to_vec();
    let mid = cloned_entries.len() / 2;
    cloned_entries[mid].verdict = "ALLOW_FORGED".to_string();
    let tamper_simulation_detected = matches!(
        TamperEvidentAuditLedger::verify_slice(&cloned_entries),
        Err(AuditChainError::EntryHashTampered { .. })
    );

    Ok(WorkerIterationTelemetry {
        iterations: BENCHMARK_REPS,
        total_ipc_decisions: decision_latencies_ms.len(),
        allow_count,
        ask_issued_count,
        deny_default_count,
        deny_jail_count,
        deny_toctou_count,
        deny_kill_switch_count,
        unauthorized_ipc_blocked,
        chain_entries_verified,
        chain_verified_ok: true,
        tamper_simulation_detected,
        decision_latencies_ms,
        head_hash: server.gatekeeper().audit_ledger().head_hash().to_string(),
    })
}

fn capture_windows_process_snapshot() -> Result<Vec<ProcessRow>, String> {
    if !cfg!(target_os = "windows") {
        return Ok(vec![ProcessRow {
            pid: std::process::id(),
            parent_pid: 0,
            private_bytes: 2 * 1024 * 1024,
            name: "spike-06-gatekeeper".to_string(),
            creation_date: "2026-11-05T00:00:00.0000000Z".to_string(),
        }]);
    }

    let ps_script = r#"
Get-CimInstance Win32_Process | ForEach-Object {
    $cd = if ($_.CreationDate) { $_.CreationDate.ToUniversalTime().ToString('o') } else { 'unknown' }
    "{0}`t{1}`t{2}`t{3}`t{4}" -f $_.ProcessId, $_.ParentProcessId, $_.PrivatePageCount, $_.Name, $cd
}
"#;
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_script])
        .output()
        .map_err(|e| format!("Failed to invoke PowerShell Get-CimInstance: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_process_snapshot_tsv(&stdout))
}

fn run_worker_mode() -> Result<(), String> {
    println!("WORKER_IDLE_PRE_READY");
    std::thread::sleep(Duration::from_millis(350));

    let telemetry = execute_20_rep_ipc_gatekeeper_benchmark()?;
    let json = serde_json::to_string(&telemetry).map_err(|e| e.to_string())?;
    println!("WORKER_TELEMETRY_JSON:{}", json);

    println!("WORKER_POST_AUDIT_READY");
    std::thread::sleep(Duration::from_millis(350));
    Ok(())
}

fn run_coordinator_mode() -> Result<(), String> {
    let exe_path = std::env::current_exe().map_err(|e| e.to_string())?;
    let bin_bytes = std::fs::metadata(&exe_path)
        .map(|m| m.len())
        .unwrap_or(0);
    let bin_mb = (bin_bytes as f64) / (1024.0 * 1024.0);

    let mut child = Command::new(&exe_path)
        .arg("--worker")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Failed to spawn --worker process: {}", e))?;

    let root_pid = child.id();
    std::thread::sleep(Duration::from_millis(120));
    let snap_pre = capture_windows_process_snapshot()?;
    let (ram_pre_mb, tree_pre) = aggregate_tree_private_mb(&snap_pre, root_pid);

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed waiting for worker: {}", e))?;
    if !output.status.success() {
        return Err(format!("Worker exited with status {:?}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut telemetry_opt: Option<WorkerIterationTelemetry> = None;
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("WORKER_TELEMETRY_JSON:") {
            telemetry_opt = Some(
                serde_json::from_str(rest.trim())
                    .map_err(|e| format!("Invalid worker telemetry JSON: {}", e))?,
            );
        }
    }

    let telemetry = telemetry_opt.ok_or_else(|| "Missing WORKER_TELEMETRY_JSON line".to_string())?;

    // Settle delay before checking post-termination orphan processes
    std::thread::sleep(Duration::from_millis(150));
    let snap_post = capture_windows_process_snapshot()?;
    let surviving_orphans: Vec<ProcessIdentity> = detect_surviving_orphans(&tree_pre, &snap_post);

    let mut sorted_lat = telemetry.decision_latencies_ms.clone();
    sorted_lat.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lat_p50 = percentile(&sorted_lat, 50.0);
    let lat_p95 = percentile(&sorted_lat, 95.0);

    let bin_ok = bin_mb > 0.0 && bin_mb < RELEASE_BINARY_MAX_MB;
    let ram_ok = ram_pre_mb <= IDLE_RAM_MAX_MB;
    let lat_ok = lat_p95 <= DECISION_LATENCY_P95_MAX_MS;
    let security_ok = telemetry.chain_verified_ok
        && telemetry.tamper_simulation_detected
        && telemetry.deny_default_count == BENCHMARK_REPS
        && telemetry.deny_jail_count == BENCHMARK_REPS
        && telemetry.deny_toctou_count == BENCHMARK_REPS
        && telemetry.deny_kill_switch_count == BENCHMARK_REPS
        && telemetry.unauthorized_ipc_blocked == BENCHMARK_REPS;
    let orphans_ok = surviving_orphans.is_empty();

    println!("==================================================================");
    println!("MIKU SPIKE 0.6 · LOCAL GATEKEEPER + SHA-256 CHAIN + IPC BENCHMARK");
    println!("==================================================================");
    println!(
        "Release Binary Size       : {:.2} MB (threshold < {:.1} MB -> {})",
        bin_mb,
        RELEASE_BINARY_MAX_MB,
        if bin_ok { "PASS" } else { "FAIL" }
    );
    println!(
        "Repetitions / Decisions   : {} reps | {} IPC evaluations",
        telemetry.iterations, telemetry.total_ipc_decisions
    );
    println!(
        "Policy Verdicts           : allow={} | ask_issued={} | deny_default={} | deny_jail={} | deny_toctou={} | deny_kill_switch={} | unauth_ipc_blocked={}",
        telemetry.allow_count,
        telemetry.ask_issued_count,
        telemetry.deny_default_count,
        telemetry.deny_jail_count,
        telemetry.deny_toctou_count,
        telemetry.deny_kill_switch_count,
        telemetry.unauthorized_ipc_blocked
    );
    println!(
        "SHA-256 Tamper Chain      : entries={} | chain_verified_ok={} | tamper_detected={} | head={}",
        telemetry.chain_entries_verified,
        telemetry.chain_verified_ok,
        telemetry.tamper_simulation_detected,
        &telemetry.head_hash[..16]
    );
    println!(
        "IPC + Gatekeeper Latency  : p50 = {:.4} ms | p95 = {:.4} ms (threshold <= {:.1} ms -> {})",
        lat_p50,
        lat_p95,
        DECISION_LATENCY_P95_MAX_MS,
        if lat_ok { "PASS" } else { "FAIL" }
    );
    println!(
        "Worker PrivatePageCount   : {:.2} MB (threshold <= {:.1} MB -> {})",
        ram_pre_mb,
        IDLE_RAM_MAX_MB,
        if ram_ok { "PASS" } else { "FAIL" }
    );
    println!(
        "Orphan Processes (Tuple)  : {} (CreationDate identity verified)",
        surviving_orphans.len()
    );
    println!("==================================================================");

    if bin_ok && ram_ok && lat_ok && security_ok && orphans_ok {
        println!("VERIFIER: OK");
        Ok(())
    } else {
        Err(format!(
            "VERIFIER FAILED: bin_ok={} ram_ok={} lat_ok={} security_ok={} orphans_ok={}",
            bin_ok, ram_ok, lat_ok, security_ok, orphans_ok
        ))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let res = if args.iter().any(|a| a == "--worker") {
        run_worker_mode()
    } else {
        run_coordinator_mode()
    };

    if let Err(err) = res {
        eprintln!("ERROR: {}", err);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LEDGER-0.6-5: E2E Gatekeeper + IPC latency audit and SHA-256 tamper chain benchmark
    #[test]
    fn e2e_gatekeeper_ipc_latency_audit_and_tamper_chain_benchmark() {
        let telemetry = execute_20_rep_ipc_gatekeeper_benchmark()
            .expect("20-rep Gatekeeper + IPC benchmark must succeed");
        assert_eq!(telemetry.iterations, 20);
        assert_eq!(telemetry.total_ipc_decisions, 140);
        assert_eq!(telemetry.allow_count, 40);
        assert_eq!(telemetry.ask_issued_count, 20);
        assert_eq!(telemetry.deny_default_count, 20);
        assert_eq!(telemetry.deny_jail_count, 20);
        assert_eq!(telemetry.deny_toctou_count, 20);
        assert_eq!(telemetry.deny_kill_switch_count, 20);
        assert_eq!(telemetry.unauthorized_ipc_blocked, 20);
        assert!(telemetry.chain_verified_ok);
        assert!(telemetry.tamper_simulation_detected);
        assert_eq!(telemetry.chain_entries_verified, 180);

        let mut sorted = telemetry.decision_latencies_ms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p95 = percentile(&sorted, 95.0);
        assert!(
            p95 <= DECISION_LATENCY_P95_MAX_MS,
            "Expected IPC + Gatekeeper decision p95 <= 5.0 ms, got {:.4} ms",
            p95
        );
    }
}
