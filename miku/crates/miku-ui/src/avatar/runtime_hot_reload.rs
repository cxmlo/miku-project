#![allow(unused_imports)]
use crate::prelude::*;

impl AvatarPackSnapshot {
    pub fn compile_and_validate(
        descriptor: &AvatarPackDescriptor,
        base_avatar_dir: &Path,
        generation: u64,
    ) -> Result<Self, UiError> {
        if descriptor.pack_id.trim().is_empty() {
            return Err(UiError::AvatarPackReloadRejected(
                "avatar pack_id cannot be empty".to_string(),
            ));
        }

        let rig: V7RigDocument = toml::from_str(&descriptor.rig_toml_raw).map_err(|e| {
            UiError::AvatarPackReloadRejected(format!("invalid rig.toml syntax or schema: {}", e))
        })?;
        if rig.meta.version != 1 {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "unsupported rig.toml meta.version {} (expected 1)",
                rig.meta.version
            )));
        }
        if rig.meta.viewbox != AVATAR_VIEWBOX_SIZE {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "invalid rig.toml viewbox {:?} (expected [128, 128])",
                rig.meta.viewbox
            )));
        }
        if rig.layers.len() != 9 {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "expected 9 layers in rig.toml, got {}",
                rig.layers.len()
            )));
        }
        if rig.accessories.items.len() != 14 {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "expected 14 accessories in rig.toml, got {}",
                rig.accessories.items.len()
            )));
        }

        // Validate physics and all 6 springs via MultiSpringAvatarPhysics constructor
        let _ = MultiSpringAvatarPhysics::from_v7_rig(&rig)?;

        let tokens: V7DesignTokens =
            serde_json::from_str(&descriptor.design_tokens_raw).map_err(|e| {
                UiError::AvatarPackReloadRejected(format!(
                    "invalid design-tokens.json syntax or schema: {}",
                    e
                ))
            })?;
        if tokens.avatar.viewbox != AVATAR_VIEWBOX_SIZE {
            return Err(UiError::AvatarPackReloadRejected(format!(
                "invalid design-tokens.json viewbox {:?}",
                tokens.avatar.viewbox
            )));
        }

        // Collect all 22 required SVG relative paths (normalized without leading `avatar/`)
        let mut required_svgs = vec![
            "body.svg".to_string(),
            "ears.svg".to_string(),
            "expressions/eyes_open.svg".to_string(),
            "expressions/eyes_blink.svg".to_string(),
            "expressions/mouth_idle.svg".to_string(),
            "expressions/mouth_talk.svg".to_string(),
        ];
        for item in rig.accessories.items.values() {
            let rel = item.svg.strip_prefix("avatar/").unwrap_or(&item.svg);
            required_svgs.push(rel.to_string());
            if let Some(back) = &item.svg_back {
                let rel_back = back.strip_prefix("avatar/").unwrap_or(back);
                required_svgs.push(rel_back.to_string());
            }
        }
        required_svgs.sort();
        required_svgs.dedup();

        let mut verified_svg_hashes = BTreeMap::new();
        for rel_path in &required_svgs {
            let svg_text = if let Some(override_svg) = descriptor.svg_overrides.get(rel_path) {
                override_svg.clone()
            } else {
                let disk_path = base_avatar_dir.join(rel_path);
                fs::read_to_string(&disk_path).map_err(|e| {
                    UiError::AvatarPackReloadRejected(format!(
                        "missing SVG asset '{}': {}",
                        disk_path.display(),
                        e
                    ))
                })?
            };
            let svg_hash = validate_single_svg_vector_asset(rel_path, &svg_text)?;
            verified_svg_hashes.insert(rel_path.clone(), svg_hash);
        }

        // Also validate any additional custom SVG overrides provided in the pack
        for (custom_rel, custom_svg) in &descriptor.svg_overrides {
            if !verified_svg_hashes.contains_key(custom_rel) {
                let svg_hash = validate_single_svg_vector_asset(custom_rel, custom_svg)?;
                verified_svg_hashes.insert(custom_rel.clone(), svg_hash);
            }
        }

        let mut hash_material = String::new();
        hash_material.push_str(&descriptor.pack_id);
        hash_material.push('|');
        hash_material.push_str(&descriptor.rig_toml_raw);
        hash_material.push('|');
        hash_material.push_str(&descriptor.design_tokens_raw);
        for (k, v) in &verified_svg_hashes {
            hash_material.push('|');
            hash_material.push_str(k);
            hash_material.push(':');
            hash_material.push_str(v);
        }
        let pack_sha256 = sha256_hex_bytes(hash_material.as_bytes());

        Ok(Self {
            pack_id: descriptor.pack_id.clone(),
            display_name: descriptor.display_name.clone(),
            rig,
            tokens,
            verified_svg_hashes,
            pack_sha256,
            generation,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductionAvatarRuntime {
    pub active_pack: AvatarPackSnapshot,
    pub physics: MultiSpringAvatarPhysics,
    pub current_mood: AvatarMood,
    pub equipped_accessories: Vec<String>,
    pub breath_phase_rad: f64,
    pub layer_stack: Vec<AvatarLayerNode>,
    pub accessory_transforms: BTreeMap<String, AccessorySlotTransform>,
}

impl ProductionAvatarRuntime {
    pub fn from_snapshot(
        snapshot: AvatarPackSnapshot,
        _base_avatar_dir: &Path,
        mood: AvatarMood,
        equipped_accessories: Vec<String>,
        breath_phase_rad: f64,
    ) -> Result<Self, UiError> {
        let physics = MultiSpringAvatarPhysics::from_v7_rig(&snapshot.rig)?;
        let eq_refs: Vec<&str> = equipped_accessories.iter().map(|s| s.as_str()).collect();
        let (layer_stack, accessory_transforms) =
            compose_avatar_layer_stack_from_rig(&snapshot.rig, mood, &eq_refs)?;
        Ok(Self {
            active_pack: snapshot,
            physics,
            current_mood: mood,
            equipped_accessories,
            breath_phase_rad,
            layer_stack,
            accessory_transforms,
        })
    }

    pub fn set_mood(&mut self, _base_avatar_dir: &Path, mood: AvatarMood) -> Result<(), UiError> {
        self.current_mood = mood;
        self.physics.apply_mood_impulse(mood);
        let eq_refs: Vec<&str> = self
            .equipped_accessories
            .iter()
            .map(|s| s.as_str())
            .collect();
        let (layers, transforms) =
            compose_avatar_layer_stack_from_rig(&self.active_pack.rig, mood, &eq_refs)?;
        self.layer_stack = layers;
        self.accessory_transforms = transforms;
        Ok(())
    }

    pub fn step_frame(&mut self, raw_dt_sec: f64) -> bool {
        let dt = raw_dt_sec.clamp(0.0, self.physics.max_dt);
        let omega = if self.current_mood == AvatarMood::Sleepy {
            self.physics.idle_cfg.breath_omega_sleeping
        } else {
            self.physics.idle_cfg.breath_omega
        };
        if !self.physics.all_springs_asleep() {
            self.breath_phase_rad =
                (self.breath_phase_rad + omega * dt) % (std::f64::consts::PI * 2.0);
        }
        self.physics.step(dt)
    }

    pub fn step_to_rest(&mut self, dt: f64, max_steps: usize) -> (usize, u32) {
        let steps_u32 = u32::try_from(max_steps).unwrap_or(u32::MAX);
        let (steps_taken, fps) = self.physics.settle_to_rest(dt, steps_u32);
        (steps_taken as usize, fps)
    }

    pub fn active_fps(&self) -> u32 {
        self.physics.current_fps
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotReloadMetrics {
    pub reloaded: bool,
    pub generation: u64,
    pub pack_sha256: String,
    pub elapsed_ms: f64,
    pub preserved_mood: AvatarMood,
    pub preserved_breath_phase_rad: f64,
    pub spring_count: usize,
}

#[derive(Debug, Clone)]
pub struct AvatarPackHotReloader {
    pub(crate) base_avatar_dir: PathBuf,
    pub(crate) runtime: ProductionAvatarRuntime,
    pub(crate) reload_commits: u64,
    pub(crate) reload_rollbacks: u64,
    pub(crate) noop_checks: u64,
    pub(crate) last_reload_latency_ms: f64,
}

impl AvatarPackHotReloader {
    pub fn new(avatar_dir: &Path) -> Result<Self, UiError> {
        let descriptor = AvatarPackDescriptor::from_directory(
            "default-miku-v7",
            "Miku Default Procedural Pack v7",
            avatar_dir,
        )?;
        let snapshot = AvatarPackSnapshot::compile_and_validate(&descriptor, avatar_dir, 1)?;
        let runtime = ProductionAvatarRuntime::from_snapshot(
            snapshot,
            avatar_dir,
            AvatarMood::Idle,
            vec!["crown".to_string(), "scarf".to_string()],
            0.0,
        )?;
        Ok(Self {
            base_avatar_dir: avatar_dir.to_path_buf(),
            runtime,
            reload_commits: 0,
            reload_rollbacks: 0,
            noop_checks: 0,
            last_reload_latency_ms: 0.0,
        })
    }

    pub fn runtime(&self) -> &ProductionAvatarRuntime {
        &self.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut ProductionAvatarRuntime {
        &mut self.runtime
    }

    pub fn reload_commits(&self) -> u64 {
        self.reload_commits
    }

    pub fn reload_rollbacks(&self) -> u64 {
        self.reload_rollbacks
    }

    pub fn noop_checks(&self) -> u64 {
        self.noop_checks
    }

    pub fn last_reload_latency_ms(&self) -> f64 {
        self.last_reload_latency_ms
    }

    pub fn try_hot_reload_descriptor(
        &mut self,
        candidate: &AvatarPackDescriptor,
    ) -> Result<HotReloadMetrics, UiError> {
        let t0 = Instant::now();
        let next_gen = self.runtime.active_pack.generation + 1;

        // 1. Compile and validate in an isolated Shadow Buffer BEFORE mutating live state
        let shadow_snapshot = match AvatarPackSnapshot::compile_and_validate(
            candidate,
            &self.base_avatar_dir,
            next_gen,
        ) {
            Ok(snap) => snap,
            Err(err) => {
                self.reload_rollbacks += 1;
                return Err(err);
            }
        };

        // 2. Fast-path deduplication if SHA-256 matches the currently active pack
        if shadow_snapshot.pack_sha256 == self.runtime.active_pack.pack_sha256 {
            self.noop_checks += 1;
            let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
            return Ok(HotReloadMetrics {
                reloaded: false,
                generation: self.runtime.active_pack.generation,
                pack_sha256: self.runtime.active_pack.pack_sha256.clone(),
                elapsed_ms,
                preserved_mood: self.runtime.current_mood,
                preserved_breath_phase_rad: self.runtime.breath_phase_rad,
                spring_count: self.runtime.physics.springs.len(),
            });
        }

        // 3. Build shadow runtime preserving kinematic state (x, v, mood, accessories, breath_phase_rad)
        let preserved_mood = self.runtime.current_mood;
        let preserved_accessories = self.runtime.equipped_accessories.clone();
        let preserved_breath = self.runtime.breath_phase_rad;

        let mut shadow_runtime = match ProductionAvatarRuntime::from_snapshot(
            shadow_snapshot,
            &self.base_avatar_dir,
            preserved_mood,
            preserved_accessories,
            preserved_breath,
        ) {
            Ok(rt) => rt,
            Err(err) => {
                self.reload_rollbacks += 1;
                return Err(err);
            }
        };
        shadow_runtime
            .physics
            .transfer_kinematic_state_from(&self.runtime.physics);

        // 4. Atomic swap
        self.runtime = shadow_runtime;
        self.reload_commits += 1;
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.last_reload_latency_ms = elapsed_ms;

        Ok(HotReloadMetrics {
            reloaded: true,
            generation: self.runtime.active_pack.generation,
            pack_sha256: self.runtime.active_pack.pack_sha256.clone(),
            elapsed_ms,
            preserved_mood,
            preserved_breath_phase_rad: preserved_breath,
            spring_count: self.runtime.physics.springs.len(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarPackAuditReport {
    pub spring_count: usize,
    pub moods_converged: usize,
    pub reload_commits: u64,
    pub reload_rollbacks: u64,
    pub max_reload_latency_ms: f64,
    pub settled_fps: f64,
    pub verified_svg_count: usize,
    pub active_pack_sha256: String,
}

pub fn run_avatar_pack_hot_reload_audit(
    avatar_dir: &Path,
    reload_cycles: usize,
) -> Result<AvatarPackAuditReport, UiError> {
    let mut reloader = AvatarPackHotReloader::new(avatar_dir)?;
    let base_desc = AvatarPackDescriptor::from_directory(
        "default-miku-v7",
        "Miku Default Procedural Pack v7",
        avatar_dir,
    )?;

    // 1. Verify all 8 moods converge to 0 fps without NaN/Inf
    let all_moods = [
        AvatarMood::Idle,
        AvatarMood::Happy,
        AvatarMood::Focused,
        AvatarMood::Sleepy,
        AvatarMood::Annoyed,
        AvatarMood::Love,
        AvatarMood::Wink,
        AvatarMood::Surprised,
    ];
    let mut moods_converged = 0usize;
    for mood in all_moods {
        reloader.runtime_mut().set_mood(avatar_dir, mood)?;
        let (_steps, fps) = reloader.runtime_mut().physics.settle_to_rest(1.0 / 60.0, 600);
        if fps == 0 && !reloader.runtime().physics.has_any_nan_or_inf() {
            moods_converged += 1;
        }
    }

    // 2. Run `reload_cycles` valid hot-reloads + 2 corrupted pack rollbacks
    let mut max_reload_latency_ms = 0.0f64;
    for idx in 0..reload_cycles {
        let mut custom_desc = base_desc.clone();
        custom_desc.pack_id = format!("custom-pack-{}", idx);
        custom_desc.display_name = format!("Custom Miku Skin #{}", idx);
        let new_stiffness = 190.0 + ((idx % 7) as f64) * 5.0;
        custom_desc.rig_toml_raw = base_desc.rig_toml_raw.replace(
            "stiffness = 190.0",
            &format!("stiffness = {:.1}", new_stiffness),
        );

        let metrics = reloader.try_hot_reload_descriptor(&custom_desc)?;
        if metrics.elapsed_ms > max_reload_latency_ms {
            max_reload_latency_ms = metrics.elapsed_ms;
        }
    }

    // 3. Inject 2 corrupted packs (broken TOML and XSS SVG) and verify clean rollback
    let mut bad_toml = base_desc.clone();
    bad_toml.pack_id = "corrupted-toml".to_string();
    bad_toml.rig_toml_raw = "[meta\nversion = \"broken\"".to_string();
    let _ = reloader.try_hot_reload_descriptor(&bad_toml);

    let mut bad_svg = base_desc.clone();
    bad_svg.pack_id = "corrupted-svg".to_string();
    bad_svg = bad_svg.with_svg_override(
        "body.svg",
        "<svg viewBox=\"0 0 128 128\"><script>alert(1)</script></svg>",
    );
    let _ = reloader.try_hot_reload_descriptor(&bad_svg);

    let (_steps, final_fps) = reloader.runtime_mut().physics.settle_to_rest(1.0 / 60.0, 600);

    Ok(AvatarPackAuditReport {
        spring_count: reloader.runtime().physics.springs.len(),
        moods_converged,
        reload_commits: reloader.reload_commits(),
        reload_rollbacks: reloader.reload_rollbacks(),
        max_reload_latency_ms,
        settled_fps: final_fps as f64,
        verified_svg_count: reloader.runtime().active_pack.verified_svg_hashes.len(),
        active_pack_sha256: reloader.runtime().active_pack.pack_sha256.clone(),
    })
}

