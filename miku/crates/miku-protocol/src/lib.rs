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

const K_SHA256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
    0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
    0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
    0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
    0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
    0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
    0xc67178f2,
];

pub fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            let j = i * 4;
            w[i] = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K_SHA256[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        let bytes = word.to_be_bytes();
        out[i * 4..i * 4 + 4].copy_from_slice(&bytes);
    }
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256_digest(data);
    let mut s = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // LEDGER-1.1-1
    #[test]
    fn test_base_contracts_rig_toml_and_design_tokens_loader_and_validation() {
        let bundle = BaseContractBundle::load_embedded_defaults()
            .expect("embedded rig.toml and design-tokens.json must validate cleanly");
        assert_eq!(bundle.rig.schema_version, 1);
        assert_eq!(bundle.rig.states.idle.fps_when_asleep, 0);
        assert_eq!(bundle.tokens.island.pill_width, 220);
        assert_eq!(bundle.tokens.island.pill_height, 36);
        assert_eq!(bundle.contract_sha256.len(), 64);

        // Reject non-zero fps_when_asleep
        let bad_rig = DEFAULT_RIG_TOML.replace("fps_when_asleep = 0", "fps_when_asleep = 60");
        let err = RigConfig::parse_and_validate(&bad_rig).unwrap_err();
        assert!(matches!(err, ProtocolError::ContractValidation(_)));

        // Reject malformed hex color using r##"..."## delimiter
        let bad_tokens = DEFAULT_DESIGN_TOKENS_JSON.replace(r##""#0B0F17""##, r##""#ZZZZZZ""##);
        let err_tokens = DesignTokens::parse_and_validate(&bad_tokens).unwrap_err();
        assert!(matches!(err_tokens, ProtocolError::ContractValidation(_)));

        // Reject unknown fields (deny_unknown_fields)
        let unknown_field_rig = format!("{}\nunexpected_root_key = 42\n", DEFAULT_RIG_TOML);
        assert!(matches!(
            RigConfig::parse_and_validate(&unknown_field_rig),
            Err(ProtocolError::MalformedToml(_))
        ));
    }

    // LEDGER-1.1-2
    #[test]
    fn test_protocol_v1_envelope_serialization_and_constant_time_token_auth() {
        let samples = vec![
            MikuLinkEnvelopeV1 {
                v: 1,
                id: "msg-01".to_string(),
                reply_to: None,
                msg: MikuLinkMessageV1::Hello {
                    client: "miku_cli".to_string(),
                    token: "tok_secret_123".to_string(),
                    capabilities: vec!["agent_events".to_string()],
                },
            },
            MikuLinkEnvelopeV1 {
                v: 1,
                id: "msg-02".to_string(),
                reply_to: Some("msg-01".to_string()),
                msg: MikuLinkMessageV1::HelloAck {
                    session_id: "sess_01".to_string(),
                    island_state: IslandState::Idle,
                },
            },
            MikuLinkEnvelopeV1 {
                v: 1,
                id: "msg-03".to_string(),
                reply_to: None,
                msg: MikuLinkMessageV1::Ping { ts: 1_762_315_641 },
            },
            MikuLinkEnvelopeV1 {
                v: 1,
                id: "msg-04".to_string(),
                reply_to: Some("msg-03".to_string()),
                msg: MikuLinkMessageV1::Pong { ts: 1_762_315_641 },
            },
            MikuLinkEnvelopeV1 {
                v: 1,
                id: "msg-05".to_string(),
                reply_to: None,
                msg: MikuLinkMessageV1::AgentEvent {
                    agent: "claude_code".to_string(),
                    kind: "needs_input".to_string(),
                    title: "Edit src/lib.rs".to_string(),
                },
            },
            MikuLinkEnvelopeV1 {
                v: 1,
                id: "msg-06".to_string(),
                reply_to: None,
                msg: MikuLinkMessageV1::PermissionRequest {
                    req_hash: "sha256:abcdef".to_string(),
                    agent: "claude_code".to_string(),
                    capability: "fs_write".to_string(),
                    target: "src/lib.rs".to_string(),
                    ttl_sec: 60,
                    pc_only: false,
                },
            },
            MikuLinkEnvelopeV1 {
                v: 1,
                id: "msg-07".to_string(),
                reply_to: Some("msg-06".to_string()),
                msg: MikuLinkMessageV1::PermissionDecision {
                    req_hash: "sha256:abcdef".to_string(),
                    decision: PermissionDecisionKind::Allow,
                    biometric_verified: true,
                },
            },
            MikuLinkEnvelopeV1 {
                v: 1,
                id: "msg-08".to_string(),
                reply_to: None,
                msg: MikuLinkMessageV1::Command {
                    session_id: "sess_01".to_string(),
                    action: "enqueue_prompt".to_string(),
                    text: "cargo test".to_string(),
                },
            },
            MikuLinkEnvelopeV1 {
                v: 1,
                id: "msg-09".to_string(),
                reply_to: Some("msg-01".to_string()),
                msg: MikuLinkMessageV1::Error {
                    code: "unsupported_version".to_string(),
                    supported: Some(vec![1]),
                    detail: None,
                },
            },
        ];

        for env in samples {
            let encoded = env.encode_json().expect("must serialize");
            let decoded = MikuLinkEnvelopeV1::decode_and_validate(&encoded).expect("must decode");
            assert_eq!(env, decoded);
        }

        // Unsupported version v=99 must return UnsupportedVersion
        let bad_version = r##"{"v":99,"id":"msg-99","msg":{"type":"ping","data":{"ts":10}}}"##;
        assert_eq!(
            MikuLinkEnvelopeV1::decode_and_validate(bad_version),
            Err(ProtocolError::UnsupportedVersion {
                received: 99,
                supported: vec![1]
            })
        );

        // Oversized frame must be rejected immediately
        let huge = "x".repeat(MAX_FRAME_BYTES + 10);
        assert!(matches!(
            MikuLinkEnvelopeV1::decode_and_validate(&huge),
            Err(ProtocolError::FrameTooLarge { .. })
        ));

        // Constant-time token comparison checks
        assert!(constant_time_token_eq(
            "miku_tok_v1_99887766",
            "miku_tok_v1_99887766"
        ));
        assert!(!constant_time_token_eq(
            "miku_tok_v1_99887766",
            "miku_tok_v1_99887767"
        ));
        assert!(!constant_time_token_eq("miku_tok_v1_99887766", "short"));
        assert!(!constant_time_token_eq("", ""));
    }
}
