use std::time::Instant;

use miku_gatekeeper::{
    CanonicalPathJail, Capability, GatekeeperEngine, PolicyEffect, ProjectProfile,
};
use miku_protocol::{sha256_hex, BaseContractBundle};
use serde::{Deserialize, Serialize};

use crate::{
    DiagnosticProbeResult, ProbeStatus, BUDGET_MAX_COLD_START_WS_MS, BUDGET_MAX_IDLE_CPU_PCT,
    BUDGET_MAX_PRIVATE_RAM_MB, BUDGET_MAX_RELEASE_BINARY_MB, BUDGET_MIN_FREE_DISK_MB,
};

// ============================================================================
// Probe 4: .miku.toml Syntax, Gatekeeper Rules & Base Contracts Validator
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MikuTomlConfig {
    pub project: MikuTomlProjectSection,
    pub browser: MikuTomlBrowserSection,
    #[serde(default)]
    pub allow: MikuTomlPolicyLists,
    #[serde(default)]
    pub ask: MikuTomlPolicyLists,
    #[serde(default)]
    pub deny: MikuTomlPolicyLists,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MikuTomlProjectSection {
    pub name: String,
    pub root: String,
    #[serde(default = "default_profile_ttl_ms")]
    pub default_ttl_ms: u64,
}

fn default_profile_ttl_ms() -> u64 {
    30_000
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MikuTomlBrowserSection {
    pub driver: String,
    pub profile_isolation: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MikuTomlPolicyLists {
    #[serde(default)]
    pub fs_read: Vec<String>,
    #[serde(default)]
    pub fs_write: Vec<String>,
    #[serde(default)]
    pub shell: Vec<String>,
    #[serde(default)]
    pub browser_domains: Vec<String>,
    #[serde(default)]
    pub process_kill: Vec<String>,
}

fn validate_fs_pattern_in_jail(jail: &CanonicalPathJail, pattern: &str) -> Result<(), String> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err("filesystem policy pattern cannot be empty".to_string());
    }
    let probe_path = trimmed.replace("**", "glob_dir").replace('*', "glob_leaf");
    jail.resolve_and_confine(&probe_path)
        .map(|_| ())
        .map_err(|e| format!("pattern '{}' violates CanonicalPathJail: {}", pattern, e))
}

pub fn parse_and_validate_miku_toml(
    raw_toml: &str,
) -> Result<(ProjectProfile, String, String), String> {
    let parsed: MikuTomlConfig =
        toml::from_str(raw_toml).map_err(|e| format!(".miku.toml syntax error: {}", e))?;

    if parsed.project.name.trim().is_empty() {
        return Err("project.name in .miku.toml cannot be empty".to_string());
    }
    if parsed.project.default_ttl_ms == 0 || parsed.project.default_ttl_ms > 3_600_000 {
        return Err("project.default_ttl_ms must be in 1..=3600000 ms".to_string());
    }

    let driver_lower = parsed.browser.driver.trim().to_ascii_lowercase();
    if driver_lower != "playwright" && driver_lower != "cdp" {
        return Err(format!(
            "unsupported browser.driver '{}'; expected 'playwright' or 'cdp'",
            parsed.browser.driver
        ));
    }
    if !parsed.browser.profile_isolation {
        return Err(
            "browser.profile_isolation must be true (isolated browser profile required by Gatekeeper)"
                .to_string(),
        );
    }

    let jail = CanonicalPathJail::new(&parsed.project.root)
        .map_err(|e| format!("invalid project.root '{}': {}", parsed.project.root, e))?;

    let mut profile = ProjectProfile::new(
        parsed.project.name.trim(),
        jail.canonical_root(),
        parsed.project.default_ttl_ms,
    );

    let sections = [
        (PolicyEffect::Allow, &parsed.allow),
        (PolicyEffect::Ask, &parsed.ask),
        (PolicyEffect::Deny, &parsed.deny),
    ];

    for (effect, lists) in sections {
        for pat in &lists.fs_read {
            validate_fs_pattern_in_jail(&jail, pat)?;
            profile = profile.with_rule(Capability::FsRead, pat, effect, false);
        }
        for pat in &lists.fs_write {
            validate_fs_pattern_in_jail(&jail, pat)?;
            profile = profile.with_rule(Capability::FsWrite, pat, effect, false);
        }
        for pat in &lists.shell {
            if pat.trim().is_empty() {
                return Err("shell policy pattern cannot be empty".to_string());
            }
            profile = profile.with_rule(Capability::Shell, pat, effect, true);
        }
        for pat in &lists.browser_domains {
            if pat.trim().is_empty() {
                return Err("browser_domains policy pattern cannot be empty".to_string());
            }
            profile = profile.with_rule(Capability::Browser, pat, effect, false);
        }
        for pat in &lists.process_kill {
            if pat.trim().is_empty() {
                return Err("process_kill policy pattern cannot be empty".to_string());
            }
            profile = profile.with_rule(Capability::Shell, pat, effect, true);
        }
    }

    let _gk = GatekeeperEngine::new(profile.clone())
        .map_err(|e| format!("GatekeeperEngine init failed: {}", e))?;
    let contracts = BaseContractBundle::load_embedded_defaults()
        .map_err(|e| format!("BaseContractBundle validation failed: {}", e))?;

    let toml_sha256 = sha256_hex(raw_toml.trim().as_bytes());
    Ok((profile, toml_sha256, contracts.contract_sha256))
}

pub fn probe_miku_toml_and_contracts(raw_toml: &str) -> DiagnosticProbeResult {
    let t0 = Instant::now();
    let outcome = parse_and_validate_miku_toml(raw_toml);
    let latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;

    match outcome {
        Ok((profile, toml_sha256, contract_sha256)) => DiagnosticProbeResult {
            probe_id: "miku_toml_contracts".to_string(),
            title: "Project .miku.toml & Base Contracts (Gatekeeper)".to_string(),
            status: ProbeStatus::Pass,
            latency_ms,
            summary: format!(
                "Profile '{}' valid ({} rules, jail='{}', toml_sha256={}.., contract_sha256={}..)",
                profile.project_name,
                profile.rules.len(),
                profile.project_root,
                &toml_sha256[..12],
                &contract_sha256[..12]
            ),
            remediation: None,
        },
        Err(err_msg) => DiagnosticProbeResult {
            probe_id: "miku_toml_contracts".to_string(),
            title: "Project .miku.toml & Base Contracts (Gatekeeper)".to_string(),
            status: ProbeStatus::Fail,
            latency_ms,
            summary: format!("Configuration validation failed: {}", err_msg),
            remediation: Some(
                "Fix .miku.toml syntax, ensure browser.profile_isolation=true, and confine all fs_read/fs_write globs inside project.root.".to_string(),
            ),
        },
    }
}

// ============================================================================
// Probe 5: Performance Budgets Auditor (BUD-01, BUD-02, BUD-07, BUD-09, DISK)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetTelemetryInput {
    pub worker_private_ram_mb: f64,
    pub release_binary_mb: f64,
    pub cold_start_ws_ms: f64,
    pub idle_cpu_pct: f64,
    pub free_disk_mb: f64,
}

impl Default for BudgetTelemetryInput {
    fn default() -> Self {
        Self {
            worker_private_ram_mb: 0.87,
            release_binary_mb: 0.92,
            cold_start_ws_ms: 3.12,
            idle_cpu_pct: 0.0,
            free_disk_mb: 25_600.0,
        }
    }
}

pub fn probe_performance_budgets(
    telemetry: &BudgetTelemetryInput,
) -> (DiagnosticProbeResult, Vec<String>) {
    let t0 = Instant::now();
    let mut violations = Vec::new();

    if !telemetry.worker_private_ram_mb.is_finite()
        || telemetry.worker_private_ram_mb <= 0.0
        || telemetry.worker_private_ram_mb > BUDGET_MAX_PRIVATE_RAM_MB
    {
        violations.push(format!(
            "BUD-01 RAM {:.2} MB > {:.1} MB",
            telemetry.worker_private_ram_mb, BUDGET_MAX_PRIVATE_RAM_MB
        ));
    }

    if !telemetry.release_binary_mb.is_finite()
        || telemetry.release_binary_mb <= 0.0
        || telemetry.release_binary_mb >= BUDGET_MAX_RELEASE_BINARY_MB
    {
        violations.push(format!(
            "BUD-09 Binary {:.2} MB >= {:.1} MB",
            telemetry.release_binary_mb, BUDGET_MAX_RELEASE_BINARY_MB
        ));
    }

    if !telemetry.cold_start_ws_ms.is_finite()
        || telemetry.cold_start_ws_ms < 0.0
        || telemetry.cold_start_ws_ms >= BUDGET_MAX_COLD_START_WS_MS
    {
        violations.push(format!(
            "BUD-07 Cold-Start->WS {:.2} ms >= {:.1} ms",
            telemetry.cold_start_ws_ms, BUDGET_MAX_COLD_START_WS_MS
        ));
    }

    if !telemetry.idle_cpu_pct.is_finite()
        || telemetry.idle_cpu_pct < 0.0
        || telemetry.idle_cpu_pct > 0.001
    {
        violations.push(format!(
            "BUD-02 Idle CPU {:.3}% != {:.1}%",
            telemetry.idle_cpu_pct, BUDGET_MAX_IDLE_CPU_PCT
        ));
    }

    if !telemetry.free_disk_mb.is_finite() || telemetry.free_disk_mb < BUDGET_MIN_FREE_DISK_MB {
        violations.push(format!(
            "DISK Free {:.1} MB < {:.1} MB",
            telemetry.free_disk_mb, BUDGET_MIN_FREE_DISK_MB
        ));
    }

    let latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
    if violations.is_empty() {
        let res = DiagnosticProbeResult {
            probe_id: "performance_budgets".to_string(),
            title: "Hard Performance Budgets (BUD-01/02/07/09)".to_string(),
            status: ProbeStatus::Pass,
            latency_ms,
            summary: format!(
                "All 5 budgets verified: RAM={:.2}/{:.0}MB, Bin={:.2}/{:.0}MB, WS={:.2}/{:.0}ms, CPU={:.1}%, Disk={:.0}MB",
                telemetry.worker_private_ram_mb,
                BUDGET_MAX_PRIVATE_RAM_MB,
                telemetry.release_binary_mb,
                BUDGET_MAX_RELEASE_BINARY_MB,
                telemetry.cold_start_ws_ms,
                BUDGET_MAX_COLD_START_WS_MS,
                telemetry.idle_cpu_pct,
                telemetry.free_disk_mb
            ),
            remediation: None,
        };
        (res, violations)
    } else {
        let joined = violations.join("; ");
        let res = DiagnosticProbeResult {
            probe_id: "performance_budgets".to_string(),
            title: "Hard Performance Budgets (BUD-01/02/07/09)".to_string(),
            status: ProbeStatus::Fail,
            latency_ms,
            summary: format!("Budget violation(s) detected: {}", joined),
            remediation: Some(
                "Inspect worker memory allocations, ensure release profile uses strip=true & lto='thin', and verify WebView2 lazy eviction.".to_string(),
            ),
        };
        (res, violations)
    }
}
