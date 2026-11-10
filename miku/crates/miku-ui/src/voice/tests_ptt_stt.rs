#![allow(unused_imports)]
use crate::prelude::*;
use std::io::{Read, Write as IoWrite};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
struct CapturedGroqHttpRequest {
    request_line: String,
    headers_lower: BTreeMap<String, String>,
    raw_body: Vec<u8>,
}

fn create_isolated_voice_4_1_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("miku_v41_{}_{}_{}", tag, std::process::id(), nanos));
    let _ = fs::create_dir_all(&dir);
    dir
}

fn permissive_groq_voice_gatekeeper(root_dir: &Path) -> GatekeeperEngine {
    let root_str = root_dir.to_string_lossy().to_string();
    let profile = ProjectProfile::new("miku-voice-groq-4-1", &root_str, 30_000)
        .with_rule(Capability::Input, "microphone:*", PolicyEffect::Allow, true)
        .with_rule(Capability::Network, "127.0.0.1*", PolicyEffect::Allow, true)
        .with_rule(Capability::Network, "api.groq.com*", PolicyEffect::Allow, true)
        .with_rule(Capability::Network, "groq-stt-key", PolicyEffect::Allow, true);
    GatekeeperEngine::new(profile).expect("build permissive groq voice gatekeeper")
}

fn build_synthetic_speech_pcm16(seed: u32, sample_count: usize) -> Vec<i16> {
    let mut pcm = Vec::with_capacity(sample_count);
    let mut state = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    for i in 0..sample_count {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let carrier = (((i as u32).wrapping_mul(19 + (seed & 7))) % 64) as i16 - 32;
        pcm.push(((state >> 16) as i16 % 1400) + carrier * 180);
    }
    pcm
}

fn spawn_local_groq_stt_server(
    status_code: u16,
    response_json: String,
    delay_ms: u64,
) -> (String, Arc<Mutex<Vec<CapturedGroqHttpRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local Groq HTTP inspection server");
    let addr = listener.local_addr().expect("local server addr");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = Arc::clone(&captured);

    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(2_000)));
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let mut expected_total: Option<usize> = None;

            loop {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if expected_total.is_none() {
                            if let Some(hdr_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                                let hdr_str = String::from_utf8_lossy(&buf[..hdr_end]);
                                let cl = hdr_str
                                    .lines()
                                    .skip(1)
                                    .filter_map(|l| l.split_once(':'))
                                    .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
                                    .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                                    .unwrap_or(0);
                                expected_total = Some(hdr_end + 4 + cl);
                            }
                        }
                        if expected_total.is_some_and(|tot| buf.len() >= tot) {
                            break;
                        }
                    }
                }
            }

            if let Some(hdr_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let hdr_text = String::from_utf8_lossy(&buf[..hdr_end]);
                let mut lines = hdr_text.lines();
                let request_line = lines.next().unwrap_or_default().to_string();
                let mut headers_lower = BTreeMap::new();
                for (k, v) in lines.filter_map(|l| l.split_once(':')) {
                    headers_lower.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                }
                captured_clone.lock().unwrap().push(CapturedGroqHttpRequest {
                    request_line,
                    headers_lower,
                    raw_body: buf[hdr_end + 4..].to_vec(),
                });
            }

            if delay_ms > 0 {
                thread::sleep(Duration::from_millis(delay_ms));
            }
            let reason = if status_code == 200 { "OK" } else { "Error" };
            let resp = format!(
                "HTTP/1.1 {status_code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_json}",
                response_json.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });

    (format!("http://127.0.0.1:{}/openai/v1/audio/transcriptions", addr.port()), captured)
}

fn press_default_ptt<C: PttAudioCaptureStream>(
    orch: &mut PttSttOrchestrator<C>,
    ts: u64,
    gk: &mut GatekeeperEngine,
    avatar: &mut ProductionAvatarRuntime,
) -> Result<(), SttEngineError> {
    orch.press_ptt_and_open_stream(DEFAULT_PTT_HOTKEY_COMBO, ts, 0x1000, 0x1000, 0x2000, gk, avatar)
}

// LEDGER-4.1-1: PTT opens microphone only while held and closes stream BEFORE sending transcription
#[test]
fn test_ptt_opens_capture_only_while_held_and_closes_stream_before_sending_transcription() {
    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_1");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("avatar reloader");
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");
    let mut gk = permissive_groq_voice_gatekeeper(&temp_dir);

    let (endpoint, captured_reqs) = spawn_local_groq_stt_server(
        200,
        r#"{"text":"ejecuta cargo test","language":"es","duration":0.40}"#.to_string(),
        0,
    );
    let config = GroqSttConfig::default().with_endpoint(endpoint).with_consent(true);
    let vault = InMemoryVoiceSecretVault::with_alias_secret(GROQ_DEFAULT_KEY_ALIAS, b"gsk_test_alias_only_key_411").unwrap();
    let capture = DeterministicFixtureCaptureDevice::with_staged_pcm(build_synthetic_speech_pcm16(1, 6_400));
    let mut orch = PttSttOrchestrator::new(avatar_dir, config, capture, Box::new(vault)).unwrap();

    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().active_stream_handles_count(), 0);
    assert!(orch.press_ptt_and_open_stream("Ctrl+Alt+Q", 1_000, 0x1000, 0x1000, 0x2000, &mut gk, avatar).is_err());
    assert!(!orch.is_capture_stream_open());

    press_default_ptt(&mut orch, 2_000, &mut gk, avatar).expect("press PTT opens stream");
    assert!(orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().active_stream_handles_count(), 1);

    let wave = orch.pump_ptt_capture_chunk(64_000, 2_100, avatar, Some(&mut bridge)).expect("pump audio");
    assert_eq!(wave.target_fps, 60.0);
    assert!(orch.is_capture_stream_open());

    let outcome = orch
        .release_ptt_and_transcribe_to_session("sess_ptt_01", 2_500, &mut gk, avatar, &mut bridge)
        .expect("release PTT closes mic and sends to Groq endpoint");
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().active_stream_handles_count(), 0);
    assert_eq!(orch.capture_device().close_calls(), 1);
    assert_eq!(orch.capture_device().retained_device_buffer_bytes(), 0);
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);
    assert_eq!(captured_reqs.lock().unwrap().len(), 1);
    assert!(matches!(outcome, PttPipelineOutcome::TranscribedAndEnqueued { .. }));

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

// LEDGER-4.1-2: In idle, error, timeout, cancellation, and Kill Switch no active capture or unreleased buffer remains
#[test]
fn test_idle_error_timeout_cancellation_and_kill_switch_leave_zero_active_capture_and_zero_buffers() {
    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_2");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("avatar reloader");
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");
    let mut gk = permissive_groq_voice_gatekeeper(&temp_dir);
    let mut audit_store = PersistentJsonlAuditStore::open_default(&temp_dir).expect("audit store");

    let (timeout_endpoint, _) = spawn_local_groq_stt_server(200, r#"{"text":"late"}"#.to_string(), 350);
    let mut config = GroqSttConfig::default().with_endpoint(timeout_endpoint).with_consent(true);
    config.timeout_ms = 60;
    let vault = InMemoryVoiceSecretVault::with_alias_secret(GROQ_DEFAULT_KEY_ALIAS, b"gsk_test_alias_only_key_412").unwrap();

    let speech_pcm = build_synthetic_speech_pcm16(2, 6_400);
    let capture = DeterministicFixtureCaptureDevice::with_staged_pcm(speech_pcm.clone());
    let mut orch = PttSttOrchestrator::new(avatar_dir, config, capture, Box::new(vault)).unwrap();

    // 1. Escape cancellation
    press_default_ptt(&mut orch, 1_000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(1_600, 1_050, avatar, Some(&mut bridge)).unwrap();
    let receipt_esc = orch.abort_and_cleanup(VoiceCancellationReason::UserPressedEscape, 1_100, avatar, Some(&mut bridge)).unwrap();
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().retained_device_buffer_bytes(), 0);
    assert_eq!(receipt_esc.zero_report.residual_non_zero_samples, 0);
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);

    // 2. Early release (< 120 ms)
    orch.capture_device_mut().restage_pcm(speech_pcm.clone());
    press_default_ptt(&mut orch, 2_000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(1_600, 2_030, avatar, Some(&mut bridge)).unwrap();
    let early_out = orch.release_ptt_and_transcribe_to_session("sess_early", 2_060, &mut gk, avatar, &mut bridge).unwrap();
    assert!(matches!(
        early_out,
        PttPipelineOutcome::Cancelled {
            reason: VoiceCancellationReason::PttReleasedTooEarly { held_ms: 60 }
        }
    ));
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);

    // 3. Network timeout during Groq transcription
    orch.capture_device_mut().restage_pcm(speech_pcm.clone());
    press_default_ptt(&mut orch, 3_000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(6_400, 3_100, avatar, Some(&mut bridge)).unwrap();
    let timeout_err = orch.release_ptt_and_transcribe_to_session("sess_timeout", 3_400, &mut gk, avatar, &mut bridge).unwrap_err();
    assert!(matches!(timeout_err, SttEngineError::NetworkTimeout(_)));
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().retained_device_buffer_bytes(), 0);
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);
    assert_eq!(*orch.voice().state(), VoiceInteractionState::Idle);

    // 4. Global Kill Switch
    orch.capture_device_mut().restage_pcm(speech_pcm);
    press_default_ptt(&mut orch, 4_000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(3_200, 4_050, avatar, Some(&mut bridge)).unwrap();
    let mut ks = UiKillSwitchManager::new();
    let trip_report = orch
        .trip_kill_switch_and_purge(&mut ks, 4_100, "voice_4_1_emergency_stop", &mut gk, avatar, &mut audit_store, &mut bridge)
        .unwrap();
    assert!(trip_report.voice_aborted);
    assert!(!orch.is_capture_stream_open());
    assert_eq!(orch.capture_device().active_stream_handles_count(), 0);
    assert_eq!(orch.capture_device().retained_device_buffer_bytes(), 0);
    assert_eq!(orch.voice().pcm_buffer().capacity_bytes(), 0);

    let _ = fs::remove_dir_all(&temp_dir);
}

// LEDGER-4.1-3: ES and EN WAV audio files traverse PTT -> Groq multipart HTTP endpoint and verify response contract
#[test]
fn test_bilingual_es_en_wav_files_traverse_groq_multipart_http_contract_and_enqueue_to_active_session() {
    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_3");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("avatar reloader");
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");
    let mut gk = permissive_groq_voice_gatekeeper(&temp_dir);

    let es_wav_path = temp_dir.join("es_utterance.wav");
    let en_wav_path = temp_dir.join("en_utterance.wav");
    let es_raw_pcm = build_synthetic_speech_pcm16(101, 16_000);
    let en_raw_pcm = build_synthetic_speech_pcm16(202, 24_000);
    fs::write(&es_wav_path, encode_riff_wav_pcm16_mono(&es_raw_pcm, VOICE_SAMPLE_RATE_HZ)).unwrap();
    fs::write(&en_wav_path, encode_riff_wav_pcm16_mono(&en_raw_pcm, VOICE_SAMPLE_RATE_HZ)).unwrap();

    let decoded_es = decode_riff_wav_pcm16_mono(&fs::read(&es_wav_path).unwrap()).unwrap();
    let decoded_en = decode_riff_wav_pcm16_mono(&fs::read(&en_wav_path).unwrap()).unwrap();
    assert_eq!(decoded_es, es_raw_pcm);
    assert_eq!(decoded_en, en_raw_pcm);

    let (es_endpoint, es_reqs) = spawn_local_groq_stt_server(
        200,
        r#"{"text":"ejecuta pruebas del proyecto","language":"Spanish","duration":1.0}"#.to_string(),
        0,
    );
    let config = GroqSttConfig::default()
        .with_endpoint(es_endpoint)
        .with_model(GroqWhisperModel::WhisperLargeV3Turbo)
        .with_language_hint(Some(SttLanguage::Es))
        .with_consent(true);
    let vault = InMemoryVoiceSecretVault::with_alias_secret(GROQ_DEFAULT_KEY_ALIAS, b"gsk_secret_bilingual_test_key").unwrap();

    let mut orch = PttSttOrchestrator::new(
        avatar_dir,
        config,
        DeterministicFixtureCaptureDevice::with_staged_pcm(decoded_es),
        Box::new(vault),
    )
    .unwrap();

    press_default_ptt(&mut orch, 5_000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(64_000, 5_100, avatar, Some(&mut bridge)).unwrap();
    let out_es = orch.release_ptt_and_transcribe_to_session("sess_bilingual", 5_500, &mut gk, avatar, &mut bridge).unwrap();

    let reqs_es = es_reqs.lock().unwrap();
    assert_eq!(reqs_es.len(), 1);
    assert_eq!(reqs_es[0].request_line, "POST /openai/v1/audio/transcriptions HTTP/1.1");
    assert_eq!(
        reqs_es[0].headers_lower.get("authorization").map(|s| s.as_str()),
        Some("Bearer gsk_secret_bilingual_test_key")
    );
    assert!(reqs_es[0].headers_lower.get("content-type").unwrap().contains("multipart/form-data; boundary="));
    let body_es_str = String::from_utf8_lossy(&reqs_es[0].raw_body);
    assert!(body_es_str.contains("name=\"model\"\r\n\r\nwhisper-large-v3-turbo"));
    assert!(body_es_str.contains("name=\"response_format\"\r\n\r\nverbose_json"));
    assert!(body_es_str.contains("name=\"language\"\r\n\r\nes"));
    assert!(body_es_str.contains("name=\"file\"; filename=\"utterance.wav\""));
    assert!(reqs_es[0].raw_body.windows(4).any(|w| w == b"RIFF"));
    drop(reqs_es);

    match out_es {
        PttPipelineOutcome::TranscribedAndEnqueued { transcript, order, .. } => {
            assert_eq!(transcript.text, "ejecuta pruebas del proyecto");
            assert_eq!(transcript.language, SttLanguage::Es);
            assert_eq!(transcript.model, GroqWhisperModel::WhisperLargeV3Turbo);
            assert_eq!(transcript.billing.billed_duration_seconds, 10);
            assert!(transcript.billing.minimum_billing_applied);
            assert_eq!(order.language, SttLanguage::Es);
        }
        other => panic!("expected TranscribedAndEnqueued, got {:?}", other),
    }

    let (en_endpoint, en_reqs) = spawn_local_groq_stt_server(
        200,
        r#"{"text":"run clippy and fix warnings","language":"en","duration":1.5}"#.to_string(),
        0,
    );
    orch.set_endpoint_url(en_endpoint).unwrap();
    orch.capture_device_mut().restage_pcm(decoded_en);
    press_default_ptt(&mut orch, 7_000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(64_000, 7_100, avatar, Some(&mut bridge)).unwrap();
    let out_en = orch.release_ptt_and_transcribe_to_session("sess_bilingual", 7_500, &mut gk, avatar, &mut bridge).unwrap();

    assert_eq!(en_reqs.lock().unwrap().len(), 1);
    match out_en {
        PttPipelineOutcome::TranscribedAndEnqueued { transcript, order, .. } => {
            assert_eq!(transcript.text, "run clippy and fix warnings");
            assert_eq!(transcript.language, SttLanguage::En);
            assert_eq!(order.language, SttLanguage::En);
        }
        other => panic!("expected TranscribedAndEnqueued, got {:?}", other),
    }
    assert_eq!(orch.session_queue().orders_for_session("sess_bilingual").len(), 2);
    assert_eq!(orch.total_billed_seconds(), 20);

    let _ = fs::remove_dir_all(&temp_dir);
}

// LEDGER-4.1-4: Gatekeeper Network authorization and explicit cloud audio consent enforcement
#[test]
fn test_gatekeeper_network_authorization_and_explicit_cloud_audio_consent_required_before_dispatch() {
    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_4");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("avatar reloader");
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");
    let mut gk = permissive_groq_voice_gatekeeper(&temp_dir);

    let vault = InMemoryVoiceSecretVault::with_alias_secret(GROQ_DEFAULT_KEY_ALIAS, b"gsk_should_not_be_read").unwrap();
    let pcm = build_synthetic_speech_pcm16(4, 6_400);
    let mut orch = PttSttOrchestrator::new(
        avatar_dir,
        GroqSttConfig::default().with_consent(false),
        DeterministicFixtureCaptureDevice::with_staged_pcm(pcm.clone()),
        Box::new(vault),
    )
    .unwrap();

    press_default_ptt(&mut orch, 1_000, &mut gk, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(6_400, 1_100, avatar, Some(&mut bridge)).unwrap();
    let err_consent = orch.release_ptt_and_transcribe_to_session("sess_414", 1_400, &mut gk, avatar, &mut bridge).unwrap_err();
    assert_eq!(err_consent, SttEngineError::CloudAudioConsentRequired);
    assert_eq!(orch.network_requests_dispatched(), 0);
    assert!(!orch.is_capture_stream_open());

    let root_str = temp_dir.to_string_lossy().to_string();
    let deny_net_profile = ProjectProfile::new("miku-voice-deny-groq-net", &root_str, 30_000)
        .with_rule(Capability::Input, "microphone:*", PolicyEffect::Allow, true)
        .with_rule(Capability::Network, "api.groq.com*", PolicyEffect::Deny, true);
    let mut gk_deny = GatekeeperEngine::new(deny_net_profile).unwrap();

    orch.set_cloud_audio_consent(true);
    orch.capture_device_mut().restage_pcm(pcm);
    press_default_ptt(&mut orch, 2_000, &mut gk_deny, avatar).unwrap();
    let _ = orch.pump_ptt_capture_chunk(6_400, 2_100, avatar, Some(&mut bridge)).unwrap();
    let err_gk = orch.release_ptt_and_transcribe_to_session("sess_414", 2_400, &mut gk_deny, avatar, &mut bridge).unwrap_err();
    assert!(matches!(err_gk, SttEngineError::GatekeeperDenied(_)));
    assert_eq!(orch.network_requests_dispatched(), 0);
    assert!(!orch.is_capture_stream_open());

    let _ = fs::remove_dir_all(&temp_dir);
}

// LEDGER-4.1-5: API key retrieved strictly by alias via secure manager; typed failures on missing key or HTTP error
#[test]
fn test_api_key_retrieved_only_by_alias_and_typed_failures_without_deterministic_fallback() {
    let temp_dir = create_isolated_voice_4_1_dir("ledger_4_1_5");
    let avatar_dir = resolve_avatar_assets_dir();
    let mut reloader = AvatarPackHotReloader::new(&avatar_dir).expect("avatar reloader");
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new().expect("ipc bridge");
    let mut gk = permissive_groq_voice_gatekeeper(&temp_dir);

    assert!(validate_voice_secret_alias("../bad").is_err());
    assert!(validate_voice_secret_alias("gsk_raw key with spaces").is_err());
    assert!(validate_voice_secret_alias("groq-stt-key").is_ok());

    let pcm = build_synthetic_speech_pcm16(5, 6_400);
    let mut orch_missing = PttSttOrchestrator::new(
        avatar_dir.clone(),
        GroqSttConfig::default().with_consent(true),
        DeterministicFixtureCaptureDevice::with_staged_pcm(pcm.clone()),
        Box::new(InMemoryVoiceSecretVault::new()),
    )
    .unwrap();

    press_default_ptt(&mut orch_missing, 1_000, &mut gk, avatar).unwrap();
    let _ = orch_missing.pump_ptt_capture_chunk(6_400, 1_100, avatar, Some(&mut bridge)).unwrap();
    let err_missing = orch_missing
        .release_ptt_and_transcribe_to_session("sess_415", 1_400, &mut gk, avatar, &mut bridge)
        .unwrap_err();
    assert_eq!(err_missing, SttEngineError::MissingApiKeyAlias(GROQ_DEFAULT_KEY_ALIAS.to_string()));
    assert_eq!(orch_missing.network_requests_dispatched(), 0);

    let (err_endpoint, _) = spawn_local_groq_stt_server(
        401,
        r#"{"error":{"message":"Invalid API Key gsk_leaked_key_in_error_999"}}"#.to_string(),
        0,
    );
    let vault = InMemoryVoiceSecretVault::with_alias_secret(GROQ_DEFAULT_KEY_ALIAS, b"gsk_leaked_key_in_error_999").unwrap();
    let mut orch_http_err = PttSttOrchestrator::new(
        avatar_dir,
        GroqSttConfig::default().with_endpoint(err_endpoint).with_consent(true),
        DeterministicFixtureCaptureDevice::with_staged_pcm(pcm),
        Box::new(vault),
    )
    .unwrap();

    press_default_ptt(&mut orch_http_err, 2_000, &mut gk, avatar).unwrap();
    let _ = orch_http_err.pump_ptt_capture_chunk(6_400, 2_100, avatar, Some(&mut bridge)).unwrap();
    let err_upstream = orch_http_err
        .release_ptt_and_transcribe_to_session("sess_415", 2_400, &mut gk, avatar, &mut bridge)
        .unwrap_err();
    match err_upstream {
        SttEngineError::UpstreamHttpError { status_code, safe_body } => {
            assert_eq!(status_code, 401);
            assert!(!safe_body.contains("gsk_leaked_key_in_error_999"));
            assert!(safe_body.contains("[REDACTED]"));
        }
        other => panic!("expected UpstreamHttpError, got {:?}", other),
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

// LEDGER-4.1-6: Official Groq endpoint/models, HTTPS enforcement, and 10s minimum billing contract
#[test]
fn test_groq_endpoint_https_enforcement_official_models_and_minimum_10s_billing_contract() {
    let def_cfg = GroqSttConfig::default();
    assert_eq!(def_cfg.endpoint_url, "https://api.groq.com/openai/v1/audio/transcriptions");
    assert_eq!(def_cfg.model, GroqWhisperModel::WhisperLargeV3Turbo);
    assert_eq!(def_cfg.model.as_str(), "whisper-large-v3-turbo");
    assert_eq!(GroqWhisperModel::WhisperLargeV3.as_str(), "whisper-large-v3");
    assert!(GroqWhisperModel::parse_id("ggml-tiny.bin").is_err());

    let parsed = parse_groq_endpoint(GROQ_STT_DEFAULT_ENDPOINT).unwrap();
    assert_eq!(parsed.scheme, "https");
    assert_eq!(parsed.host, "api.groq.com");
    assert_eq!(parsed.port, 443);
    assert_eq!(parsed.path, "/openai/v1/audio/transcriptions");

    let insecure = parse_groq_endpoint("http://api.groq.com/openai/v1/audio/transcriptions").unwrap_err();
    assert!(matches!(insecure, SttEngineError::InsecureTransport(_)));

    let short_bill = GroqBillingEstimate::compute(GroqWhisperModel::WhisperLargeV3Turbo, 2_500);
    assert_eq!(short_bill.billed_duration_seconds, 10);
    assert!(short_bill.minimum_billing_applied);
    assert!(short_bill.estimated_cost_usd_nanos > 0);

    let long_bill = GroqBillingEstimate::compute(GroqWhisperModel::WhisperLargeV3, 25_200);
    assert_eq!(long_bill.billed_duration_seconds, 26);
    assert!(!long_bill.minimum_billing_applied);
}

// LEDGER-4.1-7: Zero-leak redaction of API keys, audio, and transcripts + sub-600 line module budget
#[test]
fn test_voice_and_groq_stt_errors_redact_api_keys_audio_and_transcripts_with_zero_leaks() {
    let secret = b"gsk_live_secret_value_xyz987654321";
    let raw_diag = r#"failed near transcript="mi clave secreta es 12345" Authorization: Bearer gsk_live_secret_value_xyz987654321 token=sk-secret999999 pcm=[14320, -12090]"#;
    let safe_diag = redact_sensitive_voice_diagnostic_with_secrets(raw_diag, &[secret]);

    assert!(!safe_diag.contains("mi clave secreta es 12345"));
    assert!(!safe_diag.contains("gsk_live_secret_value_xyz987654321"));
    assert!(!safe_diag.contains("sk-secret999999"));
    assert!(!safe_diag.contains("14320"));
    assert!(safe_diag.contains("[REDACTED]"));

    let err = SttEngineError::from_untrusted_diagnostic("groq_stt", raw_diag, &[secret]);
    let rendered = err.to_string();
    assert!(!rendered.contains("gsk_live_secret_value_xyz987654321"));
    assert!(!rendered.contains("mi clave secreta es 12345"));

    let voice_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("voice");
    for entry in fs::read_dir(&voice_dir).expect("read src/voice") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            let line_count = fs::read_to_string(&path).expect("read rs").lines().count();
            assert!(
                line_count < 600,
                "voice module {} exceeded 600 lines ({})",
                path.display(),
                line_count
            );
        }
    }
}

#[test]
#[ignore = "Optional live Groq STT smoke test; requires MIKU_RUN_LIVE_GROQ_SMOKE=1 and stored credential alias"]
fn test_optional_live_groq_whisper_smoke_when_explicitly_enabled() {
    if std::env::var("MIKU_RUN_LIVE_GROQ_SMOKE").as_deref() != Ok("1") {
        return;
    }
    let store = WindowsCredentialAliasStore::new();
    let key = store
        .load_secret_by_alias(GROQ_DEFAULT_KEY_ALIAS)
        .expect("read credential store")
        .expect("live smoke requires groq-stt-key alias in Windows Credential Manager");
    let mut client = GroqWhisperHttpClient::new();
    let cfg = GroqSttConfig::default().with_consent(true);
    let pcm = build_synthetic_speech_pcm16(77, 16_000);
    let _ = client.transcribe_pcm16_mono(&cfg, &key, &pcm, VOICE_SAMPLE_RATE_HZ);
}
