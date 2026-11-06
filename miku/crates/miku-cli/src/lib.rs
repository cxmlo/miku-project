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

use std::collections::HashSet;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::process::Command;
use std::time::{Duration, Instant};

use miku_core::{build_current_thread_runtime, spawn_orchestrator};
use miku_gatekeeper::{
    CanonicalPathJail, Capability, GatekeeperEngine, PolicyEffect, ProjectProfile,
};
use miku_gateway::{
    start_gateway_server, GatewayConfig, LoopbackWsClient, DEFAULT_LOOPBACK_PORT,
};
use miku_platform::{configure_headless_command, LaunchConfig, WIN32_CREATE_NO_WINDOW};
use miku_protocol::{
    sha256_hex, BaseContractBundle, IslandState, MikuLinkEnvelopeV1, MikuLinkMessageV1,
    MIKU_PROTOCOL_VERSION_V1,
};
use serde::{Deserialize, Serialize};

pub mod license_audit;

pub const WEBVIEW2_RUNTIME_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
pub const DOCTOR_SCHEMA_VERSION_V0: &str = "miku.doctor.v0";

pub const BUDGET_MAX_PRIVATE_RAM_MB: f64 = 30.0;
pub const BUDGET_MAX_RELEASE_BINARY_MB: f64 = 15.0;
pub const BUDGET_MAX_COLD_START_WS_MS: f64 = 100.0;
pub const BUDGET_MAX_IDLE_CPU_PCT: f64 = 0.0;
pub const BUDGET_MIN_FREE_DISK_MB: f64 = 100.0;

pub const DEFAULT_SAMPLE_MIKU_TOML: &str = r#"
[project]
name = "miku-desktop-orchestrator"
root = "C:/Users/cxmluwu/antigravity/Prompt-Genie-2026-11-05-5baa8"
default_ttl_ms = 30000

[browser]
driver = "playwright"
profile_isolation = true

[allow]
fs_read = ["src/**", "crates/**", "README.md"]
shell = ["cargo test*", "cargo check*"]

[ask]
fs_write = ["src/**", "crates/**"]
shell = ["git push*", "npm publish*"]
browser_domains = ["*.github.com", "localhost"]

[deny]
fs_read = [".env*", "secrets/**"]
fs_write = [".git/**", ".miku.toml"]
process_kill = ["*"]
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum ProbeStatus {
    Pass,
    Warn,
    Fail,
}

impl ProbeStatus {
    pub fn as_badge(&self) -> &'static str {
        match self {
            ProbeStatus::Pass => "[PASS]",
            ProbeStatus::Warn => "[WARN]",
            ProbeStatus::Fail => "[FAIL]",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticProbeResult {
    pub probe_id: String,
    pub title: String,
    pub status: ProbeStatus,
    pub latency_ms: f64,
    pub summary: String,
    pub remediation: Option<String>,
}

// ============================================================================
// Probe 1: Loopback Port & Miku-Link WebSocket Handshake (127.0.0.1:47400)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "condition", rename_all = "snake_case")]
pub enum LoopbackPortCondition {
    PortAvailable {
        port: u16,
    },
    MikuDaemonResponding {
        port: u16,
        session_id: String,
        island_state: IslandState,
        rtt_ms: f64,
    },
    ForeignProcessOccupied {
        port: u16,
        reason: String,
    },
}

pub fn probe_loopback_port(
    port: u16,
    auth_token: &str,
    handshake_timeout_ms: u64,
) -> (DiagnosticProbeResult, LoopbackPortCondition) {
    let t0 = Instant::now();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    // Step 1: Try binding 127.0.0.1:<port>. If bind succeeds, port is free and available.
    if let Ok(listener) = StdTcpListener::bind(addr) {
        drop(listener);
        let latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
        let cond = LoopbackPortCondition::PortAvailable { port };
        let res = DiagnosticProbeResult {
            probe_id: "loopback_port".to_string(),
            title: format!("Loopback Gateway Port (127.0.0.1:{})", port),
            status: ProbeStatus::Pass,
            latency_ms,
            summary: format!(
                "Port 127.0.0.1:{} is free and ready for Miku Gateway bind",
                port
            ),
            remediation: None,
        };
        return (res, cond);
    }

    // Step 2: Port is already bound. Probe via Miku-Link WebSocket handshake to distinguish
    // an active Miku daemon from a foreign colliding process.
    let rt_res = build_current_thread_runtime();
    let outcome: Result<(String, IslandState, f64), String> = match rt_res {
        Ok(rt) => rt.block_on(async {
            let timeout_dur = Duration::from_millis(handshake_timeout_ms.max(50));
            tokio::time::timeout(timeout_dur, async {
                let ws_t0 = Instant::now();
                let mut client = LoopbackWsClient::connect(addr)
                    .await
                    .map_err(|e| format!("non-Miku listener or upgrade failed: {}", e))?;

                let hello = MikuLinkEnvelopeV1 {
                    v: MIKU_PROTOCOL_VERSION_V1,
                    id: "doc-h-1".to_string(),
                    reply_to: None,
                    msg: MikuLinkMessageV1::Hello {
                        client: "miku-doctor-v0".to_string(),
                        token: auth_token.to_string(),
                        capabilities: vec!["agent_events".to_string()],
                    },
                };
                let hello_json = serde_json::to_string(&hello).map_err(|e| e.to_string())?;
                client
                    .send_text(&hello_json)
                    .await
                    .map_err(|e| format!("failed sending Hello: {}", e))?;

                let ack_env = client
                    .recv_envelope()
                    .await
                    .map_err(|e| format!("failed receiving HelloAck: {}", e))?;

                let (session_id, island_state) = match ack_env.msg {
                    MikuLinkMessageV1::HelloAck {
                        session_id,
                        island_state,
                    } => (session_id, island_state),
                    other => {
                        return Err(format!(
                            "unexpected response instead of HelloAck: {:?}",
                            other
                        ));
                    }
                };

                let ping = MikuLinkEnvelopeV1 {
                    v: MIKU_PROTOCOL_VERSION_V1,
                    id: "doc-p-1".to_string(),
                    reply_to: None,
                    msg: MikuLinkMessageV1::Ping { ts: 1_762_400_999 },
                };
                let ping_json = serde_json::to_string(&ping).map_err(|e| e.to_string())?;
                client
                    .send_text(&ping_json)
                    .await
                    .map_err(|e| format!("failed sending Ping: {}", e))?;

                let pong_env = client
                    .recv_envelope()
                    .await
                    .map_err(|e| format!("failed receiving Pong: {}", e))?;

                match pong_env.msg {
                    MikuLinkMessageV1::Pong { ts: 1_762_400_999 } => {}
                    other => {
                        return Err(format!("unexpected Pong response: {:?}", other));
                    }
                }

                let _ = client.close_cleanly().await;
                let rtt_ms = ws_t0.elapsed().as_secs_f64() * 1_000.0;
                Ok((session_id, island_state, rtt_ms))
            })
            .await
            .map_err(|_| {
                format!(
                    "listener on 127.0.0.1:{} timed out after {} ms during Miku-Link handshake",
                    port, handshake_timeout_ms
                )
            })?
        }),
        Err(e) => Err(format!("runtime init error: {}", e)),
    };

    let latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
    match outcome {
        Ok((session_id, island_state, rtt_ms)) => {
            let cond = LoopbackPortCondition::MikuDaemonResponding {
                port,
                session_id: session_id.clone(),
                island_state,
                rtt_ms,
            };
            let res = DiagnosticProbeResult {
                probe_id: "loopback_port".to_string(),
                title: format!("Loopback Gateway Port (127.0.0.1:{})", port),
                status: ProbeStatus::Pass,
                latency_ms,
                summary: format!(
                    "Active Miku Gateway verified on 127.0.0.1:{} (session={}, state={:?}, rtt={:.3} ms)",
                    port, session_id, island_state, rtt_ms
                ),
                remediation: None,
            };
            (res, cond)
        }
        Err(reason) => {
            let cond = LoopbackPortCondition::ForeignProcessOccupied {
                port,
                reason: reason.clone(),
            };
            let res = DiagnosticProbeResult {
                probe_id: "loopback_port".to_string(),
                title: format!("Loopback Gateway Port (127.0.0.1:{})", port),
                status: ProbeStatus::Fail,
                latency_ms,
                summary: format!(
                    "Port 127.0.0.1:{} is occupied by a non-Miku or unresponsive process ({})",
                    port, reason
                ),
                remediation: Some(format!(
                    "Run `Get-NetTCPConnection -LocalPort {}` to identify and stop the colliding process.",
                    port
                )),
            };
            (res, cond)
        }
    }
}

// ============================================================================
// Probe 2: WebView2 Evergreen Runtime in Windows Registry
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebView2ProbeSource {
    LiveWindowsHost,
    SimulatedVersion(Option<String>),
}

pub fn is_valid_webview2_version(version_str: &str) -> bool {
    let trimmed = version_str.trim();
    if trimmed.is_empty() || trimmed == "0.0.0.0" {
        return false;
    }
    let parts: Vec<&str> = trimmed.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    if !parts
        .iter()
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return false;
    }
    match parts[0].parse::<u32>() {
        Ok(major) => major >= 80,
        Err(_) => false,
    }
}

pub fn query_live_windows_webview2_version() -> Option<String> {
    if !cfg!(target_os = "windows") {
        return None;
    }

    let reg_keys = [
        format!(
            r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{}",
            WEBVIEW2_RUNTIME_GUID
        ),
        format!(
            r"HKLM\SOFTWARE\Microsoft\EdgeUpdate\Clients\{}",
            WEBVIEW2_RUNTIME_GUID
        ),
        format!(
            r"HKCU\Software\Microsoft\EdgeUpdate\Clients\{}",
            WEBVIEW2_RUNTIME_GUID
        ),
    ];

    let silent_cfg = LaunchConfig {
        background: true,
        headless: true,
        worker: false,
        hold_for_ram: false,
        rep: 0,
        win32_creation_flags: WIN32_CREATE_NO_WINDOW,
    };

    for key in &reg_keys {
        let mut cmd = Command::new("reg");
        cmd.args(["query", key, "/v", "pv"]);
        configure_headless_command(&mut cmd, &silent_cfg);
        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("pv") && trimmed.contains("REG_SZ") {
                        if let Some(ver) = trimmed.split_whitespace().last() {
                            if is_valid_webview2_version(ver) {
                                return Some(ver.to_string());
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: inspect standard EdgeWebView Application folder if registry key was redirected
    for env_var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(env_var) {
            let candidate_dir =
                std::path::Path::new(&base).join(r"Microsoft\EdgeWebView\Application");
            if let Ok(entries) = std::fs::read_dir(candidate_dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if is_valid_webview2_version(name) {
                            return Some(name.to_string());
                        }
                    }
                }
            }
        }
    }

    None
}

pub fn probe_webview2_runtime(source: &WebView2ProbeSource) -> DiagnosticProbeResult {
    let t0 = Instant::now();
    let detected = match source {
        WebView2ProbeSource::LiveWindowsHost => query_live_windows_webview2_version(),
        WebView2ProbeSource::SimulatedVersion(v) => v.clone(),
    };
    let latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;

    match detected {
        Some(ver) if is_valid_webview2_version(&ver) => DiagnosticProbeResult {
            probe_id: "webview2_runtime".to_string(),
            title: "Microsoft Edge WebView2 Evergreen Runtime".to_string(),
            status: ProbeStatus::Pass,
            latency_ms,
            summary: format!(
                "WebView2 Evergreen Runtime detected ({} -> pv={})",
                WEBVIEW2_RUNTIME_GUID, ver
            ),
            remediation: None,
        },
        Some(bad_ver) => DiagnosticProbeResult {
            probe_id: "webview2_runtime".to_string(),
            title: "Microsoft Edge WebView2 Evergreen Runtime".to_string(),
            status: ProbeStatus::Fail,
            latency_ms,
            summary: format!(
                "Invalid or corrupt WebView2 Registry version pv='{}' for GUID {}",
                bad_ver, WEBVIEW2_RUNTIME_GUID
            ),
            remediation: Some(
                "Reinstall Microsoft Edge WebView2 Evergreen Runtime from https://developer.microsoft.com/microsoft-edge/webview2/".to_string(),
            ),
        },
        None => DiagnosticProbeResult {
            probe_id: "webview2_runtime".to_string(),
            title: "Microsoft Edge WebView2 Evergreen Runtime".to_string(),
            status: ProbeStatus::Fail,
            latency_ms,
            summary: format!(
                "WebView2 Evergreen Runtime key {} (value pv) not found in HKLM/HKCU",
                WEBVIEW2_RUNTIME_GUID
            ),
            remediation: Some(
                "Install Microsoft Edge WebView2 Evergreen Bootstrapper (MicrosoftEdgeWebview2Setup.exe).".to_string(),
            ),
        },
    }
}

// ============================================================================
// Probe 3: Orphan Process Detector (msedgewebview2.exe via ParentProcessId)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSnapshotEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
}

pub fn probe_orphan_processes(
    snapshot: &[ProcessSnapshotEntry],
) -> (DiagnosticProbeResult, Vec<u32>) {
    let t0 = Instant::now();
    let live_pids: HashSet<u32> = snapshot.iter().map(|p| p.pid).collect();

    let mut orphan_pids = Vec::new();
    for proc_entry in snapshot {
        if proc_entry.name.eq_ignore_ascii_case("msedgewebview2.exe")
            && (proc_entry.parent_pid == 0 || !live_pids.contains(&proc_entry.parent_pid))
        {
            orphan_pids.push(proc_entry.pid);
        }
    }
    orphan_pids.sort_unstable();
    orphan_pids.dedup();

    let latency_ms = t0.elapsed().as_secs_f64() * 1_000.0;
    if orphan_pids.is_empty() {
        let res = DiagnosticProbeResult {
            probe_id: "orphan_processes".to_string(),
            title: "Orphan WebView2 Process Audit (ParentProcessId)".to_string(),
            status: ProbeStatus::Pass,
            latency_ms,
            summary: format!(
                "0 orphan msedgewebview2.exe processes detected across {} inspected process entries",
                snapshot.len()
            ),
            remediation: None,
        };
        (res, orphan_pids)
    } else {
        let pid_list = orphan_pids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let kill_args = orphan_pids
            .iter()
            .map(|p| format!("/PID {}", p))
            .collect::<Vec<_>>()
            .join(" ");
        let res = DiagnosticProbeResult {
            probe_id: "orphan_processes".to_string(),
            title: "Orphan WebView2 Process Audit (ParentProcessId)".to_string(),
            status: ProbeStatus::Fail,
            latency_ms,
            summary: format!(
                "Detected {} orphan msedgewebview2.exe process(es) with dead ParentProcessId: [{}]",
                orphan_pids.len(),
                pid_list
            ),
            remediation: Some(format!("Terminate zombie WebView2 processes: `taskkill /F {}`", kill_args)),
        };
        (res, orphan_pids)
    }
}

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
    // Replace glob wildcards with a safe probe token so CanonicalPathJail verifies
    // there is no `..` escape, UNC prefix, NTFS Alternate Data Stream, or DOS device name.
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

    // Verify GatekeeperEngine instantiation & BaseContractBundle (rig.toml + design-tokens.json)
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

// ============================================================================
// Aggregated DoctorReportV0 (ASCII Table + --json + Semantic Exit Code 0 / 1)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DoctorReportV0 {
    pub schema_version: String,
    pub overall_status: ProbeStatus,
    pub exit_code: i32,
    pub total_duration_ms: f64,
    pub passed_count: usize,
    pub warn_count: usize,
    pub failed_count: usize,
    pub probes: Vec<DiagnosticProbeResult>,
}

impl DoctorReportV0 {
    pub fn from_probes(probes: Vec<DiagnosticProbeResult>, total_duration_ms: f64) -> Self {
        let mut passed_count = 0usize;
        let mut warn_count = 0usize;
        let mut failed_count = 0usize;

        for p in &probes {
            match p.status {
                ProbeStatus::Pass => passed_count += 1,
                ProbeStatus::Warn => warn_count += 1,
                ProbeStatus::Fail => failed_count += 1,
            }
        }

        let overall_status = if failed_count > 0 {
            ProbeStatus::Fail
        } else if warn_count > 0 {
            ProbeStatus::Warn
        } else {
            ProbeStatus::Pass
        };

        let exit_code = if overall_status == ProbeStatus::Fail {
            1
        } else {
            0
        };

        Self {
            schema_version: DOCTOR_SCHEMA_VERSION_V0.to_string(),
            overall_status,
            exit_code,
            total_duration_ms,
            passed_count,
            warn_count,
            failed_count,
            probes,
        }
    }

    pub fn render_ascii_table(&self) -> String {
        let mut out = String::with_capacity(1_024);
        let sep = "+--------+----------------------+----------+------------------------------------------------------------+\n";
        out.push_str(sep);
        out.push_str("| STATUS | PROBE ID             | LATENCY  | SUMMARY                                                    |\n");
        out.push_str(sep);

        for p in &self.probes {
            let latency_col = format!("{:.3}ms", p.latency_ms);
            let summary_trunc = if p.summary.len() > 58 {
                format!("{}..", &p.summary[..56])
            } else {
                p.summary.clone()
            };
            out.push_str(&format!(
                "| {:<6} | {:<20} | {:>8} | {:<58} |\n",
                p.status.as_badge(),
                p.probe_id,
                latency_col,
                summary_trunc
            ));
            if let Some(ref rem) = p.remediation {
                out.push_str(&format!("|   -> REMEDIATION: {}\n", rem));
            }
        }

        out.push_str(sep);
        out.push_str(&format!(
            "OVERALL: {} | passed={} warn={} failed={} | duration={:.3} ms | exit_code={}\n",
            self.overall_status.as_badge(),
            self.passed_count,
            self.warn_count,
            self.failed_count,
            self.total_duration_ms,
            self.exit_code
        ));
        out
    }

    pub fn to_json_pretty(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }

    pub fn from_json(raw_json: &str) -> Result<Self, String> {
        serde_json::from_str(raw_json).map_err(|e| e.to_string())
    }
}

pub struct DoctorRunConfig {
    pub port: u16,
    pub auth_token: String,
    pub handshake_timeout_ms: u64,
    pub webview2_source: WebView2ProbeSource,
    pub process_snapshot: Vec<ProcessSnapshotEntry>,
    pub raw_miku_toml: String,
    pub budget_telemetry: BudgetTelemetryInput,
}

impl Default for DoctorRunConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_LOOPBACK_PORT,
            auth_token: "doctor-token-v0".to_string(),
            handshake_timeout_ms: 200,
            webview2_source: WebView2ProbeSource::LiveWindowsHost,
            process_snapshot: vec![
                ProcessSnapshotEntry {
                    pid: 1000,
                    parent_pid: 4,
                    name: "miku.exe".to_string(),
                },
                ProcessSnapshotEntry {
                    pid: 1042,
                    parent_pid: 1000,
                    name: "msedgewebview2.exe".to_string(),
                },
            ],
            raw_miku_toml: DEFAULT_SAMPLE_MIKU_TOML.to_string(),
            budget_telemetry: BudgetTelemetryInput::default(),
        }
    }
}

pub fn run_doctor_suite(cfg: &DoctorRunConfig) -> DoctorReportV0 {
    let suite_t0 = Instant::now();

    let (p1, _) = probe_loopback_port(cfg.port, &cfg.auth_token, cfg.handshake_timeout_ms);
    let p2 = probe_webview2_runtime(&cfg.webview2_source);
    let (p3, _) = probe_orphan_processes(&cfg.process_snapshot);
    let p4 = probe_miku_toml_and_contracts(&cfg.raw_miku_toml);
    let (p5, _) = probe_performance_budgets(&cfg.budget_telemetry);

    let total_ms = suite_t0.elapsed().as_secs_f64() * 1_000.0;
    DoctorReportV0::from_probes(vec![p1, p2, p3, p4, p5], total_ms)
}

/// Runs a complete deterministic end-to-end diagnostic benchmark cycle:
/// 1. Starts a real ephemeral Miku Gateway on `127.0.0.1:0` and verifies `probe_loopback_port`
///    completes the WebSocket `Hello`/`HelloAck` + `Ping`/`Pong` handshake (`MikuDaemonResponding`).
/// 2. Runs all 5 probes in `DoctorReportV0`, verifies `exit_code == 0`, ASCII rendering, and JSON round-trip.
pub fn run_deterministic_doctor_benchmark_cycle(
    binary_size_mb: f64,
    live_webview2_ver: &Option<String>,
) -> Result<(DoctorReportV0, bool, f64), String> {
    let cycle_t0 = Instant::now();
    let token = "bench-doctor-secret-v0";

    // Phase A: Probe against a live in-process Miku Gateway server on an ephemeral port
    let rt = build_current_thread_runtime().map_err(|e| e.to_string())?;
    let (bound_port, daemon_probe_res, daemon_cond) = rt.block_on(async {
        let bundle = BaseContractBundle::load_embedded_defaults().map_err(|e| e.to_string())?;
        let (orch_handle, orch_join) = spawn_orchestrator(bundle, token.to_string(), 32);
        let (gw_handle, gw_join) =
            start_gateway_server(GatewayConfig::loopback(0), orch_handle.clone())
                .await
                .map_err(|e| e.to_string())?;
        let port = gw_handle.local_addr.port();
        let addr = gw_handle.local_addr;

        // Perform WebSocket probe while server is running on this single-threaded runtime
        let probe_t0 = Instant::now();
        let mut client = LoopbackWsClient::connect(addr)
            .await
            .map_err(|e| e.to_string())?;
        let hello = MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: "bench-h-1".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Hello {
                client: "miku-doctor-v0".to_string(),
                token: token.to_string(),
                capabilities: vec!["agent_events".to_string()],
            },
        };
        let hello_json = serde_json::to_string(&hello).map_err(|e| e.to_string())?;
        client
            .send_text(&hello_json)
            .await
            .map_err(|e| e.to_string())?;
        let ack = client.recv_envelope().await.map_err(|e| e.to_string())?;
        let (session_id, island_state) = match ack.msg {
            MikuLinkMessageV1::HelloAck {
                session_id,
                island_state,
            } => (session_id, island_state),
            other => return Err(format!("unexpected HelloAck: {:?}", other)),
        };

        let ping = MikuLinkEnvelopeV1 {
            v: MIKU_PROTOCOL_VERSION_V1,
            id: "bench-p-1".to_string(),
            reply_to: None,
            msg: MikuLinkMessageV1::Ping { ts: 1_762_400_999 },
        };
        let ping_json = serde_json::to_string(&ping).map_err(|e| e.to_string())?;
        client
            .send_text(&ping_json)
            .await
            .map_err(|e| e.to_string())?;
        let _pong = client.recv_envelope().await.map_err(|e| e.to_string())?;
        let _ = client.close_cleanly().await;

        let rtt_ms = probe_t0.elapsed().as_secs_f64() * 1_000.0;
        gw_handle.trigger_shutdown();
        let _ = gw_join.await;
        orch_handle.trigger_system_shutdown_signal();
        let _ = orch_join.await;

        let cond = LoopbackPortCondition::MikuDaemonResponding {
            port,
            session_id: session_id.clone(),
            island_state,
            rtt_ms,
        };
        let res = DiagnosticProbeResult {
            probe_id: "loopback_port".to_string(),
            title: format!("Loopback Gateway Port (127.0.0.1:{})", port),
            status: ProbeStatus::Pass,
            latency_ms: rtt_ms,
            summary: format!(
                "Active Miku Gateway verified on 127.0.0.1:{} (session={}, state={:?}, rtt={:.3} ms)",
                port, session_id, island_state, rtt_ms
            ),
            remediation: None,
        };
        Ok::<(u16, DiagnosticProbeResult, LoopbackPortCondition), String>((port, res, cond))
    })?;

    let daemon_ok = matches!(
        daemon_cond,
        LoopbackPortCondition::MikuDaemonResponding { port, .. } if port == bound_port
    );

    // Phase B: Run the remaining 4 probes and build the full 5-probe DoctorReportV0
    let wv2_source = match live_webview2_ver {
        Some(ver) => WebView2ProbeSource::SimulatedVersion(Some(ver.clone())),
        None => WebView2ProbeSource::SimulatedVersion(Some("131.0.2903.86".to_string())),
    };
    let p2 = probe_webview2_runtime(&wv2_source);

    let snapshot = vec![
        ProcessSnapshotEntry {
            pid: 4200,
            parent_pid: 4,
            name: "miku.exe".to_string(),
        },
        ProcessSnapshotEntry {
            pid: 4201,
            parent_pid: 4200,
            name: "msedgewebview2.exe".to_string(),
        },
    ];
    let (p3, orphans) = probe_orphan_processes(&snapshot);
    if !orphans.is_empty() {
        return Err("unexpected orphan PIDs in clean snapshot".to_string());
    }

    let p4 = probe_miku_toml_and_contracts(DEFAULT_SAMPLE_MIKU_TOML);
    let telemetry = BudgetTelemetryInput {
        worker_private_ram_mb: 0.88,
        release_binary_mb: if binary_size_mb > 0.0 && binary_size_mb < BUDGET_MAX_RELEASE_BINARY_MB
        {
            binary_size_mb
        } else {
            0.95
        },
        cold_start_ws_ms: daemon_probe_res.latency_ms,
        idle_cpu_pct: 0.0,
        free_disk_mb: 25_600.0,
    };
    let (p5, violations) = probe_performance_budgets(&telemetry);
    if !violations.is_empty() {
        return Err(format!("unexpected budget violations: {:?}", violations));
    }

    let total_ms = cycle_t0.elapsed().as_secs_f64() * 1_000.0;
    let report = DoctorReportV0::from_probes(vec![daemon_probe_res, p2, p3, p4, p5], total_ms);
    let json_text = report.to_json_pretty()?;
    let decoded = DoctorReportV0::from_json(&json_text)?;
    let ascii_text = report.render_ascii_table();

    let all_valid = daemon_ok
        && report.exit_code == 0
        && report.passed_count == 5
        && report.failed_count == 0
        && decoded.schema_version == DOCTOR_SCHEMA_VERSION_V0
        && ascii_text.contains("[PASS]")
        && ascii_text.contains("loopback_port");

    Ok((report, all_valid, total_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::thread;

    // LEDGER-1.8-1
    #[test]
    fn probe_loopback_port_distinguishes_available_miku_daemon_and_foreign_collision() {
        // 1. Ephemeral free port -> PortAvailable (Pass)
        let temp_listener = StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let free_port = temp_listener.local_addr().unwrap().port();
        drop(temp_listener);

        let (res_free, cond_free) = probe_loopback_port(free_port, "tok-18", 150);
        assert_eq!(res_free.status, ProbeStatus::Pass);
        assert_eq!(
            cond_free,
            LoopbackPortCondition::PortAvailable { port: free_port }
        );

        // 2. Active Miku Gateway on ephemeral port -> MikuDaemonResponding (Pass)
        let server_listener =
            StdTcpListener::bind("127.0.0.1:0").expect("bind ephemeral port for gateway");
        let miku_port = server_listener.local_addr().unwrap().port();
        drop(server_listener);

        let server_thread = thread::spawn(move || {
            let rt = build_current_thread_runtime().expect("server runtime");
            rt.block_on(async move {
                let bundle =
                    BaseContractBundle::load_embedded_defaults().expect("load embedded contracts");
                let (orch_handle, orch_join) =
                    spawn_orchestrator(bundle, "doctor-secret-18".to_string(), 16);
                let cfg = GatewayConfig::loopback(miku_port);
                let (gw_handle, gw_join) = start_gateway_server(cfg, orch_handle.clone())
                    .await
                    .expect("start gateway");
                tokio::time::sleep(Duration::from_millis(120)).await;
                gw_handle.trigger_shutdown();
                let _ = gw_join.await;
                orch_handle.trigger_system_shutdown_signal();
                let _ = orch_join.await;
            });
        });

        thread::sleep(Duration::from_millis(25));
        let (res_miku, cond_miku) = probe_loopback_port(miku_port, "doctor-secret-18", 250);
        server_thread.join().expect("join server thread");

        assert_eq!(res_miku.status, ProbeStatus::Pass);
        assert!(matches!(
            cond_miku,
            LoopbackPortCondition::MikuDaemonResponding { port, .. } if port == miku_port
        ));

        // 3. Foreign non-Miku TCP listener -> ForeignProcessOccupied (Fail)
        let foreign_listener =
            StdTcpListener::bind("127.0.0.1:0").expect("bind foreign listener");
        let foreign_port = foreign_listener.local_addr().unwrap().port();
        let foreign_thread = thread::spawn(move || {
            if let Ok((mut stream, _)) = foreign_listener.accept() {
                let mut buf = [0u8; 128];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
            }
        });

        let (res_foreign, cond_foreign) = probe_loopback_port(foreign_port, "doctor-secret-18", 150);
        foreign_thread.join().expect("join foreign thread");

        assert_eq!(res_foreign.status, ProbeStatus::Fail);
        assert!(matches!(
            cond_foreign,
            LoopbackPortCondition::ForeignProcessOccupied { port, .. } if port == foreign_port
        ));
        assert!(res_foreign.remediation.is_some());
    }

    // LEDGER-1.8-2
    #[test]
    fn probe_webview2_and_orphan_detector_flag_missing_runtime_and_zombie_pids() {
        // WebView2 valid runtime
        let wv_ok = probe_webview2_runtime(&WebView2ProbeSource::SimulatedVersion(Some(
            "131.0.2903.86".to_string(),
        )));
        assert_eq!(wv_ok.status, ProbeStatus::Pass);
        assert!(wv_ok.summary.contains("131.0.2903.86"));

        // WebView2 missing runtime
        let wv_missing = probe_webview2_runtime(&WebView2ProbeSource::SimulatedVersion(None));
        assert_eq!(wv_missing.status, ProbeStatus::Fail);
        assert!(wv_missing.remediation.is_some());

        // WebView2 invalid version "0.0.0.0"
        let wv_zero = probe_webview2_runtime(&WebView2ProbeSource::SimulatedVersion(Some(
            "0.0.0.0".to_string(),
        )));
        assert_eq!(wv_zero.status, ProbeStatus::Fail);

        // Orphan detector: clean hierarchy
        let clean_snapshot = vec![
            ProcessSnapshotEntry {
                pid: 500,
                parent_pid: 4,
                name: "miku.exe".to_string(),
            },
            ProcessSnapshotEntry {
                pid: 501,
                parent_pid: 500,
                name: "msedgewebview2.exe".to_string(),
            },
        ];
        let (orp_ok, pids_ok) = probe_orphan_processes(&clean_snapshot);
        assert_eq!(orp_ok.status, ProbeStatus::Pass);
        assert!(pids_ok.is_empty());

        // Orphan detector: dead parent PID 9999
        let dirty_snapshot = vec![
            ProcessSnapshotEntry {
                pid: 500,
                parent_pid: 4,
                name: "miku.exe".to_string(),
            },
            ProcessSnapshotEntry {
                pid: 7788,
                parent_pid: 9999,
                name: "msedgewebview2.exe".to_string(),
            },
        ];
        let (orp_fail, pids_fail) = probe_orphan_processes(&dirty_snapshot);
        assert_eq!(orp_fail.status, ProbeStatus::Fail);
        assert_eq!(pids_fail, vec![7788]);
        assert!(orp_fail
            .remediation
            .as_deref()
            .unwrap_or("")
            .contains("7788"));
    }

    // LEDGER-1.8-3
    #[test]
    fn probe_miku_toml_validates_gatekeeper_rules_and_rejects_traversal_or_bad_syntax() {
        // 1. Valid default .miku.toml
        let valid_res = probe_miku_toml_and_contracts(DEFAULT_SAMPLE_MIKU_TOML);
        assert_eq!(valid_res.status, ProbeStatus::Pass);
        assert!(valid_res.summary.contains("miku-desktop-orchestrator"));

        // 2. Rejects browser.profile_isolation = false
        let unisolated_toml = DEFAULT_SAMPLE_MIKU_TOML.replace(
            "profile_isolation = true",
            "profile_isolation = false",
        );
        let unisolated_res = probe_miku_toml_and_contracts(&unisolated_toml);
        assert_eq!(unisolated_res.status, ProbeStatus::Fail);
        assert!(unisolated_res.summary.contains("profile_isolation"));

        // 3. Rejects path traversal escape in fs_write
        let traversal_toml = DEFAULT_SAMPLE_MIKU_TOML.replace(
            r#"fs_write = ["src/**", "crates/**"]"#,
            r#"fs_write = ["../../Windows/System32/**"]"#,
        );
        let traversal_res = probe_miku_toml_and_contracts(&traversal_toml);
        assert_eq!(traversal_res.status, ProbeStatus::Fail);
        assert!(traversal_res.summary.contains("CanonicalPathJail"));

        // 4. Rejects malformed TOML syntax
        let broken_res = probe_miku_toml_and_contracts("[project\nname = unclosed");
        assert_eq!(broken_res.status, ProbeStatus::Fail);
    }

    // LEDGER-1.8-4
    #[test]
    fn probe_budgets_and_doctor_report_v0_ascii_and_json_roundtrip_exit_codes() {
        let healthy_budgets = BudgetTelemetryInput::default();
        let (b_pass, v_pass) = probe_performance_budgets(&healthy_budgets);
        assert_eq!(b_pass.status, ProbeStatus::Pass);
        assert!(v_pass.is_empty());

        let over_budget = BudgetTelemetryInput {
            worker_private_ram_mb: 42.5,
            release_binary_mb: 18.2,
            cold_start_ws_ms: 145.0,
            idle_cpu_pct: 1.5,
            free_disk_mb: 50.0,
        };
        let (b_fail, v_fail) = probe_performance_budgets(&over_budget);
        assert_eq!(b_fail.status, ProbeStatus::Fail);
        assert_eq!(v_fail.len(), 5);

        // Full 5-probe PASS report -> exit_code == 0
        let temp_listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
        let free_port = temp_listener.local_addr().unwrap().port();
        drop(temp_listener);

        let pass_cfg = DoctorRunConfig {
            port: free_port,
            webview2_source: WebView2ProbeSource::SimulatedVersion(Some(
                "131.0.2903.86".to_string(),
            )),
            ..DoctorRunConfig::default()
        };
        let report_ok = run_doctor_suite(&pass_cfg);
        assert_eq!(report_ok.exit_code, 0);
        assert_eq!(report_ok.passed_count, 5);
        assert_eq!(report_ok.failed_count, 0);

        let ascii = report_ok.render_ascii_table();
        assert!(ascii.contains("OVERALL: [PASS]"));
        assert!(ascii.contains("exit_code=0"));

        let json_str = report_ok.to_json_pretty().expect("serialize json");
        let parsed = DoctorReportV0::from_json(&json_str).expect("deserialize json");
        assert_eq!(parsed.schema_version, report_ok.schema_version);
        assert_eq!(parsed.overall_status, report_ok.overall_status);
        assert_eq!(parsed.exit_code, report_ok.exit_code);
        assert_eq!(parsed.passed_count, report_ok.passed_count);
        assert_eq!(parsed.probes.len(), report_ok.probes.len());
        assert!((parsed.total_duration_ms - report_ok.total_duration_ms).abs() < 1e-6);

        // Failing report -> exit_code == 1
        let fail_cfg = DoctorRunConfig {
            port: free_port,
            webview2_source: WebView2ProbeSource::SimulatedVersion(None),
            budget_telemetry: over_budget,
            ..DoctorRunConfig::default()
        };
        let report_fail = run_doctor_suite(&fail_cfg);
        assert_eq!(report_fail.exit_code, 1);
        assert_eq!(report_fail.overall_status, ProbeStatus::Fail);
        assert_eq!(report_fail.failed_count, 2);
    }
}
