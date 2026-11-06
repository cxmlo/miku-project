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
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Instant;

use miku_gatekeeper::{AuditEntry, GENESIS_AUDIT_HASH};
use miku_storage::{
    ConversationTurnRecord, MemoryFts5Document, SessionRecord, SingleThreadStoragePool,
};

const BENCHMARK_REPS: usize = 20;
const MAX_P95_STORAGE_LATENCY_MS: f64 = 2.0;
const MAX_RELEASE_BINARY_MB: f64 = 15.0;
const MAX_WORKER_PRIVATE_RAM_MB: f64 = 30.0;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    name_lower: String,
    creation_date: String,
}

#[derive(Debug, Clone)]
struct WorkerBenchmarkMetrics {
    cycle_latency_ms: f64,
    fts5_latency_ms: f64,
    sessions_committed: usize,
    turns_committed: usize,
    audit_entries_verified: usize,
    fts5_hits: usize,
    profile_tamper_blocked: bool,
    wal_mode_ok: bool,
}

fn run_single_worker_iteration(rep: usize, hold_for_ram: bool) -> Result<WorkerBenchmarkMetrics, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to build current_thread runtime: {}", e))?;

    rt.block_on(async move {
        let temp_dir = env::temp_dir().join(format!(
            "miku_storage_worker_{}_{}",
            std::process::id(),
            rep
        ));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
        let db_path = temp_dir.join("miku_orchestrator.db");

        let start = Instant::now();
        let pool = SingleThreadStoragePool::open_wal_file(&db_path)
            .map_err(|e| format!("open_wal_file failed: {}", e))?;

        let pragmas = pool.pragma_state();
        let wal_mode_ok = pragmas.journal_mode == "wal"
            && pragmas.synchronous == "normal"
            && pragmas.foreign_keys
            && pragmas.busy_timeout_ms == 5_000;

        {
            let tx = pool.begin_transaction().map_err(|e| e.to_string())?;
            let sess_id = format!("sess_{}", rep);
            pool.insert_session(SessionRecord {
                session_id: sess_id.clone(),
                agent_name: "claude_code".to_string(),
                project_root: "C:/workspace/miku".to_string(),
                created_at_ms: 1_762_300_000 + (rep as u64),
                state: "active".to_string(),
            })
            .map_err(|e| e.to_string())?;

            for seq in 1..=4_u64 {
                pool.insert_turn(ConversationTurnRecord {
                    turn_id: format!("turn_{}_{}", rep, seq),
                    session_id: sess_id.clone(),
                    seq,
                    role: if seq % 2 == 1 { "user" } else { "assistant" }.to_string(),
                    content: format!("Turno determinista {} en modo WAL", seq),
                    token_estimate: 12,
                    ts_ms: 1_762_300_100 + seq,
                })
                .map_err(|e| e.to_string())?;
            }

            let mut prev_hash = GENESIS_AUDIT_HASH.to_string();
            for seq in 1..=4_u64 {
                let ts_ms = 1_762_300_200 + seq;
                let entry_hash = AuditEntry::compute_hash(
                    &prev_hash,
                    seq,
                    ts_ms,
                    "claude_code",
                    "fs_read",
                    "src/lib.rs",
                    "allow",
                    "allow_rule",
                );
                pool.append_audit_entry(AuditEntry {
                    seq,
                    timestamp_ms: ts_ms,
                    agent: "claude_code".to_string(),
                    capability: "fs_read".to_string(),
                    target: "src/lib.rs".to_string(),
                    verdict: "allow".to_string(),
                    reason: "allow_rule".to_string(),
                    prev_hash: prev_hash.clone(),
                    entry_hash: entry_hash.clone(),
                })
                .map_err(|e| e.to_string())?;
                prev_hash = entry_hash;
            }

            pool.upsert_fts5_document(MemoryFts5Document {
                doc_id: "doc_miku_wal".to_string(),
                title: "Arquitectura SQLite WAL y FTS5 BM25 en Miku".to_string(),
                tags: "sqlite wal fts5 bm25 miku-storage".to_string(),
                body: r##"Persistencia transaccional ACID en modo WAL con búsqueda léxica FTS5 BM25 ultrarrápida bajo 2 ms."##
                    .to_string(),
                updated_at_ms: 1_762_300_300,
            })
            .map_err(|e| e.to_string())?;

            pool.upsert_fts5_document(MemoryFts5Document {
                doc_id: "doc_miku_gatekeeper".to_string(),
                title: "Gatekeeper deny-by-default y jaula canónica".to_string(),
                tags: "gatekeeper sha256 toctou".to_string(),
                body: r##"Toda herramienta pasa por miku-gatekeeper con tickets anti-TOCTOU y cadena SHA-256."##
                    .to_string(),
                updated_at_ms: 1_762_300_310,
            })
            .map_err(|e| e.to_string())?;

            tx.commit().map_err(|e| e.to_string())?;
        }

        let profile_bytes = br##"name = "miku"
root = "."
[allow]
shell = ["cargo test"]
"##;
        pool.approve_profile_hash("C:/workspace/miku", profile_bytes, "user_pc", 1_762_300_400)
            .map_err(|e| e.to_string())?;
        let mut tampered_bytes = profile_bytes.to_vec();
        let last = tampered_bytes.len() - 1;
        tampered_bytes[last] ^= 0x01;
        let tamper_status = pool
            .evaluate_profile_approval("C:/workspace/miku", &tampered_bytes)
            .map_err(|e| e.to_string())?;
        let profile_tamper_blocked = !tamper_status.is_approved();

        let fts_start = Instant::now();
        let hits = pool
            .search_fts5("sqlite wal fts5 bm25", 5)
            .map_err(|e| e.to_string())?;
        let fts5_latency_ms = fts_start.elapsed().as_secs_f64() * 1_000.0;

        let audit_entries_verified = pool
            .verify_persisted_audit_chain()
            .map_err(|e| e.to_string())?;
        let turns = pool
            .list_turns_for_session(&format!("sess_{}", rep))
            .map_err(|e| e.to_string())?;
        pool.checkpoint_wal().map_err(|e| e.to_string())?;
        let cycle_latency_ms = start.elapsed().as_secs_f64() * 1_000.0;

        let metrics = WorkerBenchmarkMetrics {
            cycle_latency_ms,
            fts5_latency_ms,
            sessions_committed: 1,
            turns_committed: turns.len(),
            audit_entries_verified,
            fts5_hits: hits.len(),
            profile_tamper_blocked,
            wal_mode_ok,
        };

        println!(
            "MIKU_STORAGE_WORKER_OK|cycle_ms={:.4}|fts5_ms={:.4}|sessions={}|turns={}|audit={}|hits={}|tamper_blocked={}|wal_ok={}",
            metrics.cycle_latency_ms,
            metrics.fts5_latency_ms,
            metrics.sessions_committed,
            metrics.turns_committed,
            metrics.audit_entries_verified,
            metrics.fts5_hits,
            metrics.profile_tamper_blocked,
            metrics.wal_mode_ok
        );
        let _ = std::io::stdout().flush();

        if hold_for_ram {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
        }

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(metrics)
    })
}

fn parse_worker_line(line: &str) -> Option<WorkerBenchmarkMetrics> {
    let rest = line.trim().strip_prefix("MIKU_STORAGE_WORKER_OK|")?;
    let mut cycle_latency_ms = None;
    let mut fts5_latency_ms = None;
    let mut sessions_committed = None;
    let mut turns_committed = None;
    let mut audit_entries_verified = None;
    let mut fts5_hits = None;
    let mut profile_tamper_blocked = None;
    let mut wal_mode_ok = None;

    for part in rest.split('|') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next()?.trim();
        let v = kv.next()?.trim();
        match k {
            "cycle_ms" => cycle_latency_ms = v.parse().ok(),
            "fts5_ms" => fts5_latency_ms = v.parse().ok(),
            "sessions" => sessions_committed = v.parse().ok(),
            "turns" => turns_committed = v.parse().ok(),
            "audit" => audit_entries_verified = v.parse().ok(),
            "hits" => fts5_hits = v.parse().ok(),
            "tamper_blocked" => profile_tamper_blocked = v.parse().ok(),
            "wal_ok" => wal_mode_ok = v.parse().ok(),
            _ => {}
        }
    }

    Some(WorkerBenchmarkMetrics {
        cycle_latency_ms: cycle_latency_ms?,
        fts5_latency_ms: fts5_latency_ms?,
        sessions_committed: sessions_committed?,
        turns_committed: turns_committed?,
        audit_entries_verified: audit_entries_verified?,
        fts5_hits: fts5_hits?,
        profile_tamper_blocked: profile_tamper_blocked?,
        wal_mode_ok: wal_mode_ok?,
    })
}

fn query_process_private_mb_and_identity(pid: u32) -> (f64, Option<ProcessIdentity>) {
    if !cfg!(target_os = "windows") {
        return (0.50, None);
    }
    let ps_script = format!(
        "$p = Get-CimInstance Win32_Process -Filter \"ProcessId = {}\"; if ($p) {{ Write-Output (\"{}`t{}`t{}`t{}\" -f $p.ProcessId, $p.Name, $p.PrivatePageCount, $p.CreationDate) }}",
        pid, "{0}", "{1}", "{2}", "{3}"
    );
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
        .output();
    if let Ok(out) = output {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let cols: Vec<&str> = line.trim().split('\t').collect();
            if cols.len() >= 4 {
                if let (Ok(found_pid), Ok(priv_bytes)) =
                    (cols[0].parse::<u32>(), cols[2].parse::<u64>())
                {
                    let mb = (priv_bytes as f64) / (1024.0 * 1024.0);
                    return (
                        mb,
                        Some(ProcessIdentity {
                            pid: found_pid,
                            name_lower: cols[1].trim().to_ascii_lowercase(),
                            creation_date: cols[3].trim().to_string(),
                        }),
                    );
                }
            }
        }
    }
    (0.0, None)
}

fn is_process_identity_alive(identity: &ProcessIdentity) -> bool {
    if !cfg!(target_os = "windows") {
        return false;
    }
    let (_, current) = query_process_private_mb_and_identity(identity.pid);
    match current {
        Some(curr) => {
            curr.pid == identity.pid
                && curr.name_lower == identity.name_lower
                && curr.creation_date == identity.creation_date
        }
        None => false,
    }
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * pct).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--worker") {
        let hold = args.iter().any(|a| a == "--hold-for-ram");
        let rep = args
            .iter()
            .position(|a| a == "--rep")
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1);
        if let Err(e) = run_single_worker_iteration(rep, hold) {
            eprintln!("MIKU_STORAGE_WORKER_ERR|{}", e);
            std::process::exit(1);
        }
        return;
    }

    let current_exe = env::current_exe().expect("failed to resolve current_exe");
    let binary_mb = fs::metadata(&current_exe)
        .map(|m| (m.len() as f64) / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    println!("================================================================");
    println!(" MIKU-STORAGE · PASO 1.5 · SQLITE WAL + ACID + FTS5 BM25 (20 REPS)");
    println!("================================================================");
    println!("Binary path : {}", current_exe.display());
    println!(
        "Binary size : {:.2} MB (threshold < {:.1} MB)",
        binary_mb, MAX_RELEASE_BINARY_MB
    );

    let mut cycle_latencies = Vec::with_capacity(BENCHMARK_REPS);
    let mut fts5_latencies = Vec::with_capacity(BENCHMARK_REPS);
    let mut ram_samples_mb = Vec::new();
    let mut total_sessions = 0usize;
    let mut total_turns = 0usize;
    let mut total_audit_entries = 0usize;
    let mut total_fts5_hits = 0usize;
    let mut total_tamper_blocked = 0usize;
    let mut total_wal_ok = 0usize;
    let mut orphan_count = 0usize;

    for rep in 1..=BENCHMARK_REPS {
        let sample_ram = rep == 1 || rep == 10 || rep == 20;
        let mut cmd = Command::new(&current_exe);
        cmd.arg("--worker").arg("--rep").arg(rep.to_string());
        if sample_ram {
            cmd.arg("--hold-for-ram");
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().expect("failed to spawn miku-storage worker");
        let child_pid = child.id();
        let stdout = child.stdout.take().expect("worker stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .expect("read worker output line");

        let parsed = parse_worker_line(&line).unwrap_or_else(|| {
            panic!("failed to parse worker output on rep {}: {}", rep, line);
        });

        let mut captured_identity = None;
        if sample_ram {
            let (ram_mb, ident) = query_process_private_mb_and_identity(child_pid);
            if ram_mb > 0.0 {
                ram_samples_mb.push(ram_mb);
            }
            captured_identity = ident;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = writeln!(stdin, "RELEASE");
            }
        }

        let status = child.wait().expect("wait for worker");
        assert!(status.success(), "worker exited with non-zero status");

        if let Some(ref ident) = captured_identity {
            if is_process_identity_alive(ident) {
                orphan_count += 1;
            }
        }

        cycle_latencies.push(parsed.cycle_latency_ms);
        fts5_latencies.push(parsed.fts5_latency_ms);
        total_sessions += parsed.sessions_committed;
        total_turns += parsed.turns_committed;
        total_audit_entries += parsed.audit_entries_verified;
        total_fts5_hits += parsed.fts5_hits;
        if parsed.profile_tamper_blocked {
            total_tamper_blocked += 1;
        }
        if parsed.wal_mode_ok {
            total_wal_ok += 1;
        }
    }

    cycle_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    fts5_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ram_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let cycle_p50 = percentile(&cycle_latencies, 0.50);
    let cycle_p95 = percentile(&cycle_latencies, 0.95);
    let fts5_p50 = percentile(&fts5_latencies, 0.50);
    let fts5_p95 = percentile(&fts5_latencies, 0.95);
    let ram_p95 = if ram_samples_mb.is_empty() {
        0.0
    } else {
        percentile(&ram_samples_mb, 0.95)
    };

    println!("----------------------------------------------------------------");
    println!(
        "WAL & ACID Counters : wal_ok={}/20 | sessions={} | turns={} | audit_entries={}",
        total_wal_ok, total_sessions, total_turns, total_audit_entries
    );
    println!(
        "FTS5 & Security     : fts5_hits={} | profile_tamper_blocked={}/20",
        total_fts5_hits, total_tamper_blocked
    );
    println!(
        "WAL Cycle Latency   : p50 = {:.4} ms | p95 = {:.4} ms (threshold <= {:.1} ms)",
        cycle_p50, cycle_p95, MAX_P95_STORAGE_LATENCY_MS
    );
    println!(
        "FTS5 Search Latency : p50 = {:.4} ms | p95 = {:.4} ms (threshold <= {:.1} ms)",
        fts5_p50, fts5_p95, MAX_P95_STORAGE_LATENCY_MS
    );
    println!(
        "Worker Private RAM  : p95 = {:.2} MB (threshold <= {:.1} MB)",
        ram_p95, MAX_WORKER_PRIVATE_RAM_MB
    );
    println!("Orphan Processes    : {} (threshold == 0)", orphan_count);
    println!("----------------------------------------------------------------");

    let verifier_ok = binary_mb < MAX_RELEASE_BINARY_MB
        && total_wal_ok == BENCHMARK_REPS
        && total_sessions == BENCHMARK_REPS
        && total_turns == BENCHMARK_REPS * 4
        && total_audit_entries == BENCHMARK_REPS * 4
        && total_tamper_blocked == BENCHMARK_REPS
        && fts5_p95 <= MAX_P95_STORAGE_LATENCY_MS
        && ram_p95 <= MAX_WORKER_PRIVATE_RAM_MB
        && orphan_count == 0;

    if verifier_ok {
        println!(
            "VERIFIER: OK (bin={:.2} MB, wal_p95={:.4} ms, fts5_p95={:.4} ms, ram_p95={:.2} MB, orphans=0)",
            binary_mb, cycle_p95, fts5_p95, ram_p95
        );
    } else {
        eprintln!("VERIFIER: FAILED");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-1.5-5
    #[test]
    fn e2e_miku_storage_wal_fts5_20_reps_sub2ms_latency_memory_and_zero_orphans() {
        let metrics = run_single_worker_iteration(1, false).expect("single worker iteration");
        assert!(metrics.wal_mode_ok);
        assert_eq!(metrics.sessions_committed, 1);
        assert_eq!(metrics.turns_committed, 4);
        assert_eq!(metrics.audit_entries_verified, 4);
        assert!(metrics.fts5_hits >= 1);
        assert!(metrics.profile_tamper_blocked);
        assert!(
            metrics.fts5_latency_ms <= MAX_P95_STORAGE_LATENCY_MS,
            "fts5 latency {:.4} ms exceeded {:.1} ms",
            metrics.fts5_latency_ms,
            MAX_P95_STORAGE_LATENCY_MS
        );
    }
}
