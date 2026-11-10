#![allow(unused_imports)]
use crate::prelude::*;
use std::io::{Read, Write as IoWrite};
use std::net::TcpStream;
use std::time::Duration;

pub struct PttSttOrchestrator<C: PttAudioCaptureStream> {
    voice: VoiceInteractionController,
    capture_device: C,
    stt_runtime: OnDemandLocalSttRuntime,
    model_jail: CanonicalPathJail,
    model_file_path: PathBuf,
    manifest: SttModelManifest,
    session_queue: VoiceSessionOrderQueue,
    active_ptt_started_ms: Option<u64>,
}

impl<C: PttAudioCaptureStream> PttSttOrchestrator<C> {
    pub fn new(
        avatar_dir: PathBuf,
        model_dir: &Path,
        manifest: SttModelManifest,
        model_ttl_ms: u64,
        capture_device: C,
    ) -> Result<Self, SttModelError> {
        fs::create_dir_all(model_dir).map_err(|e| SttModelError::PathJailViolation(e.to_string()))?;
        let model_jail = CanonicalPathJail::new(model_dir)
            .map_err(|e| SttModelError::PathJailViolation(e.to_string()))?;
        let filename = format!("{}.mkstt", manifest.model_id);
        let model_file_path = model_jail
            .resolve_inside_jail(Path::new(&filename))
            .map_err(|e| SttModelError::PathJailViolation(e.to_string()))?;

        Ok(Self {
            voice: VoiceInteractionController::new(avatar_dir),
            capture_device,
            stt_runtime: OnDemandLocalSttRuntime::new(model_ttl_ms, DEFAULT_MAX_STT_MODEL_BYTES),
            model_jail,
            model_file_path,
            manifest,
            session_queue: VoiceSessionOrderQueue::new(),
            active_ptt_started_ms: None,
        })
    }

    pub fn voice(&self) -> &VoiceInteractionController {
        &self.voice
    }

    pub fn capture_device(&self) -> &C {
        &self.capture_device
    }

    pub fn capture_device_mut(&mut self) -> &mut C {
        &mut self.capture_device
    }

    pub fn stt_runtime(&self) -> &OnDemandLocalSttRuntime {
        &self.stt_runtime
    }

    pub fn session_queue(&self) -> &VoiceSessionOrderQueue {
        &self.session_queue
    }

    pub fn is_capture_stream_open(&self) -> bool {
        self.capture_device.is_stream_open()
    }

    pub fn allocated_model_bytes(&self) -> usize {
        self.stt_runtime.allocated_weight_bytes()
    }

    pub fn model_file_path(&self) -> &Path {
        &self.model_file_path
    }

    pub fn inspect_model_availability(&self) -> SttModelAvailability {
        let card = self.manifest.to_download_card(DEFAULT_STT_DOWNLOAD_URL);
        if !self.model_file_path.exists() {
            return SttModelAvailability::Missing { download_card: card };
        }
        match fs::read(&self.model_file_path) {
            Ok(bytes) => match verify_and_parse_stt_model_bytes(&bytes, &self.manifest, DEFAULT_MAX_STT_MODEL_BYTES) {
                Ok(mut w) => {
                    w.zero_and_release();
                    SttModelAvailability::VerifiedOnDisk {
                        path: self.model_file_path.to_string_lossy().to_string(),
                        manifest: self.manifest.clone(),
                    }
                }
                Err(e) => SttModelAvailability::CorruptedOnDisk {
                    reason: e.to_string(),
                    download_card: card,
                },
            },
            Err(e) => SttModelAvailability::CorruptedOnDisk {
                reason: redact_sensitive_voice_diagnostic(&e.to_string()),
                download_card: card,
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn press_ptt_and_open_stream(
        &mut self,
        hotkey_pressed: &str,
        now_ms: u64,
        fg_before: usize,
        fg_after: usize,
        pill_hwnd: usize,
        gatekeeper: &mut GatekeeperEngine,
        avatar: &mut ProductionAvatarRuntime,
    ) -> Result<(), SttModelError> {
        if gatekeeper.is_kill_switch_active() {
            let _ = self.capture_device.stop_and_close_stream();
            return Err(SttModelError::KillSwitchActive);
        }
        gatekeeper
            .authorize_action(Capability::Input, "microphone:ptt_capture")
            .map_err(|e| SttModelError::GatekeeperDenied(e.to_string()))?;

        self.voice
            .trigger_push_to_talk(hotkey_pressed, now_ms, fg_before, fg_after, pill_hwnd, avatar)
            .map_err(|e| SttModelError::VoiceControllerError(e.to_string()))?;

        if let Err(err) = self.capture_device.open_capture_stream(VOICE_SAMPLE_RATE_HZ, VOICE_CHANNELS) {
            let _ = self.capture_device.stop_and_close_stream();
            let _ = self.voice.cancel_immediately(
                VoiceCancellationReason::ExternalAbort { reason: "capture_open_failed".to_string() },
                now_ms,
                avatar,
                None,
            );
            return Err(SttModelError::CaptureDeviceError(err));
        }
        self.active_ptt_started_ms = Some(now_ms);
        Ok(())
    }

    pub fn pump_ptt_capture_chunk(
        &mut self,
        max_samples: usize,
        now_ms: u64,
        avatar: &mut ProductionAvatarRuntime,
        bridge: Option<&mut WebView2IpcBridge>,
    ) -> Result<VoiceSoundwaveIndicator, SttModelError> {
        let samples = match self.capture_device.read_captured_pcm_chunk(max_samples) {
            Ok(s) => s,
            Err(err) => {
                let _ = self.capture_device.stop_and_close_stream();
                let _ = self.voice.cancel_immediately(
                    VoiceCancellationReason::ExternalAbort { reason: "capture_read_error".to_string() },
                    now_ms,
                    avatar,
                    bridge,
                );
                self.active_ptt_started_ms = None;
                return Err(SttModelError::CaptureDeviceError(err));
            }
        };
        if let Err(ui_err) = self.voice.push_captured_audio_frame(&samples, now_ms, avatar) {
            let _ = self.capture_device.stop_and_close_stream();
            self.active_ptt_started_ms = None;
            return Err(SttModelError::VoiceControllerError(ui_err.to_string()));
        }
        Ok(self.voice.soundwave().clone())
    }

    pub fn release_ptt_and_transcribe_to_session(
        &mut self,
        active_session_id: &str,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
        avatar: &mut ProductionAvatarRuntime,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<PttPipelineOutcome, SttModelError> {
        let device_zero_report = self.capture_device.stop_and_close_stream();
        let started_ms = self.active_ptt_started_ms.take().unwrap_or(now_ms);
        let held_ms = now_ms.saturating_sub(started_ms);

        if gatekeeper.is_kill_switch_active() {
            let _ = self.voice.cancel_immediately(
                VoiceCancellationReason::ExternalAbort { reason: "kill_switch_active".to_string() },
                now_ms,
                avatar,
                Some(bridge),
            );
            self.stt_runtime.unload_and_zero();
            return Err(SttModelError::KillSwitchActive);
        }

        if held_ms < PTT_MIN_HOLD_MS {
            let reason = VoiceCancellationReason::PttReleasedTooEarly { held_ms };
            let _ = self.voice.cancel_immediately(reason.clone(), now_ms, avatar, Some(bridge));
            return Ok(PttPipelineOutcome::Cancelled { reason });
        }

        let mut captured_pcm = self.voice.pcm_buffer().samples().to_vec();
        let jailed_model = match self.model_jail.resolve_inside_jail(&self.model_file_path) {
            Ok(p) => p,
            Err(e) => {
                captured_pcm.fill(0);
                let _ = self.voice.cancel_immediately(
                    VoiceCancellationReason::ExternalAbort { reason: "model_jail_error".to_string() },
                    now_ms,
                    avatar,
                    Some(bridge),
                );
                return Err(SttModelError::PathJailViolation(e.to_string()));
            }
        };

        if !jailed_model.exists() {
            captured_pcm.fill(0);
            let _ = self.voice.cancel_immediately(
                VoiceCancellationReason::ExternalAbort { reason: "stt_model_missing".to_string() },
                now_ms,
                avatar,
                Some(bridge),
            );
            return Ok(PttPipelineOutcome::ModelMissing {
                download_card: self.manifest.to_download_card(DEFAULT_STT_DOWNLOAD_URL),
            });
        }

        if matches!(self.stt_runtime.state(), SttModelLifecycleState::Unloaded) {
            gatekeeper
                .authorize_action(Capability::FsRead, &jailed_model.to_string_lossy())
                .map_err(|e| {
                    captured_pcm.fill(0);
                    let _ = self.voice.cancel_immediately(
                        VoiceCancellationReason::ExternalAbort { reason: "fs_read_denied".to_string() },
                        now_ms,
                        avatar,
                        Some(bridge),
                    );
                    SttModelError::GatekeeperDenied(e.to_string())
                })?;

            let raw_bytes = fs::read(&jailed_model).map_err(|e| {
                captured_pcm.fill(0);
                let _ = self.voice.cancel_immediately(
                    VoiceCancellationReason::ExternalAbort { reason: "model_read_failed".to_string() },
                    now_ms,
                    avatar,
                    Some(bridge),
                );
                SttModelError::InvalidModelHeader(e.to_string())
            })?;

            if let Err(err) = self.stt_runtime.load_verified_bytes(&raw_bytes, &self.manifest, now_ms) {
                captured_pcm.fill(0);
                let _ = self.voice.cancel_immediately(
                    VoiceCancellationReason::ExternalAbort { reason: "model_verify_failed".to_string() },
                    now_ms,
                    avatar,
                    Some(bridge),
                );
                return Err(err);
            }
        }

        self.voice
            .release_push_to_talk_to_thinking(now_ms, avatar, Some(bridge))
            .map_err(|e| {
                captured_pcm.fill(0);
                SttModelError::VoiceControllerError(e.to_string())
            })?;

        let transcript_res = self.stt_runtime.transcribe_pcm16_mono(&captured_pcm, VOICE_SAMPLE_RATE_HZ, now_ms);
        captured_pcm.fill(0);
        std::hint::black_box(&captured_pcm);

        let transcript = match transcript_res {
            Ok(t) => t,
            Err(err) => {
                let _ = self.voice.cancel_immediately(
                    VoiceCancellationReason::ExternalAbort { reason: "stt_inference_error".to_string() },
                    now_ms,
                    avatar,
                    Some(bridge),
                );
                return Err(err);
            }
        };

        let completion_report = self
            .voice
            .complete_utterance_and_zero_buffer(&transcript.text, now_ms, avatar, Some(bridge))
            .map_err(|e| SttModelError::VoiceControllerError(e.to_string()))?;

        let ipc_id = format!("msg_voice_order_{}", now_ms);
        let env = EnvelopeV1::new(
            ipc_id.clone(),
            now_ms,
            MikuLinkMessageV1::Command {
                session_id: active_session_id.to_string(),
                action: "voice_enqueue_prompt".to_string(),
                text: Some(transcript.text.clone()),
            },
        );
        let _ = bridge.queue_host_envelope(&env);
        let order = self.session_queue.enqueue(active_session_id, &transcript, now_ms, ipc_id);

        Ok(PttPipelineOutcome::TranscribedAndEnqueued {
            transcript,
            order,
            completion_report,
            device_zero_report,
        })
    }

    pub fn download_and_verify_model_explicit(
        &mut self,
        download_url: &str,
        user_confirmed: bool,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
    ) -> Result<ModelDownloadReceipt, SttModelError> {
        if !user_confirmed {
            return Err(SttModelError::UserConfirmationRequired);
        }
        if gatekeeper.is_kill_switch_active() {
            return Err(SttModelError::KillSwitchActive);
        }
        let jailed_dest = self
            .model_jail
            .resolve_inside_jail(&self.model_file_path)
            .map_err(|e| SttModelError::PathJailViolation(e.to_string()))?;

        let (host_port, req_path) = parse_loopback_http_url(download_url)?;
        gatekeeper
            .authorize_action(Capability::Network, &host_port)
            .map_err(|e| SttModelError::GatekeeperDenied(e.to_string()))?;
        gatekeeper
            .authorize_action(Capability::FsWrite, &jailed_dest.to_string_lossy())
            .map_err(|e| SttModelError::GatekeeperDenied(e.to_string()))?;

        let downloaded_bytes = fetch_http_bytes_bounded(&host_port, &req_path, DEFAULT_MAX_STT_MODEL_BYTES)?;
        let mut verified = verify_and_parse_stt_model_bytes(&downloaded_bytes, &self.manifest, DEFAULT_MAX_STT_MODEL_BYTES)?;
        verified.zero_and_release();

        let tmp_path = jailed_dest.with_extension("mkstt.part");
        if let Err(e) = fs::write(&tmp_path, &downloaded_bytes) {
            let _ = fs::remove_file(&tmp_path);
            return Err(SttModelError::DownloadTransportError(e.to_string()));
        }
        if let Err(e) = fs::rename(&tmp_path, &jailed_dest) {
            let _ = fs::remove_file(&tmp_path);
            return Err(SttModelError::DownloadTransportError(e.to_string()));
        }

        Ok(ModelDownloadReceipt {
            model_id: self.manifest.model_id.clone(),
            verified_size_bytes: downloaded_bytes.len(),
            verified_sha256_hex: self.manifest.expected_sha256_hex.clone(),
            saved_path: jailed_dest.to_string_lossy().to_string(),
            downloaded_at_ms: now_ms,
        })
    }

    pub fn abort_and_cleanup(
        &mut self,
        reason: VoiceCancellationReason,
        now_ms: u64,
        avatar: &mut ProductionAvatarRuntime,
        bridge: Option<&mut WebView2IpcBridge>,
    ) -> Result<VoiceCancelReceipt, UiError> {
        let _ = self.capture_device.stop_and_close_stream();
        self.active_ptt_started_ms = None;
        self.voice.cancel_immediately(reason, now_ms, avatar, bridge)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn trip_kill_switch_and_purge(
        &mut self,
        kill_switch: &mut UiKillSwitchManager,
        now_ms: u64,
        reason: &str,
        gatekeeper: &mut GatekeeperEngine,
        avatar: &mut ProductionAvatarRuntime,
        audit_store: &mut PersistentJsonlAuditStore,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<KillSwitchTripReport, UiError> {
        let _ = self.capture_device.stop_and_close_stream();
        self.active_ptt_started_ms = None;
        self.stt_runtime.unload_and_zero();
        self.session_queue.clear_and_zero_all();
        kill_switch.trip_emergency_stop(
            now_ms,
            reason,
            gatekeeper,
            &mut self.voice,
            avatar,
            audit_store,
            bridge,
        )
    }

    pub fn poll_inactivity_ttl(&mut self, now_ms: u64) -> bool {
        self.stt_runtime.poll_ttl(now_ms)
    }
}

impl<C: PttAudioCaptureStream> Drop for PttSttOrchestrator<C> {
    fn drop(&mut self) {
        let _ = self.capture_device.stop_and_close_stream();
        self.stt_runtime.unload_and_zero();
        self.session_queue.clear_and_zero_all();
    }
}

fn parse_loopback_http_url(url: &str) -> Result<(String, String), SttModelError> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| SttModelError::DownloadTransportError("unsupported URL scheme".to_string()))?;
    let (host_port, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if host_port.is_empty() {
        return Err(SttModelError::DownloadTransportError("missing host in download URL".to_string()));
    }
    Ok((host_port.to_string(), path.to_string()))
}

fn fetch_http_bytes_bounded(host_port: &str, path: &str, max_bytes: usize) -> Result<Vec<u8>, SttModelError> {
    let mut stream = TcpStream::connect(host_port)
        .map_err(|e| SttModelError::DownloadTransportError(e.to_string()))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(3_000)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(3_000)));
    let req = format!("GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", path, host_port);
    stream
        .write_all(req.as_bytes())
        .map_err(|e| SttModelError::DownloadTransportError(e.to_string()))?;

    let mut raw_resp = Vec::new();
    stream
        .take((max_bytes + 8192) as u64)
        .read_to_end(&mut raw_resp)
        .map_err(|e| SttModelError::DownloadTransportError(e.to_string()))?;

    let header_end = raw_resp
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| SttModelError::DownloadTransportError("malformed HTTP response headers".to_string()))?;
    let headers_str = String::from_utf8_lossy(&raw_resp[..header_end]);
    if !headers_str.starts_with("HTTP/1.1 200") && !headers_str.starts_with("HTTP/1.0 200") {
        return Err(SttModelError::DownloadTransportError("non-200 HTTP status".to_string()));
    }
    Ok(raw_resp[header_end + 4..].to_vec())
}
