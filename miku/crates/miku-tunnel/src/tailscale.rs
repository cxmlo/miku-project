use std::collections::HashSet;
use std::net::IpAddr;
use std::process::Command;

use miku_platform::{configure_headless_command, LaunchConfig, WIN32_CREATE_NO_WINDOW};
use miku_protocol::{constant_time_token_eq, sha256_hex, MikuLinkEnvelopeV1, MikuLinkMessageV1};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailscaleSnapshot {
    pub backend_state: String,
    pub self_dns_name: Option<String>,
    pub self_tailnet_ips: Vec<String>,
    pub online_peers: usize,
    pub cli_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailscaleRemoteSession {
    pub wss_endpoint: String,
    pub tailnet_ip: String,
    pub session_token: String,
    pub token_generation: u64,
    pub activated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailscaleDoctorStatus {
    pub healthy: bool,
    pub cli_available: bool,
    pub backend_running: bool,
    pub assigned_endpoint: Option<String>,
    pub summary: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailscaleTunnelError {
    CliUnavailable(String),
    BackendNotRunning(String),
    NoTailnetAddressAssigned,
    KillSwitchActive,
    AwayModeDisabled,
    UntrustedPeerIp(String),
    TlsPinMismatch,
    FirstFrameRejectedBeforeEventBus(String),
    JsonParseError(String),
}

impl std::fmt::Display for TailscaleTunnelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CliUnavailable(e) => write!(f, "tailscale CLI unavailable: {}", e),
            Self::BackendNotRunning(st) => write!(f, "tailscale backend not running (state={})", st),
            Self::NoTailnetAddressAssigned => write!(f, "no Tailscale IPv4/DNS assigned to host"),
            Self::KillSwitchActive => write!(f, "remote access aborted: Kill Switch is active"),
            Self::AwayModeDisabled => write!(f, "remote away mode is disabled"),
            Self::UntrustedPeerIp(ip) => {
                write!(f, "peer '{}' is outside Tailscale CGNAT/ULA mesh range", ip)
            }
            Self::TlsPinMismatch => write!(f, "TLS certificate SHA-256 pin mismatch"),
            Self::FirstFrameRejectedBeforeEventBus(reason) => {
                write!(f, "rejected on first frame before EventBus: {}", reason)
            }
            Self::JsonParseError(e) => write!(f, "tailscale status JSON parse error: {}", e),
        }
    }
}

impl std::error::Error for TailscaleTunnelError {}

pub trait TailscaleBackend {
    fn query_status_json(&self) -> Result<String, TailscaleTunnelError>;
    fn reset_serve_route(&self) -> Result<(), TailscaleTunnelError>;
}

/// Live Windows Tailscale CLI adapter using headless `CREATE_NO_WINDOW`.
#[derive(Debug, Clone, Default)]
pub struct WindowsTailscaleCliBackend {
    pub binary_name: String,
}

impl WindowsTailscaleCliBackend {
    pub fn new() -> Self {
        Self {
            binary_name: "tailscale".to_string(),
        }
    }
}

impl TailscaleBackend for WindowsTailscaleCliBackend {
    fn query_status_json(&self) -> Result<String, TailscaleTunnelError> {
        let mut cmd = Command::new(&self.binary_name);
        cmd.args(["status", "--json"]);
        configure_headless_command(
            &mut cmd,
            &LaunchConfig {
                background: true,
                headless: true,
                worker: false,
                hold_for_ram: false,
                rep: 0,
                win32_creation_flags: WIN32_CREATE_NO_WINDOW,
            },
        );
        let out = cmd
            .output()
            .map_err(|e| TailscaleTunnelError::CliUnavailable(e.to_string()))?;
        if !out.status.success() {
            return Err(TailscaleTunnelError::BackendNotRunning(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn reset_serve_route(&self) -> Result<(), TailscaleTunnelError> {
        let mut cmd = Command::new(&self.binary_name);
        cmd.args(["serve", "reset"]);
        configure_headless_command(
            &mut cmd,
            &LaunchConfig {
                background: true,
                headless: true,
                worker: false,
                hold_for_ram: false,
                rep: 0,
                win32_creation_flags: WIN32_CREATE_NO_WINDOW,
            },
        );
        let _ = cmd.output();
        Ok(())
    }
}

/// Checks whether an IP belongs exclusively to Tailscale WireGuard mesh (`100.64.0.0/10` or `fd7a:115c:a1e0::/48`).
pub fn is_tailnet_ip(raw_ip: &str) -> bool {
    let Ok(addr) = raw_ip.trim().parse::<IpAddr>() else {
        return false;
    };
    match addr {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // CGNAT 100.64.0.0/10 => first octet 100, top 2 bits of second octet == 0b01 (64..=127)
            octets[0] == 100 && (octets[1] & 0b1100_0000) == 64
        }
        IpAddr::V6(v6) => {
            let segs = v6.segments();
            segs[0] == 0xfd7a && segs[1] == 0x115c && segs[2] == 0xa1e0
        }
    }
}

/// Parses `tailscale status --json` payload.
pub fn parse_tailscale_status_json(raw_json: &str) -> Result<TailscaleSnapshot, TailscaleTunnelError> {
    let val: serde_json::Value =
        serde_json::from_str(raw_json).map_err(|e| TailscaleTunnelError::JsonParseError(e.to_string()))?;

    let backend_state = val
        .get("BackendState")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let self_obj = val.get("Self");
    let self_dns_name = self_obj
        .and_then(|s| s.get("DNSName"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim_end_matches('.').to_string())
        .filter(|s| !s.is_empty());

    let self_tailnet_ips = self_obj
        .and_then(|s| s.get("TailscaleIPs"))
        .and_then(|arr| arr.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .filter(|ip| is_tailnet_ip(ip))
                .map(|ip| ip.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let online_peers = val
        .get("Peer")
        .and_then(|p| p.as_object())
        .map(|obj| {
            obj.values()
                .filter(|peer| peer.get("Online").and_then(|o| o.as_bool()).unwrap_or(false))
                .count()
        })
        .unwrap_or(0);

    Ok(TailscaleSnapshot {
        backend_state,
        self_dns_name,
        self_tailnet_ips,
        online_peers,
        cli_available: true,
    })
}

#[derive(Debug, Clone)]
pub struct TailscaleTunnelManager {
    gateway_port: u16,
    away_mode_enabled: bool,
    kill_switch_tripped: bool,
    active_session: Option<TailscaleRemoteSession>,
    token_generation: u64,
    active_remote_peers: HashSet<String>,
    pub event_bus_dispatches: usize,
}

impl TailscaleTunnelManager {
    pub fn new(gateway_port: u16) -> Self {
        Self {
            gateway_port,
            away_mode_enabled: false,
            kill_switch_tripped: false,
            active_session: None,
            token_generation: 0,
            active_remote_peers: HashSet::new(),
            event_bus_dispatches: 0,
        }
    }

    pub fn is_away_mode_enabled(&self) -> bool {
        self.away_mode_enabled && !self.kill_switch_tripped && self.active_session.is_some()
    }

    pub fn active_session(&self) -> Option<&TailscaleRemoteSession> {
        self.active_session.as_ref()
    }

    pub fn active_peer_count(&self) -> usize {
        self.active_remote_peers.len()
    }

    /// Enables "Fuera de casa" mode on the private Tailscale mesh and mints a fresh remote token.
    pub fn enable_away_mode<B: TailscaleBackend>(
        &mut self,
        backend: &B,
        entropy_seed: &str,
        now_ms: u64,
    ) -> Result<TailscaleRemoteSession, TailscaleTunnelError> {
        if self.kill_switch_tripped {
            return Err(TailscaleTunnelError::KillSwitchActive);
        }
        let raw_json = backend.query_status_json()?;
        let snap = parse_tailscale_status_json(&raw_json)?;
        if snap.backend_state != "Running" {
            return Err(TailscaleTunnelError::BackendNotRunning(snap.backend_state));
        }

        let tailnet_ip = snap
            .self_tailnet_ips
            .first()
            .cloned()
            .ok_or(TailscaleTunnelError::NoTailnetAddressAssigned)?;

        let host = snap
            .self_dns_name
            .unwrap_or_else(|| tailnet_ip.clone());
        let wss_endpoint = format!("wss://{}:{}/ws", host, self.gateway_port);

        self.token_generation += 1;
        let token_material = format!(
            "miku_ts_tok_v1|gen={}|ts={}|host={}|seed={}",
            self.token_generation, now_ms, host, entropy_seed
        );
        let session_token = format!("ts_{}", &sha256_hex(token_material.as_bytes())[..40]);

        let session = TailscaleRemoteSession {
            wss_endpoint,
            tailnet_ip,
            session_token,
            token_generation: self.token_generation,
            activated_at_ms: now_ms,
        };
        self.away_mode_enabled = true;
        self.active_session = Some(session.clone());
        Ok(session)
    }

    /// Rotates the active Tailscale remote token immediately (disconnecting prior sessions).
    pub fn rotate_remote_session_token(
        &mut self,
        entropy_seed: &str,
        now_ms: u64,
    ) -> Result<String, TailscaleTunnelError> {
        if self.kill_switch_tripped {
            return Err(TailscaleTunnelError::KillSwitchActive);
        }
        let Some(ref mut session) = self.active_session else {
            return Err(TailscaleTunnelError::AwayModeDisabled);
        };
        self.token_generation += 1;
        let material = format!(
            "miku_ts_rot_v1|gen={}|ts={}|seed={}",
            self.token_generation, now_ms, entropy_seed
        );
        let new_tok = format!("ts_{}", &sha256_hex(material.as_bytes())[..40]);
        session.session_token = new_tok.clone();
        session.token_generation = self.token_generation;
        self.active_remote_peers.clear();
        Ok(new_tok)
    }

    /// Immediately closes all active remote sessions and tears down the Tailscale serve route.
    pub fn disable_away_mode_or_kill_switch<B: TailscaleBackend>(
        &mut self,
        backend: &B,
        from_kill_switch: bool,
    ) {
        if from_kill_switch {
            self.kill_switch_tripped = true;
        }
        self.away_mode_enabled = false;
        self.active_session = None;
        self.active_remote_peers.clear();
        let _ = backend.reset_serve_route();
    }

    pub fn reset_kill_switch_for_new_session(&mut self) {
        self.kill_switch_tripped = false;
    }

    /// Fail-closed first-frame authentication BEFORE touching the internal `EventBus`.
    pub fn authenticate_first_frame_fail_closed(
        &mut self,
        peer_ip: &str,
        tls_pin_matches: bool,
        first_frame_json: &str,
    ) -> Result<String, TailscaleTunnelError> {
        if self.kill_switch_tripped {
            return Err(TailscaleTunnelError::KillSwitchActive);
        }
        let Some(ref session) = self.active_session else {
            return Err(TailscaleTunnelError::AwayModeDisabled);
        };
        if !self.away_mode_enabled {
            return Err(TailscaleTunnelError::AwayModeDisabled);
        }
        if !is_tailnet_ip(peer_ip) {
            return Err(TailscaleTunnelError::UntrustedPeerIp(peer_ip.to_string()));
        }
        if !tls_pin_matches {
            return Err(TailscaleTunnelError::TlsPinMismatch);
        }

        let env = MikuLinkEnvelopeV1::decode_and_validate(first_frame_json).map_err(|e| {
            TailscaleTunnelError::FirstFrameRejectedBeforeEventBus(e.to_string())
        })?;

        match env.msg {
            MikuLinkMessageV1::Hello { client, token, .. } => {
                if !constant_time_token_eq(&token, &session.session_token) {
                    return Err(TailscaleTunnelError::FirstFrameRejectedBeforeEventBus(
                        "invalid or rotated Miku pairing token".to_string(),
                    ));
                }
                let peer_key = format!("{}@{}", client, peer_ip);
                self.active_remote_peers.insert(peer_key.clone());
                self.event_bus_dispatches += 1;
                Ok(peer_key)
            }
            _ => Err(TailscaleTunnelError::FirstFrameRejectedBeforeEventBus(
                "first frame must be Hello with valid pairing token".to_string(),
            )),
        }
    }

    /// Produces a deterministic health status for `miku doctor`.
    pub fn doctor_check<B: TailscaleBackend>(&self, backend: &B) -> TailscaleDoctorStatus {
        match backend.query_status_json() {
            Ok(raw) => match parse_tailscale_status_json(&raw) {
                Ok(snap) => {
                    let running = snap.backend_state == "Running";
                    let assigned = snap.self_dns_name.map(|dns| format!("wss://{}:{}/ws", dns, self.gateway_port));
                    TailscaleDoctorStatus {
                        healthy: running && !snap.self_tailnet_ips.is_empty(),
                        cli_available: true,
                        backend_running: running,
                        assigned_endpoint: assigned,
                        summary: format!(
                            "Tailscale backend={} ips={:?} online_peers={}",
                            snap.backend_state, snap.self_tailnet_ips, snap.online_peers
                        ),
                        remediation: if running {
                            None
                        } else {
                            Some("Run `tailscale up` to authenticate your personal Tailnet".to_string())
                        },
                    }
                }
                Err(e) => TailscaleDoctorStatus {
                    healthy: false,
                    cli_available: true,
                    backend_running: false,
                    assigned_endpoint: None,
                    summary: format!("Failed parsing `tailscale status --json`: {}", e),
                    remediation: Some("Verify Tailscale client version >= 1.60".to_string()),
                },
            },
            Err(err) => TailscaleDoctorStatus {
                healthy: false,
                cli_available: false,
                backend_running: false,
                assigned_endpoint: None,
                summary: format!("Tailscale unavailable (away mode stays fail-closed): {}", err),
                remediation: Some(
                    "Install free Tailscale (https://tailscale.com/download) and run `tailscale up` for away access"
                        .to_string(),
                ),
            },
        }
    }
}
