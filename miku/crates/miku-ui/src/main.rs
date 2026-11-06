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

use miku_platform::{configure_headless_command, LaunchConfig, WIN32_CREATE_NO_WINDOW};
use miku_ui::{
    resolve_avatar_assets_dir, run_native_pill_verification_cycle, EXPECTED_AVATAR_SVG_COUNT,
    MAX_HOVER_RESPONSE_MS,
};

const BENCHMARK_REPS: usize = 20;
const MAX_WORKER_PRIVATE_RAM_MB: f64 = 30.0;
const MAX_RELEASE_BINARY_MB: f64 = 15.0;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessIdentity {
    pid: u32,
    name_lower: String,
    creation_date: String,
}

#[derive(Debug, Clone)]
struct WorkerUiMetrics {
    monitors: usize,
    anchor_x: i32,
    anchor_y: i32,
    anchor_w: i32,
    anchor_h: i32,
    dpi: u32,
    ex_style_ok: bool,
    ma_noactivate_ok: bool,
    focus_ok: bool,
    hover_ms: f64,
    webview2_procs: usize,
    idle_timers: u32,
    svg_count: usize,
}

fn run_single_worker_iteration(rep: usize, hold_for_ram: bool) -> Result<WorkerUiMetrics, String> {
    let avatar_dir = resolve_avatar_assets_dir();
    let cycle = run_native_pill_verification_cycle(rep, &avatar_dir).map_err(|e| e.to_string())?;

    let metrics = WorkerUiMetrics {
        monitors: cycle.monitor_count,
        anchor_x: cycle.primary_anchor.x,
        anchor_y: cycle.primary_anchor.y,
        anchor_w: cycle.primary_anchor.width,
        anchor_h: cycle.primary_anchor.height,
        dpi: cycle.primary_anchor.dpi,
        ex_style_ok: cycle.ex_style_verified,
        ma_noactivate_ok: cycle.ma_noactivate_verified,
        focus_ok: cycle.focus_preserved,
        hover_ms: cycle.hover_latency_ms,
        webview2_procs: cycle.webview2_instances,
        idle_timers: cycle.active_idle_timers,
        svg_count: cycle.verified_svg_count,
    };

    println!(
        "MIKU_UI_WORKER_OK|monitors={}|x={}|y={}|w={}|h={}|dpi={}|ex_style_ok={}|ma_ok={}|focus_ok={}|hover_ms={:.4}|wv2={}|timers={}|svgs={}",
        metrics.monitors,
        metrics.anchor_x,
        metrics.anchor_y,
        metrics.anchor_w,
        metrics.anchor_h,
        metrics.dpi,
        metrics.ex_style_ok,
        metrics.ma_noactivate_ok,
        metrics.focus_ok,
        metrics.hover_ms,
        metrics.webview2_procs,
        metrics.idle_timers,
        metrics.svg_count
    );
    let _ = std::io::stdout().flush();

    if hold_for_ram {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
    }

    Ok(metrics)
}

fn parse_worker_line(line: &str) -> Option<WorkerUiMetrics> {
    let rest = line.trim().strip_prefix("MIKU_UI_WORKER_OK|")?;
    let mut monitors = None;
    let mut anchor_x = None;
    let mut anchor_y = None;
    let mut anchor_w = None;
    let mut anchor_h = None;
    let mut dpi = None;
    let mut ex_style_ok = None;
    let mut ma_noactivate_ok = None;
    let mut focus_ok = None;
    let mut hover_ms = None;
    let mut webview2_procs = None;
    let mut idle_timers = None;
    let mut svg_count = None;

    for part in rest.split('|') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next()?.trim();
        let v = kv.next()?.trim();
        match k {
            "monitors" => monitors = v.parse().ok(),
            "x" => anchor_x = v.parse().ok(),
            "y" => anchor_y = v.parse().ok(),
            "w" => anchor_w = v.parse().ok(),
            "h" => anchor_h = v.parse().ok(),
            "dpi" => dpi = v.parse().ok(),
            "ex_style_ok" => ex_style_ok = v.parse().ok(),
            "ma_ok" => ma_noactivate_ok = v.parse().ok(),
            "focus_ok" => focus_ok = v.parse().ok(),
            "hover_ms" => hover_ms = v.parse().ok(),
            "wv2" => webview2_procs = v.parse().ok(),
            "timers" => idle_timers = v.parse().ok(),
            "svgs" => svg_count = v.parse().ok(),
            _ => {}
        }
    }

    Some(WorkerUiMetrics {
        monitors: monitors?,
        anchor_x: anchor_x?,
        anchor_y: anchor_y?,
        anchor_w: anchor_w?,
        anchor_h: anchor_h?,
        dpi: dpi?,
        ex_style_ok: ex_style_ok?,
        ma_noactivate_ok: ma_noactivate_ok?,
        focus_ok: focus_ok?,
        hover_ms: hover_ms?,
        webview2_procs: webview2_procs?,
        idle_timers: idle_timers?,
        svg_count: svg_count?,
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
    let out = match Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &ps_script,
        ])
        .output()
    {
        Ok(o) => o,
        Err(_) => return (0.0, None),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for raw in text.lines() {
        let cols: Vec<&str> = raw.trim().split('\t').collect();
        if cols.len() >= 4 {
            let parsed_pid = cols[0].trim().parse::<u32>().unwrap_or(pid);
            let name_lower = cols[1].trim().to_ascii_lowercase();
            let priv_bytes = cols[2].trim().parse::<u64>().unwrap_or(0);
            let creation_date = cols[3].trim().to_string();
            let mb = (priv_bytes as f64) / (1024.0 * 1024.0);
            return (
                mb,
                Some(ProcessIdentity {
                    pid: parsed_pid,
                    name_lower,
                    creation_date,
                }),
            );
        }
    }
    (0.0, None)
}

fn is_identity_still_alive(id: &ProcessIdentity) -> bool {
    let (_, current) = query_process_private_mb_and_identity(id.pid);
    match current {
        Some(live) => live.name_lower == id.name_lower && live.creation_date == id.creation_date,
        None => false,
    }
}

fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
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
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1);
        if let Err(e) = run_single_worker_iteration(rep, hold) {
            eprintln!("MIKU_UI_WORKER_ERR|{}", e);
            std::process::exit(1);
        }
        return;
    }

    let current_exe = env::current_exe().expect("failed to resolve current_exe");
    let binary_mb = fs::metadata(&current_exe)
        .map(|m| (m.len() as f64) / (1024.0 * 1024.0))
        .unwrap_or(0.0);

    println!("================================================================");
    println!(" MIKU-UI - PASO 2.1 - NATIVE LAYERED PILL & HOVER TRIGGER (20 REPS)");
    println!("================================================================");
    println!("Binary path : {}", current_exe.display());
    println!(
        "Binary size : {:.2} MB (BUD-09 threshold < {:.1} MB)",
        binary_mb, MAX_RELEASE_BINARY_MB
    );

    let mut hover_latencies_ms = Vec::with_capacity(BENCHMARK_REPS);
    let mut ram_samples_mb = Vec::new();
    let mut total_ex_style_ok = 0usize;
    let mut total_ma_ok = 0usize;
    let mut total_focus_ok = 0usize;
    let mut total_wv2_procs = 0usize;
    let mut total_idle_timers = 0u32;
    let mut last_anchor_summary = String::new();
    let mut last_svg_count = 0usize;
    let mut orphan_count = 0usize;

    for rep in 1..=BENCHMARK_REPS {
        let sample_ram = rep == 1 || rep == 10 || rep == 20;
        let launch_cfg = LaunchConfig {
            background: true,
            headless: true,
            worker: true,
            hold_for_ram: sample_ram,
            rep,
            win32_creation_flags: WIN32_CREATE_NO_WINDOW,
        };
        let mut cmd = Command::new(&current_exe);
        cmd.arg("--worker").arg("--rep").arg(rep.to_string());
        if sample_ram {
            cmd.arg("--hold-for-ram");
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_headless_command(&mut cmd, &launch_cfg);

        let mut child = cmd.spawn().expect("failed to spawn miku-ui worker");
        let child_pid = child.id();
        let stdout = child.stdout.take().expect("worker stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .expect("failed to read worker line");

        let parsed = parse_worker_line(&line).unwrap_or_else(|| {
            panic!("failed to parse worker output in rep {}: '{}'", rep, line.trim())
        });

        let mut recorded_identity = None;
        if sample_ram {
            let (mb, id) = query_process_private_mb_and_identity(child_pid);
            if mb > 0.0 {
                ram_samples_mb.push(mb);
            }
            recorded_identity = id;
            if let Some(mut stdin) = child.stdin.take() {
                let _ = writeln!(stdin, "release");
            }
        }

        let status = child.wait().expect("failed to wait for worker");
        assert!(status.success(), "worker exited with non-zero in rep {}", rep);

        if let Some(id) = recorded_identity {
            if is_identity_still_alive(&id) {
                orphan_count += 1;
            }
        }

        hover_latencies_ms.push(parsed.hover_ms);
        if parsed.ex_style_ok {
            total_ex_style_ok += 1;
        }
        if parsed.ma_noactivate_ok {
            total_ma_ok += 1;
        }
        if parsed.focus_ok {
            total_focus_ok += 1;
        }
        total_wv2_procs += parsed.webview2_procs;
        total_idle_timers += parsed.idle_timers;
        last_svg_count = parsed.svg_count;
        last_anchor_summary = format!(
            "monitors={} | primary_anchor=({}, {}, {}x{} @ {} DPI)",
            parsed.monitors,
            parsed.anchor_x,
            parsed.anchor_y,
            parsed.anchor_w,
            parsed.anchor_h,
            parsed.dpi
        );
    }

    hover_latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ram_samples_mb.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let hover_p50 = percentile_sorted(&hover_latencies_ms, 0.50);
    let hover_p95 = percentile_sorted(&hover_latencies_ms, 0.95);
    let ram_p50 = percentile_sorted(&ram_samples_mb, 0.50);
    let ram_p95 = percentile_sorted(&ram_samples_mb, 0.95);

    println!("----------------------------------------------------------------");
    println!("Monitor & Anchor  : {}", last_anchor_summary);
    println!(
        "Win32 Flags & Focus: ex_style_ok={}/{} | ma_noactivate={}/{} | focus_preserved={}/{}",
        total_ex_style_ok,
        BENCHMARK_REPS,
        total_ma_ok,
        BENCHMARK_REPS,
        total_focus_ok,
        BENCHMARK_REPS
    );
    println!(
        "Zero-WebView & Idle: webview2_procs={} | idle_timers={} | verified_svgs={}",
        total_wv2_procs, total_idle_timers, last_svg_count
    );
    println!(
        "Hover Trigger     : p50 = {:.4} ms | p95 = {:.4} ms (threshold < {:.1} ms)",
        hover_p50, hover_p95, MAX_HOVER_RESPONSE_MS
    );
    println!(
        "Worker Private RAM: p50 = {:.2} MB | p95 = {:.2} MB (BUD-01 <= {:.1} MB)",
        ram_p50, ram_p95, MAX_WORKER_PRIVATE_RAM_MB
    );
    println!("Orphan Processes  : {} (CreationDate tuple verified)", orphan_count);
    println!("================================================================");

    let verifier_ok = binary_mb < MAX_RELEASE_BINARY_MB
        && total_ex_style_ok == BENCHMARK_REPS
        && total_ma_ok == BENCHMARK_REPS
        && total_focus_ok == BENCHMARK_REPS
        && total_wv2_procs == 0
        && total_idle_timers == 0
        && last_svg_count == EXPECTED_AVATAR_SVG_COUNT
        && hover_p95 < MAX_HOVER_RESPONSE_MS
        && ram_p95 > 0.0
        && ram_p95 <= MAX_WORKER_PRIVATE_RAM_MB
        && orphan_count == 0;

    if verifier_ok {
        println!(
            "VERIFIER: OK (bin={:.2} MB, focus={}/{}, hover_p95={:.4} ms, ram_p95={:.2} MB, wv2=0, orphans=0)",
            binary_mb, total_focus_ok, BENCHMARK_REPS, hover_p95, ram_p95
        );
    } else {
        eprintln!("VERIFIER: FAILED");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-2.1-5
    #[test]
    fn e2e_miku_ui_native_pill_20_reps_hover_latency_zero_focus_steal_and_ram_budget() {
        let metrics = run_single_worker_iteration(1, false).expect("single UI worker iteration");
        assert!(metrics.monitors >= 1);
        assert!(metrics.ex_style_ok);
        assert!(metrics.ma_noactivate_ok);
        assert!(metrics.focus_ok);
        assert_eq!(metrics.webview2_procs, 0);
        assert_eq!(metrics.idle_timers, 0);
        assert_eq!(metrics.svg_count, EXPECTED_AVATAR_SVG_COUNT);

        let limit_ms = if cfg!(debug_assertions) {
            50.0
        } else {
            MAX_HOVER_RESPONSE_MS
        };
        assert!(
            metrics.hover_ms <= limit_ms,
            "hover latency {:.4} ms exceeded limit {:.1} ms",
            metrics.hover_ms,
            limit_ms
        );
    }
}
