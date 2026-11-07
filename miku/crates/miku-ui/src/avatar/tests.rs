#![allow(unused_imports)]
use crate::prelude::*;

    // LEDGER-2.5-1
    #[test]
    fn test_production_avatar_multi_spring_euler_physics_converges_to_zero_fps_under_sleep_epsilon() {
        let avatar_dir = resolve_avatar_assets_dir();
        let desc = AvatarPackDescriptor::from_directory(
            "default-miku-v7",
            "Miku Default Procedural Pack v7",
            &avatar_dir,
        )
        .expect("load default avatar pack descriptor");
        let snap = AvatarPackSnapshot::compile_and_validate(&desc, &avatar_dir, 1)
            .expect("compile default avatar pack snapshot");

        let mut physics =
            MultiSpringAvatarPhysics::from_v7_rig(&snap.rig).expect("init 6-spring physics");
        assert_eq!(physics.springs.len(), EXPECTED_RIG_SPRING_COUNT);
        assert!((physics.sleep_epsilon - RIG_SLEEP_EPSILON_DEFAULT).abs() < 1e-9);
        assert_eq!(physics.current_fps, 0);
        assert!(physics.all_springs_asleep());

        // Verify exact spring parameters from rig.toml v7
        let squash = physics.springs.get("squash").unwrap();
        assert!((squash.stiffness - 190.0).abs() < 1e-6);
        assert!((squash.damping - 9.0).abs() < 1e-6);
        let ui_default = physics.springs.get("ui_default").unwrap();
        assert!((ui_default.stiffness - 300.0).abs() < 1e-6);
        assert!((ui_default.damping - 30.0).abs() < 1e-6);

        // Apply impulses across all 8 moods and verify each converges to 0 fps with 0 NaN/Inf
        for mood in [
            AvatarMood::Idle,
            AvatarMood::Happy,
            AvatarMood::Focused,
            AvatarMood::Sleepy,
            AvatarMood::Annoyed,
            AvatarMood::Love,
            AvatarMood::Wink,
            AvatarMood::Surprised,
        ] {
            physics.apply_mood_impulse(mood);
            assert_eq!(physics.current_fps, 60);
            assert!(!physics.all_springs_asleep());

            // Also test dt clamping (passing 0.25s clamps to max_dt = 0.04s without exploding)
            let _ = physics.step(0.25);
            assert!(!physics.has_any_nan_or_inf());

            let (steps, settled_fps) = physics.settle_to_rest(1.0 / 60.0, 600);
            assert!(
                steps > 0 && steps < 600,
                "mood {:?} failed to settle within 600 steps (took {})",
                mood,
                steps
            );
            assert_eq!(settled_fps, 0);
            assert!(physics.all_springs_asleep());
            assert!(!physics.has_any_nan_or_inf());
            for sp in physics.springs.values() {
                assert_eq!(sp.velocity, 0.0);
                assert!((sp.position - sp.target).abs() <= 1e-9);
            }
        }
    }

    // LEDGER-2.5-2
    #[test]
    fn test_decoupled_avatar_pack_loader_validates_custom_rig_skins_and_svg_vectors() {
        let avatar_dir = resolve_avatar_assets_dir();
        let base_desc = AvatarPackDescriptor::from_directory(
            "default-miku-v7",
            "Miku Default Procedural Pack v7",
            &avatar_dir,
        )
        .expect("load base descriptor");
        let base_snap =
            AvatarPackSnapshot::compile_and_validate(&base_desc, &avatar_dir, 1).unwrap();
        assert_eq!(
            base_snap.verified_svg_hashes.len(),
            EXPECTED_AVATAR_SVG_COUNT
        );

        // Create a decoupled custom Avatar Pack with custom spring stiffness, custom skin palette, and custom vector SVG override
        let custom_rig = base_desc
            .rig_toml_raw
            .replace("stiffness = 190.0", "stiffness = 225.0");
        let custom_tokens = base_desc
            .design_tokens_raw
            .replace("#39C5BB", "#00E5FF");
        let custom_body_svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128"><circle cx="64" cy="64" r="48" fill="#00E5FF"/></svg>"##;

        let custom_desc = AvatarPackDescriptor {
            pack_id: "cyber-neon-miku-v1".to_string(),
            display_name: "Cyber Neon Miku Pack".to_string(),
            rig_toml_raw: custom_rig,
            design_tokens_raw: custom_tokens,
            svg_overrides: BTreeMap::new(),
        }
        .with_svg_override("body.svg", custom_body_svg);

        let custom_snap = AvatarPackSnapshot::compile_and_validate(&custom_desc, &avatar_dir, 2)
            .expect("compile decoupled custom avatar pack");

        assert_eq!(custom_snap.pack_id, "cyber-neon-miku-v1");
        assert_eq!(custom_snap.generation, 2);
        assert_ne!(custom_snap.pack_sha256, base_snap.pack_sha256);
        assert_ne!(
            custom_snap.verified_svg_hashes.get("body.svg"),
            base_snap.verified_svg_hashes.get("body.svg")
        );
        assert_eq!(
            custom_snap
                .rig
                .physics
                .springs
                .get("squash")
                .unwrap()
                .stiffness,
            225.0
        );
    }

    // LEDGER-2.5-3
    #[test]
    fn test_hot_reload_detects_rig_changes_atomically_under_50ms_without_visual_flicker_or_memory_leak() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("init hot reloader");
        let base_desc = AvatarPackDescriptor::from_directory(
            "default-miku-v7",
            "Miku Default Procedural Pack v7",
            &avatar_dir,
        )
        .unwrap();

        // Set mood to Happy and step a few frames so springs are in mid-flight (non-zero position/velocity & breath phase)
        reloader
            .runtime_mut()
            .set_mood(&avatar_dir, AvatarMood::Happy)
            .unwrap();
        for _ in 0..5 {
            reloader.runtime_mut().step_frame(1.0 / 60.0);
        }
        let mid_flight_squash_pos = reloader
            .runtime()
            .physics
            .springs
            .get("squash")
            .unwrap()
            .position;
        let mid_flight_breath = reloader.runtime().breath_phase_rad;
        assert!(mid_flight_squash_pos.abs() > 0.0);
        assert!(mid_flight_breath > 0.0);

        // Execute 50 consecutive atomic hot-reloads and verify < 50.0 ms latency, state continuity (zero flicker), and bounded collections
        for i in 1..=50usize {
            let mut next_desc = base_desc.clone();
            next_desc.pack_id = format!("hot-reload-pack-{}", i);
            let next_k = 180.0 + (i as f64);
            next_desc.rig_toml_raw = base_desc
                .rig_toml_raw
                .replace("stiffness = 190.0", &format!("stiffness = {:.1}", next_k));

            let metrics = reloader
                .try_hot_reload_descriptor(&next_desc)
                .expect("atomic hot reload");
            assert!(metrics.reloaded);
            assert_eq!(metrics.generation, (i + 1) as u64);
            assert_eq!(metrics.preserved_mood, AvatarMood::Happy);
            assert!((metrics.preserved_breath_phase_rad - mid_flight_breath).abs() < 1e-9);
            assert_eq!(metrics.spring_count, EXPECTED_RIG_SPRING_COUNT);

            let limit_ms = if cfg!(debug_assertions) {
                250.0
            } else {
                MAX_AVATAR_HOT_RELOAD_LATENCY_MS
            };
            assert!(
                metrics.elapsed_ms < limit_ms,
                "hot reload #{} took {:.4} ms (limit {:.1} ms)",
                i,
                metrics.elapsed_ms,
                limit_ms
            );
        }

        // Kinematic continuity check: squash position was preserved across all 50 hot-reloads without resetting to 0!
        let post_reload_squash_pos = reloader
            .runtime()
            .physics
            .springs
            .get("squash")
            .unwrap()
            .position;
        assert!((post_reload_squash_pos - mid_flight_squash_pos).abs() < 1e-9);
        assert_eq!(reloader.reload_commits(), 50);
        assert_eq!(reloader.reload_rollbacks(), 0);
        assert_eq!(reloader.runtime().layer_stack.len(), 9);
        assert_eq!(reloader.runtime().accessory_transforms.len(), 14);
        assert_eq!(
            reloader.runtime().active_pack.verified_svg_hashes.len(),
            EXPECTED_AVATAR_SVG_COUNT
        );
    }

    // LEDGER-2.5-4
    #[test]
    fn test_corrupted_rig_or_invalid_svg_pack_rolls_back_to_active_baseline_without_panic() {
        let avatar_dir = resolve_avatar_assets_dir();
        let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("init hot reloader");
        let base_desc = AvatarPackDescriptor::from_directory(
            "default-miku-v7",
            "Miku Default Procedural Pack v7",
            &avatar_dir,
        )
        .unwrap();

        let initial_sha = reloader.runtime().active_pack.pack_sha256.clone();
        let initial_gen = reloader.runtime().active_pack.generation;

        // 1. Broken TOML syntax
        let mut broken_toml = base_desc.clone();
        broken_toml.rig_toml_raw = "[meta\nversion = ???".to_string();
        assert!(matches!(
            reloader.try_hot_reload_descriptor(&broken_toml),
            Err(UiError::AvatarPackReloadRejected(_))
        ));

        // 2. Negative spring stiffness (k <= 0)
        let mut negative_k = base_desc.clone();
        negative_k.rig_toml_raw = base_desc
            .rig_toml_raw
            .replace("stiffness = 190.0", "stiffness = -50.0");
        assert!(matches!(
            reloader.try_hot_reload_descriptor(&negative_k),
            Err(UiError::AvatarPackReloadRejected(_))
        ));

        // 3. Invalid viewBox != [128, 128]
        let mut bad_viewbox = base_desc.clone();
        bad_viewbox.rig_toml_raw = base_desc
            .rig_toml_raw
            .replace("[128, 128]", "[256, 256]");
        assert_ne!(bad_viewbox.rig_toml_raw, base_desc.rig_toml_raw);
        assert!(matches!(
            reloader.try_hot_reload_descriptor(&bad_viewbox),
            Err(UiError::AvatarPackReloadRejected(_))
        ));

        // 4. Non-zero idle_fps
        let mut bad_idle_fps = base_desc.clone();
        bad_idle_fps.rig_toml_raw = base_desc
            .rig_toml_raw
            .replace("idle_fps      = 0", "idle_fps      = 30")
            .replace("idle_fps = 0", "idle_fps = 30");
        assert_ne!(bad_idle_fps.rig_toml_raw, base_desc.rig_toml_raw);
        assert!(matches!(
            reloader.try_hot_reload_descriptor(&bad_idle_fps),
            Err(UiError::AvatarPackReloadRejected(_))
        ));

        // 5. Malicious <script> inside SVG override
        let xss_svg = base_desc.clone().with_svg_override(
            "body.svg",
            "<svg viewBox=\"0 0 128 128\"><script>evil()</script></svg>",
        );
        assert!(matches!(
            reloader.try_hot_reload_descriptor(&xss_svg),
            Err(UiError::AvatarPackReloadRejected(_))
        ));

        // 6. Oversized SVG (> 64 KB)
        let huge_svg_str = format!(
            "<svg viewBox=\"0 0 128 128\"><!-- {} --></svg>",
            "A".repeat(MAX_SINGLE_SVG_ASSET_BYTES + 100)
        );
        let huge_svg = base_desc
            .clone()
            .with_svg_override("ears.svg", huge_svg_str);
        assert!(matches!(
            reloader.try_hot_reload_descriptor(&huge_svg),
            Err(UiError::AvatarPackReloadRejected(_))
        ));

        // Verify active baseline remained 100% intact across all 6 hostile/corrupted reload attempts
        assert_eq!(reloader.reload_commits(), 0);
        assert_eq!(reloader.reload_rollbacks(), 6);
        assert_eq!(reloader.runtime().active_pack.pack_sha256, initial_sha);
        assert_eq!(reloader.runtime().active_pack.generation, initial_gen);
    }

