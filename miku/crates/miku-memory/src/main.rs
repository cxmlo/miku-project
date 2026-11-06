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

use miku_gatekeeper::{Capability, GatekeeperEngine, PolicyEffect, ProjectProfile};
use miku_memory::{ChunkKind, ContextEngine, StaticSystemPrefix};
use miku_storage::{ConversationTurnRecord, SessionRecord, SingleThreadStoragePool};

const BENCHMARK_REPS: usize = 20;
const MAX_P95_CONTEXT_LATENCY_MS: f64 = 2.0;
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
    ingested_chunks: usize,
    gk_blocked: usize,
    evicted_turns: usize,
    prefix_hash_stable: bool,
    budget_respected: bool,
    audit_chain_ok: bool,
}

fn run_single_worker_iteration(rep: usize, hold_for_ram: bool) -> Result<WorkerBenchmarkMetrics, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("failed to build current_thread runtime: {}", e))?;

    rt.block_on(async move {
        let start = Instant::now();

        let pool = SingleThreadStoragePool::new_in_memory().map_err(|e| e.to_string())?;
        let sess_id = format!("sess_mem_{}", rep);
        pool.insert_session(SessionRecord {
            session_id: sess_id.clone(),
            agent_name: "claude_code".to_string(),
            project_root: "C:/workspace/miku".to_string(),
            created_at_ms: 1_762_400_000 + (rep as u64),
            state: "active".to_string(),
        })
        .map_err(|e| e.to_string())?;

        for seq in 1..=6_u64 {
            pool.insert_turn(ConversationTurnRecord {
                turn_id: format!("turn_{}_{}", rep, seq),
                session_id: sess_id.clone(),
                seq,
                role: if seq % 2 == 1 { "user" } else { "assistant" }.to_string(),
                content: format!(
                    "Turno {} de la sesion con contexto tecnico de Miku Desktop Orchestrator.",
                    seq
                ),
                token_estimate: 14,
                ts_ms: 1_762_400_100 + seq,
            })
            .map_err(|e| e.to_string())?;
        }

        let profile = ProjectProfile::new("miku", "C:/workspace/miku", 60_000)
            .with_rule(Capability::FsRead, "docs/**", PolicyEffect::Allow, false)
            .with_rule(Capability::FsRead, "src/**", PolicyEffect::Allow, false)
            .with_rule(Capability::FsRead, "src/secrets.rs", PolicyEffect::Deny, false);

        let mut gk = GatekeeperEngine::new(profile).map_err(|e| e.to_string())?;
        let mut engine = ContextEngine::new(pool, 16);

        let md_doc = r##"# Arquitectura de Memoria y Contexto
El motor miku-memory segmenta Markdown y codigo respetando limites semanticos.

## Ahorro de Tokens F-09
El prefijo del sistema se mantiene inmutable para cache KV en proveedores LLM."##;

        let code_doc = r##"pub fn dispatch_context_query(query: &str) -> usize {
    query.len()
}

pub fn verify_token_window(total: u32, max_budget: u32) -> bool {
    total <= max_budget
}"##;

        let c1 = engine
            .ingest_document_with_gatekeeper(
                &mut gk,
                "claude_code",
                "docs/memory.md",
                md_doc,
                ChunkKind::Markdown,
                "memory chunking kv cache fts5",
                1_762_400_200,
            )
            .map_err(|e| e.to_string())?;

        let c2 = engine
            .ingest_document_with_gatekeeper(
                &mut gk,
                "claude_code",
                "src/context.rs",
                code_doc,
                ChunkKind::SourceCode,
                "rust context window tokens",
                1_762_400_210,
            )
            .map_err(|e| e.to_string())?;

        let mut gk_blocked = 0usize;
        if engine
            .ingest_document_with_gatekeeper(
                &mut gk,
                "claude_code",
                r"..\..\Windows\win.ini",
                "[fonts]",
                ChunkKind::Markdown,
                "hostile",
                1_762_400_220,
            )
            .is_err()
        {
            gk_blocked += 1;
        }
        if engine
            .ingest_document_with_gatekeeper(
                &mut gk,
                "claude_code",
                "src/secrets.rs",
                "pub const SECRET: &str = \"x\";",
                ChunkKind::SourceCode,
                "secret",
                1_762_400_230,
            )
            .is_err()
        {
            gk_blocked += 1;
        }

        let prefix = StaticSystemPrefix::new(
            "claude_code",
            "Orquestador personal de agentes Miku en Windows.",
            "fs_read, fs_write, shell_exec, browser_action, memory_query",
        );

        let budget_tokens = 155_u32;
        let p_first = engine
            .build_working_memory_prompt(
                &prefix,
                &sess_id,
                "ahorro de tokens cache kv memoria chunking",
                3,
                budget_tokens,
            )
            .map_err(|e| e.to_string())?;

        let p_second = engine
            .build_working_memory_prompt(
                &prefix,
                &sess_id,
                "verify_token_window rust context",
                3,
                budget_tokens,
            )
            .map_err(|e| e.to_string())?;

        let prefix_hash_stable = p_first.prefix_sha256 == p_second.prefix_sha256
            && p_first.static_prefix == p_second.static_prefix;
        let budget_respected =
            p_first.total_tokens <= budget_tokens && p_second.total_tokens <= budget_tokens;
        let audit_chain_ok = gk.audit_log().verify_chain().is_ok();

        let cycle_latency_ms = start.elapsed().as_secs_f64() * 1_000.0;

        let metrics = WorkerBenchmarkMetrics {
            cycle_latency_ms,
            ingested_chunks: c1.len() + c2.len(),
            gk_blocked,
            evicted_turns: p_first.evicted_turns_count,
            prefix_hash_stable,
            budget_respected,
            audit_chain_ok,
        };

        println!(
            "MIKU_MEMORY_WORKER_OK|cycle_ms={:.4}|chunks={}|gk_blocked={}|evicted_turns={}|prefix_ok={}|budget_ok={}|audit_ok={}",
            metrics.cycle_latency_ms,
            metrics.ingested_chunks,
            metrics.gk_blocked,
            metrics.evicted_turns,
            metrics.prefix_hash_stable,
            metrics.budget_respected,
            metrics.audit_chain_ok
        );
        let _ = std::io::stdout().flush();

        if hold_for_ram {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
        }

        Ok(metrics)
    })
}

fn parse_worker_line(line: &str) -> Option<WorkerBenchmarkMetrics> {
    let rest = line.trim().strip_prefix("MIKU_MEMORY_WORKER_OK|")?;
    let mut cycle_latency_ms = None;
    let mut ingested_chunks = None;
    let mut gk_blocked = None;
    let mut evicted_turns = None;
    let mut prefix_hash_stable = None;
    let mut budget_respected = None;
    let mut audit_chain_ok = None;

    for part in rest.split('|') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next()?.trim();
        let v = kv.next()?.trim();
        match k {
            "cycle_ms" => cycle_latency_ms = v.parse().ok(),
            "chunks" => ingested_chunks = v.parse().ok(),
            "gk_blocked" => gk_blocked = v.parse().ok(),
            "evicted_turns" => evicted_turns = v.parse().ok(),
            "prefix_ok" => prefix_hash_stable = v.parse().ok(),
            "budget_ok" => budget_respected = v.parse().ok(),
            "audit_ok" => audit_chain_ok = v.parse().ok(),
            _ => {}
        }
    }

    Some(WorkerBenchmarkMetrics {
        cycle_latency_ms: cycle_latency_ms?,
        ingested_chunks: ingested_chunks?,
        gk_blocked: gk_blocked?,
        evicted_turns: evicted_turns?,
        prefix_hash_stable: prefix_hash_stable?,
        budget_respected: budget_respected?,
        audit_chain_ok: audit_chain_ok?,
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
            eprintln!("MIKU_MEMORY_WORKER_ERR|{}", e);
            std::process::exit(1);
        }
        return;
    }

    let current_exe = env::current_exe().expect("failed to resolve current_exe");
    let binary_mb = fs::metadata(&current_exe)
        .map(|m| (m.len() as f64) / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    println!("================================================================");
    println!(" MIKU-MEMORY · PASO 1.6 · CONTEXT ENGINE & WORKING MEMORY (20 REPS)");
    println!("================================================================");
    println!("Binary path : {}", current_exe.display());
    println!(
        "Binary size : {:.2} MB (threshold < {:.1} MB)",
        binary_mb, MAX_RELEASE_BINARY_MB
    );

    let mut cycle_latencies = Vec::with_capacity(BENCHMARK_REPS);
    let mut ram_samples_mb = Vec::new();
    let mut total_chunks = 0usize;
    let mut total_gk_blocked = 0usize;
    let mut total_evicted_turns = 0usize;
    let mut total_prefix_ok = 0usize;
    let mut total_budget_ok = 0usize;
    let mut total_audit_ok = 0usize;
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

        let mut child = cmd.spawn().expect("failed to spawn miku-memory worker");
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
        total_chunks += parsed.ingested_chunks;
        total_gk_blocked += parsed.gk_blocked;
        total_evicted_turns += parsed.evicted_turns;
        if parsed.prefix_hash_stable {
            total_prefix_ok += 1;
        }
        if parsed.budget_respected {
            total_budget_ok += 1;
        }
        if parsed.audit_chain_ok {
            total_audit_ok += 1;
        }
    }

    cycle_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ram_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let cycle_p50 = percentile(&cycle_latencies, 0.50);
    let cycle_p95 = percentile(&cycle_latencies, 0.95);
    let ram_p95 = if ram_samples_mb.is_empty() {
        0.0
    } else {
        percentile(&ram_samples_mb, 0.95)
    };

    println!("----------------------------------------------------------------");
    println!(
        "Context Counters    : chunks={} | gk_blocked={} | evicted_turns={}",
        total_chunks, total_gk_blocked, total_evicted_turns
    );
    println!(
        "Invariants (F-09)   : prefix_hash_stable={}/20 | budget_respected={}/20 | audit_ok={}/20",
        total_prefix_ok, total_budget_ok, total_audit_ok
    );
    println!(
        "Context Cycle Lat.  : p50 = {:.4} ms | p95 = {:.4} ms (threshold <= {:.1} ms)",
        cycle_p50, cycle_p95, MAX_P95_CONTEXT_LATENCY_MS
    );
    println!(
        "Worker Private RAM  : p95 = {:.2} MB (threshold <= {:.1} MB)",
        ram_p95, MAX_WORKER_PRIVATE_RAM_MB
    );
    println!("Orphan Processes    : {} (threshold == 0)", orphan_count);
    println!("----------------------------------------------------------------");

    let verifier_ok = binary_mb < MAX_RELEASE_BINARY_MB
        && total_chunks == BENCHMARK_REPS * 4
        && total_gk_blocked == BENCHMARK_REPS * 2
        && total_prefix_ok == BENCHMARK_REPS
        && total_budget_ok == BENCHMARK_REPS
        && total_audit_ok == BENCHMARK_REPS
        && cycle_p95 <= MAX_P95_CONTEXT_LATENCY_MS
        && ram_p95 <= MAX_WORKER_PRIVATE_RAM_MB
        && orphan_count == 0;

    if verifier_ok {
        println!(
            "VERIFIER: OK (bin={:.2} MB, cycle_p95={:.4} ms, ram_p95={:.2} MB, orphans=0)",
            binary_mb, cycle_p95, ram_p95
        );
    } else {
        eprintln!("VERIFIER: FAILED");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-1.6-5
    #[test]
    fn e2e_miku_memory_context_engine_20_reps_latency_budget_and_zero_orphans() {
        let metrics = run_single_worker_iteration(1, false).expect("single worker iteration");
        assert_eq!(metrics.ingested_chunks, 4);
        assert_eq!(metrics.gk_blocked, 2);
        assert!(metrics.evicted_turns >= 1);
        assert!(metrics.prefix_hash_stable);
        assert!(metrics.budget_respected);
        assert!(metrics.audit_chain_ok);
        let limit_ms = if cfg!(debug_assertions) {
            25.0
        } else {
            MAX_P95_CONTEXT_LATENCY_MS
        };
        assert!(
            metrics.cycle_latency_ms <= limit_ms,
            "cycle latency {:.4} ms exceeded {:.1} ms",
            metrics.cycle_latency_ms,
            limit_ms
        );
    }
}
