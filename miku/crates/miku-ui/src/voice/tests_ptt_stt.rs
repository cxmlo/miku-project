#![allow(unused_imports)]
use crate::prelude::*;
use std::io::{Read, Write as IoWrite};
use std::net::TcpListener;
use std::thread;

fn create_isolated_voice_4_1_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("miku_voice_4_1_{}_{}_{}", tag, std::process::id(), nanos));
    let _ = fs::create_dir_all(&dir);
    dir
}

fn permissive_voice_gatekeeper(root_dir: &Path) -> GatekeeperEngine {
    let root_str = root_dir.to_string_lossy().to_string();
    let profile = ProjectProfile::new("miku-voice-4-1-test", &root_str, 30_000)
        .with_rule(Capability::Input, "microphone:*", PolicyEffect::Allow, true)
        .with_rule(Capability::FsRead, "**", PolicyEffect::Allow, true)
        .with_rule(Capability::FsWrite, "**", PolicyEffect::Allow, true)
        .with_rule(Capability::Network, "127.0.0.1*", PolicyEffect::Allow, true);
    GatekeeperEngine::new(profile).expect("build permissive voice gatekeeper")
}

fn spawn_single_shot_http_model_server(body: Vec<u8>, declared_content_length: Option<usize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback HTTP server");
    let addr = listener.local_addr().expect("local addr");
    let content_len = declared_content_length.unwrap_or(body.len());

    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut req_buf = [0u8; 1024];
            let _ = stream.read(&mut req_buf);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                content_len
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    format!("http://127.0.0.1:{}/models/ggml-fixture.bin", addr.port())
}

// LEDGER-4.1-1: PTT opens capture only while held and closes stream immediately on release
#[test]
fn test_ptt_opens_capture_only_while_held_and_closes_stream_immediately_on_release() {
    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_1");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("avatar reloader");
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");
    let mut gk = permissive_voice_gatekeeper(&temp_dir);

    let (raw_model, manifest) = build_valid_ggml_whisper_fixture_and_manifest("ggml-tiny-fixture.bin", 16 * 1024);
    fs::write(temp_dir.join(&manifest.model_id), &raw_model).expect("write verified GGML fixture");

    let mut corpus = DeterministicCorpusSttBackend::new();
    let es_pcm = corpus.register_utterance_fixture(1, "ejecuta cargo test", SttLanguage::Es);
    let capture = DeterministicFixtureCaptureDevice::with_staged_pcm(es_pcm);

    let mut orch = PttSttOrchestrator::with_backend(
        avatar_dir,
        &temp_dir,
        manifest,
        DEFAULT_STT_MODEL_TTL_MS,
        capture,
        Box::new(corpus),
    )
    .expect("create PttSttOrchestrator");

    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().active_stream_handles_count(), 0);
    assert_eq!(orch.capture_device().open_calls(), 0);
    assert_eq!(*orch.voice().state(), VoiceInteractionState::Idle);

    // Unrecognized hotkey must fail in VoiceInteractionController::start_ptt_session BEFORE opening stream
    assert!(orch.press_ptt_and_open_stream("Ctrl+Alt+Q", 1_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar).is_err());
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().open_calls(), 0);

    // Valid PTT press ("Alt+Space") opens capture stream immediately
    orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, 2_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar)
        .expect("press PTT opens stream");
    assert!(orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().active_stream_handles_count(), 1);
    assert_eq!(orch.capture_device().open_calls(), 1);
    assert_eq!(*orch.voice().state(), VoiceInteractionState::Listening);

    let wave = orch.pump_ptt_capture_chunk(64_000, 2_100, avatar, Some(&mut bridge)).expect("pump audio");
    assert_eq!(wave.target_fps, 60.0);
    assert!(orch.is_capture_stream_open());

    let outcome = orch
        .release_ptt_and_transcribe_to_session("sess_ptt_01", 2_500, &mut gk, avatar, &mut bridge)
        .expect("release PTT and transcribe");
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().active_stream_handles_count(), 0);
    assert_eq!(orch.capture_device().close_calls(), 1);
    assert_eq!(orch.capture_device().retained_device_buffer_bytes(), 0);
    assert_eq!(*orch.voice().state(), VoiceInteractionState::Idle);
    assert!(matches!(outcome, PttPipelineOutcome::TranscribedAndEnqueued { .. }));

    // Verify WinMM hardware capture device prepares WAVEHDR buffers when a device exists and closes cleanly
    let mut winmm_probe = WinMmWaveInCaptureProbe::new();
    if winmm_probe.open_capture_stream(VOICE_SAMPLE_RATE_HZ, VOICE_CHANNELS).is_ok() {
        assert!(winmm_probe.is_stream_open());
        assert_eq!(winmm_probe.active_stream_handles_count(), 1);
        assert!(winmm_probe.prepared_headers_count() > 0);
        let _ = winmm_probe.read_captured_pcm_chunk(320);
    }
    let zero_rep = winmm_probe.stop_and_close_stream();
    assert!(!winmm_probe.is_stream_open());
    assert_eq!(winmm_probe.active_stream_handles_count(), 0);
    assert_eq!(winmm_probe.prepared_headers_count(), 0);
    assert_eq!(zero_rep.residual_non_zero_samples, 0);
    assert_eq!(zero_rep.retained_capacity_bytes, 0);

    let _ = fs::remove_dir_all(&temp_dir);
}

// LEDGER-4.1-2: In idle, error, cancellation, and Kill Switch no active capture or unreleased buffer remains
#[test]
fn test_idle_error_cancellation_and_kill_switch_leave_zero_active_capture_and_zero_buffers() {
    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_2");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("avatar reloader");
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");
    let mut gk = permissive_voice_gatekeeper(&temp_dir);
    let mut audit_store = PersistentJsonlAuditStore::open_default(&temp_dir).expect("audit store");

    let (raw_model, manifest) = build_valid_ggml_whisper_fixture_and_manifest("ggml-tiny-fixture.bin", 16 * 1024);
    fs::write(temp_dir.join(&manifest.model_id), &raw_model).expect("write model");

    let mut corpus = DeterministicCorpusSttBackend::new();
    let speech_pcm = corpus.register_utterance_fixture(2, "cancela la cola", SttLanguage::Es);
    let capture = DeterministicFixtureCaptureDevice::with_staged_pcm(speech_pcm.clone());
    let mut orch = PttSttOrchestrator::with_backend(
        avatar_dir,
        &temp_dir,
        manifest,
        DEFAULT_STT_MODEL_TTL_MS,
        capture,
        Box::new(corpus),
    )
    .unwrap();

    // Path A: Escape cancellation during active PTT capture
    orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, 1_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(1_600, 1_050, avatar, Some(&mut bridge)).unwrap();
    assert!(orch.is_capture_stream_open());

    let receipt_esc = orch
        .abort_and_cleanup(VoiceCancellationReason::UserPressedEscape, 1_100, avatar, Some(&mut bridge))
        .expect("abort on Escape");
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().active_stream_handles_count(), 0);
    assert_eq!(orch.capture_device().retained_device_buffer_bytes(), 0);
    assert_eq!(receipt_esc.zero_report.residual_non_zero_samples, 0);
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);

    // Path B: Early PTT release (< MIN_PTT_HOLD_DURATION_MS = 120 ms)
    orch.capture_device_mut().restage_pcm(speech_pcm.clone());
    orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, 2_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(1_600, 2_030, avatar, Some(&mut bridge)).unwrap();
    let early_out = orch
        .release_ptt_and_transcribe_to_session("sess_early", 2_060, &mut gk, avatar, &mut bridge)
        .expect("early release");
    assert!(matches!(
        early_out,
        PttPipelineOutcome::Cancelled { reason: VoiceCancellationReason::PttReleasedTooEarly { held_ms: 60 } }
    ));
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().retained_device_buffer_bytes(), 0);
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);

    // Path C: Simulated hardware read error during capture
    orch.capture_device_mut().restage_pcm(speech_pcm.clone());
    orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, 3_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar).unwrap();
    orch.capture_device_mut().inject_read_error(Some("microphone disconnected mid-frame".to_string()));
    let read_err = orch.pump_ptt_capture_chunk(1_600, 3_050, avatar, Some(&mut bridge));
    assert!(read_err.is_err());
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().active_stream_handles_count(), 0);
    assert_eq!(orch.capture_device().retained_device_buffer_bytes(), 0);
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);
    assert_eq!(*orch.voice().state(), VoiceInteractionState::Idle);

    // Path D: Global Kill Switch while PTT is held
    orch.capture_device_mut().restage_pcm(speech_pcm);
    orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, 4_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(3_200, 4_050, avatar, Some(&mut bridge)).unwrap();
    assert!(orch.is_capture_stream_open());

    let mut ks = UiKillSwitchManager::new();
    let trip_report = orch
        .trip_kill_switch_and_purge(&mut ks, 4_100, "voice_4_1_emergency_stop", &mut gk, avatar, &mut audit_store, &mut bridge)
        .expect("kill switch trip");
    assert!(trip_report.voice_aborted);
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().active_stream_handles_count(), 0);
    assert_eq!(orch.capture_device().retained_device_buffer_bytes(), 0);
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);
    assert_eq!(orch.allocated_model_bytes(), 0);
    assert!(orch.session_queue().orders_for_session("sess_early").is_empty());

    let _ = fs::remove_dir_all(&temp_dir);
}

// LEDGER-4.1-3: Bilingual ES/EN audio fixtures traverse PTT -> STT pipeline, while silence/unrecognized audio and missing whisper-cli fail closed
#[test]
fn test_bilingual_es_en_audio_fixtures_traverse_local_ptt_to_stt_pipeline_and_enqueue_to_active_session() {
    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_3");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("avatar reloader");
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");
    let mut gk = permissive_voice_gatekeeper(&temp_dir);

    let (raw_model, manifest) = build_valid_ggml_whisper_fixture_and_manifest("ggml-tiny-fixture.bin", 32 * 1024);
    let model_path = temp_dir.join(&manifest.model_id);
    fs::write(&model_path, &raw_model).expect("write model");

    // 1. Verify production WhisperCliLocalBackend fails closed with LocalRuntimeBinaryUnavailable
    //    when whisper-cli is absent, never fabricating a synthetic transcript
    let mut prod_backend = WhisperCliLocalBackend::with_executable_path(temp_dir.join("nonexistent-whisper-cli.exe"));
    let weights = verify_and_parse_stt_model_bytes(&raw_model, &manifest, DEFAULT_MAX_STT_MODEL_BYTES).unwrap();
    let prod_err = prod_backend
        .decode_pcm16_mono(&weights, &model_path, &[2_000i16; 3_200], VOICE_SAMPLE_RATE_HZ)
        .unwrap_err();
    assert!(matches!(prod_err, SttModelError::LocalRuntimeBinaryUnavailable(_)));

    // 2. Verify bilingual ES/EN pipeline routing and strict rejection of uncorroborated noise
    let mut corpus = DeterministicCorpusSttBackend::new();
    let fixtures = [
        (corpus.register_utterance_fixture(10, "ejecuta cargo test", SttLanguage::Es), "ejecuta cargo test", SttLanguage::Es),
        (corpus.register_utterance_fixture(11, "aprueba el permiso", SttLanguage::Es), "aprueba el permiso", SttLanguage::Es),
        (corpus.register_utterance_fixture(12, "run clippy and fix warnings", SttLanguage::En), "run clippy and fix warnings", SttLanguage::En),
        (corpus.register_utterance_fixture(13, "open pull request", SttLanguage::En), "open pull request", SttLanguage::En),
    ];

    let capture = DeterministicFixtureCaptureDevice::new();
    let mut orch = PttSttOrchestrator::with_backend(
        avatar_dir,
        &temp_dir,
        manifest,
        DEFAULT_STT_MODEL_TTL_MS,
        capture,
        Box::new(corpus),
    )
    .unwrap();

    for (idx, (pcm, expected_phrase, expected_lang)) in fixtures.iter().enumerate() {
        let base_ts = 10_000 + (idx as u64) * 2_000;
        orch.capture_device_mut().restage_pcm(pcm.clone());
        orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, base_ts, 0x1000, 0x1000, 0x2000, &mut gk, avatar).unwrap();
        assert!(orch.is_capture_stream_open());

        while orch.capture_device().staged_remaining_samples() > 0 {
            let _ = orch.pump_ptt_capture_chunk(640, base_ts + 100, avatar, Some(&mut bridge)).expect("pump frame");
        }

        let outcome = orch
            .release_ptt_and_transcribe_to_session("sess_active_agent_41", base_ts + 400, &mut gk, avatar, &mut bridge)
            .expect("release and transcribe");
        assert!(!orch.is_capture_stream_open());
        assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);
        assert_eq!(orch.capture_device().retained_device_buffer_bytes(), 0);

        match outcome {
            PttPipelineOutcome::TranscribedAndEnqueued { transcript, order, completion_report, device_zero_report } => {
                assert_eq!(transcript.text, *expected_phrase);
                assert_eq!(transcript.language, *expected_lang);
                assert_eq!(order.session_id, "sess_active_agent_41");
                assert_eq!(order.prompt_text, *expected_phrase);
                assert_eq!(order.language, *expected_lang);
                assert_eq!(completion_report.zero_report.residual_non_zero_samples, 0);
                assert_eq!(device_zero_report.residual_non_zero_samples, 0);
            }
            other => panic!("expected TranscribedAndEnqueued, got {:?}", other),
        }
    }

    // 3. Unregistered arbitrary audio must be rejected with NoSpeechRecognized (never nearest-neighbor matched)
    orch.capture_device_mut().restage_pcm(vec![4_096i16; 4_800]);
    orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, 25_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(4_800, 25_100, avatar, Some(&mut bridge)).unwrap();
    let unknown_err = orch
        .release_ptt_and_transcribe_to_session("sess_active_agent_41", 25_400, &mut gk, avatar, &mut bridge)
        .unwrap_err();
    assert_eq!(unknown_err, SttModelError::NoSpeechRecognized);
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);

    let queued = orch.session_queue().orders_for_session("sess_active_agent_41");
    assert_eq!(queued.len(), 4);
    assert_eq!(queued[0].prompt_text, "ejecuta cargo test");
    assert_eq!(queued[1].prompt_text, "aprueba el permiso");
    assert_eq!(queued[2].prompt_text, "run clippy and fix warnings");
    assert_eq!(queued[3].prompt_text, "open pull request");

    let first_ipc = bridge.pop_next_outbound_json().expect("outbound ipc");
    assert!(first_ipc.contains("voice_enqueue_prompt"));
    assert!(first_ipc.contains("ejecuta cargo test"));

    let _ = fs::remove_dir_all(&temp_dir);
}

// LEDGER-4.1-4: Local STT path makes zero network calls and consumes zero tokens
#[test]
fn test_local_stt_transcription_makes_zero_network_calls_and_consumes_zero_provider_tokens() {
    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_4");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("avatar reloader");
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");

    let root_str = temp_dir.to_string_lossy().to_string();
    let profile = ProjectProfile::new("miku-voice-offline-strict", &root_str, 30_000)
        .with_rule(Capability::Input, "microphone:*", PolicyEffect::Allow, true)
        .with_rule(Capability::FsRead, "**", PolicyEffect::Allow, true)
        .with_rule(Capability::Network, "*", PolicyEffect::Deny, true);
    let mut gk = GatekeeperEngine::new(profile).expect("offline gatekeeper");

    let (raw_model, manifest) = build_valid_ggml_whisper_fixture_and_manifest("ggml-tiny-fixture.bin", 16 * 1024);
    fs::write(temp_dir.join(&manifest.model_id), &raw_model).expect("write model");

    let mut corpus = DeterministicCorpusSttBackend::new();
    let en_pcm = corpus.register_utterance_fixture(20, "stop active agent", SttLanguage::En);
    let capture = DeterministicFixtureCaptureDevice::with_staged_pcm(en_pcm);
    let mut orch = PttSttOrchestrator::with_backend(
        avatar_dir,
        &temp_dir,
        manifest,
        DEFAULT_STT_MODEL_TTL_MS,
        capture,
        Box::new(corpus),
    )
    .unwrap();

    orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, 5_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(64_000, 5_100, avatar, Some(&mut bridge)).unwrap();
    let outcome = orch
        .release_ptt_and_transcribe_to_session("sess_offline", 5_400, &mut gk, avatar, &mut bridge)
        .expect("local transcription succeeds with Network denied");

    assert!(matches!(outcome, PttPipelineOutcome::TranscribedAndEnqueued { .. }));
    assert_eq!(orch.stt_runtime().network_calls_made(), 0);
    assert_eq!(orch.stt_runtime().provider_tokens_consumed(), 0);

    let network_audits = gk.audit_log().entries().iter().filter(|e| e.capability == Capability::Network.as_str()).count();
    assert_eq!(network_audits, 0);

    let _ = fs::remove_dir_all(&temp_dir);
}

// LEDGER-4.1-5: Real Whisper GGML manifests & SHA-256 download/verification
#[test]
fn test_explicit_model_download_verifies_sha256_and_rejects_wrong_hash_truncated_or_unconfirmed() {
    // 1. Verify official upstream ggerganov/whisper.cpp manifests (MIT license, exact bytes, exact SHA-256)
    let tiny = SttModelManifest::default_whisper_tiny();
    assert_eq!(tiny.model_id, "ggml-tiny.bin");
    assert_eq!(tiny.license, "MIT");
    assert_eq!(tiny.expected_size_bytes, 77_691_713);
    assert_eq!(tiny.expected_sha256_hex, "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21");
    assert_eq!(tiny.download_url, "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin");

    let base = SttModelManifest::default_whisper_base();
    assert_eq!(base.model_id, "ggml-base.bin");
    assert_eq!(base.license, "MIT");
    assert_eq!(base.expected_size_bytes, 147_951_465);
    assert_eq!(base.expected_sha256_hex, "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe");

    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_5");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut gk = permissive_voice_gatekeeper(&temp_dir);

    let (valid_bytes, manifest) = build_valid_ggml_whisper_fixture_and_manifest("ggml-tiny-dl.bin", 24 * 1024);
    let capture = DeterministicFixtureCaptureDevice::new();
    let mut orch = PttSttOrchestrator::new(avatar_dir, &temp_dir, manifest.clone(), DEFAULT_STT_MODEL_TTL_MS, capture).unwrap();

    let err_unconfirmed = orch.download_and_verify_model_explicit("http://127.0.0.1:9/unused", false, 1_000, &mut gk).unwrap_err();
    assert_eq!(err_unconfirmed, SttModelError::UserConfirmationRequired);
    assert!(!orch.model_file_path().exists());

    let truncated_bytes = valid_bytes[..valid_bytes.len() / 2].to_vec();
    let url_truncated = spawn_single_shot_http_model_server(truncated_bytes, Some(valid_bytes.len()));
    let err_truncated = orch.download_and_verify_model_explicit(&url_truncated, true, 2_000, &mut gk).unwrap_err();
    assert!(matches!(err_truncated, SttModelError::IncompleteDownload { .. }));
    assert!(!orch.model_file_path().exists());
    assert_eq!(orch.allocated_model_bytes(), 0);

    let mut tampered_bytes = valid_bytes.clone();
    let last = tampered_bytes.len() - 1;
    tampered_bytes[last] ^= 0xFF;
    let url_tampered = spawn_single_shot_http_model_server(tampered_bytes, None);
    let err_tampered = orch.download_and_verify_model_explicit(&url_tampered, true, 3_000, &mut gk).unwrap_err();
    assert!(matches!(err_tampered, SttModelError::Sha256Mismatch { .. }));
    assert!(!orch.model_file_path().exists());
    assert_eq!(orch.allocated_model_bytes(), 0);

    let url_valid = spawn_single_shot_http_model_server(valid_bytes.clone(), None);
    let dl_receipt = orch.download_and_verify_model_explicit(&url_valid, true, 4_000, &mut gk).expect("verified download");
    assert_eq!(dl_receipt.verified_size_bytes, valid_bytes.len());
    assert_eq!(dl_receipt.verified_sha256_hex, manifest.expected_sha256_hex);
    assert!(orch.model_file_path().exists());
    assert!(matches!(orch.inspect_model_availability(), SttModelAvailability::VerifiedOnDisk { .. }));
    assert_eq!(orch.allocated_model_bytes(), 0);

    let _ = fs::remove_dir_all(&temp_dir);
}

// LEDGER-4.1-6: Missing model produces verified download card; inactivity TTL unloads model to 0 MB
#[test]
fn test_missing_model_exposes_typed_download_card_and_inactivity_ttl_evicts_model_to_zero_mb() {
    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_6");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("avatar reloader");
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");
    let mut gk = permissive_voice_gatekeeper(&temp_dir);

    let (raw_model, manifest) = build_valid_ggml_whisper_fixture_and_manifest("ggml-tiny.bin", 64 * 1024);
    let mut corpus = DeterministicCorpusSttBackend::new();
    let pcm = corpus.register_utterance_fixture(30, "abre el navegador", SttLanguage::Es);
    let capture = DeterministicFixtureCaptureDevice::with_staged_pcm(pcm.clone());
    let mut orch = PttSttOrchestrator::with_backend(
        avatar_dir,
        &temp_dir,
        manifest.clone(),
        1_500,
        capture,
        Box::new(corpus),
    )
    .unwrap();

    match orch.inspect_model_availability() {
        SttModelAvailability::Missing { download_card } => {
            assert_eq!(download_card.model_id, "ggml-tiny.bin");
            assert_eq!(download_card.license, "MIT");
            assert_eq!(download_card.expected_size_bytes, manifest.expected_size_bytes);
            assert_eq!(download_card.expected_sha256_hex, manifest.expected_sha256_hex);
            assert!(download_card.requires_explicit_user_action);
        }
        other => panic!("expected Missing download card, got {:?}", other),
    }

    orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, 1_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(64_000, 1_100, avatar, Some(&mut bridge)).unwrap();
    let missing_out = orch
        .release_ptt_and_transcribe_to_session("sess_missing", 1_400, &mut gk, avatar, &mut bridge)
        .expect("missing model handled gracefully");
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);
    assert_eq!(orch.allocated_model_bytes(), 0);
    assert!(matches!(missing_out, PttPipelineOutcome::ModelMissing { .. }));

    fs::write(orch.model_file_path(), &raw_model).expect("write verified model");
    assert_eq!(orch.allocated_model_bytes(), 0);

    orch.capture_device_mut().restage_pcm(pcm);
    orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, 2_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(64_000, 2_100, avatar, Some(&mut bridge)).unwrap();
    let _ = orch.release_ptt_and_transcribe_to_session("sess_ttl", 2_400, &mut gk, avatar, &mut bridge).unwrap();
    assert!(orch.allocated_model_bytes() >= 64 * 1024);

    assert!(!orch.poll_inactivity_ttl(3_500));
    assert!(orch.allocated_model_bytes() >= 64 * 1024);

    assert!(orch.poll_inactivity_ttl(3_900));
    assert_eq!(orch.allocated_model_bytes(), 0);
    assert_eq!(*orch.stt_runtime().state(), SttModelLifecycleState::Unloaded);

    let _ = fs::remove_dir_all(&temp_dir);
}

// LEDGER-4.1-7: Errors never leak audio, sensitive transcripts, or private info
#[test]
fn test_voice_and_stt_errors_redact_audio_transcripts_and_secrets_with_zero_leaks() {
    let raw_diag = r#"failed near transcript="mi clave secreta es 12345" token=sk-secret999999999999 Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9 pcm=[14320, -12090]"#;
    let safe_diag = redact_sensitive_voice_diagnostic(raw_diag);

    assert!(!safe_diag.contains("mi clave secreta es 12345"));
    assert!(!safe_diag.contains("sk-secret999999999999"));
    assert!(!safe_diag.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"));
    assert!(!safe_diag.contains("14320"));
    assert!(safe_diag.contains("[REDACTED]"));

    let err = SttModelError::from_untrusted_diagnostic("stt_parse", raw_diag);
    let rendered = err.to_string();
    assert!(!rendered.contains("mi clave secreta es 12345"));
    assert!(!rendered.contains("sk-secret999999999999"));
    assert!(!rendered.contains("14320"));

    let voice_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("voice");
    for entry in fs::read_dir(&voice_dir).expect("read src/voice") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            let line_count = fs::read_to_string(&path).expect("read rs").lines().count();
            assert!(line_count < 600, "voice module {} exceeded 600 lines ({})", path.display(), line_count);
        }
    }
}
