#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 10. Production Avatar Multi-Spring Physics, Decoupled Avatar Pack &
//     Atomic Hot-Reloader (Step 2.5 · F-02 · ADR-001 · ADR-006)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpringChannelState {
    pub name: String,
    pub stiffness: f64,
    pub damping: f64,
    pub mass: f64,
    pub position: f64,
    pub velocity: f64,
    pub target: f64,
    pub asleep: bool,
}

impl SpringChannelState {
    pub fn new(name: impl Into<String>, cfg: &V7RigSpringConfig) -> Result<Self, UiError> {
        let name_str = name.into();
        if !cfg.stiffness.is_finite() || cfg.stiffness <= 0.0 || cfg.stiffness > 5_000.0 {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "spring '{}' has invalid stiffness {}",
                name_str, cfg.stiffness
            )));
        }
        if !cfg.damping.is_finite() || cfg.damping <= 0.0 || cfg.damping > 500.0 {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "spring '{}' has invalid damping {}",
                name_str, cfg.damping
            )));
        }
        if !cfg.mass.is_finite() || cfg.mass <= 0.0 || cfg.mass > 100.0 {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "spring '{}' has invalid mass {}",
                name_str, cfg.mass
            )));
        }
        Ok(Self {
            name: name_str,
            stiffness: cfg.stiffness,
            damping: cfg.damping,
            mass: cfg.mass,
            position: 0.0,
            velocity: 0.0,
            target: 0.0,
            asleep: true,
        })
    }

    pub fn impulse(&mut self, target: f64, delta_velocity: f64) {
        self.target = target;
        self.velocity += delta_velocity;
        self.asleep = false;
    }

    pub fn step_euler(&mut self, raw_dt_sec: f64, sleep_epsilon: f64) -> bool {
        let eps = sleep_epsilon.max(1e-6);
        let dt = raw_dt_sec.clamp(0.0, RIG_MAX_DT_SEC);
        if dt <= 0.0 {
            let settled =
                (self.position - self.target).abs() <= eps && self.velocity.abs() <= eps;
            if settled {
                self.position = self.target;
                self.velocity = 0.0;
                self.asleep = true;
            }
            return self.asleep;
        }

        // Semi-implicit Euler sub-stepped at max 4 ms for unconditional stability on stiff springs (k=300)
        let substeps = ((dt / 0.004).ceil() as usize).max(1);
        let sub_dt = dt / (substeps as f64);
        for _ in 0..substeps {
            let disp = self.position - self.target;
            let accel =
                (-self.stiffness * disp - self.damping * self.velocity) / self.mass.max(1e-6);
            self.velocity += accel * sub_dt;
            self.position += self.velocity * sub_dt;
        }

        if (self.position - self.target).abs() <= eps && self.velocity.abs() <= eps {
            self.position = self.target;
            self.velocity = 0.0;
            self.asleep = true;
        } else {
            self.asleep = false;
        }
        self.asleep
    }

    pub fn has_nan_or_inf(&self) -> bool {
        !self.position.is_finite() || !self.velocity.is_finite() || !self.target.is_finite()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiSpringAvatarPhysics {
    pub springs: BTreeMap<String, SpringChannelState>,
    pub smoothing: V7RigSmoothingConfig,
    pub idle_cfg: V7RigIdleConfig,
    pub sleep_epsilon: f64,
    pub max_dt: f64,
    pub active_fps_target: u32,
    pub current_fps: u32,
    pub look_glow_value: f64,
    pub look_glow_target: f64,
    pub mouth_open_value: f64,
    pub mouth_open_target: f64,
    pub lean_value: f64,
    pub lean_target: f64,
}

impl MultiSpringAvatarPhysics {
    pub fn from_v7_rig(rig: &V7RigDocument) -> Result<Self, UiError> {
        if rig.physics.idle_fps != 0 {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "rig.toml physics.idle_fps must be 0, got {}",
                rig.physics.idle_fps
            )));
        }
        if !rig.physics.sleep_epsilon.is_finite()
            || rig.physics.sleep_epsilon <= 0.0
            || rig.physics.sleep_epsilon > 0.05
        {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "rig.toml physics.sleep_epsilon {} out of valid range (0.0, 0.05]",
                rig.physics.sleep_epsilon
            )));
        }
        if !rig.physics.max_dt.is_finite()
            || rig.physics.max_dt <= 0.0
            || rig.physics.max_dt > RIG_MAX_DT_SEC
        {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "rig.toml physics.max_dt {} out of valid range (0.0, {}]",
                rig.physics.max_dt, RIG_MAX_DT_SEC
            )));
        }

        let mut springs = BTreeMap::new();
        for expected_name in EXPECTED_RIG_SPRING_NAMES {
            let cfg = rig.physics.springs.get(expected_name).ok_or_else(|| {
                UiError::AvatarPackReloadRejected(format!(
                    "rig.toml missing required physics.springs.{}",
                    expected_name
                ))
            })?;
            let state = SpringChannelState::new(expected_name, cfg)?;
            springs.insert(expected_name.to_string(), state);
        }

        if springs.len() != EXPECTED_RIG_SPRING_COUNT {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "expected {} springs in rig.toml, got {}",
                EXPECTED_RIG_SPRING_COUNT,
                springs.len()
            )));
        }

        Ok(Self {
            springs,
            smoothing: rig.physics.smoothing.clone(),
            idle_cfg: rig.physics.idle.clone(),
            sleep_epsilon: rig.physics.sleep_epsilon,
            max_dt: rig.physics.max_dt,
            active_fps_target: rig.physics.active_fps.max(60),
            current_fps: 0,
            look_glow_value: 0.0,
            look_glow_target: 0.0,
            mouth_open_value: 0.0,
            mouth_open_target: 0.0,
            lean_value: 0.0,
            lean_target: 0.0,
        })
    }

    pub fn apply_mood_impulse(&mut self, mood: AvatarMood) {
        let (squash_t, jump_t, tail_t, ui_t, chase_t, acc_t, glow_t, mouth_t, lean_t) = match mood {
            AvatarMood::Idle => (0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.35, 0.0, 0.0),
            AvatarMood::Listening => (-0.02, -1.2, 0.18, 1.02, 0.14, 0.12, 0.90, 0.25, 0.08),
            AvatarMood::Thinking => (-0.03, 0.0, 0.15, 1.01, 0.18, 0.10, 0.85, 0.05, 0.10),
            AvatarMood::Talking => (0.04, -2.2, 0.32, 1.03, 0.12, 0.20, 0.80, 0.65, 0.04),
            AvatarMood::Working | AvatarMood::Focused => {
                (-0.03, 0.0, 0.12, 1.0, 0.22, 0.10, 0.95, 0.0, 0.12)
            }
            AvatarMood::Happy => (0.08, -4.5, 0.45, 1.04, 0.15, 0.25, 0.85, 0.55, 0.06),
            AvatarMood::Sleepy | AvatarMood::Sleeping => {
                (0.05, 1.5, -0.10, 0.96, 0.0, -0.05, 0.15, 0.10, -0.04)
            }
            AvatarMood::Annoyed => (0.12, -2.0, 0.65, 1.02, -0.18, 0.30, 1.0, 0.35, -0.10),
            AvatarMood::Love => (0.09, -5.0, 0.55, 1.06, 0.20, 0.35, 0.90, 0.45, 0.08),
            AvatarMood::Wink => (0.04, -1.8, 0.30, 1.02, 0.10, 0.15, 0.75, 0.20, 0.05),
            AvatarMood::Surprised => (-0.14, -7.5, 0.80, 1.08, 0.30, 0.50, 1.0, 0.85, -0.06),
            AvatarMood::Error | AvatarMood::Alert => {
                (0.10, -3.0, 0.50, 1.01, -0.22, 0.28, 1.0, 0.40, -0.12)
            }
        };

        if let Some(s) = self.springs.get_mut("squash") {
            s.impulse(squash_t, 0.65);
        }
        if let Some(s) = self.springs.get_mut("jump") {
            s.impulse(jump_t, -12.0);
        }
        if let Some(s) = self.springs.get_mut("tail") {
            s.impulse(tail_t, 2.4);
        }
        if let Some(s) = self.springs.get_mut("ui_default") {
            s.impulse(ui_t, 0.5);
        }
        if let Some(s) = self.springs.get_mut("chase") {
            s.impulse(chase_t, 1.2);
        }
        if let Some(s) = self.springs.get_mut("accessory_follow") {
            s.impulse(acc_t, 1.8);
        }

        self.look_glow_target = glow_t;
        self.mouth_open_target = mouth_t;
        self.lean_target = lean_t;
        self.current_fps = self.active_fps_target;
    }

    pub fn step(&mut self, raw_dt_sec: f64) -> bool {
        let dt = raw_dt_sec.clamp(0.0, self.max_dt);
        let eps = self.sleep_epsilon;

        let mut all_asleep = true;
        for spring in self.springs.values_mut() {
            if !spring.step_euler(dt, eps) {
                all_asleep = false;
            }
        }

        // Exponential smoothing channels: v += (target - v) * (1 - exp(-rate * dt))
        let alpha_glow = 1.0 - (-self.smoothing.look_and_glow.max(1.0) * dt).exp();
        self.look_glow_value += (self.look_glow_target - self.look_glow_value) * alpha_glow;
        if (self.look_glow_value - self.look_glow_target).abs() <= eps {
            self.look_glow_value = self.look_glow_target;
        } else {
            all_asleep = false;
        }

        let alpha_mouth = 1.0 - (-self.smoothing.mouth.max(1.0) * dt).exp();
        self.mouth_open_value += (self.mouth_open_target - self.mouth_open_value) * alpha_mouth;
        if (self.mouth_open_value - self.mouth_open_target).abs() <= eps {
            self.mouth_open_value = self.mouth_open_target;
        } else {
            all_asleep = false;
        }

        let alpha_lean = 1.0 - (-self.smoothing.lean.max(1.0) * dt).exp();
        self.lean_value += (self.lean_target - self.lean_value) * alpha_lean;
        if (self.lean_value - self.lean_target).abs() <= eps {
            self.lean_value = self.lean_target;
        } else {
            all_asleep = false;
        }

        self.current_fps = if all_asleep {
            0
        } else {
            self.active_fps_target
        };
        all_asleep
    }

    pub fn settle_to_rest(&mut self, dt_sec: f64, max_steps: u32) -> (u32, u32) {
        let mut steps = 0u32;
        while steps < max_steps {
            steps += 1;
            if self.step(dt_sec) {
                break;
            }
        }
        (steps, self.current_fps)
    }

    pub fn all_springs_asleep(&self) -> bool {
        self.current_fps == 0 && self.springs.values().all(|s| s.asleep)
    }

    pub fn is_at_rest(&self) -> bool {
        self.all_springs_asleep()
    }

    pub fn has_any_nan_or_inf(&self) -> bool {
        self.springs.values().any(|s| s.has_nan_or_inf())
            || !self.look_glow_value.is_finite()
            || !self.mouth_open_value.is_finite()
            || !self.lean_value.is_finite()
    }

    pub fn transfer_kinematic_state_from(&mut self, prev: &MultiSpringAvatarPhysics) {
        for (name, next_spring) in self.springs.iter_mut() {
            if let Some(prev_spring) = prev.springs.get(name) {
                next_spring.position = prev_spring.position;
                next_spring.velocity = prev_spring.velocity;
                next_spring.target = prev_spring.target;
                next_spring.asleep = prev_spring.asleep;
            }
        }
        self.look_glow_value = prev.look_glow_value;
        self.look_glow_target = prev.look_glow_target;
        self.mouth_open_value = prev.mouth_open_value;
        self.mouth_open_target = prev.mouth_open_target;
        self.lean_value = prev.lean_value;
        self.lean_target = prev.lean_target;
        self.current_fps = prev.current_fps;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvatarPackDescriptor {
    pub pack_id: String,
    pub display_name: String,
    pub rig_toml_raw: String,
    pub design_tokens_raw: String,
    pub svg_overrides: BTreeMap<String, String>,
}

impl AvatarPackDescriptor {
    pub fn from_directory(
        pack_id: impl Into<String>,
        display_name: impl Into<String>,
        avatar_dir: &Path,
    ) -> Result<Self, UiError> {
        let rig_path = avatar_dir.join("rig.toml");
        let tokens_path = avatar_dir.join("design-tokens.json");
        let rig_toml_raw = fs::read_to_string(&rig_path).map_err(|e| {
            UiError::AvatarPackReloadRejected(format!("failed to read {}: {}", rig_path.display(), e))
        })?;
        let design_tokens_raw = fs::read_to_string(&tokens_path).map_err(|e| {
            UiError::AvatarPackReloadRejected(format!(
                "failed to read {}: {}",
                tokens_path.display(),
                e
            ))
        })?;

        Ok(Self {
            pack_id: pack_id.into(),
            display_name: display_name.into(),
            rig_toml_raw,
            design_tokens_raw,
            svg_overrides: BTreeMap::new(),
        })
    }

    pub fn with_svg_override(mut self, relative_svg_path: impl Into<String>, svg_content: impl Into<String>) -> Self {
        self.svg_overrides
            .insert(relative_svg_path.into(), svg_content.into());
        self
    }
}

pub(crate) fn validate_single_svg_vector_asset(label: &str, content: &str) -> Result<String, UiError> {
    let byte_len = content.len();
    if byte_len == 0 || byte_len > MAX_SINGLE_SVG_ASSET_BYTES {
        return Err(UiError::AvatarPackReloadRejected(format!(
            "SVG '{}' size {} bytes outside valid range 1..={}",
            label, byte_len, MAX_SINGLE_SVG_ASSET_BYTES
        )));
    }
    let trimmed = content.trim();
    if !trimmed.contains("<svg") || !trimmed.contains("</svg>") {
        return Err(UiError::AvatarPackReloadRejected(format!(
            "SVG '{}' missing <svg>..</svg> root tags",
            label
        )));
    }
    if !trimmed.contains("viewBox=\"0 0 128 128\"") {
        return Err(UiError::AvatarPackReloadRejected(format!(
            "SVG '{}' must declare canonical viewBox=\"0 0 128 128\"",
            label
        )));
    }
    let lower = trimmed.to_ascii_lowercase();
    for forbidden in ["<script", "javascript:", "onerror=", "onload=", "<iframe", "<object"] {
        if lower.contains(forbidden) {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "SVG '{}' contains forbidden active vector '{}'",
                label, forbidden
            )));
        }
    }
    Ok(sha256_hex_bytes(trimmed.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarPackSnapshot {
    pub pack_id: String,
    pub display_name: String,
    pub rig: V7RigDocument,
    pub tokens: V7DesignTokens,
    pub verified_svg_hashes: BTreeMap<String, String>,
    pub pack_sha256: String,
    pub generation: u64,
}

