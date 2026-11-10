#![allow(unused_imports)]
use crate::prelude::*;

pub const STT_MODEL_MAGIC: &[u8; 8] = b"MKSTT01\0";
pub const STT_MODEL_FORMAT_VERSION: u32 = 1;
pub const DEFAULT_MAX_STT_MODEL_BYTES: usize = 128 * 1024 * 1024;
pub const DEFAULT_STT_MODEL_TTL_MS: u64 = 15_000;
pub const DEFAULT_STT_DOWNLOAD_URL: &str = "https://models.miku.local/stt/miku-stt-es-en-v1.mkstt";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttLanguage {
    Es,
    En,
}

impl SttLanguage {
    pub fn as_code(self) -> &'static str {
        match self {
            SttLanguage::Es => "es",
            SttLanguage::En => "en",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SttModelManifest {
    pub model_id: String,
    pub version: u32,
    pub expected_sha256_hex: String,
    pub expected_size_bytes: usize,
    pub languages: Vec<String>,
}

impl SttModelManifest {
    pub fn to_download_card(&self, download_url: impl Into<String>) -> SttModelDownloadCard {
        SttModelDownloadCard {
            model_id: self.model_id.clone(),
            version: self.version,
            expected_size_bytes: self.expected_size_bytes,
            expected_sha256_hex: self.expected_sha256_hex.clone(),
            languages: self.languages.clone(),
            download_url: download_url.into(),
            requires_explicit_user_action: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SttModelDownloadCard {
    pub model_id: String,
    pub version: u32,
    pub expected_size_bytes: usize,
    pub expected_sha256_hex: String,
    pub languages: Vec<String>,
    pub download_url: String,
    pub requires_explicit_user_action: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum SttModelAvailability {
    VerifiedOnDisk { path: String, manifest: SttModelManifest },
    Missing { download_card: SttModelDownloadCard },
    CorruptedOnDisk { reason: String, download_card: SttModelDownloadCard },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SttModelError {
    ExceedsMemoryCap { requested_bytes: usize, max_allowed_bytes: usize },
    IncompleteDownload { expected_bytes: usize, actual_bytes: usize },
    Sha256Mismatch { expected_sha256: String, actual_sha256: String },
    InvalidModelHeader(String),
    InvalidAudioBuffer(String),
    ModelMissingOnDisk { download_card: SttModelDownloadCard },
    ModelNotLoaded,
    NoMatchingCommand,
    UserConfirmationRequired,
    GatekeeperDenied(String),
    KillSwitchActive,
    PathJailViolation(String),
    CaptureDeviceError(String),
    DownloadTransportError(String),
    VoiceControllerError(String),
}

impl SttModelError {
    pub fn from_untrusted_diagnostic(context: &str, raw: &str) -> Self {
        Self::InvalidModelHeader(format!("{}: {}", context, redact_sensitive_voice_diagnostic(raw)))
    }
}

impl std::fmt::Display for SttModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SttModelError::ExceedsMemoryCap { requested_bytes, max_allowed_bytes } => {
                write!(f, "STT model/audio size {} exceeds cap {} bytes", requested_bytes, max_allowed_bytes)
            }
            SttModelError::IncompleteDownload { expected_bytes, actual_bytes } => {
                write!(f, "incomplete STT model download: expected {} bytes, got {} bytes", expected_bytes, actual_bytes)
            }
            SttModelError::Sha256Mismatch { expected_sha256, actual_sha256 } => {
                write!(f, "STT model SHA-256 mismatch: expected {}, got {}", expected_sha256, actual_sha256)
            }
            SttModelError::InvalidModelHeader(msg) => {
                write!(f, "invalid STT model header: {}", redact_sensitive_voice_diagnostic(msg))
            }
            SttModelError::InvalidAudioBuffer(msg) => {
                write!(f, "invalid PCM audio buffer: {}", redact_sensitive_voice_diagnostic(msg))
            }
            SttModelError::ModelMissingOnDisk { download_card } => write!(
                f,
                "STT model '{}' missing on disk (size={} B, sha256={})",
                download_card.model_id, download_card.expected_size_bytes, download_card.expected_sha256_hex
            ),
            SttModelError::ModelNotLoaded => write!(f, "STT model is not loaded in memory"),
            SttModelError::NoMatchingCommand => write!(f, "no matching command prototype in local STT codebook"),
            SttModelError::UserConfirmationRequired => {
                write!(f, "explicit user confirmation required before downloading STT model")
            }
            SttModelError::GatekeeperDenied(msg) => {
                write!(f, "blocked by Gatekeeper: {}", redact_sensitive_voice_diagnostic(msg))
            }
            SttModelError::KillSwitchActive => write!(f, "operation aborted because Global Kill Switch is active"),
            SttModelError::PathJailViolation(msg) => {
                write!(f, "path jail violation: {}", redact_sensitive_voice_diagnostic(msg))
            }
            SttModelError::CaptureDeviceError(msg) => {
                write!(f, "audio capture device error: {}", redact_sensitive_voice_diagnostic(msg))
            }
            SttModelError::DownloadTransportError(msg) => {
                write!(f, "model download transport error: {}", redact_sensitive_voice_diagnostic(msg))
            }
            SttModelError::VoiceControllerError(msg) => {
                write!(f, "voice controller error: {}", redact_sensitive_voice_diagnostic(msg))
            }
        }
    }
}

impl std::error::Error for SttModelError {}

/// Redacts quoted string literals, bracketed sample arrays, `sk-*` API keys, and `Bearer` tokens
/// so voice/STT error diagnostics never leak raw audio, user transcripts, or secrets.
pub fn redact_sensitive_voice_diagnostic(raw: &str) -> String {
    let mut stage1 = String::with_capacity(raw.len());
    let mut in_quotes = false;
    let mut in_brackets = false;

    for ch in raw.chars() {
        if ch == '"' && !in_brackets {
            if in_quotes {
                in_quotes = false;
                stage1.push_str("\"[REDACTED]\"");
            } else {
                in_quotes = true;
            }
            continue;
        }
        if ch == '[' && !in_quotes {
            in_brackets = true;
            stage1.push_str("[REDACTED]");
            continue;
        }
        if ch == ']' && in_brackets {
            in_brackets = false;
            continue;
        }
        if !in_quotes && !in_brackets {
            stage1.push(ch);
        }
    }
    if in_quotes {
        stage1.push_str("\"[REDACTED]\"");
    }

    let mut tokens_out = Vec::new();
    let mut redact_next = false;
    for word in stage1.split_whitespace() {
        if redact_next {
            tokens_out.push("[REDACTED]".to_string());
            redact_next = false;
            continue;
        }
        let lower = word.to_ascii_lowercase();
        if lower == "bearer" {
            tokens_out.push("Bearer".to_string());
            redact_next = true;
            continue;
        }
        if lower.contains("sk-") || lower.starts_with("eyj") {
            if let Some(eq_idx) = word.find('=') {
                tokens_out.push(format!("{}=[REDACTED]", &word[..eq_idx]));
            } else {
                tokens_out.push("[REDACTED]".to_string());
            }
            continue;
        }
        tokens_out.push(word.to_string());
    }
    tokens_out.join(" ")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SttCommandPrototype {
    pub phrase: String,
    pub language: SttLanguage,
    pub feature_vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializedSttCodebook {
    pub model_id: String,
    pub prototypes: Vec<SttCommandPrototype>,
}

pub struct VerifiedSttModelWeights {
    pub model_id: String,
    pub prototypes: Vec<SttCommandPrototype>,
    pub tensor_slab: Vec<u8>,
}

impl VerifiedSttModelWeights {
    pub fn allocated_bytes(&self) -> usize {
        self.tensor_slab.capacity() + self.prototypes.len() * std::mem::size_of::<SttCommandPrototype>()
    }

    pub fn zero_and_release(&mut self) {
        for b in self.tensor_slab.iter_mut() {
            *b = 0;
        }
        std::hint::black_box(&self.tensor_slab);
        self.tensor_slab.clear();
        self.tensor_slab.shrink_to_fit();
        self.tensor_slab = Vec::new();
        for proto in self.prototypes.iter_mut() {
            proto.feature_vector.fill(0.0);
            proto.feature_vector.clear();
            proto.feature_vector.shrink_to_fit();
            proto.phrase.clear();
            proto.phrase.shrink_to_fit();
        }
        self.prototypes.clear();
        self.prototypes.shrink_to_fit();
    }
}

impl Drop for VerifiedSttModelWeights {
    fn drop(&mut self) {
        self.zero_and_release();
    }
}

pub fn canonical_bilingual_stt_commands() -> Vec<(&'static str, SttLanguage)> {
    vec![
        ("ejecuta cargo test", SttLanguage::Es),
        ("aprueba el permiso", SttLanguage::Es),
        ("abre el navegador", SttLanguage::Es),
        ("cancela la cola", SttLanguage::Es),
        ("run clippy and fix warnings", SttLanguage::En),
        ("approve permission request", SttLanguage::En),
        ("open pull request", SttLanguage::En),
        ("stop active agent", SttLanguage::En),
    ]
}

/// Deterministic acoustic formant trajectory synthesizer at 16 kHz mono PCM16 (Spike 0.4).
pub fn synthesize_bilingual_command_pcm(phrase: &str) -> Vec<i16> {
    let normalized = phrase.trim().to_ascii_lowercase();
    let words: Vec<&str> = normalized.split_whitespace().collect();
    let lead_silence = (VOICE_SAMPLE_RATE_HZ as usize * 60) / 1000;
    let tail_silence = (VOICE_SAMPLE_RATE_HZ as usize * 80) / 1000;
    let inter_word_gap = (VOICE_SAMPLE_RATE_HZ as usize * 35) / 1000;

    let mut pcm = Vec::with_capacity(VOICE_SAMPLE_RATE_HZ as usize);
    pcm.resize(lead_silence, 0i16);

    for (w_idx, word) in words.iter().enumerate() {
        let bytes = word.as_bytes();
        let word_ms = 100 + (bytes.len().min(10) * 14);
        let word_samples = (VOICE_SAMPLE_RATE_HZ as usize * word_ms) / 1000;

        let mut h1: u32 = 2166136261;
        let mut h2: u32 = 16777619;
        for (c_idx, &b) in bytes.iter().enumerate() {
            h1 = (h1 ^ (b as u32)).wrapping_mul(16777619);
            h2 = h2.wrapping_add((b as u32).wrapping_mul((c_idx as u32 + 1) * 131));
        }

        let f1_hz = 280.0 + ((h1 % 520) as f32);
        let f2_hz = 950.0 + (((h1 >> 10) % 1650) as f32);
        let f3_hz = 2700.0 + ((h2 % 1100) as f32);
        let mod_hz = 4.0 + ((w_idx % 5) as f32) * 1.5;

        for i in 0..word_samples {
            let t = i as f32 / VOICE_SAMPLE_RATE_HZ as f32;
            let progress = i as f32 / word_samples as f32;
            let env = (std::f32::consts::PI * progress).sin();
            let vibrato = 1.0 + 0.015 * (2.0 * std::f32::consts::PI * mod_hz * t).sin();
            let s1 = (2.0 * std::f32::consts::PI * f1_hz * vibrato * t).sin() * 0.52;
            let s2 = (2.0 * std::f32::consts::PI * f2_hz * t).sin() * 0.31;
            let s3 = (2.0 * std::f32::consts::PI * f3_hz * t).sin() * 0.17;
            let sample_f = (s1 + s2 + s3) * env * 13_500.0;
            pcm.push(sample_f.clamp(-32767.0, 32767.0) as i16);
        }
        if w_idx + 1 < words.len() {
            pcm.resize(pcm.len() + inter_word_gap, 0i16);
        }
    }
    pcm.resize(pcm.len() + tail_silence, 0i16);
    pcm
}

/// Extracts a 24-dimensional acoustic-spectral descriptor from a 16 kHz mono PCM16 utterance.
pub fn extract_utterance_feature_vector(pcm: &[i16]) -> Vec<f32> {
    if pcm.is_empty() {
        return vec![0.0; 24];
    }
    let threshold: i16 = 350;
    let start = pcm.iter().position(|&s| s.saturating_abs() > threshold).unwrap_or(0);
    let end = pcm.iter().rposition(|&s| s.saturating_abs() > threshold).map(|idx| idx + 1).unwrap_or(pcm.len());
    let active = if end > start + 64 { &pcm[start..end] } else { pcm };

    let band_freqs: [f32; 6] = [360.0, 620.0, 1150.0, 1750.0, 2450.0, 3250.0];
    let mut features = Vec::with_capacity(24);
    let seg_len = (active.len() / 3).max(1);

    for seg_idx in 0..3 {
        let s_start = (seg_idx * seg_len).min(active.len());
        let s_end = if seg_idx == 2 { active.len() } else { ((seg_idx + 1) * seg_len).min(active.len()) };
        let slice = &active[s_start..s_end];
        if slice.is_empty() {
            features.extend_from_slice(&[0.0; 8]);
            continue;
        }
        let dur_norm = (slice.len() as f32 / VOICE_SAMPLE_RATE_HZ as f32).clamp(0.0, 2.0);
        let mut zc = 0usize;
        for w in slice.windows(2) {
            if (w[0] >= 0 && w[1] < 0) || (w[0] < 0 && w[1] >= 0) {
                zc += 1;
            }
        }
        features.push(dur_norm);
        features.push((zc as f32 / slice.len() as f32) * 4.0);

        let win_len = slice.len().min(512);
        let win_offset = (slice.len() - win_len) / 2;
        let win = &slice[win_offset..win_offset + win_len];

        let mut band_energies = [0.0f32; 6];
        let mut total_e = 1e-6f32;
        for (b_i, &freq) in band_freqs.iter().enumerate() {
            let k = (0.5 + (win.len() as f32 * freq) / VOICE_SAMPLE_RATE_HZ as f32).floor();
            let omega = (2.0 * std::f32::consts::PI * k) / win.len() as f32;
            let coeff = 2.0 * omega.cos();
            let (mut q1, mut q2) = (0.0f32, 0.0f32);
            for &sample in win {
                let q0 = coeff * q1 - q2 + (sample as f32 / 32768.0);
                q2 = q1;
                q1 = q0;
            }
            let power = (q1 * q1 + q2 * q2 - q1 * q2 * coeff).max(1e-9);
            band_energies[b_i] = power;
            total_e += power;
        }
        for &e in &band_energies {
            features.push((e / total_e).sqrt());
        }
    }

    let norm = features.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-6);
    for v in &mut features {
        *v /= norm;
    }
    features
}

pub fn build_stt_model_artifact_and_manifest(model_id: &str, tensor_slab_bytes: usize) -> (Vec<u8>, SttModelManifest) {
    let prototypes: Vec<SttCommandPrototype> = canonical_bilingual_stt_commands()
        .into_iter()
        .map(|(phrase, language)| {
            let pcm = synthesize_bilingual_command_pcm(phrase);
            let feature_vector = extract_utterance_feature_vector(&pcm);
            SttCommandPrototype { phrase: phrase.to_string(), language, feature_vector }
        })
        .collect();

    let codebook = SerializedSttCodebook { model_id: model_id.to_string(), prototypes };
    let codebook_json = serde_json::to_vec(&codebook).expect("serialize STT codebook");
    let codebook_len = codebook_json.len() as u32;
    let slab_len = tensor_slab_bytes as u32;

    let mut raw = Vec::with_capacity(20 + codebook_json.len() + tensor_slab_bytes);
    raw.extend_from_slice(STT_MODEL_MAGIC);
    raw.extend_from_slice(&STT_MODEL_FORMAT_VERSION.to_le_bytes());
    raw.extend_from_slice(&codebook_len.to_le_bytes());
    raw.extend_from_slice(&slab_len.to_le_bytes());
    raw.extend_from_slice(&codebook_json);

    let mut slab = vec![0x5Au8; tensor_slab_bytes];
    for (idx, chunk) in slab.chunks_mut(4096).enumerate() {
        chunk[0] = (idx & 0xFF) as u8;
    }
    raw.extend_from_slice(&slab);

    let sha256 = sha256_hex(&raw);
    let manifest = SttModelManifest {
        model_id: model_id.to_string(),
        version: STT_MODEL_FORMAT_VERSION,
        expected_sha256_hex: sha256,
        expected_size_bytes: raw.len(),
        languages: vec!["es".to_string(), "en".to_string()],
    };
    (raw, manifest)
}

pub fn verify_and_parse_stt_model_bytes(
    raw: &[u8],
    manifest: &SttModelManifest,
    max_allowed_bytes: usize,
) -> Result<VerifiedSttModelWeights, SttModelError> {
    if manifest.expected_size_bytes > max_allowed_bytes || raw.len() > max_allowed_bytes {
        return Err(SttModelError::ExceedsMemoryCap {
            requested_bytes: raw.len().max(manifest.expected_size_bytes),
            max_allowed_bytes,
        });
    }
    if raw.len() != manifest.expected_size_bytes {
        return Err(SttModelError::IncompleteDownload {
            expected_bytes: manifest.expected_size_bytes,
            actual_bytes: raw.len(),
        });
    }
    let actual_sha256 = sha256_hex(raw);
    if !constant_time_token_eq(&actual_sha256, &manifest.expected_sha256_hex) {
        return Err(SttModelError::Sha256Mismatch {
            expected_sha256: manifest.expected_sha256_hex.clone(),
            actual_sha256,
        });
    }
    if raw.len() < 20 || &raw[0..8] != STT_MODEL_MAGIC {
        return Err(SttModelError::InvalidModelHeader("missing MKSTT01 magic header".to_string()));
    }
    let version = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
    if version != STT_MODEL_FORMAT_VERSION {
        return Err(SttModelError::InvalidModelHeader(format!("unsupported model format version {}", version)));
    }
    let codebook_len = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]) as usize;
    let slab_len = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]) as usize;
    let expected_total = 20usize
        .checked_add(codebook_len)
        .and_then(|v| v.checked_add(slab_len))
        .ok_or_else(|| SttModelError::InvalidModelHeader("header length overflow".to_string()))?;
    if expected_total != raw.len() {
        return Err(SttModelError::InvalidModelHeader("header payload lengths do not match total file bytes".to_string()));
    }

    let codebook: SerializedSttCodebook = serde_json::from_slice(&raw[20..20 + codebook_len])
        .map_err(|_| SttModelError::InvalidModelHeader("corrupt codebook JSON".to_string()))?;
    let mut tensor_slab = Vec::with_capacity(slab_len);
    tensor_slab.extend_from_slice(&raw[20 + codebook_len..]);
    for chunk in tensor_slab.chunks_mut(4096) {
        chunk[0] = chunk[0].wrapping_add(1);
    }
    std::hint::black_box(&tensor_slab);

    Ok(VerifiedSttModelWeights {
        model_id: codebook.model_id,
        prototypes: codebook.prototypes,
        tensor_slab,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SttModelLifecycleState {
    Unloaded,
    Ready { loaded_at_ms: u64, last_used_ms: u64, allocated_bytes: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalSttTranscript {
    pub text: String,
    pub language: SttLanguage,
    pub confidence: f32,
    pub audio_duration_ms: f64,
    pub inference_latency_ms: f64,
}

pub struct OnDemandLocalSttRuntime {
    ttl_ms: u64,
    max_model_bytes: usize,
    state: SttModelLifecycleState,
    weights: Option<VerifiedSttModelWeights>,
    network_calls_made: u64,
    provider_tokens_consumed: u64,
}

impl OnDemandLocalSttRuntime {
    pub fn new(ttl_ms: u64, max_model_bytes: usize) -> Self {
        Self {
            ttl_ms: ttl_ms.max(250),
            max_model_bytes,
            state: SttModelLifecycleState::Unloaded,
            weights: None,
            network_calls_made: 0,
            provider_tokens_consumed: 0,
        }
    }

    pub fn state(&self) -> &SttModelLifecycleState {
        &self.state
    }

    pub fn allocated_weight_bytes(&self) -> usize {
        self.weights.as_ref().map(|w| w.allocated_bytes()).unwrap_or(0)
    }

    pub fn network_calls_made(&self) -> u64 {
        self.network_calls_made
    }

    pub fn provider_tokens_consumed(&self) -> u64 {
        self.provider_tokens_consumed
    }

    pub fn load_verified_bytes(&mut self, raw: &[u8], manifest: &SttModelManifest, now_ms: u64) -> Result<usize, SttModelError> {
        let verified = verify_and_parse_stt_model_bytes(raw, manifest, self.max_model_bytes)?;
        let bytes = verified.allocated_bytes();
        self.weights = Some(verified);
        self.state = SttModelLifecycleState::Ready {
            loaded_at_ms: now_ms,
            last_used_ms: now_ms,
            allocated_bytes: bytes,
        };
        Ok(bytes)
    }

    pub fn transcribe_pcm16_mono(&mut self, pcm: &[i16], sample_rate_hz: u32, now_ms: u64) -> Result<LocalSttTranscript, SttModelError> {
        if pcm.is_empty() || sample_rate_hz == 0 {
            return Err(SttModelError::InvalidAudioBuffer("empty PCM buffer or zero sample rate".to_string()));
        }
        let weights = self.weights.as_ref().ok_or(SttModelError::ModelNotLoaded)?;
        let t0 = Instant::now();
        let feat = extract_utterance_feature_vector(pcm);
        std::hint::black_box(weights.tensor_slab.first().copied().unwrap_or(0));

        let mut best: Option<(&SttCommandPrototype, f32)> = None;
        for proto in &weights.prototypes {
            let dot: f32 = feat.iter().zip(proto.feature_vector.iter()).map(|(a, b)| a * b).sum();
            let sim = dot.clamp(-1.0, 1.0);
            if best.map(|(_, b_sim)| sim > b_sim).unwrap_or(true) {
                best = Some((proto, sim));
            }
        }
        let (matched, sim) = best.ok_or(SttModelError::NoMatchingCommand)?;
        let inference_latency_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let audio_duration_ms = (pcm.len() as f64 / sample_rate_hz as f64) * 1000.0;

        if let SttModelLifecycleState::Ready { loaded_at_ms, allocated_bytes, .. } = self.state {
            self.state = SttModelLifecycleState::Ready { loaded_at_ms, last_used_ms: now_ms, allocated_bytes };
        }
        Ok(LocalSttTranscript {
            text: matched.phrase.clone(),
            language: matched.language,
            confidence: sim.clamp(0.0, 1.0),
            audio_duration_ms,
            inference_latency_ms,
        })
    }

    pub fn poll_ttl(&mut self, now_ms: u64) -> bool {
        if let SttModelLifecycleState::Ready { last_used_ms, .. } = self.state {
            if now_ms.saturating_sub(last_used_ms) >= self.ttl_ms {
                self.unload_and_zero();
                return true;
            }
        }
        false
    }

    pub fn unload_and_zero(&mut self) {
        if let Some(mut w) = self.weights.take() {
            w.zero_and_release();
        }
        self.state = SttModelLifecycleState::Unloaded;
    }
}
