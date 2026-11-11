#![allow(unused_imports)]
use crate::prelude::*;
use std::sync::atomic::{compiler_fence, Ordering as AtomicOrdering};

pub struct PttSttOrchestrator<C: PttAudioCaptureStream> {
    voice: VoiceInteractionController,
    capture_device: C,
    config: GroqSttConfig,
    secret_store: Box<dyn VoiceSecretAliasStore>,
    transport: Box<dyn GroqSttTransport>,
    ducking: AudioDuckingController,
    wake_word_config: OptionalWakeWordConfig,
    session_queue: VoiceSessionOrderQueue,
    network_requests_dispatched: u64,
    total_billed_seconds: u64,
}

impl<C: PttAudioCaptureStream> PttSttOrchestrator<C> {
    pub fn new(
        base_avatar_dir: PathBuf,
        config: GroqSttConfig,
        capture_device: C,
        secret_store: Box<dyn VoiceSecretAliasStore>,
    ) -> Result<Self, SttEngineError> {
        Self::with_transport(
            base_avatar_dir,
            config,
            capture_device,
            secret_store,
            Box::new(GroqWhisperHttpClient::new()),
        )
    }

    pub fn with_transport(
        base_avatar_dir: PathBuf,
        config: GroqSttConfig,
        capture_device: C,
        secret_store: Box<dyn VoiceSecretAliasStore>,
        transport: Box<dyn GroqSttTransport>,
    ) -> Result<Self, SttEngineError> {
        validate_voice_secret_alias(&config.api_key_alias)?;
        let _ = parse_groq_endpoint(&config.endpoint_url)?;

        Ok(Self {
            voice: VoiceInteractionController::new_default(base_avatar_dir),
            capture_device,
            config,
            secret_store,
            transport,
            ducking: AudioDuckingController::new_default_winmm(),
            wake_word_config: OptionalWakeWordConfig::default(),
            session_queue: VoiceSessionOrderQueue::new(),
            network_requests_dispatched: 0,
            total_billed_seconds: 0,
        })
    }

    pub fn with_ducking_controller(mut self, ducking: AudioDuckingController) -> Self {
        self.ducking = ducking;
        self
    }

    pub fn ducking(&self) -> &AudioDuckingController {
        &self.ducking
    }

    pub fn ducking_mut(&mut self) -> &mut AudioDuckingController {
        &mut self.ducking
    }

    pub fn wake_word_config(&self) -> &OptionalWakeWordConfig {
        &self.wake_word_config
    }

    pub fn set_wake_word_enabled(&mut self, enabled: bool) {
        self.wake_word_config.enabled_by_user = enabled;
    }

    pub fn set_wake_word_power_policy(&mut self, policy: VoicePowerPolicy) {
        self.wake_word_config.power_policy = policy;
        self.voice.update_power_policy(policy);
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

    pub fn config(&self) -> &GroqSttConfig {
        &self.config
    }

    pub fn set_cloud_audio_consent(&mut self, granted: bool) {
        self.config.cloud_audio_consent_granted = granted;
    }

    pub fn set_endpoint_url(&mut self, endpoint_url: impl Into<String>) -> Result<(), SttEngineError> {
        let url = endpoint_url.into();
        let _ = parse_groq_endpoint(&url)?;
        self.config.endpoint_url = url;
        Ok(())
    }

    pub fn set_api_key_alias(&mut self, alias: impl Into<String>) -> Result<(), SttEngineError> {
        let a = alias.into();
        validate_voice_secret_alias(&a)?;
        self.config.api_key_alias = a;
        Ok(())
    }

    pub fn session_queue(&self) -> &VoiceSessionOrderQueue {
        &self.session_queue
    }

    pub fn is_capture_stream_open(&self) -> bool {
        self.capture_device.is_stream_open()
    }

    pub const fn network_requests_dispatched(&self) -> u64 {
        self.network_requests_dispatched
    }

    pub const fn total_billed_seconds(&self) -> u64 {
        self.total_billed_seconds
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
    ) -> Result<(), SttEngineError> {
        let verdict = gatekeeper.evaluate_request(
            "miku-voice-ptt",
            Capability::Input,
            "microphone:ptt_stream",
            hotkey_pressed,
            now_ms,
        );
        if !matches!(verdict, GatekeeperVerdict::Allow { .. }) {
            return Err(SttEngineError::GatekeeperDenied(format!(
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
            .map_err(|e| {
                SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(&e.to_string()))
            })?;

        if let Err(err) = self
            .capture_device
            .open_capture_stream(VOICE_SAMPLE_RATE_HZ, VOICE_CHANNELS)
        {
            let _ = self.capture_device.stop_and_close_stream();
            let _ = self.ducking.restore_original_volume(now_ms);
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                now_ms,
                avatar,
                None,
            );
            return Err(SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(
                &err.to_string(),
            )));
        }

        if let Err(err) = self.ducking.engage_ducking(now_ms) {
            let _ = self.capture_device.stop_and_close_stream();
            let _ = self.ducking.restore_original_volume(now_ms);
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                now_ms,
                avatar,
                None,
            );
            return Err(SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(
                &err.to_string(),
            )));
        }
        Ok(())
    }

    pub fn trigger_optional_wake_word_and_open_stream(
        &mut self,
        confidence: f32,
        now_ms: u64,
        gatekeeper: &mut GatekeeperEngine,
        avatar: &mut ProductionAvatarRuntime,
    ) -> Result<(), SttEngineError> {
        if !self.wake_word_config.is_listening_allowed() {
            return Err(SttEngineError::UiFailure(
                "optional wake word is disabled by default or suspended by power policy".to_string(),
            ));
        }

        let verdict = gatekeeper.evaluate_request(
            "miku-voice-wakeword",
            Capability::Input,
            "microphone:wake_word_stream",
            &self.wake_word_config.phrase,
            now_ms,
        );
        if !matches!(verdict, GatekeeperVerdict::Allow { .. }) {
            return Err(SttEngineError::GatekeeperDenied(format!(
                "wake-word microphone capture denied by Gatekeeper: {:?}",
                verdict
            )));
        }

        self.voice
            .on_wake_word_detected(confidence, now_ms, avatar)
            .map_err(|e| {
                SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(&e.to_string()))
            })?;

        if let Err(err) = self
            .capture_device
            .open_capture_stream(VOICE_SAMPLE_RATE_HZ, VOICE_CHANNELS)
        {
            let _ = self.capture_device.stop_and_close_stream();
            let _ = self.ducking.restore_original_volume(now_ms);
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                now_ms,
                avatar,
                None,
            );
            return Err(SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(
                &err.to_string(),
            )));
        }

        if let Err(err) = self.ducking.engage_ducking(now_ms) {
            let _ = self.capture_device.stop_and_close_stream();
            let _ = self.ducking.restore_original_volume(now_ms);
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                now_ms,
                avatar,
                None,
            );
            return Err(SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(
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
    ) -> Result<VoiceSoundwaveIndicator, SttEngineError> {
        let chunk = match self.capture_device.read_captured_pcm_chunk(max_samples) {
            Ok(c) => c,
            Err(err) => {
                let _ = self.capture_device.stop_and_close_stream();
                let _ = self.ducking.restore_original_volume(now_ms);
                let _ = self.voice.cancel_cooperatively(
                    VoiceCancellationReason::ExplicitApiAbort,
                    now_ms,
                    avatar,
                    bridge,
                );
                return Err(SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(
                    &err.to_string(),
                )));
            }
        };

        match self.voice.ingest_pcm_frame(&chunk, now_ms, avatar, bridge) {
            Ok(wave) => {
                if *self.voice.state() != VoiceInteractionState::Listening {
                    let _ = self.capture_device.stop_and_close_stream();
                    let _ = self.ducking.restore_original_volume(now_ms);
                }
                Ok(wave)
            }
            Err(err) => {
                let _ = self.capture_device.stop_and_close_stream();
                let _ = self.ducking.restore_original_volume(now_ms);
                let _ = self.voice.zero_and_reclaim();
                Err(SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(
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
    ) -> Result<PttPipelineOutcome, SttEngineError> {
        // 1. Always close the microphone stream, restore system audio volume to 100% of original,
        // and zero hardware buffers BEFORE any cloud call.
        let device_zero_report = self.capture_device.stop_and_close_stream();
        let _ = self.ducking.restore_original_volume(now_ms);

        let state_after_release = match self
            .voice
            .release_ptt_and_begin_processing(now_ms, avatar, Some(bridge))
        {
            Ok(s) => s,
            Err(err) => {
                let _ = self.voice.zero_and_reclaim();
                return Err(SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(
                    &err.to_string(),
                )));
            }
        };

        if let VoiceInteractionState::Cancelled { reason } = state_after_release {
            let _ = self.voice.zero_and_reclaim();
            return Ok(PttPipelineOutcome::Cancelled { reason });
        }

        let mut captured_pcm = self.voice.pcm_buffer().samples().to_vec();
        let _ = self.voice.zero_and_reclaim();

        // 2. Enforce explicit cloud audio consent before reading credentials or touching network
        if !self.config.cloud_audio_consent_granted {
            zeroize_i16_slice(&mut captured_pcm);
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                now_ms,
                avatar,
                Some(bridge),
            );
            return Err(SttEngineError::CloudAudioConsentRequired);
        }

        // 3. Validate endpoint transport security and Gatekeeper Network authorization
        let endpoint_target = match parse_groq_endpoint(&self.config.endpoint_url) {
            Ok(t) => t,
            Err(err) => {
                zeroize_i16_slice(&mut captured_pcm);
                let _ = self.voice.cancel_cooperatively(
                    VoiceCancellationReason::ExplicitApiAbort,
                    now_ms,
                    avatar,
                    Some(bridge),
                );
                return Err(err);
            }
        };

        let net_verdict = gatekeeper.evaluate_request(
            "miku-voice-stt",
            Capability::Network,
            &endpoint_target.host,
            &self.config.endpoint_url,
            now_ms,
        );
        if !matches!(net_verdict, GatekeeperVerdict::Allow { .. }) {
            zeroize_i16_slice(&mut captured_pcm);
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                now_ms,
                avatar,
                Some(bridge),
            );
            return Err(SttEngineError::GatekeeperDenied(format!(
                "Network access to Groq STT host '{}' denied by Gatekeeper: {:?}",
                endpoint_target.host, net_verdict
            )));
        }

        // 4. Authorize and retrieve API key strictly by alias
        if let Err(err) = validate_voice_secret_alias(&self.config.api_key_alias) {
            zeroize_i16_slice(&mut captured_pcm);
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                now_ms,
                avatar,
                Some(bridge),
            );
            return Err(err);
        }

        let cred_verdict = gatekeeper.evaluate_request(
            "secret-manager",
            Capability::Network,
            &self.config.api_key_alias,
            r#"{"action":"read_credential"}"#,
            now_ms,
        );
        if !matches!(cred_verdict, GatekeeperVerdict::Allow { .. }) {
            zeroize_i16_slice(&mut captured_pcm);
            let _ = self.voice.cancel_cooperatively(
                VoiceCancellationReason::ExplicitApiAbort,
                now_ms,
                avatar,
                Some(bridge),
            );
            return Err(SttEngineError::GatekeeperDenied(format!(
                "Credential alias '{}' denied by Gatekeeper: {:?}",
                self.config.api_key_alias, cred_verdict
            )));
        }

        let mut api_key = match self
            .secret_store
            .load_secret_by_alias(&self.config.api_key_alias)
        {
            Ok(Some(k)) if !k.is_empty() => k,
            Ok(_) => {
                zeroize_i16_slice(&mut captured_pcm);
                let _ = self.voice.cancel_cooperatively(
                    VoiceCancellationReason::ExplicitApiAbort,
                    now_ms,
                    avatar,
                    Some(bridge),
                );
                return Err(SttEngineError::MissingApiKeyAlias(
                    self.config.api_key_alias.clone(),
                ));
            }
            Err(err) => {
                zeroize_i16_slice(&mut captured_pcm);
                let _ = self.voice.cancel_cooperatively(
                    VoiceCancellationReason::ExplicitApiAbort,
                    now_ms,
                    avatar,
                    Some(bridge),
                );
                return Err(err);
            }
        };

        // 5. Dispatch audio to Groq STT endpoint and immediately zeroize PCM & API key
        self.network_requests_dispatched = self.network_requests_dispatched.saturating_add(1);
        let stt_res = self.transport.transcribe_pcm16_mono(
            &self.config,
            &api_key,
            &captured_pcm,
            VOICE_SAMPLE_RATE_HZ,
        );
        zeroize_i16_slice(&mut captured_pcm);
        api_key.zeroize_now();

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

        self.total_billed_seconds = self
            .total_billed_seconds
            .saturating_add(transcript.billing.billed_duration_seconds);

        let completion_report = self
            .voice
            .complete_utterance(&transcript.text, active_session_id, avatar, bridge)
            .map_err(|e| {
                SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(&e.to_string()))
            })?;

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
    ) -> Result<VoiceCancelReceipt, SttEngineError> {
        let _ = self.capture_device.stop_and_close_stream();
        let _ = self.ducking.restore_original_volume(now_ms);
        self.voice
            .cancel_cooperatively(reason, now_ms, avatar, bridge)
            .map_err(|e| {
                SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(&e.to_string()))
            })
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
    ) -> Result<KillSwitchTripReport, SttEngineError> {
        let _ = self.capture_device.stop_and_close_stream();
        let _ = self.ducking.restore_original_volume(now_ms);
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
            .map_err(|e| {
                SttEngineError::UiFailure(redact_sensitive_voice_diagnostic(&e.to_string()))
            })
    }
}

fn zeroize_i16_slice(buf: &mut Vec<i16>) {
    for sample in buf.iter_mut() {
        unsafe {
            std::ptr::write_volatile(sample, 0);
        }
    }
    compiler_fence(AtomicOrdering::SeqCst);
    buf.clear();
    buf.shrink_to_fit();
}
