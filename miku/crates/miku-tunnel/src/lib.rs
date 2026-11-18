//! Secure remote access manager via Tailscale private mesh VPN (ADR-013 · F-17).
//!
//! Replaces Cloudflare Tunnel per user decision with zero-cost personal Tailscale mesh:
//! - Zero public router ports opened (`100.64.0.0/10` / `*.ts.net` WireGuard peers only)
//! - Fail-closed first-frame pairing token verification before touching `EventBus`
//! - Immediate token & session cutoff on Global Kill Switch or leaving Away Mode

pub mod push_wake;
pub mod tailscale;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_push_wake;

pub use push_wake::{
    build_zero_pii_wake_payload, serialize_verified_zero_pii_json, PushWakeClient,
    PushWakeClientConfig, PushWakeError, WakeAgentState, ZeroPiiWakePayload,
};
pub use tailscale::{
    is_tailnet_ip, parse_tailscale_status_json, TailscaleBackend, TailscaleDoctorStatus,
    TailscaleRemoteSession, TailscaleSnapshot, TailscaleTunnelError, TailscaleTunnelManager,
    WindowsTailscaleCliBackend,
};
