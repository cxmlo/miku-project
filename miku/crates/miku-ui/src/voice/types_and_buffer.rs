#![allow(unused_imports)]
use crate::prelude::*;

// ============================================================================
// 13. Step 2.8: Push-to-Talk & Wake-Word Voice Interaction Loop, Reactive
//     Pill Soundwave Indicator & Cooperative Cancellation (F-05 / F-16 / ADR-001)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum VoiceTriggerMode {
    PushToTalk {
        hotkey: String,
    },
    WakeWord {
        phrase: String,
        confidence_threshold: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VoicePowerPolicy {
    pub on_battery_power: bool,
    pub gaming_mode_active: bool,
    pub allow_wake_word_on_battery: bool,
}

impl VoicePowerPolicy {
    /// Enforces `BUD-05` (`< 0.5%` wake-word idle CPU, suspended on battery or gaming mode by default).
    pub fn is_wake_word_permitted(&self) -> bool {
        if self.gaming_mode_active {
            return false;
        }
        if self.on_battery_power && !self.allow_wake_word_on_battery {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum VoiceCancellationReason {
    UserPressedEscape,
    PttReleasedTooEarly { held_ms: u64 },
    SilenceTimeoutExceeded { silence_ms: u64 },
    GamingModeInterrupted,
    ExplicitApiAbort,
    KillSwitchInvoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VoiceInteractionState {
    Idle,
    Listening,
    Processing,
    Speaking,
    Cancelled { reason: VoiceCancellationReason },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceSoundwaveIndicator {
    pub bands: [f32; SOUNDWAVE_BAND_COUNT],
    pub rms_db: f32,
    pub voice_activity_detected: bool,
    pub target_fps: f32,
    pub last_update_latency_ms: f64,
}

impl Default for VoiceSoundwaveIndicator {
    fn default() -> Self {
        Self::idle()
    }
}

impl VoiceSoundwaveIndicator {
    pub fn idle() -> Self {
        Self {
            bands: [0.0; SOUNDWAVE_BAND_COUNT],
            rms_db: -96.0,
            voice_activity_detected: false,
            target_fps: 0.0,
            last_update_latency_ms: 0.0,
        }
    }

    pub fn reset_to_idle(&mut self) {
        self.bands = [0.0; SOUNDWAVE_BAND_COUNT];
        self.rms_db = -96.0;
        self.voice_activity_detected = false;
        self.target_fps = 0.0;
    }

    /// Computes 5 normalized pill soundwave bands in `[0.0, 1.0]` from `16 kHz mono i16` PCM samples
    /// in `< 16.0 ms` (`60.0 fps` when active; `0.00 fps` when idle).
    pub fn update_from_pcm(&mut self, samples: &[i16], vad_rms_threshold: f32) -> f64 {
        let t0 = Instant::now();
        if samples.is_empty() {
            self.reset_to_idle();
            let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
            self.last_update_latency_ms = elapsed_ms;
            return elapsed_ms;
        }

        let total_len = samples.len();
        let mut sum_sq = 0.0f64;
        for &s in samples {
            let norm = (s as f64) / 32768.0;
            sum_sq += norm * norm;
        }
        let rms = (sum_sq / (total_len as f64)).sqrt() as f32;
        self.rms_db = if rms > 1e-6 {
            20.0 * rms.log10()
        } else {
            -96.0
        };
        self.voice_activity_detected = rms >= vad_rms_threshold;

        // Symmetric pill weights: center band [2] is dominant, flanked by [1]/[3] and outer [0]/[4]
        const PILL_WEIGHTS: [f32; SOUNDWAVE_BAND_COUNT] = [0.62, 0.84, 1.00, 0.84, 0.62];
        let seg_len = (total_len / SOUNDWAVE_BAND_COUNT).max(1);

        for band_idx in 0..SOUNDWAVE_BAND_COUNT {
            let start = (band_idx * seg_len).min(total_len.saturating_sub(1));
            let end = if band_idx + 1 == SOUNDWAVE_BAND_COUNT {
                total_len
            } else {
                ((band_idx + 1) * seg_len).min(total_len)
            };
            let slice = &samples[start..end.max(start + 1)];
            let mut band_sq = 0.0f64;
            for &s in slice {
                let norm = (s as f64) / 32768.0;
                band_sq += norm * norm;
            }
            let band_rms = (band_sq / (slice.len() as f64)).sqrt() as f32;
            let raw_level = (band_rms * 3.2 * PILL_WEIGHTS[band_idx]).clamp(0.0, 1.0);
            let smoothed = (0.65 * raw_level + 0.35 * self.bands[band_idx]).clamp(0.0, 1.0);
            self.bands[band_idx] = if smoothed < 0.005 { 0.0 } else { smoothed };
        }

        self.target_fps = 60.0;
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        self.last_update_latency_ms = elapsed_ms;
        elapsed_ms
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceBufferZeroReport {
    pub zeroed_samples: usize,
    pub residual_non_zero_samples: usize,
    pub retained_capacity_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct VoicePcmRingBuffer {
    pub(crate) samples: Vec<i16>,
    pub(crate) capacity_limit: usize,
    pub(crate) total_samples_written: u64,
}

impl Default for VoicePcmRingBuffer {
    fn default() -> Self {
        Self::new(VOICE_PCM_RING_CAPACITY_SAMPLES)
    }
}

impl VoicePcmRingBuffer {
    pub fn new(capacity_limit: usize) -> Self {
        Self {
            samples: Vec::new(),
            capacity_limit: capacity_limit.max(160),
            total_samples_written: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn capacity_bytes(&self) -> usize {
        self.samples.capacity() * std::mem::size_of::<i16>()
    }

    pub fn total_samples_written(&self) -> u64 {
        self.total_samples_written
    }

    pub fn samples(&self) -> &[i16] {
        &self.samples
    }

    pub fn push_pcm_slice(&mut self, chunk: &[i16]) {
        if chunk.is_empty() {
            return;
        }
        self.total_samples_written += chunk.len() as u64;
        if chunk.len() >= self.capacity_limit {
            let tail = &chunk[chunk.len() - self.capacity_limit..];
            self.samples.clear();
            self.samples.extend_from_slice(tail);
            return;
        }
        let overflow = (self.samples.len() + chunk.len()).saturating_sub(self.capacity_limit);
        if overflow > 0 {
            self.samples.drain(0..overflow);
        }
        self.samples.extend_from_slice(chunk);
    }

    /// Overwrites 100% of stored PCM samples with `0i16` before dropping heap capacity,
    /// guaranteeing 0 residual voice samples and 0 retained buffer bytes in idle/cancelled states.
    pub fn zero_and_reclaim(&mut self) -> VoiceBufferZeroReport {
        let zeroed_samples = self.samples.len();
        self.samples.fill(0i16);
        let residual_non_zero_samples = self.samples.iter().filter(|&&s| s != 0).count();
        self.samples.clear();
        self.samples.shrink_to_fit();
        self.samples = Vec::new();
        let retained_capacity_bytes = self.samples.capacity() * std::mem::size_of::<i16>();
        VoiceBufferZeroReport {
            zeroed_samples,
            residual_non_zero_samples,
            retained_capacity_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceCancelReceipt {
    pub reason: VoiceCancellationReason,
    pub previous_state: VoiceInteractionState,
    pub cancel_latency_ms: f64,
    pub zero_report: VoiceBufferZeroReport,
    pub island_state: IslandState,
    pub avatar_mood: AvatarMood,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceUtteranceCompletionReport {
    pub transcript_cell: WebViewIpcPayloadV1,
    pub zero_report: VoiceBufferZeroReport,
    pub ipc_envelope_id: String,
    pub island_state: IslandState,
    pub avatar_mood: AvatarMood,
}

pub struct VoiceInteractionController {
    pub(crate) base_avatar_dir: PathBuf,
    pub(crate) ptt_hotkey: String,
    pub(crate) wake_word_phrase: String,
    pub(crate) wake_word_confidence_threshold: f32,
    pub(crate) silence_timeout_ms: u64,
    pub(crate) vad_rms_threshold: f32,
    pub(crate) power_policy: VoicePowerPolicy,
    pub(crate) state: VoiceInteractionState,
    pub(crate) island_state: IslandState,
    pub(crate) soundwave: VoiceSoundwaveIndicator,
    pub(crate) pcm_buffer: VoicePcmRingBuffer,
    pub(crate) active_trigger: Option<VoiceTriggerMode>,
    pub(crate) session_started_at_ms: u64,
    pub(crate) last_voice_activity_at_ms: u64,
    pub(crate) ptt_sessions_started: u64,
    pub(crate) wake_word_sessions_started: u64,
    pub(crate) wake_word_blocked_by_power_count: u64,
    pub(crate) completed_utterances_count: u64,
    pub(crate) cancelled_sessions_count: u64,
    pub(crate) pcm_zero_fills_count: u64,
    pub(crate) max_soundwave_latency_ms: f64,
    pub(crate) max_cancel_latency_ms: f64,
    pub(crate) next_ipc_seq: u64,
}

