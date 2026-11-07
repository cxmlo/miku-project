#![allow(unused_imports)]
use crate::prelude::*;

impl VoiceInteractionController {
    pub fn new(
        base_avatar_dir: PathBuf,
        ptt_hotkey: impl Into<String>,
        wake_word_phrase: impl Into<String>,
        wake_word_confidence_threshold: f32,
        silence_timeout_ms: u64,
        power_policy: VoicePowerPolicy,
    ) -> Self {
        Self {
            base_avatar_dir,
            ptt_hotkey: ptt_hotkey.into(),
            wake_word_phrase: wake_word_phrase.into(),
            wake_word_confidence_threshold: wake_word_confidence_threshold.clamp(0.1, 1.0),
            silence_timeout_ms: silence_timeout_ms.max(250),
            vad_rms_threshold: 0.02,
            power_policy,
            state: VoiceInteractionState::Idle,
            island_state: IslandState::Idle,
            soundwave: VoiceSoundwaveIndicator::idle(),
            pcm_buffer: VoicePcmRingBuffer::new(VOICE_PCM_RING_CAPACITY_SAMPLES),
            active_trigger: None,
            session_started_at_ms: 0,
            last_voice_activity_at_ms: 0,
            ptt_sessions_started: 0,
            wake_word_sessions_started: 0,
            wake_word_blocked_by_power_count: 0,
            completed_utterances_count: 0,
            cancelled_sessions_count: 0,
            pcm_zero_fills_count: 0,
            max_soundwave_latency_ms: 0.0,
            max_cancel_latency_ms: 0.0,
            next_ipc_seq: 1,
        }
    }

    pub fn new_default(base_avatar_dir: PathBuf) -> Self {
        Self::new(
            base_avatar_dir,
            DEFAULT_PTT_HOTKEY_COMBO,
            DEFAULT_WAKE_WORD_PHRASE,
            0.80,
            DEFAULT_VOICE_SILENCE_TIMEOUT_MS,
            VoicePowerPolicy::default(),
        )
    }

    pub fn state(&self) -> &VoiceInteractionState {
        &self.state
    }

    pub fn island_state(&self) -> IslandState {
        self.island_state
    }

    pub fn soundwave(&self) -> &VoiceSoundwaveIndicator {
        &self.soundwave
    }

    pub fn pcm_buffer(&self) -> &VoicePcmRingBuffer {
        &self.pcm_buffer
    }

    pub fn pcm_buffer_mut(&mut self) -> &mut VoicePcmRingBuffer {
        &mut self.pcm_buffer
    }

    pub fn zero_and_reclaim(&mut self) -> VoiceBufferZeroReport {
        self.pcm_zero_fills_count += 1;
        self.pcm_buffer.zero_and_reclaim()
    }

    pub fn power_policy(&self) -> VoicePowerPolicy {
        self.power_policy
    }

    pub fn set_power_policy(&mut self, policy: VoicePowerPolicy) {
        self.power_policy = policy;
    }

    pub fn ptt_sessions_started(&self) -> u64 {
        self.ptt_sessions_started
    }

    pub fn wake_word_sessions_started(&self) -> u64 {
        self.wake_word_sessions_started
    }

    pub fn wake_word_blocked_by_power_count(&self) -> u64 {
        self.wake_word_blocked_by_power_count
    }

    pub fn completed_utterances_count(&self) -> u64 {
        self.completed_utterances_count
    }

    pub fn cancelled_sessions_count(&self) -> u64 {
        self.cancelled_sessions_count
    }

    pub fn pcm_zero_fills_count(&self) -> u64 {
        self.pcm_zero_fills_count
    }

    pub fn max_soundwave_latency_ms(&self) -> f64 {
        self.max_soundwave_latency_ms
    }

    pub fn max_cancel_latency_ms(&self) -> f64 {
        self.max_cancel_latency_ms
    }

    /// Starts a non-blocking Push-to-Talk session after validating the configured hotkey and
    /// verifying the Win32 `WS_EX_NOACTIVATE` zero-focus-steal invariant.
    pub fn start_ptt_session(
        &mut self,
        hotkey_pressed: &str,
        now_ms: u64,
        fg_before: usize,
        fg_after: usize,
        pill_hwnd: usize,
        avatar: &mut ProductionAvatarRuntime,
    ) -> Result<(), UiError> {
        let _ = verify_foreground_focus_invariant(
            fg_before,
            fg_after,
            pill_hwnd,
            MA_NOACTIVATE_RESULT,
        )?;

        if !hotkey_pressed
            .trim()
            .eq_ignore_ascii_case(self.ptt_hotkey.trim())
        {
            return Err(UiError::VoiceInteractionError(format!(
                "unrecognized PTT hotkey '{}'; expected '{}'",
                hotkey_pressed, self.ptt_hotkey
            )));
        }

        let _ = self.pcm_buffer.zero_and_reclaim();
        self.state = VoiceInteractionState::Listening;
        self.island_state = IslandState::Listening;
        self.active_trigger = Some(VoiceTriggerMode::PushToTalk {
            hotkey: self.ptt_hotkey.clone(),
        });
        self.session_started_at_ms = now_ms;
        self.last_voice_activity_at_ms = now_ms;
        self.ptt_sessions_started += 1;
        avatar.set_mood(&self.base_avatar_dir, AvatarMood::Listening)?;
        Ok(())
    }

    /// Triggers a local Wake-Word session (`"Hey Miku"`), enforcing `VoicePowerPolicy` (`BUD-05`)
    /// and the Win32 `WS_EX_NOACTIVATE` zero-focus-steal invariant.
    pub fn trigger_wake_word(
        &mut self,
        detected_phrase: &str,
        confidence: f32,
        now_ms: u64,
        fg_before: usize,
        fg_after: usize,
        pill_hwnd: usize,
        avatar: &mut ProductionAvatarRuntime,
    ) -> Result<(), UiError> {
        let _ = verify_foreground_focus_invariant(
            fg_before,
            fg_after,
            pill_hwnd,
            MA_NOACTIVATE_RESULT,
        )?;

        if !self.power_policy.is_wake_word_permitted() {
            self.wake_word_blocked_by_power_count += 1;
            return Err(UiError::VoiceInteractionError(
                "wake-word trigger suspended by battery/gaming power policy (BUD-05)".to_string(),
            ));
        }

        if !detected_phrase
            .trim()
            .eq_ignore_ascii_case(self.wake_word_phrase.trim())
        {
            return Err(UiError::VoiceInteractionError(format!(
                "wake-word phrase '{}' did not match '{}'",
                detected_phrase, self.wake_word_phrase
            )));
        }

        if !confidence.is_finite() || confidence < self.wake_word_confidence_threshold {
            return Err(UiError::VoiceInteractionError(format!(
                "wake-word confidence {:.3} below threshold {:.3}",
                confidence, self.wake_word_confidence_threshold
            )));
        }

        let _ = self.pcm_buffer.zero_and_reclaim();
        self.state = VoiceInteractionState::Listening;
        self.island_state = IslandState::Listening;
        self.active_trigger = Some(VoiceTriggerMode::WakeWord {
            phrase: self.wake_word_phrase.clone(),
            confidence_threshold: self.wake_word_confidence_threshold,
        });
        self.session_started_at_ms = now_ms;
        self.last_voice_activity_at_ms = now_ms;
        self.wake_word_sessions_started += 1;
        avatar.set_mood(&self.base_avatar_dir, AvatarMood::Listening)?;
        Ok(())
    }

    /// Ingests a `16 kHz mono i16` PCM frame while in `Listening`, updating the 5-band pill
    /// soundwave indicator in `< 16.0 ms` and checking for automatic silence timeout cancellation.
    pub fn ingest_pcm_frame(
        &mut self,
        samples: &[i16],
        now_ms: u64,
        avatar: &mut ProductionAvatarRuntime,
        bridge: Option<&mut WebView2IpcBridge>,
    ) -> Result<VoiceSoundwaveIndicator, UiError> {
        if self.state != VoiceInteractionState::Listening {
            return Err(UiError::VoiceInteractionError(format!(
                "cannot ingest PCM frame in state {:?}",
                self.state
            )));
        }

        self.pcm_buffer.push_pcm_slice(samples);
        let wave_ms = self
            .soundwave
            .update_from_pcm(samples, self.vad_rms_threshold);
        if wave_ms > self.max_soundwave_latency_ms {
            self.max_soundwave_latency_ms = wave_ms;
        }

        if self.soundwave.voice_activity_detected {
            self.last_voice_activity_at_ms = now_ms;
        } else {
            let silence_ms = now_ms.saturating_sub(self.last_voice_activity_at_ms);
            if silence_ms >= self.silence_timeout_ms {
                let _ = self.cancel_cooperatively(
                    VoiceCancellationReason::SilenceTimeoutExceeded { silence_ms },
                    now_ms,
                    avatar,
                    bridge,
                )?;
                return Ok(self.soundwave.clone());
            }
        }

        Ok(self.soundwave.clone())
    }

    /// Releases the PTT hotkey: if held for `< MIN_PTT_HOLD_DURATION_MS` (`120 ms`), cancels
    /// cooperatively (`PttReleasedTooEarly`); otherwise transitions `Listening -> Processing`.
    pub fn release_ptt_and_begin_processing(
        &mut self,
        now_ms: u64,
        avatar: &mut ProductionAvatarRuntime,
        bridge: Option<&mut WebView2IpcBridge>,
    ) -> Result<VoiceInteractionState, UiError> {
        if self.state != VoiceInteractionState::Listening {
            return Err(UiError::VoiceInteractionError(format!(
                "cannot release PTT in state {:?}",
                self.state
            )));
        }

        let held_ms = now_ms.saturating_sub(self.session_started_at_ms);
        if held_ms < MIN_PTT_HOLD_DURATION_MS {
            let receipt = self.cancel_cooperatively(
                VoiceCancellationReason::PttReleasedTooEarly { held_ms },
                now_ms,
                avatar,
                bridge,
            )?;
            return Ok(VoiceInteractionState::Cancelled {
                reason: receipt.reason,
            });
        }

        self.state = VoiceInteractionState::Processing;
        self.soundwave.reset_to_idle();
        avatar.set_mood(&self.base_avatar_dir, AvatarMood::Thinking)?;
        Ok(self.state.clone())
    }

    /// Transitions from `Processing` (or `Listening` for fast-path wake-word) into `Speaking`,
    /// driving the 5-band pill wave from the synthesized response envelope and setting `AvatarMood::Talking`.
    pub fn begin_speaking_response(
        &mut self,
        tts_envelope_samples: &[i16],
        avatar: &mut ProductionAvatarRuntime,
    ) -> Result<VoiceSoundwaveIndicator, UiError> {
        if !matches!(
            self.state,
            VoiceInteractionState::Processing | VoiceInteractionState::Listening
        ) {
            return Err(UiError::VoiceInteractionError(format!(
                "cannot enter Speaking from state {:?}",
                self.state
            )));
        }

        self.state = VoiceInteractionState::Speaking;
        let wave_ms = self
            .soundwave
            .update_from_pcm(tts_envelope_samples, self.vad_rms_threshold);
        if wave_ms > self.max_soundwave_latency_ms {
            self.max_soundwave_latency_ms = wave_ms;
        }
        avatar.set_mood(&self.base_avatar_dir, AvatarMood::Talking)?;
        Ok(self.soundwave.clone())
    }

    /// Finalizes a voice utterance: zero-fills and reclaims the PCM ring buffer, sanitizes the
    /// transcript via `sanitize_for_text_content`, enqueues the sanitized command/transcript over
    /// `WebView2IpcBridge`, and settles the pill and avatar back to `Idle` (`0.00 fps`).
    pub fn complete_utterance(
        &mut self,
        raw_transcript: &str,
        session_id: &str,
        avatar: &mut ProductionAvatarRuntime,
        bridge: &mut WebView2IpcBridge,
    ) -> Result<VoiceUtteranceCompletionReport, UiError> {
        if matches!(
            self.state,
            VoiceInteractionState::Idle | VoiceInteractionState::Cancelled { .. }
        ) {
            return Err(UiError::VoiceInteractionError(
                "cannot complete utterance when voice controller is Idle/Cancelled".to_string(),
            ));
        }

        let zero_report = self.pcm_buffer.zero_and_reclaim();
        self.pcm_zero_fills_count += 1;
        self.soundwave.reset_to_idle();
        self.state = VoiceInteractionState::Idle;
        self.island_state = IslandState::Idle;
        self.active_trigger = None;
        avatar.set_mood(&self.base_avatar_dir, AvatarMood::Idle)?;

        let transcript_cell = sanitize_for_text_content("miku-voice-transcript", raw_transcript)?;
        let safe_text_str = match &transcript_cell {
            WebViewIpcPayloadV1::DomSafeTextRender { safe_text, .. } => safe_text.clone(),
            _ => escape_html_entities_for_text_content(raw_transcript),
        };

        let env_id = format!("voice-utt-{}", self.next_ipc_seq);
        self.next_ipc_seq += 1;

        let cmd_env = WebViewIpcEnvelopeV1::new(
            env_id.clone(),
            None,
            OVERVIEW_VIRTUAL_INDEX_URL,
            WebViewIpcPayloadV1::MikuLink(MikuLinkMessageV1::Command {
                session_id: session_id.to_string(),
                action: "voice_enqueue_prompt".to_string(),
                text: safe_text_str,
            }),
        );
        bridge.enqueue_outbound(cmd_env)?;

        let dom_env = WebViewIpcEnvelopeV1::new(
            format!("{}-dom", env_id),
            Some(env_id.clone()),
            OVERVIEW_VIRTUAL_INDEX_URL,
            transcript_cell.clone(),
        );
        bridge.enqueue_outbound(dom_env)?;

        self.completed_utterances_count += 1;

        Ok(VoiceUtteranceCompletionReport {
            transcript_cell,
            zero_report,
            ipc_envelope_id: env_id,
            island_state: self.island_state,
            avatar_mood: avatar.current_mood,
        })
    }

    /// Cooperatively cancels any active voice interaction (`Listening`, `Processing`, or `Speaking`)
    /// in `< 5.0 ms`, zero-filling the PCM ring buffer, resetting the pill wave to `0.00 fps`,
    /// restoring `AvatarMood::Idle`, and optionally notifying `WebView2IpcBridge`.
    pub fn cancel_cooperatively(
        &mut self,
        reason: VoiceCancellationReason,
        now_ms: u64,
        avatar: &mut ProductionAvatarRuntime,
        bridge: Option<&mut WebView2IpcBridge>,
    ) -> Result<VoiceCancelReceipt, UiError> {
        let t0 = Instant::now();
        let previous_state = self.state.clone();
        let zero_report = self.pcm_buffer.zero_and_reclaim();
        self.pcm_zero_fills_count += 1;
        self.soundwave.reset_to_idle();
        self.state = VoiceInteractionState::Idle;
        self.island_state = IslandState::Idle;
        self.active_trigger = None;
        self.session_started_at_ms = 0;
        self.last_voice_activity_at_ms = now_ms;
        self.cancelled_sessions_count += 1;
        avatar.set_mood(&self.base_avatar_dir, AvatarMood::Idle)?;

        if let Some(br) = bridge {
            let cancel_badge = sanitize_for_text_content(
                "miku-voice-status",
                &format!("VOICE_CANCELLED:{:?}", reason),
            )?;
            let env = WebViewIpcEnvelopeV1::new(
                format!("voice-cancel-{}", self.next_ipc_seq),
                None,
                OVERVIEW_VIRTUAL_INDEX_URL,
                cancel_badge,
            );
            self.next_ipc_seq += 1;
            br.enqueue_outbound(env)?;
        }

        let cancel_latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
        if cancel_latency_ms > self.max_cancel_latency_ms {
            self.max_cancel_latency_ms = cancel_latency_ms;
        }

        Ok(VoiceCancelReceipt {
            reason,
            previous_state,
            cancel_latency_ms,
            zero_report,
            island_state: self.island_state,
            avatar_mood: avatar.current_mood,
        })
    }
}

