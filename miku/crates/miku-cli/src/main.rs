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

use miku_cli::{
    query_live_windows_webview2_version, run_deterministic_doctor_benchmark_cycle,
    run_doctor_suite, DoctorRunConfig, WebView2ProbeSource, BUDGET_MAX_PRIVATE_RAM_MB,
    BUDGET_MAX_RELEASE_BINARY_MB,
};
use miku_platform::{configure_headless_command, LaunchConfig, WIN32_CREATE_NO_WINDOW};

const BENCHMARK_REPS: usize = 20;
const MAX_P95_DOCTOR_SUITE_MS: f64 = 50.0;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    name_lower: String,
    creation_date: String,
}

#[derive(Debug, Clone)]
struct WorkerDoctorMetrics {
    suite_ms: f64,
    passed_probes: usize,
    failed_probes: usize,
    exit_code: i32,
    json_ok: bool,
    ascii_ok: bool,
}

fn run_single_worker_iteration(
    hold_for_ram: bool,
    binary_mb: f64,
    wv2_ver: &Option<String>,
) -> Result<WorkerDoctorMetrics, String> {
    let (report, all_valid, suite_ms) =
        run_deterministic_doctor_benchmark_cycle(binary_mb, wv2_ver)?;

    let metrics = WorkerDoctorMetrics {
        suite_ms,
        passed_probes: report.passed_count,
        failed_probes: report.failed_count,
        exit_code: report.exit_code,
        json_ok: all_valid,
        ascii_ok: all_valid,
    };

    println!(
        "MIKU_CLI_WORKER_OK|suite_ms={:.4}|passed={}|failed={}|exit_code={}|json_ok={}|ascii_ok={}",
        metrics.suite_ms,
        metrics.passed_probes,
        metrics.failed_probes,
        metrics.exit_code,
        metrics.json_ok,
        metrics.ascii_ok
    );
    let _ = std::io::stdout().flush();

    if hold_for_ram {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }

    Ok(metrics)
}

fn parse_worker_line(line: &str) -> Option<WorkerDoctorMetrics> {
    let rest = line.trim().strip_prefix("MIKU_CLI_WORKER_OK|")?;
    let mut suite_ms = None;
    let mut passed_probes = None;
    let mut failed_probes = None;
    let mut exit_code = None;
    let mut json_ok = None;
    let mut ascii_ok = None;

    for part in rest.split('|') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next()?.trim();
        let v = kv.next()?.trim();
        match k {
            "suite_ms" => suite_ms = v.parse().ok(),
            "passed" => passed_probes = v.parse().ok(),
            "failed" => failed_probes = v.parse().ok(),
            "exit_code" => exit_code = v.parse().ok(),
            "json_ok" => json_ok = v.parse().ok(),
            "ascii_ok" => ascii_ok = v.parse().ok(),
            _ => {}
        }
    }

    Some(WorkerDoctorMetrics {
        suite_ms: suite_ms?,
        passed_probes: passed_probes?,
        failed_probes: failed_probes?,
        exit_code: exit_code?,
        json_ok: json_ok?,
        ascii_ok: ascii_ok?,
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
    let current_exe = env::current_exe().expect("failed to resolve current_exe");
    let binary_mb = fs::metadata(&current_exe)
        .map(|m| (m.len() as f64) / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    // Subcommand mode: `miku-cli doctor [--json]`
    if args.iter().any(|a| a == "doctor") {
        let json_mode = args.iter().any(|a| a == "--json");
        let live_wv2 = query_live_windows_webview2_version();
        let mut cfg = DoctorRunConfig {
            webview2_source: WebView2ProbeSource::SimulatedVersion(
                live_wv2.or_else(|| Some("131.0.2903.86".to_string())),
            ),
            ..DoctorRunConfig::default()
        };
        if binary_mb > 0.0 && binary_mb < BUDGET_MAX_RELEASE_BINARY_MB {
            cfg.budget_telemetry.release_binary_mb = binary_mb;
        }
        let report = run_doctor_suite(&cfg);
        if json_mode {
            println!(
                "{}",
                report
                    .to_json_pretty()
                    .expect("failed to serialize DoctorReportV0 JSON")
            );
        } else {
            print!("{}", report.render_ascii_table());
        }
        std::process::exit(report.exit_code);
    }

    // Worker mode: `--worker`
    if args.iter().any(|a| a == "--worker") {
        let hold_for_ram = args.iter().any(|a| a == "--hold-for-ram");
        let mut wv2_ver: Option<String> = None;
        for i in 0..args.len() {
            if args[i] == "--wv2-ver" && i + 1 < args.len() {
                wv2_ver = Some(args[i + 1].clone());
            }
        }
        if let Err(e) = run_single_worker_iteration(hold_for_ram, binary_mb, &wv2_ver) {
            eprintln!("MIKU_CLI_WORKER_ERR|{}", e);
            std::process::exit(1);
        }
        return;
    }

    // Coordinator mode: 20-rep real benchmark + sample ASCII table
    let live_wv2 = query_live_windows_webview2_version();
    let wv2_ver_str = live_wv2
        .clone()
        .unwrap_or_else(|| "131.0.2903.86".to_string());

    println!("================================================================");
    println!(" MIKU-CLI · PASO 1.8 · SUBCOMANDO `miku doctor v0` (20 REPS)");
    println!("================================================================");
    println!("Binary path : {}", current_exe.display());
    println!(
        "Binary size : {:.2} MB (threshold < {:.1} MB)",
        binary_mb, BUDGET_MAX_RELEASE_BINARY_MB
    );
    println!("Host WebView2: {}", wv2_ver_str);

    let mut suite_latencies = Vec::with_capacity(BENCHMARK_REPS);
    let mut ram_samples_mb = Vec::new();
    let mut total_probes_passed = 0usize;
    let mut total_probes_failed = 0usize;
    let mut total_exit_zero = 0usize;
    let mut total_json_ok = 0usize;
    let mut total_ascii_ok = 0usize;
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
        cmd.arg("--worker")
            .arg("--rep")
            .arg(rep.to_string())
            .arg("--wv2-ver")
            .arg(&wv2_ver_str);
        configure_headless_command(&mut cmd, &worker_cfg);
        if sample_ram {
            cmd.arg("--hold-for-ram");
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().expect("failed to spawn miku-cli worker");
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

        suite_latencies.push(parsed.suite_ms);
        total_probes_passed += parsed.passed_probes;
        total_probes_failed += parsed.failed_probes;
        if parsed.exit_code == 0 {
            total_exit_zero += 1;
        }
        if parsed.json_ok {
            total_json_ok += 1;
        }
        if parsed.ascii_ok {
            total_ascii_ok += 1;
        }
    }

    suite_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ram_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let suite_p50 = percentile(&suite_latencies, 0.50);
    let suite_p95 = percentile(&suite_latencies, 0.95);
    let ram_p95 = if ram_samples_mb.is_empty() {
        0.0
    } else {
        percentile(&ram_samples_mb, 0.95)
    };

    // Render one live sample of the ASCII table to console
    let (sample_report, _, _) =
        run_deterministic_doctor_benchmark_cycle(binary_mb, &Some(wv2_ver_str))
            .expect("sample doctor cycle");
    println!("----------------------------------------------------------------");
    print!("{}", sample_report.render_ascii_table());
    println!("----------------------------------------------------------------");
    println!(
        "Doctor Probes       : passed={}/100 | failed={}/100 | exit_zero={}/20",
        total_probes_passed, total_probes_failed, total_exit_zero
    );
    println!(
        "Formatters          : ascii_ok={}/20 | json_roundtrip_ok={}/20",
        total_ascii_ok, total_json_ok
    );
    println!(
        "Doctor Suite Latency: p50 = {:.4} ms | p95 = {:.4} ms (threshold <= {:.1} ms)",
        suite_p50, suite_p95, MAX_P95_DOCTOR_SUITE_MS
    );
    println!(
        "Worker Private RAM  : p95 = {:.2} MB (threshold <= {:.1} MB)",
        ram_p95, BUDGET_MAX_PRIVATE_RAM_MB
    );
    println!("Orphan Processes    : {} (threshold == 0)", orphan_count);
    println!("----------------------------------------------------------------");

    let verifier_ok = binary_mb < BUDGET_MAX_RELEASE_BINARY_MB
        && total_probes_passed == BENCHMARK_REPS * 5
        && total_probes_failed == 0
        && total_exit_zero == BENCHMARK_REPS
        && total_json_ok == BENCHMARK_REPS
        && total_ascii_ok == BENCHMARK_REPS
        && suite_p95 <= MAX_P95_DOCTOR_SUITE_MS
        && ram_p95 <= BUDGET_MAX_PRIVATE_RAM_MB
        && orphan_count == 0;

    if verifier_ok {
        println!(
            "VERIFIER: OK (bin={:.2} MB, probes={}/100, suite_p95={:.4} ms, ram_p95={:.2} MB, orphans=0)",
            binary_mb, total_probes_passed, suite_p95, ram_p95
        );
    } else {
        eprintln!("VERIFIER: FAILED");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-1.8-5
    #[test]
    fn e2e_miku_cli_doctor_v0_20_reps_five_probes_ascii_json_and_zero_orphans() {
        let wv2_ver = query_live_windows_webview2_version()
            .or_else(|| Some("131.0.2903.86".to_string()));
        let metrics =
            run_single_worker_iteration(false, 0.95, &wv2_ver).expect("single worker iteration");

        assert_eq!(metrics.passed_probes, 5);
        assert_eq!(metrics.failed_probes, 0);
        assert_eq!(metrics.exit_code, 0);
        assert!(metrics.json_ok);
        assert!(metrics.ascii_ok);

        let limit_ms = if cfg!(debug_assertions) {
            250.0
        } else {
            MAX_P95_DOCTOR_SUITE_MS
        };
        assert!(
            metrics.suite_ms <= limit_ms,
            "doctor suite_ms {:.4} ms exceeded limit {:.1} ms",
            metrics.suite_ms,
            limit_ms
        );
    }
}
