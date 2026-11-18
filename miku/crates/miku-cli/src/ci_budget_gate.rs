use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::unified_runtime::run_unified_smoke_summary;
use crate::{
    BUDGET_MAX_COLD_START_WS_MS, BUDGET_MAX_PRIVATE_RAM_MB, BUDGET_MAX_RELEASE_BINARY_MB,
};

pub const BUDGET_MAX_TOTAL_BOOT_MS: f64 = 1000.0;
pub const DEFAULT_CI_MEDIAN_RUNS: usize = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CiBudgetThresholds {
    pub max_idle_ram_mb: f64,
    pub max_pill_start_ms: f64,
    pub max_cold_boot_ms: f64,
    pub max_binary_mb: f64,
    pub runs: usize,
}

impl Default for CiBudgetThresholds {
    fn default() -> Self {
        Self {
            max_idle_ram_mb: BUDGET_MAX_PRIVATE_RAM_MB,
            max_pill_start_ms: BUDGET_MAX_COLD_START_WS_MS,
            max_cold_boot_ms: BUDGET_MAX_TOTAL_BOOT_MS,
            max_binary_mb: BUDGET_MAX_RELEASE_BINARY_MB,
            runs: DEFAULT_CI_MEDIAN_RUNS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CiBenchmarkSample {
    pub pill_ready_ms: f64,
    pub cold_start_ms: f64,
    pub idle_ram_mb: f64,
    pub orphans: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CiBudgetReport {
    pub passed: bool,
    pub exit_code: i32,
    pub median_runs: usize,
    pub median_pill_ms: f64,
    pub median_cold_ms: f64,
    pub median_ram_mb: f64,
    pub binary_mb: f64,
    pub binary_bytes: u64,
    pub max_orphans: usize,
    pub violations: Vec<String>,
    pub thresholds: CiBudgetThresholds,
}

fn median_f64(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    }
}

/// Evaluates raw run samples and binary byte size against hard Step 5.1 CI budgets.
/// Any breach sets `passed = false` and `exit_code = 1` so CI fails automatically.
pub fn evaluate_samples_against_thresholds(
    thresholds: &CiBudgetThresholds,
    samples: &[CiBenchmarkSample],
    binary_bytes: u64,
) -> CiBudgetReport {
    let pills: Vec<f64> = samples.iter().map(|s| s.pill_ready_ms).collect();
    let colds: Vec<f64> = samples.iter().map(|s| s.cold_start_ms).collect();
    let rams: Vec<f64> = samples.iter().map(|s| s.idle_ram_mb).collect();

    let median_pill_ms = median_f64(&pills);
    let median_cold_ms = median_f64(&colds);
    let median_ram_mb = median_f64(&rams);
    let binary_mb = (binary_bytes as f64) / (1024.0 * 1024.0);
    let max_orphans = samples.iter().map(|s| s.orphans).max().unwrap_or(0);

    let mut violations = Vec::new();
    if samples.is_empty() {
        violations.push("no benchmark samples collected".to_string());
    }
    if median_ram_mb > thresholds.max_idle_ram_mb {
        violations.push(format!(
            "idle RAM median {:.2} MB > {:.2} MB",
            median_ram_mb, thresholds.max_idle_ram_mb
        ));
    }
    if median_pill_ms > thresholds.max_pill_start_ms {
        violations.push(format!(
            "pill startup median {:.2} ms > {:.2} ms",
            median_pill_ms, thresholds.max_pill_start_ms
        ));
    }
    if median_cold_ms > thresholds.max_cold_boot_ms {
        violations.push(format!(
            "cold boot median {:.2} ms > {:.2} ms",
            median_cold_ms, thresholds.max_cold_boot_ms
        ));
    }
    if binary_mb > thresholds.max_binary_mb {
        violations.push(format!(
            "binary size {:.3} MB > {:.2} MB",
            binary_mb, thresholds.max_binary_mb
        ));
    }
    if max_orphans > 0 {
        violations.push(format!("orphan WebView2 children detected: {}", max_orphans));
    }

    let passed = violations.is_empty();
    CiBudgetReport {
        passed,
        exit_code: if passed { 0 } else { 1 },
        median_runs: samples.len(),
        median_pill_ms,
        median_cold_ms,
        median_ram_mb,
        binary_mb,
        binary_bytes,
        max_orphans,
        violations,
        thresholds: thresholds.clone(),
    }
}

/// Runs `thresholds.runs` live boot/shutdown iterations on `MikuUnifiedRuntime` and measures
/// `exe_path` size, returning the median benchmark gate report.
pub fn run_live_ci_budget_gate(
    thresholds: &CiBudgetThresholds,
    exe_path: &Path,
) -> Result<CiBudgetReport, String> {
    let binary_bytes = fs::metadata(exe_path)
        .map_err(|e| format!("failed to stat {}: {}", exe_path.display(), e))?
        .len();
    let run_count = thresholds.runs.max(1);
    let mut samples = Vec::with_capacity(run_count);

    for idx in 0..run_count {
        let dir = env::temp_dir().join(format!("miku_ci_bench_{}_{}", std::process::id(), idx));
        let (boot, shut) = run_unified_smoke_summary(&dir)?;
        let _ = fs::remove_dir_all(&dir);
        samples.push(CiBenchmarkSample {
            pill_ready_ms: boot.pill_ready_ms,
            cold_start_ms: boot.cold_start_ms,
            idle_ram_mb: boot.process_private_mb,
            orphans: shut.live_webview2_child_pids,
        });
    }

    Ok(evaluate_samples_against_thresholds(
        thresholds,
        &samples,
        binary_bytes,
    ))
}

fn parse_flag_value<T: std::str::FromStr>(args: &[String], flag: &str) -> Option<T> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .and_then(|w| w[1].parse::<T>().ok())
}

/// Handles `miku ci-bench [--runs 5] [--max-ram-mb 30] [--max-start-ms 100] [--max-boot-ms 1000] [--max-bin-mb 15] [--bin-path PATH] [--json]`.
pub fn execute_cli_ci_bench_command(
    args: &[String],
) -> Result<Option<(CiBudgetReport, bool)>, String> {
    if !args.iter().any(|a| a == "ci-bench" || a == "bench-gate") {
        return Ok(None);
    }

    let mut thresholds = CiBudgetThresholds::default();
    if let Some(runs) = parse_flag_value::<usize>(args, "--runs") {
        thresholds.runs = runs.max(1);
    }
    if let Some(ram) = parse_flag_value::<f64>(args, "--max-ram-mb") {
        thresholds.max_idle_ram_mb = ram;
    }
    if let Some(pill) = parse_flag_value::<f64>(args, "--max-start-ms") {
        thresholds.max_pill_start_ms = pill;
    }
    if let Some(boot) = parse_flag_value::<f64>(args, "--max-boot-ms") {
        thresholds.max_cold_boot_ms = boot;
    }
    if let Some(bin) = parse_flag_value::<f64>(args, "--max-bin-mb") {
        thresholds.max_binary_mb = bin;
    }

    let bin_path = parse_flag_value::<String>(args, "--bin-path")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_exe().expect("resolve current_exe"));
    let json_mode = args.iter().any(|a| a == "--json");

    let report = run_live_ci_budget_gate(&thresholds, &bin_path)?;
    Ok(Some((report, json_mode)))
}

/// Shared CLI dispatch helper for both `miku` and `miku-cli`.
pub fn maybe_run_ci_bench_and_exit(args: &[String]) {
    match execute_cli_ci_bench_command(args) {
        Ok(Some((report, json_mode))) => {
            if json_mode {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
                );
            } else {
                println!(
                    "MIKU_CI_BENCH_{}|runs={}|pill_ms={:.2}|cold_ms={:.2}|ram_mb={:.2}|bin_mb={:.3}|orphans={}",
                    if report.passed { "PASS" } else { "FAIL" },
                    report.median_runs,
                    report.median_pill_ms,
                    report.median_cold_ms,
                    report.median_ram_mb,
                    report.binary_mb,
                    report.max_orphans
                );
                if !report.violations.is_empty() {
                    eprintln!("VIOLATIONS: {}", report.violations.join("; "));
                }
            }
            std::process::exit(report.exit_code);
        }
        Ok(None) => {}
        Err(e) => {
            eprintln!("MIKU_CI_BENCH_ERR: {}", e);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license_audit::resolve_repo_root;

    #[test]
    fn test_5_1_ci_budget_gate_passes_live_5_run_median_within_hard_budgets() {
        let _wv2_guard = crate::live_ui_host::CLI_LIVE_WV2_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let exe = env::current_exe().expect("current test exe");
        // Use conservative 5-run median check (test exe itself may include debug symbols,
        // so pass a realistic binary cap only if running debug test binary, while release
        // `miku.exe` is validated strictly at < 15.0 MB by release CI).
        let exe_mb = fs::metadata(&exe).unwrap().len() as f64 / (1024.0 * 1024.0);
        let thresholds = CiBudgetThresholds {
            max_binary_mb: exe_mb.max(BUDGET_MAX_RELEASE_BINARY_MB) + 1.0,
            ..CiBudgetThresholds::default()
        };
        let report = run_live_ci_budget_gate(&thresholds, &exe).expect("5-run ci gate");
        assert!(report.passed, "violations: {:?}", report.violations);
        assert_eq!(report.exit_code, 0);
        assert_eq!(report.median_runs, 5);
        assert!(report.median_ram_mb <= BUDGET_MAX_PRIVATE_RAM_MB);
        assert!(report.median_pill_ms <= BUDGET_MAX_COLD_START_WS_MS);
        assert!(report.median_cold_ms <= BUDGET_MAX_TOTAL_BOOT_MS);
    }

    #[test]
    fn test_5_1_ci_budget_gate_fails_with_nonzero_exit_on_simulated_threshold_breach() {
        let thresholds = CiBudgetThresholds::default();
        let bad_samples = vec![
            CiBenchmarkSample {
                pill_ready_ms: 145.0,
                cold_start_ms: 1120.0,
                idle_ram_mb: 34.5,
                orphans: 1,
            },
            CiBenchmarkSample {
                pill_ready_ms: 130.0,
                cold_start_ms: 1050.0,
                idle_ram_mb: 33.2,
                orphans: 0,
            },
            CiBenchmarkSample {
                pill_ready_ms: 120.0,
                cold_start_ms: 1020.0,
                idle_ram_mb: 31.8,
                orphans: 0,
            },
        ];
        // 16.0 MB simulated binary > 15.0 MB
        let rep = evaluate_samples_against_thresholds(&thresholds, &bad_samples, 16 * 1024 * 1024);
        assert!(!rep.passed);
        assert_eq!(rep.exit_code, 1);
        assert_eq!(rep.violations.len(), 5);
    }

    #[test]
    fn test_5_1_github_ci_workflow_enforces_release_build_and_ci_bench_gate() {
        let ci_path = resolve_repo_root().join(".github/workflows/ci.yml");
        let raw = fs::read_to_string(&ci_path).expect("read ci.yml");
        assert!(raw.contains("cargo build --manifest-path miku/Cargo.toml --release --bin miku"));
        assert!(raw.contains("ci-bench --runs 5"));
    }
}
