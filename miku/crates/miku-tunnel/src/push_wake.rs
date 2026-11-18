use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Canonical state enum allowed in zero-PII mobile wake-up signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeAgentState {
    NeedsPermission,
    Completed,
    Error,
    Paused,
    Ready,
}

impl WakeAgentState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NeedsPermission => "needs_permission",
            Self::Completed => "completed",
            Self::Error => "error",
            Self::Paused => "paused",
            Self::Ready => "ready",
        }
    }
}

/// Strict zero-PII wake-up envelope: contains ONLY agent identifier, coarse state, and timestamp.
/// Never carries file paths, source code, diffs, prompts, user names, or credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroPiiWakePayload {
    pub agent_name: String,
    pub state: WakeAgentState,
    pub event_ts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushWakeError {
    DisabledByDefault,
    InvalidAgentIdentifier(String),
    PotentialPiiOrSecretDetected(String),
    UntrustedEndpoint(String),
    TransportFailed(String),
}

impl std::fmt::Display for PushWakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DisabledByDefault => write!(f, "push wake relay is optional and disabled by default"),
            Self::InvalidAgentIdentifier(msg) => write!(f, "invalid agent_name for wake signal: {}", msg),
            Self::PotentialPiiOrSecretDetected(field) => {
                write!(f, "zero-PII violation blocked before transmission: {}", field)
            }
            Self::UntrustedEndpoint(url) => write!(f, "untrusted wake relay endpoint: {}", url),
            Self::TransportFailed(e) => write!(f, "wake relay transport failed: {}", e),
        }
    }
}

impl std::error::Error for PushWakeError {}

/// Validates `agent_name` and guarantees zero paths, prompts, code, or secrets can leak.
pub fn build_zero_pii_wake_payload(
    agent_name: &str,
    state: WakeAgentState,
    event_ts: u64,
) -> Result<ZeroPiiWakePayload, PushWakeError> {
    let trimmed = agent_name.trim();
    if trimmed.is_empty() || trimmed.len() > 32 {
        return Err(PushWakeError::InvalidAgentIdentifier(
            "agent_name length must be 1..=32 ASCII chars".to_string(),
        ));
    }

    let lower = trimmed.to_ascii_lowercase();
    for forbidden in ["gsk_", "sk-", "bearer", "c:", "http", ".rs", ".ts", "\\", "/"] {
        if lower.contains(forbidden) {
            return Err(PushWakeError::PotentialPiiOrSecretDetected(
                forbidden.to_string(),
            ));
        }
    }

    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(PushWakeError::InvalidAgentIdentifier(
            "only [A-Za-z0-9_-] characters are allowed".to_string(),
        ));
    }

    Ok(ZeroPiiWakePayload {
        agent_name: trimmed.to_string(),
        state,
        event_ts,
    })
}

/// Verifies serialized JSON contains strictly the 3 allowed fields (`agent_name`, `state`, `event_ts`).
pub fn serialize_verified_zero_pii_json(
    payload: &ZeroPiiWakePayload,
) -> Result<String, PushWakeError> {
    let validated = build_zero_pii_wake_payload(&payload.agent_name, payload.state, payload.event_ts)?;
    let val = serde_json::to_value(&validated)
        .map_err(|e| PushWakeError::TransportFailed(e.to_string()))?;
    let obj = val
        .as_object()
        .ok_or_else(|| PushWakeError::TransportFailed("expected JSON object".to_string()))?;

    if obj.len() != 3
        || !obj.contains_key("agent_name")
        || !obj.contains_key("state")
        || !obj.contains_key("event_ts")
    {
        return Err(PushWakeError::PotentialPiiOrSecretDetected(
            "unexpected extra fields in wake payload".to_string(),
        ));
    }

    serde_json::to_string(&validated).map_err(|e| PushWakeError::TransportFailed(e.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushWakeClientConfig {
    pub enabled: bool,
    pub relay_host_port: Option<String>,
    pub relay_path: String,
    pub require_tailnet_or_loopback: bool,
}

impl Default for PushWakeClientConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            relay_host_port: None,
            relay_path: "/wake".to_string(),
            require_tailnet_or_loopback: true,
        }
    }
}

/// Optional zero-PII push wake dispatcher over Tailscale or self-hosted relay.
#[derive(Debug, Clone)]
pub struct PushWakeClient {
    config: PushWakeClientConfig,
}

impl PushWakeClient {
    pub fn new(config: PushWakeClientConfig) -> Self {
        Self { config }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    pub fn dispatch_wake_signal(
        &self,
        agent_name: &str,
        state: WakeAgentState,
        event_ts: u64,
    ) -> Result<u16, PushWakeError> {
        if !self.config.enabled {
            return Err(PushWakeError::DisabledByDefault);
        }
        let target = self
            .config
            .relay_host_port
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| PushWakeError::UntrustedEndpoint("missing host:port".to_string()))?;

        if self.config.require_tailnet_or_loopback {
            let host = target.split(':').next().unwrap_or("");
            let is_local = host == "127.0.0.1" || host == "localhost" || host == "::1";
            if !is_local && !crate::tailscale::is_tailnet_ip(host) {
                return Err(PushWakeError::UntrustedEndpoint(target.to_string()));
            }
        }

        let payload = build_zero_pii_wake_payload(agent_name, state, event_ts)?;
        let json_body = serialize_verified_zero_pii_json(&payload)?;
        let req = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.config.relay_path,
            target,
            json_body.len(),
            json_body
        );

        let mut stream = TcpStream::connect(target)
            .map_err(|e| PushWakeError::TransportFailed(e.to_string()))?;
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        stream
            .write_all(req.as_bytes())
            .map_err(|e| PushWakeError::TransportFailed(e.to_string()))?;
        let mut resp = [0u8; 256];
        let n = stream
            .read(&mut resp)
            .map_err(|e| PushWakeError::TransportFailed(e.to_string()))?;
        let head = String::from_utf8_lossy(&resp[..n]);
        let status_code = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(500);
        Ok(status_code)
    }
}
