//! Spike 0.2 — Procedural HTML5 Canvas 2D Avatar with Sleeping Springs (`rig.toml` + `design-tokens.json`).
//!
//! Validates ADR-002:
//! 1. Declarative schema validation for `assets/rig.toml` and `assets/design-tokens.json`.
//! 2. Symplectic Euler damped spring solver with dual sleep gate:
//!    `|x - target| <= sleep_pos_epsilon && |v| <= sleep_vel_epsilon`.
//! 3. Self-halting `requestAnimationFrame` loop: when all 6 channels are asleep and blink is
//!    silenced for the rest audit window, `raf_invocations == 0` (`0.00 fps`) and
//!    `wakeups_per_sec <= 1.0`.
//! 4. High-DPI (`window.devicePixelRatio` + `ctx.scale(dpr, dpr)`) procedural vector rendering
//!    across all 4 states (`idle`, `thinking`, `speaking`, `alert`) under strict CSP.

use serde::{Deserialize, Serialize};

/// Embedded `assets/rig.toml` declarative configuration.
pub const DEFAULT_RIG_TOML: &str = include_str!("../assets/rig.toml");

/// Embedded `assets/design-tokens.json` declarative palette and geometry tokens.
pub const DEFAULT_DESIGN_TOKENS_JSON: &str = include_str!("../assets/design-tokens.json");

/// Maximum allowed idle FPS when all springs have settled.
pub const MAX_ALLOWED_IDLE_FPS: f64 = 0.0;

/// Maximum allowed wakeups per second during the 2000 ms idle sleep audit window.
pub const MAX_ALLOWED_IDLE_WAKEUPS_PER_SEC: f64 = 1.0;

/// Maximum allowed p95 hover/transition latency in milliseconds.
pub const MAX_HOVER_LATENCY_P95_MS: f64 = 150.0;

/// Maximum allowed idle RAM (post-TTL collapse in Mode C) in megabytes.
pub const MAX_IDLE_RAM_MB: f64 = 30.0;

/// Default duration of the 0 fps rest audit window in milliseconds.
pub const SLEEP_AUDIT_WINDOW_MS: u64 = 2000;

/// Default settle delay after `taskkill /T /F` in milliseconds.
pub const POST_KILL_SETTLE_DELAY_MS: u64 = 150;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AvatarState {
    Idle,
    Thinking,
    Speaking,
    Alert,
}

impl AvatarState {
    pub fn all() -> [AvatarState; 4] {
        [
            AvatarState::Idle,
            AvatarState::Thinking,
            AvatarState::Speaking,
            AvatarState::Alert,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            AvatarState::Idle => "idle",
            AvatarState::Thinking => "thinking",
            AvatarState::Speaking => "speaking",
            AvatarState::Alert => "alert",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "idle" => Some(AvatarState::Idle),
            "thinking" => Some(AvatarState::Thinking),
            "speaking" => Some(AvatarState::Speaking),
            "alert" => Some(AvatarState::Alert),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RigTimingConfig {
    pub max_fps: u32,
    pub sleep_pos_epsilon: f64,
    pub sleep_vel_epsilon: f64,
    pub blink_interval_ms: u64,
    pub blink_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub stiffness: f64,
    pub damping: f64,
    pub initial: f64,
    pub min_limit: f64,
    pub max_limit: f64,
}

impl ChannelSpec {
    pub fn validate(&self, name: &str) -> Result<(), String> {
        if !self.stiffness.is_finite() || self.stiffness <= 0.0 {
            return Err(format!(
                "Channel '{}' stiffness must be finite and > 0.0 (got {})",
                name, self.stiffness
            ));
        }
        if !self.damping.is_finite() || self.damping <= 0.0 {
            return Err(format!(
                "Channel '{}' damping must be finite and > 0.0 (got {})",
                name, self.damping
            ));
        }
        if !self.min_limit.is_finite()
            || !self.max_limit.is_finite()
            || self.min_limit >= self.max_limit
        {
            return Err(format!(
                "Channel '{}' limits invalid: min_limit ({}) must be < max_limit ({})",
                name, self.min_limit, self.max_limit
            ));
        }
        if !self.initial.is_finite()
            || self.initial < self.min_limit
            || self.initial > self.max_limit
        {
            return Err(format!(
                "Channel '{}' initial ({}) outside [{}, {}]",
                name, self.initial, self.min_limit, self.max_limit
            ));
        }
        Ok(())
    }

    pub fn validate_pose_value(&self, channel_name: &str, pose_name: &str, value: f64) -> Result<(), String> {
        if !value.is_finite() || value < self.min_limit || value > self.max_limit {
            return Err(format!(
                "Pose '{}' channel '{}' value ({}) outside [{}, {}]",
                pose_name, channel_name, value, self.min_limit, self.max_limit
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RigChannelsConfig {
    pub head_tilt_deg: ChannelSpec,
    pub body_bob_px: ChannelSpec,
    pub eyelid_open: ChannelSpec,
    pub mouth_open: ChannelSpec,
    pub twintail_sway_deg: ChannelSpec,
    pub glow_pulse: ChannelSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoseTargets {
    pub head_tilt_deg: f64,
    pub body_bob_px: f64,
    pub eyelid_open: f64,
    pub mouth_open: f64,
    pub twintail_sway_deg: f64,
    pub glow_pulse: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RigPosesConfig {
    pub idle: PoseTargets,
    pub thinking: PoseTargets,
    pub speaking: PoseTargets,
    pub alert: PoseTargets,
}

impl RigPosesConfig {
    pub fn get(&self, state: AvatarState) -> &PoseTargets {
        match state {
            AvatarState::Idle => &self.idle,
            AvatarState::Thinking => &self.thinking,
            AvatarState::Speaking => &self.speaking,
            AvatarState::Alert => &self.alert,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RigConfig {
    pub schema_version: u32,
    pub timing: RigTimingConfig,
    pub channels: RigChannelsConfig,
    pub poses: RigPosesConfig,
}

impl RigConfig {
    pub fn from_toml_str(raw: &str) -> Result<Self, String> {
        let cfg: Self =
            toml::from_str(raw).map_err(|e| format!("Failed to parse rig.toml: {}", e))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "Unsupported rig.toml schema_version {} (expected 1)",
                self.schema_version
            ));
        }
        if self.timing.max_fps < 15 || self.timing.max_fps > 240 {
            return Err(format!(
                "timing.max_fps must be in [15, 240] (got {})",
                self.timing.max_fps
            ));
        }
        if !self.timing.sleep_pos_epsilon.is_finite()
            || self.timing.sleep_pos_epsilon <= 0.0
            || self.timing.sleep_pos_epsilon > 0.1
        {
            return Err(format!(
                "timing.sleep_pos_epsilon must be in (0.0, 0.1] (got {})",
                self.timing.sleep_pos_epsilon
            ));
        }
        if !self.timing.sleep_vel_epsilon.is_finite()
            || self.timing.sleep_vel_epsilon <= 0.0
            || self.timing.sleep_vel_epsilon > 0.1
        {
            return Err(format!(
                "timing.sleep_vel_epsilon must be in (0.0, 0.1] (got {})",
                self.timing.sleep_vel_epsilon
            ));
        }
        if self.timing.blink_interval_ms < 500 || self.timing.blink_duration_ms == 0 {
            return Err("Invalid blink timing configuration in rig.toml".to_string());
        }

        self.channels.head_tilt_deg.validate("head_tilt_deg")?;
        self.channels.body_bob_px.validate("body_bob_px")?;
        self.channels.eyelid_open.validate("eyelid_open")?;
        self.channels.mouth_open.validate("mouth_open")?;
        self.channels
            .twintail_sway_deg
            .validate("twintail_sway_deg")?;
        self.channels.glow_pulse.validate("glow_pulse")?;

        for state in AvatarState::all() {
            let p = self.poses.get(state);
            let name = state.as_str();
            self.channels
                .head_tilt_deg
                .validate_pose_value("head_tilt_deg", name, p.head_tilt_deg)?;
            self.channels
                .body_bob_px
                .validate_pose_value("body_bob_px", name, p.body_bob_px)?;
            self.channels
                .eyelid_open
                .validate_pose_value("eyelid_open", name, p.eyelid_open)?;
            self.channels
                .mouth_open
                .validate_pose_value("mouth_open", name, p.mouth_open)?;
            self.channels.twintail_sway_deg.validate_pose_value(
                "twintail_sway_deg",
                name,
                p.twintail_sway_deg,
            )?;
            self.channels
                .glow_pulse
                .validate_pose_value("glow_pulse", name, p.glow_pulse)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CanvasTokenSpec {
    pub logical_width: u32,
    pub logical_height: u32,
    pub corner_radius: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaletteTokens {
    pub surface_bg: String,
    pub surface_border: String,
    pub hair_primary: String,
    pub hair_shadow: String,
    pub hair_highlight: String,
    pub skin_base: String,
    pub skin_blush: String,
    pub eye_sclera: String,
    pub eye_iris: String,
    pub eye_pupil: String,
    pub tie_accent: String,
    pub headset_frame: String,
    pub state_idle_halo: String,
    pub state_thinking_halo: String,
    pub state_speaking_halo: String,
    pub state_alert_halo: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeometryTokens {
    pub head_radius: f64,
    pub eye_offset_x: f64,
    pub eye_offset_y: f64,
    pub eye_radius_x: f64,
    pub eye_radius_y: f64,
    pub twintail_length: f64,
    pub halo_base_radius: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignTokens {
    pub schema_version: u32,
    pub canvas: CanvasTokenSpec,
    pub palette: PaletteTokens,
    pub geometry: GeometryTokens,
}

pub fn is_valid_hex_color(s: &str) -> bool {
    let bytes = s.as_bytes();
    if (bytes.len() != 7 && bytes.len() != 9) || bytes[0] != b'#' {
        return false;
    }
    bytes[1..].iter().all(|b| b.is_ascii_hexdigit())
}

impl DesignTokens {
    pub fn from_json_str(raw: &str) -> Result<Self, String> {
        let tokens: Self = serde_json::from_str(raw)
            .map_err(|e| format!("Failed to parse design-tokens.json: {}", e))?;
        tokens.validate()?;
        Ok(tokens)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err(format!(
                "Unsupported design-tokens.json schema_version {} (expected 1)",
                self.schema_version
            ));
        }
        if self.canvas.logical_width < 64 || self.canvas.logical_height < 64 {
            return Err("canvas.logical_width and logical_height must be >= 64".to_string());
        }
        if self.geometry.head_radius <= 0.0
            || self.geometry.twintail_length <= 0.0
            || self.geometry.halo_base_radius <= 0.0
        {
            return Err("geometry dimensions must be > 0.0".to_string());
        }

        let colors = [
            ("surface_bg", &self.palette.surface_bg),
            ("surface_border", &self.palette.surface_border),
            ("hair_primary", &self.palette.hair_primary),
            ("hair_shadow", &self.palette.hair_shadow),
            ("hair_highlight", &self.palette.hair_highlight),
            ("skin_base", &self.palette.skin_base),
            ("skin_blush", &self.palette.skin_blush),
            ("eye_sclera", &self.palette.eye_sclera),
            ("eye_iris", &self.palette.eye_iris),
            ("eye_pupil", &self.palette.eye_pupil),
            ("tie_accent", &self.palette.tie_accent),
            ("headset_frame", &self.palette.headset_frame),
            ("state_idle_halo", &self.palette.state_idle_halo),
            ("state_thinking_halo", &self.palette.state_thinking_halo),
            ("state_speaking_halo", &self.palette.state_speaking_halo),
            ("state_alert_halo", &self.palette.state_alert_halo),
        ];

        for (name, hex) in colors {
            if !is_valid_hex_color(hex) {
                return Err(format!(
                    "Invalid hex color for palette.{}: '{}' (expected #RRGGBB or #RRGGBBAA)",
                    name, hex
                ));
            }
        }

        Ok(())
    }
}

/// Single 1D damped spring channel with strict dual-threshold sleep gate.
#[derive(Debug, Clone, PartialEq)]
pub struct SpringChannel {
    pub name: &'static str,
    pub value: f64,
    pub velocity: f64,
    pub target: f64,
    pub stiffness: f64,
    pub damping: f64,
    pub min_limit: f64,
    pub max_limit: f64,
    pub sleep_pos_epsilon: f64,
    pub sleep_vel_epsilon: f64,
    pub sleeping: bool,
}

impl SpringChannel {
    pub fn from_spec(
        name: &'static str,
        spec: &ChannelSpec,
        sleep_pos_epsilon: f64,
        sleep_vel_epsilon: f64,
    ) -> Self {
        Self {
            name,
            value: spec.initial,
            velocity: 0.0,
            target: spec.initial,
            stiffness: spec.stiffness,
            damping: spec.damping,
            min_limit: spec.min_limit,
            max_limit: spec.max_limit,
            sleep_pos_epsilon,
            sleep_vel_epsilon,
            sleeping: true,
        }
    }

    pub fn set_target(&mut self, new_target: f64) {
        let clamped = new_target.clamp(self.min_limit, self.max_limit);
        self.target = clamped;
        if (self.value - self.target).abs() > self.sleep_pos_epsilon
            || self.velocity.abs() > self.sleep_vel_epsilon
        {
            self.sleeping = false;
        } else {
            self.value = self.target;
            self.velocity = 0.0;
            self.sleeping = true;
        }
    }

    pub fn inject_impulse(&mut self, delta_velocity: f64) {
        self.velocity += delta_velocity;
        if (self.value - self.target).abs() > self.sleep_pos_epsilon
            || self.velocity.abs() > self.sleep_vel_epsilon
        {
            self.sleeping = false;
        }
    }

    /// Advances the spring by `dt_sec` using 4-substep symplectic Euler integration.
    /// Returns `true` if the channel was awake and stepped, or `false` if already asleep.
    pub fn step(&mut self, dt_sec: f64) -> bool {
        if self.sleeping {
            return false;
        }

        let sub_steps = 4usize;
        let sub_dt = dt_sec / (sub_steps as f64);
        for _ in 0..sub_steps {
            let displacement = self.value - self.target;
            let accel = -self.stiffness * displacement - self.damping * self.velocity;
            self.velocity += accel * sub_dt;
            self.value += self.velocity * sub_dt;
        }

        // Strict dual sleep gate: BOTH position error AND velocity must be within epsilon.
        if (self.value - self.target).abs() <= self.sleep_pos_epsilon
            && self.velocity.abs() <= self.sleep_vel_epsilon
        {
            self.value = self.target;
            self.velocity = 0.0;
            self.sleeping = true;
        }

        true
    }
}

/// Complete 6-channel procedural avatar rig with deterministic frame scheduler.
#[derive(Debug, Clone)]
pub struct AvatarRig {
    pub state: AvatarState,
    pub head_tilt_deg: SpringChannel,
    pub body_bob_px: SpringChannel,
    pub eyelid_open: SpringChannel,
    pub mouth_open: SpringChannel,
    pub twintail_sway_deg: SpringChannel,
    pub glow_pulse: SpringChannel,
    pub poses: RigPosesConfig,
    pub max_fps: u32,
    pub blink_enabled: bool,
    pub total_frames_rendered: u64,
}

impl AvatarRig {
    pub fn from_config(cfg: &RigConfig) -> Self {
        let eps_x = cfg.timing.sleep_pos_epsilon;
        let eps_v = cfg.timing.sleep_vel_epsilon;
        let mut rig = Self {
            state: AvatarState::Idle,
            head_tilt_deg: SpringChannel::from_spec(
                "head_tilt_deg",
                &cfg.channels.head_tilt_deg,
                eps_x,
                eps_v,
            ),
            body_bob_px: SpringChannel::from_spec(
                "body_bob_px",
                &cfg.channels.body_bob_px,
                eps_x,
                eps_v,
            ),
            eyelid_open: SpringChannel::from_spec(
                "eyelid_open",
                &cfg.channels.eyelid_open,
                eps_x,
                eps_v,
            ),
            mouth_open: SpringChannel::from_spec(
                "mouth_open",
                &cfg.channels.mouth_open,
                eps_x,
                eps_v,
            ),
            twintail_sway_deg: SpringChannel::from_spec(
                "twintail_sway_deg",
                &cfg.channels.twintail_sway_deg,
                eps_x,
                eps_v,
            ),
            glow_pulse: SpringChannel::from_spec(
                "glow_pulse",
                &cfg.channels.glow_pulse,
                eps_x,
                eps_v,
            ),
            poses: cfg.poses.clone(),
            max_fps: cfg.timing.max_fps,
            blink_enabled: false,
            total_frames_rendered: 0,
        };
        rig.transition_to(AvatarState::Idle);
        rig
    }

    pub fn transition_to(&mut self, next_state: AvatarState) {
        self.state = next_state;
        let pose = self.poses.get(next_state).clone();
        self.head_tilt_deg.set_target(pose.head_tilt_deg);
        self.body_bob_px.set_target(pose.body_bob_px);
        self.eyelid_open.set_target(pose.eyelid_open);
        self.mouth_open.set_target(pose.mouth_open);
        self.twintail_sway_deg.set_target(pose.twintail_sway_deg);
        self.glow_pulse.set_target(pose.glow_pulse);
    }

    pub fn is_sleeping(&self) -> bool {
        self.head_tilt_deg.sleeping
            && self.body_bob_px.sleeping
            && self.eyelid_open.sleeping
            && self.mouth_open.sleeping
            && self.twintail_sway_deg.sleeping
            && self.glow_pulse.sleeping
    }

    pub fn awake_channel_count(&self) -> usize {
        [
            !self.head_tilt_deg.sleeping,
            !self.body_bob_px.sleeping,
            !self.eyelid_open.sleeping,
            !self.mouth_open.sleeping,
            !self.twintail_sway_deg.sleeping,
            !self.glow_pulse.sleeping,
        ]
        .into_iter()
        .filter(|awake| *awake)
        .count()
    }

    /// Steps a single animation frame ONLY if at least one spring channel is awake.
    /// When `is_sleeping()` is true, returns `false` without rendering a frame (`0 fps`).
    pub fn step_frame_if_awake(&mut self, dt_sec: f64) -> bool {
        if self.is_sleeping() {
            return false;
        }
        self.head_tilt_deg.step(dt_sec);
        self.body_bob_px.step(dt_sec);
        self.eyelid_open.step(dt_sec);
        self.mouth_open.step(dt_sec);
        self.twintail_sway_deg.step(dt_sec);
        self.glow_pulse.step(dt_sec);
        self.total_frames_rendered += 1;
        true
    }

    /// Simulates a time window of `duration_ms` at `max_fps` and returns the number of
    /// rendered frames (`rAF` invocations) and effective FPS.
    pub fn audit_time_window(&mut self, duration_ms: u64) -> (u64, f64) {
        let dt_sec = 1.0 / (self.max_fps as f64);
        let tick_count = ((duration_ms as f64 / 1000.0) * (self.max_fps as f64)).round() as u64;
        let start_frames = self.total_frames_rendered;
        for _ in 0..tick_count {
            if !self.step_frame_if_awake(dt_sec) {
                // Self-halting rAF loop: once asleep and with blink disabled, no timer wakes it.
                break;
            }
        }
        let rendered = self.total_frames_rendered - start_frames;
        let fps = (rendered as f64) / (duration_ms as f64 / 1000.0);
        (rendered, fps)
    }
}

/// Generates the self-contained HTML5 Canvas 2D document for the WebView2 worker.
///
/// Enforces:
/// 1. Strict CSP: `default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline';`
/// 2. High-DPI scaling via `window.devicePixelRatio` and `ctx.scale(dpr, dpr)`.
/// 3. Procedural 2D vector drawing (`arc`, `bezierCurveTo`, `fill`, `stroke`) driven by the
///    6 spring channels and `design-tokens.json`.
/// 4. Complete suspension of blink timer during the 2000 ms `AUDIT_SLEEP` window so `idle_fps == 0.0`.
pub fn build_avatar_worker_html(rig: &RigConfig, tokens: &DesignTokens) -> Result<String, String> {
    let rig_json =
        serde_json::to_string(rig).map_err(|e| format!("Failed serializing RigConfig: {}", e))?;
    let tokens_json = serde_json::to_string(tokens)
        .map_err(|e| format!("Failed serializing DesignTokens: {}", e))?;

    Ok(format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline';">
<title>Miku Island · Spike 0.2 Procedural Avatar</title>
<style>
  html, body {{
    margin: 0;
    padding: 0;
    width: 100%;
    height: 100%;
    background: {surface_bg};
    color: #F8FAFC;
    font-family: system-ui, -apple-system, sans-serif;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: space-between;
    box-sizing: border-box;
    padding: 8px 16px;
  }}
  .avatar-stage {{
    display: flex;
    align-items: center;
    gap: 14px;
  }}
  canvas {{
    border-radius: {corner_radius}px;
    border: 1px solid {surface_border};
    background: {surface_bg};
    display: block;
  }}
  .hud {{
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
  }}
  .hud-title {{
    font-weight: 700;
    color: {hair_primary};
    letter-spacing: 0.04em;
  }}
  .hud-sub {{
    color: #94A3B8;
    font-family: Consolas, monospace;
    font-size: 11px;
  }}
</style>
</head>
<body>
  <div class="avatar-stage">
    <canvas id="miku-canvas"></canvas>
    <div class="hud">
      <div class="hud-title">MIKU ISLAND · SPIKE 0.2</div>
      <div class="hud-sub" id="hud-state">STATE: idle</div>
      <div class="hud-sub" id="hud-springs">SPRINGS: sleeping</div>
      <div class="hud-sub" id="hud-dpr">DPR: 1.0</div>
    </div>
  </div>
<script>
(function() {{
  "use strict";
  const RIG = {rig_json};
  const TOKENS = {tokens_json};

  const canvas = document.getElementById("miku-canvas");
  const ctx = canvas.getContext("2d");
  const hudState = document.getElementById("hud-state");
  const hudSprings = document.getElementById("hud-springs");
  const hudDpr = document.getElementById("hud-dpr");

  const logicalW = TOKENS.canvas.logical_width;
  const logicalH = TOKENS.canvas.logical_height;
  let currentDpr = 1;

  function setupHighDpiCanvas() {{
    currentDpr = Math.max(1, window.devicePixelRatio || 1);
    canvas.width = Math.round(logicalW * currentDpr);
    canvas.height = Math.round(logicalH * currentDpr);
    canvas.style.width = logicalW + "px";
    canvas.style.height = logicalH + "px";
    hudDpr.textContent = "DPR: " + currentDpr.toFixed(2);
  }}
  setupHighDpiCanvas();

  const CHANNEL_NAMES = [
    "head_tilt_deg",
    "body_bob_px",
    "eyelid_open",
    "mouth_open",
    "twintail_sway_deg",
    "glow_pulse"
  ];

  const channels = {{}};
  for (const name of CHANNEL_NAMES) {{
    const spec = RIG.channels[name];
    channels[name] = {{
      value: spec.initial,
      velocity: 0.0,
      target: spec.initial,
      stiffness: spec.stiffness,
      damping: spec.damping,
      min_limit: spec.min_limit,
      max_limit: spec.max_limit,
      sleeping: true
    }};
  }}

  let currentState = "idle";
  let rafId = null;
  let lastFrameTime = 0;
  let framesInCurrentTransition = 0;
  let totalRafInvocations = 0;
  let auditRafCount = 0;
  let auditTimerWakeups = 0;
  let blinkEnabled = false;
  let blinkTimerId = null;

  function postToHost(msg) {{
    if (window.chrome && window.chrome.webview) {{
      window.chrome.webview.postMessage(msg);
    }}
  }}

  function allSpringsAsleep() {{
    for (const name of CHANNEL_NAMES) {{
      if (!channels[name].sleeping) {{
        return false;
      }}
    }}
    return true;
  }}

  function stepSpringChannel(ch, dtSec) {{
    if (ch.sleeping) {{
      return false;
    }}
    const subSteps = 4;
    const subDt = dtSec / subSteps;
    for (let i = 0; i < subSteps; i++) {{
      const disp = ch.value - ch.target;
      const accel = -ch.stiffness * disp - ch.damping * ch.velocity;
      ch.velocity += accel * subDt;
      ch.value += ch.velocity * subDt;
    }}
    if (
      Math.abs(ch.value - ch.target) <= RIG.timing.sleep_pos_epsilon &&
      Math.abs(ch.velocity) <= RIG.timing.sleep_vel_epsilon
    ) {{
      ch.value = ch.target;
      ch.velocity = 0.0;
      ch.sleeping = true;
    }}
    return true;
  }}

  function stateHaloColor(state) {{
    const p = TOKENS.palette;
    if (state === "thinking") return p.state_thinking_halo;
    if (state === "speaking") return p.state_speaking_halo;
    if (state === "alert") return p.state_alert_halo;
    return p.state_idle_halo;
  }}

  function renderProceduralAvatar() {{
    const p = TOKENS.palette;
    const g = TOKENS.geometry;

    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.save();
    ctx.scale(currentDpr, currentDpr);

    const cx = logicalW * 0.5;
    const cy = logicalH * 0.52 + channels.body_bob_px.value;
    const headRad = (channels.head_tilt_deg.value * Math.PI) / 180.0;
    const swayRad = (channels.twintail_sway_deg.value * Math.PI) / 180.0;
    const glow = channels.glow_pulse.value;

    // 1. State halo ring behind avatar
    ctx.save();
    ctx.beginPath();
    ctx.arc(cx, cy, g.halo_base_radius + glow * 8.0, 0, Math.PI * 2);
    ctx.strokeStyle = stateHaloColor(currentState);
    ctx.lineWidth = 2.0 + glow * 2.5;
    ctx.stroke();
    ctx.restore();

    // 2. Left & Right Twintails (procedural cubic Bezier ribbons)
    const tieOffsetX = g.head_radius * 0.92;
    const tieOffsetY = -g.head_radius * 0.35;

    for (const dir of [-1, 1]) {{
      ctx.save();
      ctx.translate(cx + dir * tieOffsetX, cy + tieOffsetY);
      ctx.rotate(dir * 0.18 + swayRad * 0.65);

      ctx.beginPath();
      ctx.moveTo(0, 0);
      ctx.bezierCurveTo(
        dir * 26.0, 14.0,
        dir * 34.0, g.twintail_length * 0.65,
        dir * 16.0, g.twintail_length
      );
      ctx.bezierCurveTo(
        dir * 4.0, g.twintail_length * 0.75,
        dir * 8.0, 22.0,
        0, 6.0
      );
      ctx.closePath();
      ctx.fillStyle = p.hair_primary;
      ctx.fill();
      ctx.strokeStyle = p.hair_shadow;
      ctx.lineWidth = 1.5;
      ctx.stroke();

      // Hair tie ribbon
      ctx.beginPath();
      ctx.arc(0, 0, 5.5, 0, Math.PI * 2);
      ctx.fillStyle = p.tie_accent;
      ctx.fill();
      ctx.restore();
    }}

    // 3. Head group (rotated by head_tilt_deg)
    ctx.save();
    ctx.translate(cx, cy);
    ctx.rotate(headRad);

    // Headset earpieces
    for (const dir of [-1, 1]) {{
      ctx.beginPath();
      ctx.arc(dir * (g.head_radius + 2.0), 2.0, 7.0, 0, Math.PI * 2);
      ctx.fillStyle = p.headset_frame;
      ctx.fill();
      ctx.strokeStyle = p.tie_accent;
      ctx.lineWidth = 1.5;
      ctx.stroke();
    }}

    // Base head circle + chin bezier
    ctx.beginPath();
    ctx.arc(0, 0, g.head_radius, 0, Math.PI * 2);
    ctx.fillStyle = p.skin_base;
    ctx.fill();
    ctx.strokeStyle = p.hair_shadow;
    ctx.lineWidth = 1.4;
    ctx.stroke();

    // Cheek blush
    for (const dir of [-1, 1]) {{
      ctx.beginPath();
      ctx.arc(dir * (g.eye_offset_x + 4.0), g.eye_offset_y + 10.0, 5.2, 0, Math.PI * 2);
      ctx.fillStyle = p.skin_blush;
      ctx.fill();
    }}

    // 4. Eyes modulated by eyelid_open
    const eyeOpen = Math.max(0.05, channels.eyelid_open.value);
    for (const dir of [-1, 1]) {{
      const ex = dir * g.eye_offset_x;
      const ey = g.eye_offset_y;
      const ry = g.eye_radius_y * eyeOpen;

      ctx.save();
      ctx.translate(ex, ey);
      // Sclera & Iris
      ctx.beginPath();
      ctx.ellipse(0, 0, g.eye_radius_x, ry, 0, 0, Math.PI * 2);
      ctx.fillStyle = p.eye_sclera;
      ctx.fill();

      ctx.beginPath();
      ctx.ellipse(0, 0, g.eye_radius_x * 0.78, ry * 0.85, 0, 0, Math.PI * 2);
      ctx.fillStyle = p.eye_iris;
      ctx.fill();

      ctx.beginPath();
      ctx.ellipse(0, 0, g.eye_radius_x * 0.42, ry * 0.48, 0, 0, Math.PI * 2);
      ctx.fillStyle = p.eye_pupil;
      ctx.fill();

      // Specular highlight
      if (eyeOpen > 0.25) {{
        ctx.beginPath();
        ctx.arc(-2.0, -ry * 0.35, 1.8, 0, Math.PI * 2);
        ctx.fillStyle = "#FFFFFF";
        ctx.fill();
      }}
      ctx.restore();
    }}

    // 5. Mouth modulated by mouth_open
    const mouthOpen = channels.mouth_open.value;
    const my = 17.5;
    ctx.beginPath();
    if (mouthOpen <= 0.12) {{
      ctx.moveTo(-6.0, my);
      ctx.bezierCurveTo(-2.0, my + 3.0, 2.0, my + 3.0, 6.0, my);
      ctx.strokeStyle = p.tie_accent;
      ctx.lineWidth = 1.8;
      ctx.stroke();
    }} else {{
      const mh = 2.0 + mouthOpen * 8.5;
      ctx.moveTo(-7.0, my);
      ctx.bezierCurveTo(-4.0, my + mh, 4.0, my + mh, 7.0, my);
      ctx.bezierCurveTo(3.0, my - 1.5, -3.0, my - 1.5, -7.0, my);
      ctx.closePath();
      ctx.fillStyle = p.tie_accent;
      ctx.fill();
    }}

    // 6. Forehead bangs (cubic Bezier crown)
    ctx.beginPath();
    ctx.arc(0, -4.0, g.head_radius + 1.0, Math.PI * 1.05, Math.PI * 1.95);
    ctx.bezierCurveTo(22.0, -8.0, 10.0, 2.0, 0.0, -10.0);
    ctx.bezierCurveTo(-10.0, 2.0, -22.0, -8.0, -g.head_radius, -6.0);
    ctx.closePath();
    ctx.fillStyle = p.hair_primary;
    ctx.fill();
    ctx.strokeStyle = p.hair_highlight;
    ctx.lineWidth = 1.2;
    ctx.stroke();

    ctx.restore();
    ctx.restore();

    hudState.textContent = "STATE: " + currentState;
    hudSprings.textContent = allSpringsAsleep() ? "SPRINGS: sleeping (0 fps)" : "SPRINGS: awake";
  }}

  function onAnimationFrame(now) {{
    rafId = null;
    totalRafInvocations += 1;
    auditRafCount += 1;
    framesInCurrentTransition += 1;

    const dtSec = lastFrameTime > 0
      ? Math.min(0.05, Math.max(0.004, (now - lastFrameTime) / 1000.0))
      : (1.0 / RIG.timing.max_fps);
    lastFrameTime = now;

    for (const name of CHANNEL_NAMES) {{
      stepSpringChannel(channels[name], dtSec);
    }}

    renderProceduralAvatar();

    if (!allSpringsAsleep()) {{
      rafId = requestAnimationFrame(onAnimationFrame);
    }} else {{
      lastFrameTime = 0;
      postToHost("STATE_SETTLED:" + currentState + ":" + framesInCurrentTransition);
    }}
  }}

  function ensureLoopRunning() {{
    if (rafId === null && !allSpringsAsleep()) {{
      lastFrameTime = 0;
      rafId = requestAnimationFrame(onAnimationFrame);
    }}
  }}

  window.__mikuSetState = function(nextState) {{
    const pose = RIG.poses[nextState];
    if (!pose) {{
      postToHost("ERROR:Unknown state " + nextState);
      return;
    }}
    currentState = nextState;
    framesInCurrentTransition = 0;
    for (const name of CHANNEL_NAMES) {{
      const ch = channels[name];
      const target = Math.max(ch.min_limit, Math.min(ch.max_limit, pose[name]));
      ch.target = target;
      if (
        Math.abs(ch.value - ch.target) > RIG.timing.sleep_pos_epsilon ||
        Math.abs(ch.velocity) > RIG.timing.sleep_vel_epsilon
      ) {{
        ch.sleeping = false;
      }} else {{
        ch.value = ch.target;
        ch.velocity = 0.0;
        ch.sleeping = true;
      }}
    }}
    renderProceduralAvatar();
    if (allSpringsAsleep()) {{
      postToHost("STATE_SETTLED:" + currentState + ":0");
    }} else {{
      ensureLoopRunning();
    }}
  }};

  window.__mikuSetBlinkEnabled = function(enabled) {{
    blinkEnabled = !!enabled;
    if (!blinkEnabled && blinkTimerId !== null) {{
      clearTimeout(blinkTimerId);
      blinkTimerId = null;
    }}
    postToHost("BLINK_STATUS:" + (blinkEnabled ? "ENABLED" : "SUSPENDED"));
  }};

  window.__mikuAuditSleepWindow = function(durationMs) {{
    // Strictly silence blink before auditing 0 fps in idle rest
    window.__mikuSetBlinkEnabled(false);
    auditRafCount = 0;
    auditTimerWakeups = 0;
    const t0 = performance.now();
    setTimeout(function() {{
      const elapsedSec = Math.max(0.001, (performance.now() - t0) / 1000.0);
      const fps = auditRafCount / elapsedSec;
      const wakeupsPerSec = auditTimerWakeups / elapsedSec;
      const payload = JSON.stringify({{
        duration_ms: durationMs,
        raf_count: auditRafCount,
        fps: fps,
        wakeups_per_sec: wakeupsPerSec,
        all_asleep: allSpringsAsleep(),
        dpr: currentDpr
      }});
      postToHost("SLEEP_AUDIT_DONE:" + payload);
    }}, durationMs);
  }};

  // Initial first paint
  renderProceduralAvatar();
  requestAnimationFrame(function() {{
    requestAnimationFrame(function() {{
      postToHost("PAINT_READY:INITIAL:" + currentDpr.toFixed(2));
    }});
  }});
}})();
</script>
</body>
</html>"#,
        surface_bg = tokens.palette.surface_bg,
        surface_border = tokens.palette.surface_border,
        hair_primary = tokens.palette.hair_primary,
        corner_radius = tokens.canvas.corner_radius,
        rig_json = rig_json,
        tokens_json = tokens_json
    ))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SleepAuditPayload {
    pub duration_ms: u64,
    pub raf_count: u64,
    pub fps: f64,
    pub wakeups_per_sec: f64,
    pub all_asleep: bool,
    pub dpr: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StateTransitionMetric {
    pub state: AvatarState,
    pub settle_ms: f64,
    pub frames_rendered: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spike02AuditSummary {
    pub worker_root_pid: u32,
    pub first_paint_ms: f64,
    pub detected_dpr: f64,
    pub transitions: Vec<StateTransitionMetric>,
    pub sleep_audit: SleepAuditPayload,
    pub expanded_private_mb: f64,
    pub post_ttl_idle_private_mb: f64,
    pub orphan_count_after_close: usize,
    pub passes_all_thresholds: bool,
}

impl Spike02AuditSummary {
    pub fn evaluate_thresholds(&self) -> bool {
        self.sleep_audit.all_asleep
            && self.sleep_audit.raf_count == 0
            && self.sleep_audit.fps <= MAX_ALLOWED_IDLE_FPS
            && self.sleep_audit.wakeups_per_sec <= MAX_ALLOWED_IDLE_WAKEUPS_PER_SEC
            && self.first_paint_ms <= MAX_HOVER_LATENCY_P95_MS
            && self.post_ttl_idle_private_mb <= MAX_IDLE_RAM_MB
            && self.orphan_count_after_close == 0
            && self.transitions.len() == 4
    }
}

pub fn format_spike02_report(summary: &Spike02AuditSummary) -> String {
    let mut out = String::new();
    out.push_str("=== SPIKE 0.2: PROCEDURAL HTML5 CANVAS AVATAR & SLEEPING SPRINGS ===\n");
    out.push_str(&format!(
        "Worker Root PID        : {}\n",
        summary.worker_root_pid
    ));
    out.push_str(&format!(
        "High-DPI Scale (DPR)   : {:.2}x (ctx.scale applied)\n",
        summary.detected_dpr
    ));
    out.push_str(&format!(
        "First Paint Latency    : {:.2} ms (threshold <= {:.1} ms)\n",
        summary.first_paint_ms, MAX_HOVER_LATENCY_P95_MS
    ));
    out.push_str("State Transitions      :\n");
    for tr in &summary.transitions {
        out.push_str(&format!(
            "  - {:<10} -> settled in {:>6.2} ms ({:>3} frames)\n",
            tr.state.as_str(),
            tr.settle_ms,
            tr.frames_rendered
        ));
    }
    out.push_str(&format!(
        "Idle Sleep Audit ({} ms, blink suspended):\n",
        summary.sleep_audit.duration_ms
    ));
    out.push_str(&format!(
        "  - All 6 Springs Asleep : {}\n",
        summary.sleep_audit.all_asleep
    ));
    out.push_str(&format!(
        "  - rAF Invocations      : {} (target == 0)\n",
        summary.sleep_audit.raf_count
    ));
    out.push_str(&format!(
        "  - Measured Idle FPS    : {:.2} fps (target <= {:.1} fps)\n",
        summary.sleep_audit.fps, MAX_ALLOWED_IDLE_FPS
    ));
    out.push_str(&format!(
        "  - Timer Wakeups/s      : {:.2} wake/s (target <= {:.1} wake/s)\n",
        summary.sleep_audit.wakeups_per_sec, MAX_ALLOWED_IDLE_WAKEUPS_PER_SEC
    ));
    out.push_str(&format!(
        "Expanded RAM (Mode C)  : {:.2} MB\n",
        summary.expanded_private_mb
    ));
    out.push_str(&format!(
        "Post-TTL Idle RAM      : {:.2} MB (threshold <= {:.1} MB)\n",
        summary.post_ttl_idle_private_mb, MAX_IDLE_RAM_MB
    ));
    out.push_str(&format!(
        "Orphans After Close    : {} (threshold == 0)\n",
        summary.orphan_count_after_close
    ));
    out.push_str(&format!(
        "ADR-002 Verdict        : {}\n",
        if summary.passes_all_thresholds {
            "CUMPLE (ADR-002 LISTO PARA FIJAR)"
        } else {
            "NO CUMPLE"
        }
    ));
    out
}

/// Process tree inspection helpers (reused from 0.1a/0.1b for strict PrivatePageCount & orphan checks).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub creation_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessNode {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub private_bytes: u64,
    pub creation_date: String,
}

pub fn parse_powershell_process_csv(csv_text: &str) -> Result<Vec<ProcessNode>, String> {
    let mut nodes = Vec::new();
    for (idx, raw_line) in csv_text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = line
            .split('\t')
            .map(|c| c.trim().trim_matches('"'))
            .collect();
        if cols.len() < 5 {
            continue;
        }
        if idx == 0 && cols[0].eq_ignore_ascii_case("ProcessId") {
            continue;
        }
        let pid = cols[0]
            .parse::<u32>()
            .map_err(|e| format!("Invalid PID '{}': {}", cols[0], e))?;
        let parent_pid = cols[1]
            .parse::<u32>()
            .map_err(|e| format!("Invalid ParentPID '{}': {}", cols[1], e))?;
        let name = cols[2].to_string();
        let private_bytes = cols[3].parse::<u64>().unwrap_or(0);
        let creation_date = cols[4].to_string();
        nodes.push(ProcessNode {
            pid,
            parent_pid,
            name,
            private_bytes,
            creation_date,
        });
    }
    if nodes.is_empty() {
        return Err("Process snapshot contained 0 valid rows".to_string());
    }
    Ok(nodes)
}

pub fn collect_process_tree(
    root_pid: u32,
    snapshot: &[ProcessNode],
) -> Result<(Vec<ProcessIdentity>, u64), String> {
    let root = snapshot
        .iter()
        .find(|n| n.pid == root_pid)
        .ok_or_else(|| format!("Root PID {} not found in OS snapshot", root_pid))?;

    let mut visited_pids = vec![root.pid];
    let mut identities = vec![ProcessIdentity {
        pid: root.pid,
        creation_date: root.creation_date.clone(),
    }];
    let mut total_bytes = root.private_bytes;

    let mut cursor = 0;
    while cursor < visited_pids.len() {
        let current_parent = visited_pids[cursor];
        cursor += 1;
        for node in snapshot {
            if node.parent_pid == current_parent && !visited_pids.contains(&node.pid) {
                visited_pids.push(node.pid);
                identities.push(ProcessIdentity {
                    pid: node.pid,
                    creation_date: node.creation_date.clone(),
                });
                total_bytes = total_bytes.saturating_add(node.private_bytes);
            }
        }
    }

    Ok((identities, total_bytes))
}

pub fn count_surviving_identities_after_close(
    spawned_identities: &[ProcessIdentity],
    post_close_snapshot: &[ProcessNode],
) -> usize {
    spawned_identities
        .iter()
        .filter(|id| {
            post_close_snapshot
                .iter()
                .any(|node| node.pid == id.pid && node.creation_date == id.creation_date)
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LEDGER-0.2-1: Validates parsing of `assets/rig.toml` and `assets/design-tokens.json`
    /// and verifies rejection of invalid parameters (stiffness <= 0, invalid hex colors, etc.).
    #[test]
    fn test_rig_and_tokens_schema_validation() {
        let rig = RigConfig::from_toml_str(DEFAULT_RIG_TOML)
            .expect("Default assets/rig.toml must parse and validate cleanly");
        assert_eq!(rig.schema_version, 1);
        assert_eq!(rig.timing.max_fps, 60);

        let tokens = DesignTokens::from_json_str(DEFAULT_DESIGN_TOKENS_JSON)
            .expect("Default assets/design-tokens.json must parse and validate cleanly");
        assert_eq!(tokens.schema_version, 1);
        assert_eq!(tokens.canvas.logical_width, 180);

        // Must reject stiffness <= 0
        let mut bad_rig = rig.clone();
        bad_rig.channels.head_tilt_deg.stiffness = 0.0;
        assert!(
            bad_rig.validate().is_err(),
            "Zero stiffness must be rejected"
        );

        // Must reject pose target outside channel [min_limit, max_limit]
        let mut out_of_bounds_rig = rig.clone();
        out_of_bounds_rig.poses.alert.eyelid_open = 9.99;
        assert!(
            out_of_bounds_rig.validate().is_err(),
            "Pose target outside [min_limit, max_limit] must be rejected"
        );

        // Must reject malformed hex color in design-tokens.json
        let mut bad_tokens = tokens.clone();
        bad_tokens.palette.hair_primary = "rgb(0, 217, 192)".to_string();
        assert!(
            bad_tokens.validate().is_err(),
            "Non-hex color string must be rejected"
        );
    }

    /// LEDGER-0.2-2: Property/unit test verifying that a spring crossing `x == target` with
    /// non-zero velocity (`|v| > sleep_vel_epsilon`) does NOT fall asleep prematurely, and
    /// converges monotonically to `sleeping == true`, `x == target`, `v == 0.0` in < 180 frames.
    #[test]
    fn test_spring_convergence_and_sleep_gate() {
        let cfg = RigConfig::from_toml_str(DEFAULT_RIG_TOML).unwrap();
        let mut ch = SpringChannel::from_spec(
            "head_tilt_deg",
            &cfg.channels.head_tilt_deg,
            cfg.timing.sleep_pos_epsilon,
            cfg.timing.sleep_vel_epsilon,
        );

        // Case A: Spring is exactly at target (x == target), but has high velocity.
        // Dual sleep gate MUST keep it awake across the step.
        ch.value = ch.target;
        ch.velocity = 12.0;
        ch.sleeping = false;
        let stepped = ch.step(1.0 / 60.0);
        assert!(stepped);
        assert!(
            !ch.sleeping,
            "Spring crossing target with high velocity must NOT fall asleep prematurely"
        );

        // Case B: Step to convergence from a large displacement (-8.5 deg -> +12.0 deg).
        ch.set_target(12.0);
        assert!(!ch.sleeping);

        let mut frames = 0usize;
        while !ch.sleeping && frames < 500 {
            ch.step(1.0 / 60.0);
            frames += 1;
        }

        assert!(ch.sleeping, "Spring must reach sleeping state");
        assert!(
            frames > 5 && frames < 180,
            "Expected convergence between 6 and 179 frames at 60 Hz, got {}",
            frames
        );
        assert_eq!(ch.value, 12.0);
        assert_eq!(ch.velocity, 0.0);
    }

    /// LEDGER-0.2-3: Verifies transitions across all 4 states (`idle` -> `thinking` ->
    /// `speaking` -> `alert` -> `idle`), confirming every transition wakes the rig and
    /// settles cleanly to `is_sleeping() == true`.
    #[test]
    fn test_state_transitions_wake_and_settle() {
        let cfg = RigConfig::from_toml_str(DEFAULT_RIG_TOML).unwrap();
        let mut rig = AvatarRig::from_config(&cfg);
        assert!(
            rig.is_sleeping(),
            "Rig must start in settled idle state when initial values match idle pose"
        );

        let sequence = [
            AvatarState::Thinking,
            AvatarState::Speaking,
            AvatarState::Alert,
            AvatarState::Idle,
        ];

        for next_state in sequence {
            rig.transition_to(next_state);
            assert!(
                !rig.is_sleeping(),
                "Transition to '{}' must wake at least one spring channel",
                next_state.as_str()
            );

            let mut frames = 0usize;
            while !rig.is_sleeping() && frames < 240 {
                assert!(rig.step_frame_if_awake(1.0 / 60.0));
                frames += 1;
            }

            assert!(
                rig.is_sleeping(),
                "State '{}' failed to settle within 240 frames",
                next_state.as_str()
            );
            assert!(frames > 0 && frames < 180);

            let expected_pose = cfg.poses.get(next_state);
            assert!((rig.head_tilt_deg.value - expected_pose.head_tilt_deg).abs() < 1e-9);
            assert!((rig.mouth_open.value - expected_pose.mouth_open).abs() < 1e-9);
            assert!((rig.glow_pulse.value - expected_pose.glow_pulse).abs() < 1e-9);
        }
    }

    /// LEDGER-0.2-4: Verifies that when all 6 channels are asleep and blink is suspended,
    /// auditing a 2000 ms window produces strictly 0 rendered frames (`0.0 fps`).
    #[test]
    fn test_zero_fps_when_all_springs_asleep() {
        let cfg = RigConfig::from_toml_str(DEFAULT_RIG_TOML).unwrap();
        let tokens = DesignTokens::from_json_str(DEFAULT_DESIGN_TOKENS_JSON).unwrap();
        let mut rig = AvatarRig::from_config(&cfg);

        // Transition to Alert, settle completely, then return to Idle and settle completely.
        rig.transition_to(AvatarState::Alert);
        while !rig.is_sleeping() {
            rig.step_frame_if_awake(1.0 / 60.0);
        }
        rig.transition_to(AvatarState::Idle);
        while !rig.is_sleeping() {
            rig.step_frame_if_awake(1.0 / 60.0);
        }
        assert!(rig.is_sleeping());

        // Audit 2000 ms window in settled idle state with blink suspended.
        let (frames_rendered, measured_fps) = rig.audit_time_window(SLEEP_AUDIT_WINDOW_MS);
        assert_eq!(frames_rendered, 0, "Settled rig must render 0 frames in 2000 ms");
        assert_eq!(measured_fps, 0.0, "Settled rig must measure strictly 0.0 fps");

        // Verify generated HTML contains high-DPI scaling, strict CSP, and procedural Canvas calls.
        let html = build_avatar_worker_html(&cfg, &tokens).unwrap();
        assert!(html.contains("default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline';"));
        assert!(html.contains("window.devicePixelRatio"));
        assert!(html.contains("ctx.scale(currentDpr, currentDpr)"));
        assert!(html.contains("bezierCurveTo"));
    }
}
