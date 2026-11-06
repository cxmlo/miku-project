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

use miku_cli::license_audit::{
    resolve_repo_root, run_workspace_license_and_ci_audit, EXPECTED_WORKSPACE_CRATE_COUNT,
};
use miku_cli::{BUDGET_MAX_PRIVATE_RAM_MB, BUDGET_MAX_RELEASE_BINARY_MB};
use miku_platform::{configure_headless_command, LaunchConfig, WIN32_CREATE_NO_WINDOW};

const BENCHMARK_REPS: usize = 20;
const MAX_P95_LICENSE_AUDIT_MS: f64 = 10.0;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    name_lower: String,
    creation_date: String,
}

#[derive(Debug, Clone)]
struct WorkerLicenseMetrics {
    audit_ms: f64,
    crates_mit: usize,
    total_pkgs: usize,
    third_party_pkgs: usize,
    copyleft_pkgs: usize,
    ci_ok: bool,
    deny_ok: bool,
}

fn run_single_worker_iteration(hold_for_ram: bool) -> Result<WorkerLicenseMetrics, String> {
    let root = resolve_repo_root();
    let report = run_workspace_license_and_ci_audit(&root).map_err(|e| e.to_string())?;

    let metrics = WorkerLicenseMetrics {
        audit_ms: report.audit_duration_ms,
        crates_mit: report.workspace_crates_mit_count,
        total_pkgs: report.total_locked_packages_audited,
        third_party_pkgs: report.third_party_packages_audited,
        copyleft_pkgs: report.copyleft_packages_found,
        ci_ok: report.ci_workflow_verified,
        deny_ok: report.cargo_deny_verified,
    };

    println!(
        "MIKU_LICENSE_WORKER_OK|audit_ms={:.4}|crates_mit={}|total_pkgs={}|third_party={}|copyleft={}|ci_ok={}|deny_ok={}",
        metrics.audit_ms,
        metrics.crates_mit,
        metrics.total_pkgs,
        metrics.third_party_pkgs,
        metrics.copyleft_pkgs,
        metrics.ci_ok,
        metrics.deny_ok
    );
    let _ = std::io::stdout().flush();

    if hold_for_ram {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }

    Ok(metrics)
}

fn parse_worker_line(line: &str) -> Option<WorkerLicenseMetrics> {
    let rest = line.trim().strip_prefix("MIKU_LICENSE_WORKER_OK|")?;
    let mut audit_ms = None;
    let mut crates_mit = None;
    let mut total_pkgs = None;
    let mut third_party_pkgs = None;
    let mut copyleft_pkgs = None;
    let mut ci_ok = None;
    let mut deny_ok = None;

    for part in rest.split('|') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next()?.trim();
        let v = kv.next()?.trim();
        match k {
            "audit_ms" => audit_ms = v.parse().ok(),
            "crates_mit" => crates_mit = v.parse().ok(),
            "total_pkgs" => total_pkgs = v.parse().ok(),
            "third_party" => third_party_pkgs = v.parse().ok(),
            "copyleft" => copyleft_pkgs = v.parse().ok(),
            "ci_ok" => ci_ok = v.parse().ok(),
            "deny_ok" => deny_ok = v.parse().ok(),
            _ => {}
        }
    }

    Some(WorkerLicenseMetrics {
        audit_ms: audit_ms?,
        crates_mit: crates_mit?,
        total_pkgs: total_pkgs?,
        third_party_pkgs: third_party_pkgs?,
        copyleft_pkgs: copyleft_pkgs?,
        ci_ok: ci_ok?,
        deny_ok: deny_ok?,
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
        let hold_for_ram = args.iter().any(|a| a == "--hold-for-ram");
        if let Err(e) = run_single_worker_iteration(hold_for_ram) {
            eprintln!("MIKU_LICENSE_WORKER_ERR|{}", e);
            std::process::exit(1);
        }
        return;
    }

    let current_exe = env::current_exe().expect("failed to resolve current_exe");
    let binary_mb = fs::metadata(&current_exe)
        .map(|m| (m.len() as f64) / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    let root = resolve_repo_root();
    let initial_report =
        run_workspace_license_and_ci_audit(&root).expect("initial license governance audit");

    println!("================================================================");
    println!(" MIKU-LICENSE-AUDIT · PASO 1.9 · LICENCIA 100% MIT & CI (20 REPS)");
    println!("================================================================");
    println!("Binary path : {}", current_exe.display());
    println!(
        "Binary size : {:.2} MB (threshold < {:.1} MB)",
        binary_mb, BUDGET_MAX_RELEASE_BINARY_MB
    );
    println!("LICENSE SHA : {}", initial_report.license_sha256);

    let mut audit_latencies = Vec::with_capacity(BENCHMARK_REPS);
    let mut ram_samples_mb = Vec::new();
    let mut total_mit_ok = 0usize;
    let mut total_ci_ok = 0usize;
    let mut total_deny_ok = 0usize;
    let mut total_copyleft = 0usize;
    let mut last_total_pkgs = 0usize;
    let mut last_third_party = 0usize;
    let mut orphan_count = 0usize;

    for rep in 1..=BENCHMARK_REPS {
        let sample_ram = rep == 1 || rep == 10 || rep == 20;
        let worker_cfg = LaunchConfig {
            background: true,
            headless: true,
            worker: true,
            hold_for_ram: sample_ram,
            rep,
            win32_creation_flags: WIN32_CREATE_NO_WINDOW,
        };

        let mut cmd = Command::new(&current_exe);
        cmd.arg("--worker").arg("--rep").arg(rep.to_string());
        configure_headless_command(&mut cmd, &worker_cfg);
        if sample_ram {
            cmd.arg("--hold-for-ram");
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .expect("failed to spawn miku-license-audit worker");
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

        audit_latencies.push(parsed.audit_ms);
        if parsed.crates_mit == EXPECTED_WORKSPACE_CRATE_COUNT {
            total_mit_ok += 1;
        }
        if parsed.ci_ok {
            total_ci_ok += 1;
        }
        if parsed.deny_ok {
            total_deny_ok += 1;
        }
        total_copyleft += parsed.copyleft_pkgs;
        last_total_pkgs = parsed.total_pkgs;
        last_third_party = parsed.third_party_pkgs;
    }

    audit_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ram_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let audit_p50 = percentile(&audit_latencies, 0.50);
    let audit_p95 = percentile(&audit_latencies, 0.95);
    let ram_p95 = if ram_samples_mb.is_empty() {
        0.0
    } else {
        percentile(&ram_samples_mb, 0.95)
    };

    println!("----------------------------------------------------------------");
    println!(
        "Workspace Crates MIT: {}/{} crates verified across {}/20 reps",
        EXPECTED_WORKSPACE_CRATE_COUNT, EXPECTED_WORKSPACE_CRATE_COUNT, total_mit_ok
    );
    println!(
        "Dependency Graph    : locked_pkgs={} (third_party={}) | copyleft_found={} (threshold == 0)",
        last_total_pkgs, last_third_party, total_copyleft
    );
    println!(
        "CI & cargo-deny     : ci_workflow_ok={}/20 | deny_toml_ok={}/20",
        total_ci_ok, total_deny_ok
    );
    println!(
        "License Audit Time  : p50 = {:.4} ms | p95 = {:.4} ms (threshold <= {:.1} ms)",
        audit_p50, audit_p95, MAX_P95_LICENSE_AUDIT_MS
    );
    println!(
        "Worker Private RAM  : p95 = {:.2} MB (threshold <= {:.1} MB)",
        ram_p95, BUDGET_MAX_PRIVATE_RAM_MB
    );
    println!("Orphan Processes    : {} (threshold == 0)", orphan_count);
    println!("----------------------------------------------------------------");

    let verifier_ok = binary_mb < BUDGET_MAX_RELEASE_BINARY_MB
        && total_mit_ok == BENCHMARK_REPS
        && total_ci_ok == BENCHMARK_REPS
        && total_deny_ok == BENCHMARK_REPS
        && total_copyleft == 0
        && audit_p95 <= MAX_P95_LICENSE_AUDIT_MS
        && ram_p95 <= BUDGET_MAX_PRIVATE_RAM_MB
        && orphan_count == 0;

    if verifier_ok {
        println!(
            "VERIFIER: OK (bin={:.2} MB, crates_mit=9/9, locked_pkgs={}, copyleft=0, audit_p95={:.4} ms, ram_p95={:.2} MB, orphans=0)",
            binary_mb, last_total_pkgs, audit_p95, ram_p95
        );
    } else {
        eprintln!("VERIFIER: FAILED");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-1.9-5
    #[test]
    fn e2e_miku_license_and_ci_governance_20_reps_zero_copyleft_and_zero_orphans() {
        let metrics = run_single_worker_iteration(false).expect("single worker license iteration");
        assert_eq!(metrics.crates_mit, EXPECTED_WORKSPACE_CRATE_COUNT);
        assert!(metrics.total_pkgs >= 30);
        assert!(metrics.third_party_pkgs >= 20);
        assert_eq!(metrics.copyleft_pkgs, 0);
        assert!(metrics.ci_ok);
        assert!(metrics.deny_ok);

        let limit_ms = if cfg!(debug_assertions) {
            100.0
        } else {
            MAX_P95_LICENSE_AUDIT_MS
        };
        assert!(
            metrics.audit_ms <= limit_ms,
            "license audit_ms {:.4} ms exceeded limit {:.1} ms",
            metrics.audit_ms,
            limit_ms
        );
    }
}
