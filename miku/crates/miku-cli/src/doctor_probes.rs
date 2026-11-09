use std::collections::HashSet;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::process::Command;
use std::time::{Duration, Instant};

use miku_core::build_current_thread_runtime;
use miku_gateway::LoopbackWsClient;
use miku_platform::{configure_headless_command, LaunchConfig, WIN32_CREATE_NO_WINDOW};
use miku_protocol::{
    IslandState, MikuLinkEnvelopeV1, MikuLinkMessageV1, MIKU_PROTOCOL_VERSION_V1,
};
use serde::{Deserialize, Serialize};

use crate::{DiagnosticProbeResult, ProbeStatus, WEBVIEW2_RUNTIME_GUID};

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
