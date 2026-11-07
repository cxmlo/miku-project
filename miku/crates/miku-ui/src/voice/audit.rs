#![allow(unused_imports)]
use crate::prelude::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceInteractionAuditReport {
    pub ptt_sessions_completed: u64,
    pub wake_word_sessions_completed: u64,
    pub wake_word_power_blocks: u64,
    pub cooperative_cancellations: u64,
    pub pcm_zero_fills_verified: u64,
    pub residual_pcm_bytes: usize,
    pub max_soundwave_latency_ms: f64,
    pub max_cancel_latency_ms: f64,
    pub idle_wave_fps: f32,
    pub focus_preserved_and_xss_safe: bool,
}

pub fn run_voice_interaction_loop_audit(
    avatar_dir: &Path,
    cycles: usize,
) -> Result<VoiceInteractionAuditReport, UiError> {
    let mut reloader = AvatarPackHotReloader::new(avatar_dir)?;
    let avatar = reloader.runtime_mut();
    let mut bridge = WebView2IpcBridge::new()?;
    let mut ctrl = VoiceInteractionController::new_default(avatar_dir.to_path_buf());

    // Deterministic synthetic 20 ms 16 kHz PCM speech frame (320 samples) and silent frame
    let speech_frame: Vec<i16> = (0..320)
        .map(|idx| {
            let phase = (idx as f64) * 2.0 * std::f64::consts::PI / 32.0;
            (phase.sin() * 14_000.0) as i16
        })
        .collect();
    let silent_frame = vec![0i16; 320];

    let mut focus_and_xss_ok = true;
    let total_cycles = cycles.max(1);

    for i in 0..total_cycles {
        let base_ts = 100_000 + (i as u64) * 20_000;

        // 1. Complete Push-to-Talk session: Idle -> Listening -> Processing -> Speaking -> Idle
        ctrl.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            base_ts,
            0x0022_00AA,
            0x0022_00AA,
            0x0099_00FF,
            avatar,
        )?;
        let wave_listen =
            ctrl.ingest_pcm_frame(&speech_frame, base_ts + 40, avatar, Some(&mut bridge))?;
        if wave_listen.bands[2] <= 0.0 || wave_listen.target_fps != 60.0 {
            focus_and_xss_ok = false;
        }
        let st_proc = ctrl.release_ptt_and_begin_processing(
            base_ts + 300,
            avatar,
            Some(&mut bridge),
        )?;
        if st_proc != VoiceInteractionState::Processing {
            focus_and_xss_ok = false;
        }
        let _ = ctrl.begin_speaking_response(&speech_frame, avatar)?;
        let utt_ptt = ctrl.complete_utterance(
            "<script>alert('xss')</script> cargo test --workspace",
            "sess_voice_01",
            avatar,
            &mut bridge,
        )?;
        if utt_ptt.zero_report.residual_non_zero_samples != 0
            || utt_ptt.zero_report.retained_capacity_bytes != 0
        {
            focus_and_xss_ok = false;
        }
        if let WebViewIpcPayloadV1::DomSafeTextRender {
            ref safe_text,
            csp_enforced,
            ..
        } = utt_ptt.transcript_cell
        {
            if !csp_enforced || safe_text.contains("<script>") {
                focus_and_xss_ok = false;
            }
        } else {
            focus_and_xss_ok = false;
        }

        // 2. Complete Wake-Word session on AC power ("Hey Miku")
        ctrl.set_power_policy(VoicePowerPolicy {
            on_battery_power: false,
            gaming_mode_active: false,
            allow_wake_word_on_battery: false,
        });
        ctrl.trigger_wake_word(
            DEFAULT_WAKE_WORD_PHRASE,
            0.95,
            base_ts + 1_000,
            0x0022_00AA,
            0x0022_00AA,
            0x0099_00FF,
            avatar,
        )?;
        let _ = ctrl.ingest_pcm_frame(&speech_frame, base_ts + 1_040, avatar, Some(&mut bridge))?;
        let utt_ww = ctrl.complete_utterance(
            "show gatekeeper status",
            "sess_voice_01",
            avatar,
            &mut bridge,
        )?;
        if utt_ww.zero_report.residual_non_zero_samples != 0
            || utt_ww.zero_report.retained_capacity_bytes != 0
        {
            focus_and_xss_ok = false;
        }

        // 3. Battery power policy blocks Wake-Word ("Hey Miku") while preserving PTT
        ctrl.set_power_policy(VoicePowerPolicy {
            on_battery_power: true,
            gaming_mode_active: false,
            allow_wake_word_on_battery: false,
        });
        if ctrl
            .trigger_wake_word(
                DEFAULT_WAKE_WORD_PHRASE,
                0.95,
                base_ts + 2_000,
                0x0022_00AA,
                0x0022_00AA,
                0x0099_00FF,
                avatar,
            )
            .is_ok()
        {
            focus_and_xss_ok = false;
        }
        ctrl.set_power_policy(VoicePowerPolicy::default());

        // 4A. Cooperative cancellation vector 1: UserPressedEscape
        ctrl.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            base_ts + 3_000,
            0x0022_00AA,
            0x0022_00AA,
            0x0099_00FF,
            avatar,
        )?;
        let _ = ctrl.ingest_pcm_frame(&speech_frame, base_ts + 3_020, avatar, Some(&mut bridge))?;
        let cancel_esc = ctrl.cancel_cooperatively(
            VoiceCancellationReason::UserPressedEscape,
            base_ts + 3_050,
            avatar,
            Some(&mut bridge),
        )?;
        if cancel_esc.zero_report.residual_non_zero_samples != 0
            || cancel_esc.zero_report.retained_capacity_bytes != 0
        {
            focus_and_xss_ok = false;
        }

        // 4B. Cooperative cancellation vector 2: PttReleasedTooEarly (< 120 ms)
        ctrl.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            base_ts + 4_000,
            0x0022_00AA,
            0x0022_00AA,
            0x0099_00FF,
            avatar,
        )?;
        let _ = ctrl.ingest_pcm_frame(&speech_frame, base_ts + 4_020, avatar, Some(&mut bridge))?;
        let early_state = ctrl.release_ptt_and_begin_processing(
            base_ts + 4_060,
            avatar,
            Some(&mut bridge),
        )?;
        if !matches!(
            early_state,
            VoiceInteractionState::Cancelled {
                reason: VoiceCancellationReason::PttReleasedTooEarly { held_ms: 60 }
            }
        ) {
            focus_and_xss_ok = false;
        }

        // 4C. Cooperative cancellation vector 3: SilenceTimeoutExceeded (>= 3,500 ms)
        ctrl.start_ptt_session(
            DEFAULT_PTT_HOTKEY_COMBO,
            base_ts + 5_000,
            0x0022_00AA,
            0x0022_00AA,
            0x0099_00FF,
            avatar,
        )?;
        let _ = ctrl.ingest_pcm_frame(&silent_frame, base_ts + 5_100, avatar, Some(&mut bridge))?;
        let _ = ctrl.ingest_pcm_frame(&silent_frame, base_ts + 8_600, avatar, Some(&mut bridge))?;
        if *ctrl.state() != VoiceInteractionState::Idle {
            focus_and_xss_ok = false;
        }

        let _ = bridge.drain_all_buffers();
    }

    let (_, settled_fps) = avatar.physics.settle_to_rest(1.0 / 60.0, 600);
    if settled_fps != 0 {
        focus_and_xss_ok = false;
    }

    Ok(VoiceInteractionAuditReport {
        ptt_sessions_completed: total_cycles as u64,
        wake_word_sessions_completed: total_cycles as u64,
        wake_word_power_blocks: ctrl.wake_word_blocked_by_power_count(),
        cooperative_cancellations: ctrl.cancelled_sessions_count(),
        pcm_zero_fills_verified: ctrl.pcm_zero_fills_count(),
        residual_pcm_bytes: ctrl.pcm_buffer().capacity_bytes(),
        max_soundwave_latency_ms: ctrl.max_soundwave_latency_ms(),
        max_cancel_latency_ms: ctrl.max_cancel_latency_ms(),
        idle_wave_fps: ctrl.soundwave().target_fps,
        focus_preserved_and_xss_safe: focus_and_xss_ok,
    })
}

