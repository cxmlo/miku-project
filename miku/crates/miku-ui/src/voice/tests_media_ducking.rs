use crate::prelude::*;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn avatar_assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/avatar")
}

fn temp_test_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "miku_voice_4_2_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    let _ = fs::create_dir_all(&dir);
    dir
}

fn build_voice_gatekeeper_allow_all() -> GatekeeperEngine {
    let mut profile = ProjectProfile::new("voice-4-2-profile", ".");
    profile.allow_rules.insert(
        Capability::Input,
        vec![
            "microphone:*".to_string(),
            "media:transport_control".to_string(),
        ],
    );
    profile.allow_rules.insert(
        Capability::Network,
        vec![
            "api.groq.com*".to_string(),
            "127.0.0.1*".to_string(),
            "groq_stt_key".to_string(),
        ],
    );
    GatekeeperEngine::new(profile, ".", 60).expect("gatekeeper init")
}

struct MockGroqOkTransport;

impl GroqSttTransport for MockGroqOkTransport {
    fn transcribe_pcm16_mono(
        &self,
        config: &GroqSttConfig,
        _api_key: &ZeroizingBytes,
        pcm_samples: &[i16],
        sample_rate_hz: u32,
    ) -> Result<GroqSttTranscript, SttEngineError> {
        let billing = GroqBillingEstimate::from_pcm_samples(pcm_samples.len(), sample_rate_hz);
        Ok(GroqSttTranscript {
            text: "reproduce siguiente pista y corre cargo test".to_string(),
            language: config.language_hint.unwrap_or(VoiceSttLanguage::Es),
            model_used: config.model.as_str().to_string(),
            audio_duration_ms: billing.actual_duration_ms,
            billing,
        })
    }
}

#[test]
fn test_audio_ducking_attenuates_to_20_percent_and_restores_exact_original_volume() {
    let backend = SimulatedAudioMediaBackend::new(0.85, false);
    let initial_snapshot = backend.current_snapshot();
    let mut controller = AudioDuckingController::new(Box::new(backend));

    assert_eq!(*controller.state(), AudioDuckingState::Normal);
    assert!(!controller.is_ducked());

    let duck_receipt = controller.engage_ducking(1_000).expect("engage ducking");
    assert!(!duck_receipt.was_already_ducked);
    assert_eq!(duck_receipt.original_snapshot, initial_snapshot);
    assert!((duck_receipt.ducked_volume - DEFAULT_AUDIO_DUCKING_TARGET_LEVEL).abs() < 1e-6);
    assert!(controller.is_ducked());

    let active_snap = controller.current_backend_snapshot().expect("ducked snap");
    assert!((active_snap.volume_scalar - 0.20).abs() < 1e-4);

    let restore_receipt = controller
        .restore_original_volume(1_500)
        .expect("restore")
        .expect("was ducked");
    assert_eq!(restore_receipt.restored_snapshot, initial_snapshot);
    assert_eq!(restore_receipt.duck_duration_ms, 500);
    assert!(!controller.is_ducked());

    let final_snap = controller.current_backend_snapshot().expect("restored snap");
    assert_eq!(final_snap, initial_snapshot);
    assert!((final_snap.volume_scalar - 0.85).abs() < 1e-6);

    // Volume already <= 20% (12%) must NEVER be raised to 20%, and must restore to 12%
    let low_backend = SimulatedAudioMediaBackend::new(0.12, false);
    let low_initial = low_backend.current_snapshot();
    let mut low_controller = AudioDuckingController::new(Box::new(low_backend));
    let low_duck = low_controller.engage_ducking(2_000).expect("duck low");
    assert!((low_duck.ducked_volume - 0.12).abs() < 1e-6);
    assert_eq!(low_controller.current_backend_snapshot().unwrap(), low_initial);
    let low_restored = low_controller.restore_original_volume(2_200).unwrap().unwrap();
    assert_eq!(low_restored.restored_snapshot, low_initial);

    // Muted audio must remain muted during ducking and restore muted state
    let muted_backend = SimulatedAudioMediaBackend::new(0.75, true);
    let muted_initial = muted_backend.current_snapshot();
    let mut muted_controller = AudioDuckingController::new(Box::new(muted_backend));
    let _ = muted_controller.engage_ducking(3_000).expect("duck muted");
    assert!(muted_controller.current_backend_snapshot().unwrap().muted);
    let muted_restored = muted_controller.restore_original_volume(3_100).unwrap().unwrap();
    assert_eq!(muted_restored.restored_snapshot, muted_initial);
}

#[test]
fn test_audio_ducking_restores_exact_volume_on_capture_error_escape_early_release_and_kill_switch()
{
    let temp = temp_test_dir("error_paths");
    let mut gk = build_voice_gatekeeper_allow_all();
    let mut avatar = ProductionAvatarRuntime::load_from_dir(avatar_assets_dir()).expect("avatar");
    let mut bridge = WebView2IpcBridge::new("miku-voice-4-2-token");

    let mut cfg = GroqSttConfig::default_with_consent(true);
    cfg.endpoint_url = "http://127.0.0.1:9/openai/v1/audio/transcriptions".to_string();

    let mut secrets = InMemoryVoiceSecretAliasStore::new();
    secrets.insert_alias("groq_stt_key", b"gsk_test_dummy_secret_value_4_2".to_vec());

    let original_snap = SystemAudioSnapshot::from_scalar(0.78, false);
    let duck_controller =
        AudioDuckingController::new(Box::new(SimulatedAudioMediaBackend::new(0.78, false)));

    let mut orch = PttSttOrchestrator::with_transport(
        avatar_assets_dir(),
        cfg,
        WasapiCaptureDevice::new_loopback(),
        Box::new(secrets),
        Box::new(MockGroqOkTransport),
    )
    .expect("orchestrator")
    .with_ducking_controller(duck_controller);

    // Path A: open_capture_stream failure leaves volume untouched at 78%
    orch.capture_device_mut().fail_on_open = true;
    let err = orch
        .press_ptt_and_open_stream(PTT_DEFAULT_HOTKEY, 1_000, 0x10, 0x10, 0x99, &mut gk, &mut avatar)
        .expect_err("open stream should fail");
    assert!(matches!(err, SttEngineError::UiFailure(_)));
    assert!(!orch.ducking().is_ducked());
    assert_eq!(orch.ducking().current_backend_snapshot().unwrap(), original_snap);
    orch.capture_device_mut().fail_on_open = false;

    // Path B: read_captured_pcm_chunk error mid-recording restores 78% immediately
    orch.press_ptt_and_open_stream(PTT_DEFAULT_HOTKEY, 2_000, 0x10, 0x10, 0x99, &mut gk, &mut avatar)
        .expect("open ok");
    assert!(orch.ducking().is_ducked());
    assert!((orch.ducking().current_backend_snapshot().unwrap().volume_scalar - 0.20).abs() < 1e-4);

    orch.capture_device_mut().fail_on_read = true;
    let read_err = orch
        .pump_ptt_capture_chunk(320, 2_100, &mut avatar, Some(&mut bridge))
        .expect_err("read failure");
    assert!(matches!(read_err, SttEngineError::UiFailure(_)));
    assert!(!orch.ducking().is_ducked());
    assert_eq!(orch.ducking().current_backend_snapshot().unwrap(), original_snap);
    orch.capture_device_mut().fail_on_read = false;

    // Path C: Cooperative cancellation (`Escape`) restores 78% immediately
    orch.press_ptt_and_open_stream(PTT_DEFAULT_HOTKEY, 3_000, 0x10, 0x10, 0x99, &mut gk, &mut avatar)
        .expect("open ok");
    assert!(orch.ducking().is_ducked());
    let _ = orch
        .abort_and_cleanup(
            VoiceCancellationReason::UserPressedEscape,
            3_200,
            &mut avatar,
            Some(&mut bridge),
        )
        .expect("escape abort");
    assert!(!orch.ducking().is_ducked());
    assert_eq!(orch.ducking().current_backend_snapshot().unwrap(), original_snap);

    // Path D: Early release (< 120 ms debounce) restores 78%
    orch.press_ptt_and_open_stream(PTT_DEFAULT_HOTKEY, 4_000, 0x10, 0x10, 0x99, &mut gk, &mut avatar)
        .expect("open ok");
    assert!(orch.ducking().is_ducked());
    let early = orch
        .release_ptt_and_transcribe_to_session("sess-4-2", 4_050, &mut gk, &mut avatar, &mut bridge)
        .expect("early release outcome");
    assert!(matches!(early, PttPipelineOutcome::Cancelled { .. }));
    assert!(!orch.ducking().is_ducked());
    assert_eq!(orch.ducking().current_backend_snapshot().unwrap(), original_snap);

    // Path E: Global Kill Switch (`Ctrl+Alt+Shift+K`) mid-recording restores 78% immediately
    orch.press_ptt_and_open_stream(PTT_DEFAULT_HOTKEY, 5_000, 0x10, 0x10, 0x99, &mut gk, &mut avatar)
        .expect("open ok");
    assert!(orch.ducking().is_ducked());
    let mut ks = UiKillSwitchManager::new();
    let mut audit_store =
        PersistentJsonlAuditStore::new(temp.join("audit.jsonl"), 64 * 1024).expect("audit store");
    let _ = orch
        .trip_kill_switch_and_purge(
            &mut ks,
            5_150,
            "emergency kill switch during ducking",
            &mut gk,
            &mut avatar,
            &mut audit_store,
            &mut bridge,
        )
        .expect("kill switch trip");
    assert!(!orch.ducking().is_ducked());
    assert_eq!(orch.ducking().current_backend_snapshot().unwrap(), original_snap);

    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_idempotent_double_duck_and_drop_guard_crash_recovery_never_leave_audio_attenuated() {
    let temp = temp_test_dir("recovery");
    let recovery_path = temp.join("ducking_recovery.json");
    let initial_92 = SystemAudioSnapshot::from_scalar(0.92, false);

    {
        let mut ctrl =
            AudioDuckingController::new(Box::new(SimulatedAudioMediaBackend::new(0.92, false)))
                .with_recovery_file(recovery_path.clone());

        let first = ctrl.engage_ducking(1_000).expect("first duck");
        assert!(!first.was_already_ducked);
        assert_eq!(first.original_snapshot, initial_92);
        assert!(recovery_path.exists());

        let second = ctrl.engage_ducking(1_100).expect("second duck idempotent");
        assert!(second.was_already_ducked);
        assert_eq!(second.original_snapshot, initial_92);
        assert_eq!(ctrl.duck_transitions_count(), 1);

        let restored = ctrl.restore_original_volume(1_500).expect("restore").expect("receipt");
        assert_eq!(restored.restored_snapshot, initial_92);
        assert!(!recovery_path.exists());
        assert!(ctrl.restore_original_volume(1_600).expect("noop").is_none());
        assert_eq!(ctrl.restore_transitions_count(), 1);
    }

    {
        let mut ctrl =
            AudioDuckingController::new(Box::new(SimulatedAudioMediaBackend::new(0.92, false)))
                .with_recovery_file(recovery_path.clone());
        let _ = ctrl.engage_ducking(2_000).expect("duck before drop");
        assert!(recovery_path.exists());
    }
    assert!(!recovery_path.exists(), "Drop guard must remove the recovery file");

    let crash_record = DuckingRecoveryRecord {
        original_snapshot: initial_92,
        ducked_volume: 0.20,
        engaged_at_ms: 9_000,
    };
    fs::write(&recovery_path, serde_json::to_string(&crash_record).unwrap()).expect("write journal");

    let mut post_crash_ctrl =
        AudioDuckingController::new(Box::new(SimulatedAudioMediaBackend::new(0.20, false)))
            .with_recovery_file(recovery_path.clone());

    let recovered = post_crash_ctrl
        .recover_interrupted_ducking_from_disk()
        .expect("recover from disk")
        .expect("snapshot recovered");
    assert_eq!(recovered, initial_92);
    assert_eq!(post_crash_ctrl.current_backend_snapshot().unwrap(), initial_92);
    assert!(!recovery_path.exists());

    let _ = fs::remove_dir_all(&temp);
}

#[test]
fn test_ptt_orchestrator_integrates_ducking_during_listening_and_restores_on_release_or_failure() {
    let mut gk = build_voice_gatekeeper_allow_all();
    let mut avatar = ProductionAvatarRuntime::load_from_dir(avatar_assets_dir()).expect("avatar");
    let mut bridge = WebView2IpcBridge::new("miku-voice-4-2-e2e");

    let mut cfg = GroqSttConfig::default_with_consent(true);
    cfg.endpoint_url = "http://127.0.0.1:9/openai/v1/audio/transcriptions".to_string();

    let mut secrets = InMemoryVoiceSecretAliasStore::new();
    secrets.insert_alias("groq_stt_key", b"gsk_test_dummy_secret_value_4_2".to_vec());

    let initial_88 = SystemAudioSnapshot::from_scalar(0.88, false);
    let duck_ctrl =
        AudioDuckingController::new(Box::new(SimulatedAudioMediaBackend::new(0.88, false)));

    let mut dev = WasapiCaptureDevice::new_loopback();
    dev.enqueue_simulated_microphone_pcm(&vec![1400i16; 3_200]);

    let mut orch = PttSttOrchestrator::with_transport(
        avatar_assets_dir(),
        cfg,
        dev,
        Box::new(secrets),
        Box::new(MockGroqOkTransport),
    )
    .expect("orchestrator")
    .with_ducking_controller(duck_ctrl);

    orch.press_ptt_and_open_stream(PTT_DEFAULT_HOTKEY, 1_000, 0x20, 0x20, 0x88, &mut gk, &mut avatar)
        .expect("press ptt");
    assert!(orch.ducking().is_ducked());
    assert!((orch.ducking().current_backend_snapshot().unwrap().volume_scalar - 0.20).abs() < 1e-4);

    let _ = orch
        .pump_ptt_capture_chunk(3_200, 1_100, &mut avatar, Some(&mut bridge))
        .expect("pump chunk");
    assert!(orch.ducking().is_ducked());

    let outcome = orch
        .release_ptt_and_transcribe_to_session("sess-duck-ok", 1_300, &mut gk, &mut avatar, &mut bridge)
        .expect("transcribe ok");
    assert!(matches!(outcome, PttPipelineOutcome::TranscribedAndEnqueued { .. }));
    assert!(!orch.ducking().is_ducked());
    assert_eq!(orch.ducking().current_backend_snapshot().unwrap(), initial_88);

    orch.capture_device_mut()
        .enqueue_simulated_microphone_pcm(&vec![0i16; 640]);
    orch.press_ptt_and_open_stream(PTT_DEFAULT_HOTKEY, 5_000, 0x20, 0x20, 0x88, &mut gk, &mut avatar)
        .expect("press ptt silence test");
    assert!(orch.ducking().is_ducked());

    let _ = orch
        .pump_ptt_capture_chunk(
            640,
            5_000 + VOICE_SILENCE_AUTO_STOP_MS + 100,
            &mut avatar,
            Some(&mut bridge),
        )
        .expect("pump silence");
    assert!(!orch.ducking().is_ducked());
    assert_eq!(orch.ducking().current_backend_snapshot().unwrap(), initial_88);
}

#[test]
fn test_optional_wake_word_is_disabled_by_default_with_zero_idle_cpu_and_respects_power_policy() {
    let default_cfg = OptionalWakeWordConfig::default();
    assert!(!default_cfg.enabled_by_user);
    assert!(!default_cfg.is_listening_allowed());
    assert_eq!(default_cfg.active_idle_audio_streams(), 0);
    assert_eq!(default_cfg.estimated_idle_cpu_pct(), 0.0);

    let mut gk = build_voice_gatekeeper_allow_all();
    let mut avatar = ProductionAvatarRuntime::load_from_dir(avatar_assets_dir()).expect("avatar");
    let mut secrets = InMemoryVoiceSecretAliasStore::new();
    secrets.insert_alias("groq_stt_key", b"gsk_test_dummy_secret_value_4_2".to_vec());

    let initial_80 = SystemAudioSnapshot::from_scalar(0.80, false);
    let duck_ctrl =
        AudioDuckingController::new(Box::new(SimulatedAudioMediaBackend::new(0.80, false)));

    let mut orch = PttSttOrchestrator::with_transport(
        avatar_assets_dir(),
        GroqSttConfig::default_with_consent(true),
        WasapiCaptureDevice::new_loopback(),
        Box::new(secrets),
        Box::new(MockGroqOkTransport),
    )
    .expect("orchestrator")
    .with_ducking_controller(duck_ctrl);

    let err_default = orch
        .trigger_optional_wake_word_and_open_stream(0.95, 1_000, &mut gk, &mut avatar)
        .expect_err("wake word must be disabled by default");
    assert!(matches!(err_default, SttEngineError::UiFailure(_)));
    assert!(!orch.is_capture_stream_open());
    assert!(!orch.ducking().is_ducked());

    orch.set_wake_word_enabled(true);
    let err_battery = orch
        .trigger_optional_wake_word_and_open_stream(0.95, 1_100, &mut gk, &mut avatar)
        .expect_err("wake word must remain suspended on battery by default");
    assert!(matches!(err_battery, SttEngineError::UiFailure(_)));
    assert!(!orch.is_capture_stream_open());

    orch.set_wake_word_power_policy(VoicePowerPolicy {
        on_battery: false,
        allow_wake_word_on_battery: false,
        gaming_mode_active: false,
    });
    orch.trigger_optional_wake_word_and_open_stream(0.95, 1_200, &mut gk, &mut avatar)
        .expect("wake word allowed on AC when opted in");
    assert!(orch.is_capture_stream_open());
    assert!(orch.ducking().is_ducked());
    assert!((orch.ducking().current_backend_snapshot().unwrap().volume_scalar - 0.20).abs() < 1e-4);

    let _ = orch
        .abort_and_cleanup(VoiceCancellationReason::UserPressedEscape, 1_400, &mut avatar, None)
        .expect("cleanup wake word session");
    assert!(!orch.is_capture_stream_open());
    assert!(!orch.ducking().is_ducked());
    assert_eq!(orch.ducking().current_backend_snapshot().unwrap(), initial_80);
}

#[test]
fn test_island_media_controls_and_track_metadata_sanitization_over_ipc_bridge() {
    let mut gk = build_voice_gatekeeper_allow_all();
    let mut bridge = WebView2IpcBridge::new("miku-island-media-token");

    let initial_track = MediaTrackMetadata {
        title: "<script>alert('xss')</script> World Is Mine".to_string(),
        artist: "ryo (supercell) <img src=x onerror=alert(1)>".to_string(),
        album: "supercell".to_string(),
        status: MediaPlaybackStatus::Playing,
        duration_ms: 255_000,
        position_ms: 42_000,
    };

    let backend = SimulatedAudioMediaBackend::new(0.80, false).with_track(initial_track);
    let mut media_ctrl = IslandMediaController::new(Box::new(backend));

    let rendered = media_ctrl
        .sync_current_track_to_island(&mut bridge)
        .expect("sync track")
        .expect("payload");
    match rendered {
        WebViewIpcPayloadV1::DomSafeTextRender {
            target_element_id,
            safe_text,
        } => {
            assert_eq!(target_element_id, "miku-island-media-track");
            assert!(!safe_text.contains("<script>"));
            assert!(!safe_text.contains("<img"));
            assert!(safe_text.contains("&lt;script&gt;"));
            assert!(safe_text.contains("World Is Mine"));
        }
        other => panic!("expected DomSafeTextRender, got {:?}", other),
    }

    let after_toggle = media_ctrl
        .execute_transport_command(
            MediaTransportCommand::TogglePlayPause,
            2_000,
            &mut gk,
            Some(&mut bridge),
        )
        .expect("toggle")
        .expect("track");
    assert_eq!(after_toggle.status, MediaPlaybackStatus::Paused);

    let after_play = media_ctrl
        .execute_transport_command(MediaTransportCommand::Play, 2_100, &mut gk, Some(&mut bridge))
        .expect("play")
        .expect("track");
    assert_eq!(after_play.status, MediaPlaybackStatus::Playing);

    let after_next = media_ctrl
        .execute_transport_command(
            MediaTransportCommand::NextTrack,
            2_200,
            &mut gk,
            Some(&mut bridge),
        )
        .expect("next")
        .expect("track");
    assert_eq!(after_next.position_ms, 0);

    let deny_profile = ProjectProfile::new("deny-media-profile", ".");
    let mut deny_gk = GatekeeperEngine::new(deny_profile, ".", 60).expect("deny gk");
    assert!(media_ctrl
        .execute_transport_command(
            MediaTransportCommand::Stop,
            2_300,
            &mut deny_gk,
            Some(&mut bridge),
        )
        .is_err());
}

#[test]
fn test_winmm_audio_and_media_backend_probe_and_sub_600_line_modular_budget() {
    let mut winmm = WinMmSystemAudioMediaBackend::new().with_os_media_keys_enabled(false);
    winmm.set_track_metadata(Some(MediaTrackMetadata {
        title: "Melt".to_string(),
        artist: "ryo".to_string(),
        album: "supercell".to_string(),
        status: MediaPlaybackStatus::Playing,
        duration_ms: 258_000,
        position_ms: 12_000,
    }));

    let original = winmm.read_volume_snapshot().expect("read winmm snapshot");
    winmm.apply_volume_snapshot(&original).expect("re-apply snapshot");
    winmm
        .dispatch_media_command(MediaTransportCommand::TogglePlayPause)
        .expect("toggle media track state");
    assert_eq!(
        winmm.query_current_track().unwrap().unwrap().status,
        MediaPlaybackStatus::Paused
    );

    let voice_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/voice");
    let mut checked = 0usize;
    let mut max_lines = 0usize;
    for entry in fs::read_dir(&voice_dir).expect("read src/voice") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let lines = fs::read_to_string(&path).expect("read rs file").lines().count();
            assert!(
                lines < 600,
                "file {} has {} lines, exceeding strict 600-line modular limit",
                path.display(),
                lines
            );
            max_lines = max_lines.max(lines);
            checked += 1;
        }
    }
    assert!(checked >= 12);
    println!("VOICE_4_2_MODULAR_AUDIT: files={} max_lines={} (< 600)", checked, max_lines);
}
