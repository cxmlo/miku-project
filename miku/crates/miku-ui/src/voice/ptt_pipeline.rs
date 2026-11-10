#![allow(unused_imports)]
use crate::prelude::*;
use std::io::{Read, Write as IoWrite};
use std::net::TcpStream;

pub struct PttSttOrchestrator<C: PttAudioCaptureStream> {
    voice: VoiceInteractionController,
    capture_device: C,
    stt_runtime: OnDemandLocalSttRuntime,
    manifest: SttModelManifest,
    jail: CanonicalPathJail,
    model_file_path: PathBuf,
    session_queue: VoiceSessionOrderQueue,
}

impl<C: PttAudioCaptureStream> PttSttOrchestrator<C> {
    pub fn new(
        base_avatar_dir: PathBuf,
        model_jail_root: &Path,
        manifest: SttModelManifest,
        ttl_ms: u64,
        capture_device: C,
    ) -> Result<Self, SttModelError> {
        Self::with_backend(
            base_avatar_dir,
            model_jail_root,
            manifest,
            ttl_ms,
            capture_device,
            Box::new(WhisperCliLocalBackend::new()),
        )
    }

    pub fn with_backend(
        base_avatar_dir: PathBuf,
        model_jail_root: &Path,
        manifest: SttModelManifest,
        ttl_ms: u64,
        capture_device: C,
        backend: Box<dyn LocalSttDecoderBackend>,
    ) -> Result<Self, SttModelError> {
        let root_str = model_jail_root.to_string_lossy().to_string();
        let jail = CanonicalPathJail::new(&root_str)
            .map_err(|e| SttModelError::PathJailViolation(format!("{:?}", e)))?;
        let confined_str = jail
            .resolve_and_confine(&manifest.model_id)
            .map_err(|e| SttModelError::PathJailViolation(format!("{:?}", e)))?;
        let model_file_path = PathBuf::from(confined_str);

        Ok(Self {
            voice: VoiceInteractionController::new_default(base_avatar_dir),
            capture_device,
            stt_runtime: OnDemandLocalSttRuntime::with_backend(
                ttl_ms,
                DEFAULT_MAX_STT_MODEL_BYTES,
                backend,
            ),
            manifest,
            jail,
            model_file_path,
            session_queue: VoiceSessionOrderQueue::new(),
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
        if !self.model_file_path.is_file() {
            return SttModelAvailability::Missing {
                download_card: self.manifest.to_download_card(),
            };
        }
        let Ok(raw) = fs::read(&self.model_file_path) else {
            return SttModelAvailability::CorruptedOnDisk {
                download_card: self.manifest.to_download_card(),
                reason: "failed to read model file".to_string(),
            };
        };
        match verify_and_parse_stt_model_bytes(&raw, &self.manifest, DEFAULT_MAX_STT_MODEL_BYTES) {
            Ok(_) => SttModelAvailability::VerifiedOnDisk {
                path: self.model_file_path.clone(),
                size_bytes: raw.len(),
            },
            Err(err) => SttModelAvailability::CorruptedOnDisk {
                download_card: self.manifest.to_download_card(),
                reason: format!("{:?}", err),
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
        let verdict = gatekeeper.evaluate_request(
            "miku-voice-ptt",
            Capability::Input,
            "microphone:ptt_stream",
            hotkey_pressed,
            now_ms,
        );
        if verdict != GatekeeperVerdict::Allow {
            return Err(SttModelError::GatekeeperDenied(format!(
                "microphone capture denied by Gatekeeper: {:?}",
                verdict
            )));
        }

        self.voice
            .start_ptt_session(
                hotkey_pressed,
                now_ms,
                fg_before,
                fg_after,
                pill_hwnd,
                avatar,
            )
            .map_err(|e| SttModelError::UiFailure(redact_sensitive_voice_diagnostic(&e.to_string())))?;

        if let Err(err) = self
            .capture_device
            .open_capture_stream(VOICE_SAMPLE_RATE_HZ, VOICE_CHANNELS)
        {
            let _ = self.capture_device.stop_and_close_stream();
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                now_ms,
                avatar,
                None,
            );
            return Err(SttModelError::UiFailure(redact_sensitive_voice_diagnostic(
                &err.to_string(),
            )));
        }
        Ok(())
    }

    pub fn pump_ptt_capture_chunk(
        &mut self,
        max_samples: usize,
        now_ms: u64,
        avatar: &mut ProductionAvatarRuntime,
        bridge: Option<&mut WebView2IpcBridge>,
    ) -> Result<VoiceSoundwaveIndicator, SttModelError> {
        let chunk = match self.capture_device.read_captured_pcm_chunk(max_samples) {
            Ok(c) => c,
            Err(err) => {
                let _ = self.capture_device.stop_and_close_stream();
                let _ = self.voice.cancel_cooperatively(
                    VoiceCancellationReason::ExplicitApiAbort,
                    now_ms,
                    avatar,
                    bridge,
                );
                return Err(SttModelError::UiFailure(redact_sensitive_voice_diagnostic(
                    &err.to_string(),
                )));
            }
        };

        match self.voice.ingest_pcm_frame(&chunk, now_ms, avatar, bridge) {
            Ok(wave) => {
                if *self.voice.state() != VoiceInteractionState::Listening {
                    let _ = self.capture_device.stop_and_close_stream();
                }
                Ok(wave)
            }
            Err(err) => {
                let _ = self.capture_device.stop_and_close_stream();
                let _ = self.voice.zero_and_reclaim();
                Err(SttModelError::UiFailure(redact_sensitive_voice_diagnostic(
                    &err.to_string(),
                )))
            }
        }
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

        let state_after_release = match self
            .voice
            .release_ptt_and_begin_processing(now_ms, avatar, Some(bridge))
        {
            Ok(s) => s,
            Err(err) => {
                let _ = self.voice.zero_and_reclaim();
                return Err(SttModelError::UiFailure(redact_sensitive_voice_diagnostic(
                    &err.to_string(),
                )));
            }
        };

        if let VoiceInteractionState::Cancelled { reason } = state_after_release {
            let _ = self.voice.zero_and_reclaim();
            return Ok(PttPipelineOutcome::Cancelled { reason });
        }

        let mut captured_pcm = self.voice.pcm_buffer().samples().to_vec();

        if !self.model_file_path.is_file() {
            captured_pcm.fill(0);
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                now_ms,
                avatar,
                Some(bridge),
            );
            return Ok(PttPipelineOutcome::ModelMissing {
                download_card: self.manifest.to_download_card(),
            });
        }

        if *self.stt_runtime.state() == SttModelLifecycleState::Unloaded {
            let path_str = self.model_file_path.to_string_lossy().to_string();
            let fs_verdict = gatekeeper.evaluate_request(
                "miku-voice-stt",
                Capability::FsRead,
                &path_str,
                "load_verified_stt_model",
                now_ms,
            );
            if fs_verdict != GatekeeperVerdict::Allow {
                captured_pcm.fill(0);
                let _ = self.voice.cancel_cooperatively(
                    VoiceCancellationReason::ExplicitApiAbort,
                    now_ms,
                    avatar,
                    Some(bridge),
                );
                return Err(SttModelError::GatekeeperDenied(format!(
                    "FsRead denied for STT model: {:?}",
                    fs_verdict
                )));
            }

            let raw_model = match fs::read(&self.model_file_path) {
                Ok(b) => b,
                Err(e) => {
                    captured_pcm.fill(0);
                    let _ = self.voice.cancel_cooperatively(
                        VoiceCancellationReason::ExplicitApiAbort,
                        now_ms,
                        avatar,
                        Some(bridge),
                    );
                    return Err(SttModelError::IoFailure(redact_sensitive_voice_diagnostic(
                        &e.to_string(),
                    )));
                }
            };

            if let Err(err) =
                self.stt_runtime
                    .load_verified_bytes(&raw_model, &self.manifest, now_ms)
            {
                captured_pcm.fill(0);
                let _ = self.voice.cancel_cooperatively(
                    VoiceCancellationReason::ExplicitApiAbort,
                    now_ms,
                    avatar,
                    Some(bridge),
                );
                return Err(err);
            }
        }

        let stt_res = self.stt_runtime.transcribe_pcm16_mono(
            &self.model_file_path,
            &captured_pcm,
            VOICE_SAMPLE_RATE_HZ,
            now_ms,
        );
        captured_pcm.fill(0);

        let transcript = match stt_res {
            Ok(t) => t,
            Err(err) => {
                let _ = self.voice.cancel_cooperatively(
                    VoiceCancellationReason::ExplicitApiAbort,
                    now_ms,
                    avatar,
                    Some(bridge),
                );
                return Err(err);
            }
        };

        let completion_report = self
            .voice
            .complete_utterance(&transcript.text, active_session_id, avatar, bridge)
            .map_err(|e| SttModelError::UiFailure(redact_sensitive_voice_diagnostic(&e.to_string())))?;

        let prompt_text = match &completion_report.transcript_cell {
            WebViewIpcPayloadV1::DomSafeTextRender { safe_text, .. } => safe_text.clone(),
            _ => escape_html_entities_for_text_content(&transcript.text),
        };

        let order = VoiceAgentSessionOrder {
            session_id: active_session_id.to_string(),
            prompt_text,
            language: transcript.language,
            ipc_envelope_id: completion_report.ipc_envelope_id.clone(),
            enqueued_at_ms: now_ms,
        };
        self.session_queue.enqueue(order.clone());

        Ok(PttPipelineOutcome::TranscribedAndEnqueued {
            transcript,
            order,
            completion_report,
            device_zero_report,
        })
    }

    pub fn abort_and_cleanup(
        &mut self,
        reason: VoiceCancellationReason,
        now_ms: u64,
        avatar: &mut ProductionAvatarRuntime,
        bridge: Option<&mut WebView2IpcBridge>,
    ) -> Result<VoiceCancelReceipt, SttModelError> {
        let _ = self.capture_device.stop_and_close_stream();
        self.voice
            .cancel_cooperatively(reason, now_ms, avatar, bridge)
            .map_err(|e| SttModelError::UiFailure(redact_sensitive_voice_diagnostic(&e.to_string())))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn trip_kill_switch_and_purge(
        &mut self,
        kill_switch: &mut UiKillSwitchManager,
        now_ms: u64,
        raw_reason: &str,
        gatekeeper: &mut GatekeeperEngine,
        avatar: &mut ProductionAvatarRuntime,
        audit_store: &mut PersistentJsonlAuditStore,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<KillSwitchTripReport, SttModelError> {
        let _ = self.capture_device.stop_and_close_stream();
        let _ = self.stt_runtime.unload_and_zero();
        let _ = self.session_queue.clear_all();
        kill_switch
            .trip(
                now_ms,
                raw_reason,
                gatekeeper,
                &mut self.voice,
                avatar,
                audit_store,
                bridge,
            )
            .map_err(|e| SttModelError::UiFailure(redact_sensitive_voice_diagnostic(&e.to_string())))
    }

    pub fn download_and_verify_model_explicit(
        &mut self,
        override_url: &str,
        user_confirmed: bool,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
    ) -> Result<ModelDownloadReceipt, SttModelError> {
        if !user_confirmed {
            return Err(SttModelError::UserConfirmationRequired);
        }

        let target_url = if override_url.trim().is_empty() {
            self.manifest.download_url.as_str()
        } else {
            override_url.trim()
        };
        let host_target = extract_http_host_port(target_url);
        let net_verdict = gatekeeper.evaluate_request(
            "miku-voice-stt",
            Capability::Network,
            &host_target,
            target_url,
            now_ms,
        );
        if net_verdict != GatekeeperVerdict::Allow {
            return Err(SttModelError::GatekeeperDenied(format!(
                "Network download denied by Gatekeeper: {:?}",
                net_verdict
            )));
        }

        let final_str = self.model_file_path.to_string_lossy().to_string();
        let fs_verdict = gatekeeper.evaluate_request(
            "miku-voice-stt",
            Capability::FsWrite,
            &final_str,
            "write_verified_stt_model",
            now_ms,
        );
        if fs_verdict != GatekeeperVerdict::Allow {
            return Err(SttModelError::GatekeeperDenied(format!(
                "FsWrite denied by Gatekeeper: {:?}",
                fs_verdict
            )));
        }

        let raw_bytes = fetch_http_bytes_bounded(target_url, DEFAULT_MAX_STT_MODEL_BYTES)?;
        let part_rel = format!("{}.part", self.manifest.model_id);
        let part_confined = self
            .jail
            .resolve_and_confine(&part_rel)
            .map_err(|e| SttModelError::PathJailViolation(format!("{:?}", e)))?;
        let part_path = PathBuf::from(part_confined);

        fs::write(&part_path, &raw_bytes)
            .map_err(|e| SttModelError::IoFailure(redact_sensitive_voice_diagnostic(&e.to_string())))?;

        let verified = match verify_and_parse_stt_model_bytes(
            &raw_bytes,
            &self.manifest,
            DEFAULT_MAX_STT_MODEL_BYTES,
        ) {
            Ok(v) => v,
            Err(err) => {
                zero_and_remove_part_file(&part_path, raw_bytes.len());
                return Err(err);
            }
        };

        if let Err(e) = fs::rename(&part_path, &self.model_file_path) {
            zero_and_remove_part_file(&part_path, raw_bytes.len());
            return Err(SttModelError::IoFailure(redact_sensitive_voice_diagnostic(
                &e.to_string(),
            )));
        }

        Ok(ModelDownloadReceipt {
            model_id: self.manifest.model_id.clone(),
            final_path: self.model_file_path.clone(),
            verified_size_bytes: raw_bytes.len(),
            verified_sha256_hex: verified.sha256_hex,
        })
    }

    pub fn poll_inactivity_ttl(&mut self, now_ms: u64) -> bool {
        self.stt_runtime.poll_ttl(now_ms)
    }
}

fn zero_and_remove_part_file(path: &Path, byte_len: usize) {
    if path.is_file() {
        let zeros = vec![0u8; byte_len.min(256 * 1024)];
        let _ = fs::write(path, &zeros);
        let _ = fs::remove_file(path);
    }
}

fn extract_http_host_port(url: &str) -> String {
    let stripped = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    stripped.split('/').next().unwrap_or(stripped).to_string()
}

fn fetch_http_bytes_bounded(url: &str, max_bytes: usize) -> Result<Vec<u8>, SttModelError> {
    let stripped = url.strip_prefix("http://").ok_or_else(|| {
        SttModelError::IoFailure("explicit loopback HTTP transport required".to_string())
    })?;
    let (host_port, path_part) = match stripped.find('/') {
        Some(idx) => (&stripped[..idx], &stripped[idx..]),
        None => (stripped, "/"),
    };
    let mut stream = TcpStream::connect(host_port)
        .map_err(|e| SttModelError::IoFailure(redact_sensitive_voice_diagnostic(&e.to_string())))?;
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path_part, host_port
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| SttModelError::IoFailure(redact_sensitive_voice_diagnostic(&e.to_string())))?;

    let mut raw_resp = Vec::new();
    stream
        .read_to_end(&mut raw_resp)
        .map_err(|e| SttModelError::IoFailure(redact_sensitive_voice_diagnostic(&e.to_string())))?;

    let Some(hdr_end) = raw_resp.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err(SttModelError::IoFailure(
            "malformed HTTP response header".to_string(),
        ));
    };
    let body = &raw_resp[hdr_end + 4..];
    if body.len() > max_bytes {
        return Err(SttModelError::SizeExceedsMaxLimit {
            actual_bytes: body.len(),
            max_bytes,
        });
    }
    Ok(body.to_vec())
}
