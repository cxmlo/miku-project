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

use serde::{Deserialize, Serialize};

pub const MIKU_PROTOCOL_VERSION_V1: u8 = 1;
pub const SUPPORTED_PROTOCOL_VERSIONS: &[u8] = &[MIKU_PROTOCOL_VERSION_V1];
pub const MAX_FRAME_BYTES: usize = 65_536;
pub const MAX_AUTH_FRAME_BYTES: usize = 16_384;

pub const DEFAULT_RIG_TOML: &str = include_str!("../../../assets/rig.toml");
pub const DEFAULT_DESIGN_TOKENS_JSON: &str = include_str!("../../../assets/design-tokens.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolError {
    FrameTooLarge {
        size_bytes: usize,
        max_bytes: usize,
    },
    UnsupportedVersion {
        received: u8,
        supported: Vec<u8>,
    },
    MalformedJson(String),
    MalformedToml(String),
    Unauthorized(String),
    ContractValidation(String),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::FrameTooLarge {
                size_bytes,
                max_bytes,
            } => write!(
                f,
                "frame size {} exceeds maximum allowed {} bytes",
                size_bytes, max_bytes
            ),
            ProtocolError::UnsupportedVersion {
                received,
                supported,
            } => write!(
                f,
                "unsupported protocol version {}; supported: {:?}",
                received, supported
            ),
            ProtocolError::MalformedJson(msg) => write!(f, "malformed JSON: {}", msg),
            ProtocolError::MalformedToml(msg) => write!(f, "malformed TOML: {}", msg),
            ProtocolError::Unauthorized(msg) => write!(f, "unauthorized: {}", msg),
            ProtocolError::ContractValidation(msg) => write!(f, "contract validation error: {}", msg),
        }
    }
}

impl std::error::Error for ProtocolError {}

// ============================================================================
// 1. Base Configuration Contracts: rig.toml & design-tokens.json
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigConfig {
    pub schema_version: u32,
    pub spring: SpringParams,
    pub states: RigStates,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpringParams {
    pub stiffness: f64,
    pub damping: f64,
    pub sleep_pos_epsilon: f64,
    pub sleep_vel_epsilon: f64,
    pub max_dt_sec: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigStates {
    pub idle: RigPose,
    pub thinking: RigPose,
    pub speaking: RigPose,
    pub alert: RigPose,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigPose {
    pub head_y: f64,
    pub head_tilt_deg: f64,
    pub twintail_sway_deg: f64,
    pub eye_open: f64,
    pub mouth_open: f64,
    pub glow_alpha: f64,
    pub blink_interval_ms: [u32; 2],
    pub fps_when_asleep: u32,
}

impl RigConfig {
    pub fn parse_and_validate(raw_toml: &str) -> Result<Self, ProtocolError> {
        let cfg: RigConfig =
            toml::from_str(raw_toml).map_err(|e| ProtocolError::MalformedToml(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.schema_version != 1 {
            return Err(ProtocolError::ContractValidation(format!(
                "unsupported rig.toml schema_version {}",
                self.schema_version
            )));
        }
        let s = &self.spring;
        if !s.stiffness.is_finite() || s.stiffness <= 0.0 || s.stiffness > 5_000.0 {
            return Err(ProtocolError::ContractValidation(
                "spring.stiffness must be finite in (0.0, 5000.0]".to_string(),
            ));
        }
        if !s.damping.is_finite() || s.damping <= 0.0 || s.damping > 1_000.0 {
            return Err(ProtocolError::ContractValidation(
                "spring.damping must be finite in (0.0, 1000.0]".to_string(),
            ));
        }
        if !s.sleep_pos_epsilon.is_finite()
            || s.sleep_pos_epsilon <= 0.0
            || s.sleep_pos_epsilon > 1.0
        {
            return Err(ProtocolError::ContractValidation(
                "spring.sleep_pos_epsilon must be in (0.0, 1.0]".to_string(),
            ));
        }
        if !s.sleep_vel_epsilon.is_finite()
            || s.sleep_vel_epsilon <= 0.0
            || s.sleep_vel_epsilon > 1.0
        {
            return Err(ProtocolError::ContractValidation(
                "spring.sleep_vel_epsilon must be in (0.0, 1.0]".to_string(),
            ));
        }
        if !s.max_dt_sec.is_finite() || s.max_dt_sec <= 0.0 || s.max_dt_sec > 0.1 {
            return Err(ProtocolError::ContractValidation(
                "spring.max_dt_sec must be in (0.0, 0.1]".to_string(),
            ));
        }

        for (name, pose) in [
            ("idle", &self.states.idle),
            ("thinking", &self.states.thinking),
            ("speaking", &self.states.speaking),
            ("alert", &self.states.alert),
        ] {
            pose.validate(name)?;
        }
        Ok(())
    }
}

impl RigPose {
    pub fn validate(&self, state_name: &str) -> Result<(), ProtocolError> {
        if self.fps_when_asleep != 0 {
            return Err(ProtocolError::ContractValidation(format!(
                "state '{}' must enforce fps_when_asleep == 0 (got {})",
                state_name, self.fps_when_asleep
            )));
        }
        if !self.eye_open.is_finite() || self.eye_open < 0.0 || self.eye_open > 1.5 {
            return Err(ProtocolError::ContractValidation(format!(
                "state '{}' eye_open out of bounds",
                state_name
            )));
        }
        if !self.mouth_open.is_finite() || self.mouth_open < 0.0 || self.mouth_open > 1.5 {
            return Err(ProtocolError::ContractValidation(format!(
                "state '{}' mouth_open out of bounds",
                state_name
            )));
        }
        if !self.glow_alpha.is_finite() || self.glow_alpha < 0.0 || self.glow_alpha > 1.0 {
            return Err(ProtocolError::ContractValidation(format!(
                "state '{}' glow_alpha must be in [0.0, 1.0]",
                state_name
            )));
        }
        if self.blink_interval_ms[0] == 0
            || self.blink_interval_ms[0] >= self.blink_interval_ms[1]
        {
            return Err(ProtocolError::ContractValidation(format!(
                "state '{}' blink_interval_ms must satisfy 0 < min < max",
                state_name
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignTokens {
    pub schema_version: u32,
    pub island: IslandGeometryTokens,
    pub colors: ColorPaletteTokens,
    pub motion: MotionPolicyTokens,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IslandGeometryTokens {
    pub pill_width: u32,
    pub pill_height: u32,
    pub expanded_width: u32,
    pub expanded_height: u32,
    pub top_margin_px: u32,
    pub corner_radius_px: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorPaletteTokens {
    pub surface: String,
    pub surface_elevated: String,
    pub accent: String,
    pub alert: String,
    pub text_primary: String,
    pub hair_teal: String,
    pub eye_cyan: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MotionPolicyTokens {
    pub respect_windows_reduced_motion: bool,
    pub hover_expand_delay_ms: u32,
    pub webview_ttl_secs: u64,
}

impl DesignTokens {
    pub fn parse_and_validate(raw_json: &str) -> Result<Self, ProtocolError> {
        let tokens: DesignTokens = serde_json::from_str(raw_json)
            .map_err(|e| ProtocolError::MalformedJson(e.to_string()))?;
        tokens.validate()?;
        Ok(tokens)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.schema_version != 1 {
            return Err(ProtocolError::ContractValidation(format!(
                "unsupported design-tokens.json schema_version {}",
                self.schema_version
            )));
        }
        let g = &self.island;
        if g.pill_width < 80
            || g.pill_height < 20
            || g.expanded_width <= g.pill_width
            || g.expanded_height <= g.pill_height
        {
            return Err(ProtocolError::ContractValidation(
                "invalid island geometry bounds in design-tokens.json".to_string(),
            ));
        }

        for (label, color) in [
            ("surface", &self.colors.surface),
            ("surface_elevated", &self.colors.surface_elevated),
            ("accent", &self.colors.accent),
            ("alert", &self.colors.alert),
            ("text_primary", &self.colors.text_primary),
            ("hair_teal", &self.colors.hair_teal),
            ("eye_cyan", &self.colors.eye_cyan),
        ] {
            if !is_valid_hex_color(color) {
                return Err(ProtocolError::ContractValidation(format!(
                    "invalid hex color for '{}': {}",
                    label, color
                )));
            }
        }

        if self.motion.hover_expand_delay_ms == 0 || self.motion.hover_expand_delay_ms > 2_000 {
            return Err(ProtocolError::ContractValidation(
                "motion.hover_expand_delay_ms must be in [1, 2000]".to_string(),
            ));
        }
        if self.motion.webview_ttl_secs == 0 || self.motion.webview_ttl_secs > 3_600 {
            return Err(ProtocolError::ContractValidation(
                "motion.webview_ttl_secs must be in [1, 3600]".to_string(),
            ));
        }

        Ok(())
    }
}

pub fn is_valid_hex_color(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' {
        return false;
    }
    bytes[1..].iter().all(|b| b.is_ascii_hexdigit())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaseContractBundle {
    pub rig: RigConfig,
    pub tokens: DesignTokens,
    pub contract_sha256: String,
}

impl BaseContractBundle {
    pub fn load_embedded_defaults() -> Result<Self, ProtocolError> {
        Self::from_str_pair(DEFAULT_RIG_TOML, DEFAULT_DESIGN_TOKENS_JSON)
    }

    pub fn from_str_pair(rig_toml: &str, tokens_json: &str) -> Result<Self, ProtocolError> {
        let rig = RigConfig::parse_and_validate(rig_toml)?;
        let tokens = DesignTokens::parse_and_validate(tokens_json)?;
        let mut combined = Vec::with_capacity(rig_toml.len() + tokens_json.len() + 16);
        combined.extend_from_slice(b"MIKU_CONTRACT_V1:");
        combined.extend_from_slice(rig_toml.trim().as_bytes());
        combined.push(b'|');
        combined.extend_from_slice(tokens_json.trim().as_bytes());
        let contract_sha256 = sha256_hex(&combined);
        Ok(Self {
            rig,
            tokens,
            contract_sha256,
        })
    }
}

// ============================================================================
// 2. Miku-Link Protocol v1 (Pure Serde, Zero I/O, 9 Message Variants)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IslandState {
    Idle,
    Peek,
    Expanded,
    Listening,
    Alert,
    Gaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecisionKind {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MikuLinkEnvelopeV1 {
    pub v: u8,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    pub msg: MikuLinkMessageV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum MikuLinkMessageV1 {
    Hello {
        client: String,
        token: String,
        capabilities: Vec<String>,
    },
    HelloAck {
        session_id: String,
        island_state: IslandState,
    },
    Ping {
        ts: u64,
    },
    Pong {
        ts: u64,
    },
    AgentEvent {
        agent: String,
        kind: String,
        title: String,
    },
    PermissionRequest {
        req_hash: String,
        agent: String,
        capability: String,
        target: String,
        ttl_sec: u32,
        pc_only: bool,
    },
    PermissionDecision {
        req_hash: String,
        decision: PermissionDecisionKind,
        biometric_verified: bool,
    },
    Command {
        session_id: String,
        action: String,
        text: String,
    },
    Error {
        code: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        supported: Option<Vec<u8>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

impl MikuLinkEnvelopeV1 {
    pub fn decode_and_validate(raw_json: &str) -> Result<Self, ProtocolError> {
        let size_bytes = raw_json.len();
        if size_bytes > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge {
                size_bytes,
                max_bytes: MAX_FRAME_BYTES,
            });
        }

        // Inspect version field first so unsupported versions produce a typed error
        // even if future payload shapes differ.
        let val: serde_json::Value = serde_json::from_str(raw_json)
            .map_err(|e| ProtocolError::MalformedJson(e.to_string()))?;
        let v_u64 = val
            .get("v")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ProtocolError::MalformedJson("missing numeric field 'v'".to_string()))?;
        let v = u8::try_from(v_u64).unwrap_or(u8::MAX);
        if v != MIKU_PROTOCOL_VERSION_V1 {
            return Err(ProtocolError::UnsupportedVersion {
                received: v,
                supported: SUPPORTED_PROTOCOL_VERSIONS.to_vec(),
            });
        }

        let env: MikuLinkEnvelopeV1 = serde_json::from_value(val)
            .map_err(|e| ProtocolError::MalformedJson(e.to_string()))?;

        if env.id.trim().is_empty() {
            return Err(ProtocolError::MalformedJson(
                "envelope 'id' cannot be empty".to_string(),
            ));
        }

        if matches!(
            env.msg,
            MikuLinkMessageV1::Hello { .. } | MikuLinkMessageV1::PermissionDecision { .. }
        ) && size_bytes > MAX_AUTH_FRAME_BYTES
        {
            return Err(ProtocolError::FrameTooLarge {
                size_bytes,
                max_bytes: MAX_AUTH_FRAME_BYTES,
            });
        }

        Ok(env)
    }

    pub fn encode_json(&self) -> Result<String, ProtocolError> {
        serde_json::to_string(self).map_err(|e| ProtocolError::MalformedJson(e.to_string()))
    }
}

/// Constant-time token comparison (no early exit on mismatch).
pub fn constant_time_token_eq(expected: &str, provided: &str) -> bool {
    let a = expected.as_bytes();
    let b = provided.as_bytes();
    let mut diff: u8 = if a.len() == b.len() { 0 } else { 1 };
    let max_len = a.len().max(b.len());
    for i in 0..max_len {
        let byte_a = if i < a.len() { a[i] } else { 0 };
        let byte_b = if i < b.len() { b[i] } else { 0 };
        diff |= byte_a ^ byte_b;
    }
    diff == 0 && !a.is_empty()
}

// ============================================================================
// 3. Self-Contained FIPS 180-4 SHA-256 Helper (Zero External C Deps)
// ============================================================================

pub mod sha256;
pub use sha256::*;


pub mod claude;
pub use claude::*;

pub mod session_queue;
pub use session_queue::*;

#[cfg(test)]
mod tests;



