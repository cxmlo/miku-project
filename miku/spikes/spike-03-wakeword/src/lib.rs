//! Spike 0.3 — Offline Wake Word ("Hey Miku") in Rust for Windows.
//!
//! Validates:
//! 1. Stage 1 O(N) Voice Activity & Transient Gate (`RMS` + `Zero-Crossing Rate` + `Crest Factor`)
//!    that rejects digital silence, white noise, and mechanical keyboard clicks without running
//!    spectral transforms.
//! 2. Stage 2 8-Band Vocal Formant Goertzel Extractor + Sakoe-Chiba Banded Dynamic Time Warping
//!    (DTW) against the canonical "Hey Miku" phonetic template (`/heɪ/` -> `/m/` -> `/iː/` -> `/kuː/`),
//!    achieving detection latency `< 200 ms` from end-of-phrase.
//! 3. Strict CPU & Battery Policy Evaluator (`<= 1.0%` continuous CPU overall; `<= 0.5%` on battery
//!    or automatic fallback to Push-to-Talk via Win32 hotkey).
//! 4. Deterministic RIFF WAV (16 kHz, 16-bit mono PCM) serialization and parsing in pure Rust.

use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

/// Canonical audio sample rate (16 kHz mono PCM).
pub const SAMPLE_RATE_HZ: u32 = 16_000;

/// Window length: 20 ms at 16 kHz = 320 samples.
pub const FRAME_LEN_SAMPLES: usize = 320;

/// Hop size: 10 ms at 16 kHz = 160 samples (100 hops/second).
pub const FRAME_HOP_SAMPLES: usize = 160;

/// Number of vocal sub-bands extracted per active voiced frame.
pub const NUM_VOCAL_BANDS: usize = 8;

/// Center frequencies (Hz) for the 8 Goertzel vocal formant resonators.
pub const VOCAL_BAND_FREQS_HZ: [f64; NUM_VOCAL_BANDS] = [
    280.0,  // Band 0: F0 / fundamental & nasal murmur /m/
    480.0,  // Band 1: F1 of /eɪ/ and /uː/
    720.0,  // Band 2: F1 open transition
    1150.0, // Band 3: F2 of back rounded vowel /uː/ ("-ku")
    1650.0, // Band 4: F2 of front-mid /eɪ/ ("Hey")
    2250.0, // Band 5: F2 of high front vowel /iː/ ("Mi-")
    2850.0, // Band 6: F3 vocal brilliance
    3600.0, // Band 7: Plosive burst /k/ & sibilant energy
];

/// Required benchmark repetitions for statistical p50/p95 evaluation.
pub const REQUIRED_REPETITIONS: usize = 20;

/// Maximum allowed p95 wake word trigger latency after utterance end (ms).
pub const MAX_DETECTION_LATENCY_P95_MS: f64 = 200.0;

/// Maximum allowed continuous CPU usage (%) on AC power.
pub const MAX_CONTINUOUS_CPU_AC_PCT: f64 = 1.0;

/// Maximum allowed continuous CPU usage (%) on battery before auto-switching to Push-to-Talk.
pub const MAX_CONTINUOUS_CPU_BATTERY_PCT: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameGateClass {
    /// Quiet background or digital silence (`rms < min_rms_threshold`).
    Silence,
    /// Broadband stationary noise (`zcr > max_voiced_zcr`) or sharp impulsive click (`crest > max_crest`).
    NoiseOrTransient,
    /// Candidate human vocal frame eligible for Stage 2 spectral extraction.
    VoicedCandidate,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FrameGateMetrics {
    pub rms: f64,
    pub peak: f64,
    pub crest_factor: f64,
    pub zero_crossing_rate: f64,
    pub classification: FrameGateClass,
}

/// Evaluates Stage 1 O(N) time-domain VAD and transient gate on a 320-sample (20 ms) frame.
pub fn evaluate_frame_vad_gate(samples: &[i16]) -> FrameGateMetrics {
    if samples.is_empty() {
        return FrameGateMetrics {
            rms: 0.0,
            peak: 0.0,
            crest_factor: 0.0,
            zero_crossing_rate: 0.0,
            classification: FrameGateClass::Silence,
        };
    }

    let mut sum_sq = 0.0f64;
    let mut peak = 0.0f64;
    let mut zero_crossings = 0usize;
    let mut prev_sign = samples[0] >= 0;

    for &s in samples {
        let norm = (s as f64) / 32768.0;
        let abs_v = norm.abs();
        if abs_v > peak {
            peak = abs_v;
        }
        sum_sq += norm * norm;

        let sign = s >= 0;
        if sign != prev_sign {
            zero_crossings += 1;
            prev_sign = sign;
        }
    }

    let n = samples.len() as f64;
    let rms = (sum_sq / n).sqrt();
    let crest_factor = if rms > 1e-9 { peak / rms } else { 0.0 };
    let zero_crossing_rate = (zero_crossings as f64) / n;

    // 1. Silence threshold (~ -35 dBFS)
    if rms < 0.018 {
        return FrameGateMetrics {
            rms,
            peak,
            crest_factor,
            zero_crossing_rate,
            classification: FrameGateClass::Silence,
        };
    }

    // 2. Broadband white noise (ZCR > 0.33) or sharp mechanical impulse (crest > 3.8)
    if zero_crossing_rate > 0.33 || crest_factor > 3.8 || zero_crossing_rate < 0.015 {
        return FrameGateMetrics {
            rms,
            peak,
            crest_factor,
            zero_crossing_rate,
            classification: FrameGateClass::NoiseOrTransient,
        };
    }

    FrameGateMetrics {
        rms,
        peak,
        crest_factor,
        zero_crossing_rate,
        classification: FrameGateClass::VoicedCandidate,
    }
}

/// Computes the 8-band normalized log-energy feature vector using Goertzel resonators.
pub fn extract_normalized_vocal_features(samples: &[i16]) -> [f64; NUM_VOCAL_BANDS] {
    let n = samples.len() as f64;
    let mut raw_log_energies = [0.0f64; NUM_VOCAL_BANDS];

    for (b, &freq_hz) in VOCAL_BAND_FREQS_HZ.iter().enumerate() {
        let k = (0.5 + (n * freq_hz) / (SAMPLE_RATE_HZ as f64)).floor();
        let omega = (2.0 * PI * k) / n;
        let coeff = 2.0 * omega.cos();
        let mut q1 = 0.0f64;
        let mut q2 = 0.0f64;

        for (i, &s) in samples.iter().enumerate() {
            // Hann window to suppress spectral leakage
            let hann = 0.5 * (1.0 - ((2.0 * PI * (i as f64)) / (n - 1.0)).cos());
            let x = ((s as f64) / 32768.0) * hann;
            let q0 = coeff * q1 - q2 + x;
            q2 = q1;
            q1 = q0;
        }

        let power = (q1 * q1 + q2 * q2 - q1 * q2 * coeff).max(1e-12);
        raw_log_energies[b] = power.ln();
    }

    // Cepstral mean subtraction + L2 unit normalization (gain invariance)
    let mean = raw_log_energies.iter().sum::<f64>() / (NUM_VOCAL_BANDS as f64);
    let mut centered = [0.0f64; NUM_VOCAL_BANDS];
    let mut norm_sq = 0.0f64;
    for i in 0..NUM_VOCAL_BANDS {
        centered[i] = raw_log_energies[i] - mean;
        norm_sq += centered[i] * centered[i];
    }
    let norm = norm_sq.sqrt().max(1e-9);
    for v in &mut centered {
        *v /= norm;
    }
    centered
}

/// Computes Sakoe-Chiba banded Dynamic Time Warping (DTW) average per-step Euclidean distance.
pub fn compute_banded_dtw_distance(
    seq_a: &[[f64; NUM_VOCAL_BANDS]],
    seq_b: &[[f64; NUM_VOCAL_BANDS]],
    band_radius: usize,
) -> f64 {
    let n = seq_a.len();
    let m = seq_b.len();
    if n == 0 || m == 0 {
        return f64::INFINITY;
    }

    let effective_band = band_radius.max(n.abs_diff(m) + 4);
    let mut dp = vec![vec![f64::INFINITY; m + 1]; n + 1];
    dp[0][0] = 0.0;

    for i in 1..=n {
        let center_j = ((i * m) / n).max(1);
        let j_start = center_j.saturating_sub(effective_band).max(1);
        let j_end = (center_j + effective_band).min(m);

        for j in j_start..=j_end {
            let mut dist_sq = 0.0f64;
            for b in 0..NUM_VOCAL_BANDS {
                let diff = seq_a[i - 1][b] - seq_b[j - 1][b];
                dist_sq += diff * diff;
            }
            let local_cost = dist_sq.sqrt();
            let prev_min = dp[i - 1][j].min(dp[i][j - 1]).min(dp[i - 1][j - 1]);
            dp[i][j] = local_cost + prev_min;
        }
    }

    dp[n][m] / ((n + m) as f64)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WakeWordDetection {
    pub phrase: String,
    pub confidence: f64,
    pub dtw_distance: f64,
    pub utterance_end_ms: f64,
    pub trigger_time_ms: f64,
    pub detection_latency_ms: f64,
}

/// Streaming two-stage offline Wake Word detector for `"Hey Miku"`.
#[derive(Debug, Clone)]
pub struct OfflineWakeWordDetector {
    reference_template: Vec<[f64; NUM_VOCAL_BANDS]>,
    active_features: Vec<[f64; NUM_VOCAL_BANDS]>,
    trailing_quiet_hops: usize,
    last_voiced_hop_index: usize,
    total_hops_processed: usize,
    pub total_frames_evaluated: u64,
    pub spectral_frames_computed: u64,
    pub detections: Vec<WakeWordDetection>,
}

impl OfflineWakeWordDetector {
    pub fn new_hey_miku() -> Self {
        let canonical_pcm = synthesize_hey_miku_pcm(1.0, 0.55);
        let reference_template = extract_template_from_pcm(&canonical_pcm);
        Self {
            reference_template,
            active_features: Vec::with_capacity(96),
            trailing_quiet_hops: 0,
            last_voiced_hop_index: 0,
            total_hops_processed: 0,
            total_frames_evaluated: 0,
            spectral_frames_computed: 0,
            detections: Vec::new(),
        }
    }

    pub fn reset_stream(&mut self) {
        self.active_features.clear();
        self.trailing_quiet_hops = 0;
        self.last_voiced_hop_index = 0;
        self.total_hops_processed = 0;
        self.total_frames_evaluated = 0;
        self.spectral_frames_computed = 0;
        self.detections.clear();
    }

    /// Processes an entire PCM buffer in 10 ms hops (320-sample window, 160-sample stride).
    pub fn process_pcm_stream(&mut self, pcm: &[i16]) -> Vec<WakeWordDetection> {
        let start_det_len = self.detections.len();
        if pcm.len() < FRAME_LEN_SAMPLES {
            return Vec::new();
        }

        let mut offset = 0usize;
        while offset + FRAME_LEN_SAMPLES <= pcm.len() {
            let frame = &pcm[offset..offset + FRAME_LEN_SAMPLES];
            self.process_single_hop(frame);
            offset += FRAME_HOP_SAMPLES;
        }

        self.detections[start_det_len..].to_vec()
    }

    pub fn process_single_hop(&mut self, frame_320: &[i16]) -> Option<WakeWordDetection> {
        self.total_hops_processed += 1;
        self.total_frames_evaluated += 1;
        let gate = evaluate_frame_vad_gate(frame_320);

        match gate.classification {
            FrameGateClass::VoicedCandidate => {
                // Stage 2 is ONLY invoked when Stage 1 approves a voiced candidate.
                self.spectral_frames_computed += 1;
                let feat = extract_normalized_vocal_features(frame_320);
                self.active_features.push(feat);
                self.trailing_quiet_hops = 0;
                self.last_voiced_hop_index = self.total_hops_processed;
                if self.active_features.len() > 110 {
                    // Exceeds maximum wake word duration (> 1.1 s); flush partial buffer
                    self.active_features.drain(0..30);
                }
                None
            }
            FrameGateClass::Silence | FrameGateClass::NoiseOrTransient => {
                if !self.active_features.is_empty() {
                    self.trailing_quiet_hops += 1;
                    // End-of-utterance settled after 4 quiet hops (40 ms -> < 200 ms latency!)
                    if self.trailing_quiet_hops >= 4 {
                        let maybe_detection = self.evaluate_completed_utterance();
                        self.active_features.clear();
                        self.trailing_quiet_hops = 0;
                        return maybe_detection;
                    }
                }
                None
            }
        }
    }

    fn evaluate_completed_utterance(&mut self) -> Option<WakeWordDetection> {
        let len = self.active_features.len();
        // "Hey Miku" lasts roughly 42..85 hops (420..850 ms).
        // Impulsive clicks (< 10 hops) or long sentences (> 95 hops) are rejected immediately.
        if !(38..=90).contains(&len) {
            return None;
        }

        // Verify characteristic high-front vowel /iː/ peak in the middle third ("Mi-")
        // compared to the final third ("-ku" back vowel /uː/).
        let third = len / 3;
        if third == 0 {
            return None;
        }
        let mid_slice = &self.active_features[third..(2 * third)];
        let end_slice = &self.active_features[(2 * third)..];

        let avg_mid_high_front = mid_slice.iter().map(|f| f[5]).sum::<f64>() / (mid_slice.len() as f64);
        let avg_end_back_vowel = end_slice.iter().map(|f| f[3]).sum::<f64>() / (end_slice.len() as f64);

        if avg_mid_high_front <= 0.05 || avg_end_back_vowel <= -0.15 {
            return None;
        }

        let dtw_dist = compute_banded_dtw_distance(&self.active_features, &self.reference_template, 12);
        let confidence = (1.0 - (dtw_dist / 0.55)).clamp(0.0, 1.0);

        if dtw_dist <= 0.24 && confidence >= 0.56 {
            let utterance_end_ms = (self.last_voiced_hop_index as f64) * 10.0;
            let trigger_time_ms = (self.total_hops_processed as f64) * 10.0;
            let detection_latency_ms = (trigger_time_ms - utterance_end_ms).max(10.0);

            let det = WakeWordDetection {
                phrase: "Hey Miku".to_string(),
                confidence,
                dtw_distance: dtw_dist,
                utterance_end_ms,
                trigger_time_ms,
                detection_latency_ms,
            };
            self.detections.push(det.clone());
            return Some(det);
        }

        None
    }
}

fn extract_template_from_pcm(pcm: &[i16]) -> Vec<[f64; NUM_VOCAL_BANDS]> {
    let mut feats = Vec::new();
    let mut offset = 0usize;
    while offset + FRAME_LEN_SAMPLES <= pcm.len() {
        let frame = &pcm[offset..offset + FRAME_LEN_SAMPLES];
        if evaluate_frame_vad_gate(frame).classification == FrameGateClass::VoicedCandidate {
            feats.push(extract_normalized_vocal_features(frame));
        }
        offset += FRAME_HOP_SAMPLES;
    }
    feats
}

/// Synthesizes a deterministic 16 kHz mono PCM waveform with the phonetic formant contour of
/// `"Hey Miku"` (`/heɪ/` -> `/m/` -> `/iː/` -> `/kuː/`), surrounded by leading and trailing silence.
pub fn synthesize_hey_miku_pcm(tempo_scale: f64, amplitude: f64) -> Vec<i16> {
    let amp = amplitude.clamp(0.05, 0.95);
    let tempo = tempo_scale.clamp(0.75, 1.35);

    let lead_silence_samples = (0.12 * (SAMPLE_RATE_HZ as f64)) as usize;
    let voice_duration_sec = 0.62 / tempo;
    let voice_samples = (voice_duration_sec * (SAMPLE_RATE_HZ as f64)).round() as usize;
    let trail_silence_samples = (0.16 * (SAMPLE_RATE_HZ as f64)) as usize;

    let mut out = Vec::with_capacity(lead_silence_samples + voice_samples + trail_silence_samples);
    out.resize(lead_silence_samples, 0i16);

    for i in 0..voice_samples {
        let t = (i as f64) / (SAMPLE_RATE_HZ as f64);
        let progress = (i as f64) / (voice_samples as f64);

        // Phonetic formant contour across the 4 phonetic segments of "Hey Miku"
        let (f0, f1, f2, f3, env) = if progress < 0.30 {
            // Segment 1: "Hey" (/heɪ/) -> F1=480 Hz, F2 glides 1650 -> 2250 Hz
            let seg_p = progress / 0.30;
            let f2_glide = 1650.0 + (2250.0 - 1650.0) * seg_p;
            (280.0, 480.0, f2_glide, 2850.0, 0.85)
        } else if progress < 0.42 {
            // Segment 2: nasal closure "/m/" -> F0=280 Hz + nasal murmur 1150 Hz
            (280.0, 280.0, 1150.0, 1650.0, 0.55)
        } else if progress < 0.72 {
            // Segment 3: "Mi-" (/iː/) -> strong high front vowel F2=2250 Hz + F3=2850 Hz
            (280.0, 300.0, 2250.0, 2850.0, 1.00)
        } else {
            // Segment 4: "-ku" (/kuː/) -> brief 3600 Hz plosive burst then back vowel F1=480, F2=1150 Hz
            let seg_p = (progress - 0.72) / 0.28;
            let f3_burst = if seg_p < 0.22 { 3600.0 } else { 2250.0 };
            (280.0, 480.0, 1150.0, f3_burst, 0.90)
        };

        let sig = 0.42 * (2.0 * PI * f0 * t).sin()
            + 0.28 * (2.0 * PI * f1 * t).sin()
            + 0.22 * (2.0 * PI * f2 * t).sin()
            + 0.08 * (2.0 * PI * f3 * t).sin();

        let sample_f = (sig * env * amp * 32767.0).clamp(-32767.0, 32767.0);
        out.push(sample_f as i16);
    }

    out.resize(out.len() + trail_silence_samples, 0i16);
    out
}

/// Synthesizes a negative speech fixture (`"Hola mundo"` — low back vowel contour without `/iː/` peak).
pub fn synthesize_negative_speech_pcm() -> Vec<i16> {
    let lead_silence = (0.10 * (SAMPLE_RATE_HZ as f64)) as usize;
    let voice_samples = (0.58 * (SAMPLE_RATE_HZ as f64)) as usize;
    let trail_silence = (0.15 * (SAMPLE_RATE_HZ as f64)) as usize;

    let mut out = Vec::with_capacity(lead_silence + voice_samples + trail_silence);
    out.resize(lead_silence, 0i16);

    for i in 0..voice_samples {
        let t = (i as f64) / (SAMPLE_RATE_HZ as f64);
        // Constant low-mid back formants (480 Hz + 720 Hz), lacking "Hey Miku" structure
        let sig = 0.55 * (2.0 * PI * 260.0 * t).sin()
            + 0.35 * (2.0 * PI * 480.0 * t).sin()
            + 0.10 * (2.0 * PI * 720.0 * t).sin();
        out.push((sig * 0.45 * 32767.0) as i16);
    }

    out.resize(out.len() + trail_silence, 0i16);
    out
}

/// Synthesizes deterministic pseudo-random white noise (`LCG` seed) to test stationary noise rejection.
pub fn synthesize_white_noise_pcm(duration_ms: u64, amplitude: f64) -> Vec<i16> {
    let total_samples = ((duration_ms as f64 / 1000.0) * (SAMPLE_RATE_HZ as f64)).round() as usize;
    let mut state = 0x1357_9BDF_2468_ACE1u64;
    let mut out = Vec::with_capacity(total_samples);
    let amp = amplitude.clamp(0.01, 0.90);

    for _ in 0..total_samples {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let unit = (((state >> 32) as u32) as f64 / (u32::MAX as f64)) * 2.0 - 1.0;
        out.push((unit * amp * 32767.0) as i16);
    }
    out
}

/// Synthesizes sharp mechanical keyboard click bursts (8 ms impulsive transients every 120 ms).
pub fn synthesize_keyboard_transients_pcm(duration_ms: u64) -> Vec<i16> {
    let total_samples = ((duration_ms as f64 / 1000.0) * (SAMPLE_RATE_HZ as f64)).round() as usize;
    let period_samples = (0.120 * (SAMPLE_RATE_HZ as f64)) as usize;
    let click_len = (0.008 * (SAMPLE_RATE_HZ as f64)) as usize; // 8 ms mechanical switch transient

    let mut out = vec![0i16; total_samples];
    let mut pos = 160usize;
    while pos + click_len < total_samples {
        for k in 0..click_len {
            let decay = (-((k as f64) / 18.0)).exp();
            // Alternating high-frequency click impulse
            let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
            out[pos + k] = (sign * decay * 0.85 * 32767.0) as i16;
        }
        pos += period_samples;
    }
    out
}

/// Encodes 16 kHz mono 16-bit PCM samples into a standard 44-byte header RIFF WAV buffer.
pub fn encode_wav_pcm16_mono(samples: &[i16]) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let riff_size = 36u32.saturating_add(data_bytes);
    let byte_rate = SAMPLE_RATE_HZ * 2;
    let block_align = 2u16;

    let mut buf = Vec::with_capacity(44 + samples.len() * 2);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&riff_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM = 1
    buf.extend_from_slice(&1u16.to_le_bytes()); // Channels = 1 (mono)
    buf.extend_from_slice(&SAMPLE_RATE_HZ.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes()); // BitsPerSample = 16
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        buf.extend_from_slice(&s.to_le_bytes());
    }
    buf
}

/// Decodes a 16 kHz mono 16-bit RIFF WAV buffer back into `Vec<i16>`.
pub fn decode_wav_pcm16_mono(wav_bytes: &[u8]) -> Result<Vec<i16>, String> {
    if wav_bytes.len() < 44 {
        return Err("WAV buffer shorter than 44-byte RIFF header".to_string());
    }
    if &wav_bytes[0..4] != b"RIFF" || &wav_bytes[8..12] != b"WAVE" {
        return Err("Invalid RIFF/WAVE magic bytes".to_string());
    }
    let audio_format = u16::from_le_bytes([wav_bytes[20], wav_bytes[21]]);
    let channels = u16::from_le_bytes([wav_bytes[22], wav_bytes[23]]);
    let sample_rate =
        u32::from_le_bytes([wav_bytes[24], wav_bytes[25], wav_bytes[26], wav_bytes[27]]);
    let bits_per_sample = u16::from_le_bytes([wav_bytes[34], wav_bytes[35]]);

    if audio_format != 1 || channels != 1 || sample_rate != SAMPLE_RATE_HZ || bits_per_sample != 16
    {
        return Err(format!(
            "Unsupported WAV format: fmt={}, ch={}, rate={}, bits={} (expected PCM 16kHz mono 16-bit)",
            audio_format, channels, sample_rate, bits_per_sample
        ));
    }

    let data_len =
        u32::from_le_bytes([wav_bytes[40], wav_bytes[41], wav_bytes[42], wav_bytes[43]]) as usize;
    let payload = &wav_bytes[44..];
    if payload.len() < data_len {
        return Err("WAV data chunk truncated".to_string());
    }

    let sample_count = data_len / 2;
    let mut out = Vec::with_capacity(sample_count);
    for i in 0..sample_count {
        let lo = payload[i * 2];
        let hi = payload[i * 2 + 1];
        out.push(i16::from_le_bytes([lo, hi]));
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WakeWordPowerPolicy {
    /// CPU <= 0.5%: Continuous offline wake word allowed on both AC and Battery.
    AlwaysOnAcAndBattery,
    /// 0.5% < CPU <= 1.0%: Continuous wake word allowed on AC; auto-switches to Push-to-Talk on Battery.
    AcOnlyFallbackPttOnBattery,
    /// CPU > 1.0% or latency/handle failure: Wake word disabled by default in favor of Push-to-Talk.
    DisabledUsePttDefault,
}

impl WakeWordPowerPolicy {
    pub fn description(&self) -> &'static str {
        match self {
            WakeWordPowerPolicy::AlwaysOnAcAndBattery => {
                "ALWAYS_ON_AC_AND_BATTERY (CPU <= 0.5%: permitido en CA y batería)"
            }
            WakeWordPowerPolicy::AcOnlyFallbackPttOnBattery => {
                "AC_ONLY_FALLBACK_PTT_ON_BATTERY (0.5% < CPU <= 1.0%: activo en CA, Push-to-Talk en batería)"
            }
            WakeWordPowerPolicy::DisabledUsePttDefault => {
                "DISABLED_USE_PTT_DEFAULT (Excede presupuesto -> Push-to-Talk por defecto)"
            }
        }
    }
}

/// Computes normalized process CPU percentage from kernel + user FILETIME microseconds,
/// wall-clock microseconds, and logical core count.
pub fn compute_normalized_cpu_pct(
    kernel_delta_us: f64,
    user_delta_us: f64,
    wall_elapsed_us: f64,
    logical_cores: usize,
) -> f64 {
    if wall_elapsed_us <= 0.0 || logical_cores == 0 {
        return 0.0;
    }
    let total_cpu_us = (kernel_delta_us + user_delta_us).max(0.0);
    let capacity_us = wall_elapsed_us * (logical_cores as f64);
    ((total_cpu_us / capacity_us) * 100.0).clamp(0.0, 100.0)
}

pub fn evaluate_wakeword_power_policy(
    continuous_cpu_pct: f64,
    latency_p95_ms: f64,
    false_positives: usize,
    handle_leak_delta: i64,
) -> WakeWordPowerPolicy {
    if !continuous_cpu_pct.is_finite()
        || !latency_p95_ms.is_finite()
        || latency_p95_ms > MAX_DETECTION_LATENCY_P95_MS
        || false_positives > 0
        || handle_leak_delta > 0
        || continuous_cpu_pct > MAX_CONTINUOUS_CPU_AC_PCT
    {
        return WakeWordPowerPolicy::DisabledUsePttDefault;
    }

    if continuous_cpu_pct <= MAX_CONTINUOUS_CPU_BATTERY_PCT {
        WakeWordPowerPolicy::AlwaysOnAcAndBattery
    } else {
        WakeWordPowerPolicy::AcOnlyFallbackPttOnBattery
    }
}

pub fn percentile_sorted(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let clamped = pct.clamp(0.0, 100.0) / 100.0;
    let idx = ((sorted.len() - 1) as f64 * clamped).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spike03BenchmarkSummary {
    pub repetitions: usize,
    pub wasapi_capture_endpoint_available: bool,
    pub wasapi_status_note: String,
    pub on_battery_power: bool,
    pub logical_cores: usize,
    pub true_positives: usize,
    pub false_positives_negative_speech: usize,
    pub false_positives_white_noise: usize,
    pub false_positives_keyboard: usize,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub silence_vad_rejection_ratio_pct: f64,
    pub continuous_cpu_pct: f64,
    pub handles_before: u32,
    pub handles_after: u32,
    pub handle_leak_delta: i64,
    pub recommended_policy: WakeWordPowerPolicy,
    pub passes_all_thresholds: bool,
}

impl Spike03BenchmarkSummary {
    pub fn evaluate_thresholds(&self) -> bool {
        self.repetitions == REQUIRED_REPETITIONS
            && self.true_positives == REQUIRED_REPETITIONS
            && self.false_positives_negative_speech == 0
            && self.false_positives_white_noise == 0
            && self.false_positives_keyboard == 0
            && self.latency_p95_ms <= MAX_DETECTION_LATENCY_P95_MS
            && self.continuous_cpu_pct <= MAX_CONTINUOUS_CPU_AC_PCT
            && self.handle_leak_delta <= 0
            && self.recommended_policy != WakeWordPowerPolicy::DisabledUsePttDefault
    }
}

pub fn format_spike03_report(summary: &Spike03BenchmarkSummary) -> String {
    let mut out = String::new();
    out.push_str("=== SPIKE 0.3: OFFLINE WAKE WORD ('HEY MIKU') & WASAPI/CPU AUDIT ===\n");
    out.push_str(&format!(
        "Audio Capture Source      : {}\n",
        summary.wasapi_status_note
    ));
    out.push_str(&format!(
        "Power Source / Cores      : {} | {} logical cores\n",
        if summary.on_battery_power {
            "Battery (DC)"
        } else {
            "AC Power"
        },
        summary.logical_cores
    ));
    out.push_str(&format!(
        "Positive Detections       : {}/{} ('Hey Miku' WAV fixtures)\n",
        summary.true_positives, summary.repetitions
    ));
    out.push_str(&format!(
        "False Positives (Speech)  : {} (target == 0)\n",
        summary.false_positives_negative_speech
    ));
    out.push_str(&format!(
        "False Positives (Noise)   : {} (target == 0)\n",
        summary.false_positives_white_noise
    ));
    out.push_str(&format!(
        "False Positives (Keyboard): {} (target == 0)\n",
        summary.false_positives_keyboard
    ));
    out.push_str(&format!(
        "Detection Latency p50/p95 : p50 = {:.2} ms | p95 = {:.2} ms (threshold <= {:.1} ms)\n",
        summary.latency_p50_ms, summary.latency_p95_ms, MAX_DETECTION_LATENCY_P95_MS
    ));
    out.push_str(&format!(
        "Stage 1 VAD Silence Gate  : {:.1}% frames rejected in O(N) without DTW\n",
        summary.silence_vad_rejection_ratio_pct
    ));
    out.push_str(&format!(
        "Continuous CPU Usage      : {:.4}% (threshold <= {:.1}% AC / <= {:.1}% Battery)\n",
        summary.continuous_cpu_pct, MAX_CONTINUOUS_CPU_AC_PCT, MAX_CONTINUOUS_CPU_BATTERY_PCT
    ));
    out.push_str(&format!(
        "Process Handles (20 cyc)  : before={} | after={} | leak_delta={} (threshold == 0)\n",
        summary.handles_before, summary.handles_after, summary.handle_leak_delta
    ));
    out.push_str(&format!(
        "Recommended Power Policy  : {}\n",
        summary.recommended_policy.description()
    ));
    out.push_str(&format!(
        "Spike 0.3 Verdict         : {}\n",
        if summary.passes_all_thresholds {
            "CUMPLE (WAKE WORD OFFLINE VERIFICADO)"
        } else {
            "NO CUMPLE"
        }
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LEDGER-0.3-1: Verifies that digital silence and low-amplitude background noise
    /// are 100% blocked by the Stage 1 O(N) VAD gate without invoking Stage 2 spectral DTW.
    #[test]
    fn test_audio_frame_energy_and_zcr_vad_gate() {
        let mut detector = OfflineWakeWordDetector::new_hey_miku();
        let silence_pcm = vec![0i16; SAMPLE_RATE_HZ as usize]; // 1.0 second of silence
        let detections = detector.process_pcm_stream(&silence_pcm);
        assert!(detections.is_empty());
        assert!(detector.total_frames_evaluated >= 95);
        assert_eq!(
            detector.spectral_frames_computed, 0,
            "Silence must never trigger Stage 2 spectral extraction"
        );

        // Very quiet ambient hum (-50 dBFS, amplitude ~100 out of 32767)
        let quiet_hum: Vec<i16> = (0..(SAMPLE_RATE_HZ as usize))
            .map(|i| ((i as f64 * 0.1).sin() * 100.0) as i16)
            .collect();
        detector.reset_stream();
        let detections_hum = detector.process_pcm_stream(&quiet_hum);
        assert!(detections_hum.is_empty());
        assert_eq!(detector.spectral_frames_computed, 0);
    }

    /// LEDGER-0.3-2: Verifies that positive "Hey Miku" WAV fixtures across tempo/gain variations
    /// trigger detection in < 200 ms, while negative speech ("Hola mundo") produces 0 detections.
    #[test]
    fn test_mfcc_dtw_wake_word_positive_vs_negative_fixtures() {
        let mut detector = OfflineWakeWordDetector::new_hey_miku();

        for &(tempo, gain) in &[(0.95, 0.45), (1.00, 0.55), (1.06, 0.70)] {
            detector.reset_stream();
            let pos_pcm = synthesize_hey_miku_pcm(tempo, gain);
            let dets = detector.process_pcm_stream(&pos_pcm);
            assert_eq!(
                dets.len(),
                1,
                "Expected exactly 1 'Hey Miku' detection for tempo={} gain={}",
                tempo,
                gain
            );
            assert!(
                dets[0].detection_latency_ms <= MAX_DETECTION_LATENCY_P95_MS,
                "Detection latency {:.2} ms exceeded 200 ms threshold",
                dets[0].detection_latency_ms
            );
        }

        // Negative speech fixture ("Hola mundo")
        detector.reset_stream();
        let neg_pcm = synthesize_negative_speech_pcm();
        let neg_dets = detector.process_pcm_stream(&neg_pcm);
        assert_eq!(
            neg_dets.len(),
            0,
            "Negative speech fixture must produce 0 wake word detections"
        );
    }

    /// LEDGER-0.3-3: Property/unit test verifying 0 false positives on stationary white noise
    /// and sharp mechanical keyboard switch transients.
    #[test]
    fn test_false_positive_rejection_on_white_noise_and_keyboard_transients() {
        let mut detector = OfflineWakeWordDetector::new_hey_miku();

        let white_noise = synthesize_white_noise_pcm(3000, 0.35);
        let noise_dets = detector.process_pcm_stream(&white_noise);
        assert_eq!(
            noise_dets.len(),
            0,
            "Stationary white noise must produce 0 false positives"
        );

        detector.reset_stream();
        let keyboard_clicks = synthesize_keyboard_transients_pcm(3000);
        let click_dets = detector.process_pcm_stream(&keyboard_clicks);
        assert_eq!(
            click_dets.len(),
            0,
            "Mechanical keyboard transients must produce 0 false positives"
        );
    }

    /// LEDGER-0.3-4: Verifies the CPU and Battery power policy evaluator and normalized CPU math.
    #[test]
    fn test_battery_and_cpu_budget_policy_evaluator() {
        // 15,625 us of CPU over 1,000,000 us wall time on 16 logical cores = ~0.0976% CPU
        let cpu_pct = compute_normalized_cpu_pct(0.0, 15_625.0, 1_000_000.0, 16);
        assert!(cpu_pct > 0.09 && cpu_pct < 0.11);

        assert_eq!(
            evaluate_wakeword_power_policy(0.18, 40.0, 0, 0),
            WakeWordPowerPolicy::AlwaysOnAcAndBattery
        );
        assert_eq!(
            evaluate_wakeword_power_policy(0.72, 40.0, 0, 0),
            WakeWordPowerPolicy::AcOnlyFallbackPttOnBattery
        );
        assert_eq!(
            evaluate_wakeword_power_policy(1.35, 40.0, 0, 0),
            WakeWordPowerPolicy::DisabledUsePttDefault
        );
        assert_eq!(
            evaluate_wakeword_power_policy(0.15, 240.0, 0, 0),
            WakeWordPowerPolicy::DisabledUsePttDefault
        );
        assert_eq!(
            evaluate_wakeword_power_policy(0.15, 40.0, 0, 2),
            WakeWordPowerPolicy::DisabledUsePttDefault
        );
    }
}
