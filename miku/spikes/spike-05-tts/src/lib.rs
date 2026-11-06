use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Instant;

pub const DEFAULT_SAMPLE_RATE_HZ: u32 = 24_000;
pub const FALLBACK_SAMPLE_RATE_HZ: u32 = 16_000;
pub const CHUNK_DURATION_MS: u32 = 40; // 40 ms per streaming chunk (960 samples at 24 kHz)
pub const VOICE_PACK_MAGIC: &[u8; 8] = b"MKTTS01\0";
pub const VOICE_PACK_VERSION: u32 = 1;
pub const DEFAULT_MAX_VOICE_PACK_BYTES: usize = 128 * 1024 * 1024; // 128 MB cap
pub const MAX_INPUT_TEXT_BYTES: usize = 4_096; // 4 KB text cap per utterance
pub const BENCHMARK_VOICE_SLAB_BYTES: usize = 48 * 1024 * 1024; // 48 MB (> 45 MB active peak)

// ============================================================================
// 1. SELF-CONTAINED FIPS 180-4 SHA-256 IMPLEMENTATION
// ============================================================================

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const SHA256_H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

#[inline]
fn compress_sha256_block(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for i in 0..16 {
        let j = i * 4;
        w[i] = u32::from_be_bytes([block[j], block[j + 1], block[j + 2], block[j + 3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }

    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];

    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(SHA256_K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(maj);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }

    state[0] = state[0].wrapping_add(a);
    state[1] = state[1].wrapping_add(b);
    state[2] = state[2].wrapping_add(c);
    state[3] = state[3].wrapping_add(d);
    state[4] = state[4].wrapping_add(e);
    state[5] = state[5].wrapping_add(f);
    state[6] = state[6].wrapping_add(g);
    state[7] = state[7].wrapping_add(h);
}

pub fn sha256_digest(data: &[u8]) -> [u8; 32] {
    let mut state = SHA256_H0;
    let mut chunks = data.chunks_exact(64);
    for chunk in &mut chunks {
        let mut block = [0u8; 64];
        block.copy_from_slice(chunk);
        compress_sha256_block(&mut state, &block);
    }

    let rem = chunks.remainder();
    let bit_len = (data.len() as u64).wrapping_mul(8);

    let mut tail = [0u8; 128];
    tail[..rem.len()].copy_from_slice(rem);
    tail[rem.len()] = 0x80;

    if rem.len() < 56 {
        tail[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let mut block = [0u8; 64];
        block.copy_from_slice(&tail[..64]);
        compress_sha256_block(&mut state, &block);
    } else {
        tail[120..128].copy_from_slice(&bit_len.to_be_bytes());
        let mut block1 = [0u8; 64];
        let mut block2 = [0u8; 64];
        block1.copy_from_slice(&tail[..64]);
        block2.copy_from_slice(&tail[64..128]);
        compress_sha256_block(&mut state, &block1);
        compress_sha256_block(&mut state, &block2);
    }

    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = sha256_digest(data);
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", byte);
    }
    hex
}

pub fn constant_time_hex_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.trim().as_bytes();
    let b_bytes = b.trim().as_bytes();
    if a_bytes.len() != b_bytes.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a_bytes.iter().zip(b_bytes.iter()) {
        diff |= x.to_ascii_lowercase() ^ y.to_ascii_lowercase();
    }
    diff == 0
}

// ============================================================================
// 2. VOICE PACK MANIFEST, SHA-256 VERIFIER & FORMANT TABLE
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    Es,
    En,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TtsError {
    ExceedsMemoryCap {
        requested_bytes: usize,
        max_allowed_bytes: usize,
    },
    SizeMismatch {
        expected_bytes: usize,
        actual_bytes: usize,
    },
    IntegrityMismatch {
        expected_sha256: String,
        actual_sha256: String,
    },
    InvalidVoicePackHeader(String),
    TextTooLong {
        actual_bytes: usize,
        max_bytes: usize,
    },
    EmptySanitizedText,
    UnsupportedSampleRate(u32),
    VoicePackNotLoaded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoicePackManifest {
    pub voice_id: String,
    pub version: u32,
    pub default_sample_rate_hz: u32,
    pub expected_sha256_hex: String,
    pub expected_size_bytes: usize,
    pub languages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhonemeFormantEntry {
    pub symbol: String,
    pub f1_hz: f32,
    pub f2_hz: f32,
    pub f3_hz: f32,
    pub voiceless_mix: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializedVoiceHeader {
    pub voice_id: String,
    pub base_pitch_es_hz: f32,
    pub base_pitch_en_hz: f32,
    pub formants: Vec<PhonemeFormantEntry>,
}

pub struct VerifiedVoiceBank {
    pub voice_id: String,
    pub base_pitch_es_hz: f32,
    pub base_pitch_en_hz: f32,
    pub formants: Vec<PhonemeFormantEntry>,
    pub acoustic_tensor_slab: Vec<u8>,
}

impl VerifiedVoiceBank {
    pub fn allocated_bytes(&self) -> usize {
        self.acoustic_tensor_slab.capacity()
            + self.formants.len() * std::mem::size_of::<PhonemeFormantEntry>()
    }

    pub fn zero_and_release(&mut self) {
        for b in self.acoustic_tensor_slab.iter_mut() {
            *b = 0;
        }
        std::hint::black_box(&self.acoustic_tensor_slab);
        self.acoustic_tensor_slab = Vec::new();
        self.formants.clear();
        self.formants.shrink_to_fit();
    }
}

pub fn canonical_bilingual_tts_phrases() -> Vec<(&'static str, Language)> {
    vec![
        (
            "Permiso aprobado. Ejecutando cargo test en el repositorio actual.",
            Language::Es,
        ),
        (
            "Alerta del Gatekeeper: un agente solicita escritura en configuración.",
            Language::Es,
        ),
        (
            "Build succeeded with zero warnings. Ready for the next step.",
            Language::En,
        ),
        (
            "Permission request received from Claude Code. Press Y to allow.",
            Language::En,
        ),
    ]
}

pub fn build_voice_pack_and_manifest(
    voice_id: &str,
    tensor_slab_bytes: usize,
) -> (Vec<u8>, VoicePackManifest) {
    let formants = vec![
        PhonemeFormantEntry {
            symbol: "a".to_string(),
            f1_hz: 730.0,
            f2_hz: 1090.0,
            f3_hz: 2440.0,
            voiceless_mix: 0.02,
        },
        PhonemeFormantEntry {
            symbol: "e".to_string(),
            f1_hz: 530.0,
            f2_hz: 1840.0,
            f3_hz: 2480.0,
            voiceless_mix: 0.02,
        },
        PhonemeFormantEntry {
            symbol: "i".to_string(),
            f1_hz: 270.0,
            f2_hz: 2290.0,
            f3_hz: 3010.0,
            voiceless_mix: 0.03,
        },
        PhonemeFormantEntry {
            symbol: "o".to_string(),
            f1_hz: 570.0,
            f2_hz: 840.0,
            f3_hz: 2410.0,
            voiceless_mix: 0.02,
        },
        PhonemeFormantEntry {
            symbol: "u".to_string(),
            f1_hz: 300.0,
            f2_hz: 870.0,
            f3_hz: 2240.0,
            voiceless_mix: 0.02,
        },
        PhonemeFormantEntry {
            symbol: "s".to_string(),
            f1_hz: 420.0,
            f2_hz: 1650.0,
            f3_hz: 3300.0,
            voiceless_mix: 0.45,
        },
    ];

    let header_struct = SerializedVoiceHeader {
        voice_id: voice_id.to_string(),
        base_pitch_es_hz: 235.0,
        base_pitch_en_hz: 228.0,
        formants,
    };
    let header_json =
        serde_json::to_vec(&header_struct).expect("Voice header serialization cannot fail");
    let header_len = header_json.len() as u32;
    let slab_len = tensor_slab_bytes as u32;

    let total_size = 8 + 4 + 4 + 4 + header_json.len() + tensor_slab_bytes;
    let mut raw = Vec::with_capacity(total_size);
    raw.extend_from_slice(VOICE_PACK_MAGIC);
    raw.extend_from_slice(&VOICE_PACK_VERSION.to_le_bytes());
    raw.extend_from_slice(&header_len.to_le_bytes());
    raw.extend_from_slice(&slab_len.to_le_bytes());
    raw.extend_from_slice(&header_json);

    let mut slab = vec![0x3Cu8; tensor_slab_bytes];
    for (idx, page) in slab.chunks_mut(4096).enumerate() {
        page[0] = (idx & 0xFF) as u8;
    }
    raw.extend_from_slice(&slab);

    let sha256 = sha256_hex(&raw);
    let manifest = VoicePackManifest {
        voice_id: voice_id.to_string(),
        version: VOICE_PACK_VERSION,
        default_sample_rate_hz: DEFAULT_SAMPLE_RATE_HZ,
        expected_sha256_hex: sha256,
        expected_size_bytes: raw.len(),
        languages: vec!["es".to_string(), "en".to_string()],
    };

    (raw, manifest)
}

pub fn verify_and_parse_voice_pack(
    raw: &[u8],
    manifest: &VoicePackManifest,
    max_allowed_bytes: usize,
) -> Result<VerifiedVoiceBank, TtsError> {
    if manifest.expected_size_bytes > max_allowed_bytes || raw.len() > max_allowed_bytes {
        return Err(TtsError::ExceedsMemoryCap {
            requested_bytes: raw.len().max(manifest.expected_size_bytes),
            max_allowed_bytes,
        });
    }

    if raw.len() != manifest.expected_size_bytes {
        return Err(TtsError::SizeMismatch {
            expected_bytes: manifest.expected_size_bytes,
            actual_bytes: raw.len(),
        });
    }

    let actual_sha256 = sha256_hex(raw);
    if !constant_time_hex_eq(&actual_sha256, &manifest.expected_sha256_hex) {
        return Err(TtsError::IntegrityMismatch {
            expected_sha256: manifest.expected_sha256_hex.clone(),
            actual_sha256,
        });
    }

    if raw.len() < 20 {
        return Err(TtsError::InvalidVoicePackHeader(
            "Voice pack shorter than 20-byte header".to_string(),
        ));
    }

    if &raw[0..8] != VOICE_PACK_MAGIC {
        return Err(TtsError::InvalidVoicePackHeader(
            "Invalid MKTTS01 magic header".to_string(),
        ));
    }

    let version = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
    if version != VOICE_PACK_VERSION {
        return Err(TtsError::InvalidVoicePackHeader(format!(
            "Unsupported voice pack version: {}",
            version
        )));
    }

    let header_len = u32::from_le_bytes([raw[12], raw[13], raw[14], raw[15]]) as usize;
    let slab_len = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]) as usize;

    let expected_total = 20usize
        .checked_add(header_len)
        .and_then(|v| v.checked_add(slab_len))
        .ok_or_else(|| TtsError::InvalidVoicePackHeader("Header length overflow".to_string()))?;

    if expected_total > max_allowed_bytes {
        return Err(TtsError::ExceedsMemoryCap {
            requested_bytes: expected_total,
            max_allowed_bytes,
        });
    }

    if expected_total != raw.len() {
        return Err(TtsError::InvalidVoicePackHeader(format!(
            "Header payload lengths ({}) != raw file length ({})",
            expected_total,
            raw.len()
        )));
    }

    let parsed_header: SerializedVoiceHeader = serde_json::from_slice(&raw[20..20 + header_len])
        .map_err(|e| TtsError::InvalidVoicePackHeader(format!("Corrupt header JSON: {}", e)))?;

    // Allocate and touch every 4 KB page of the acoustic tensor slab so OS PrivatePageCount commits it
    let mut acoustic_tensor_slab = Vec::with_capacity(slab_len);
    acoustic_tensor_slab.extend_from_slice(&raw[20 + header_len..]);
    for page in acoustic_tensor_slab.chunks_mut(4096) {
        page[0] = page[0].wrapping_add(1);
    }
    std::hint::black_box(&acoustic_tensor_slab);

    Ok(VerifiedVoiceBank {
        voice_id: parsed_header.voice_id,
        base_pitch_es_hz: parsed_header.base_pitch_es_hz,
        base_pitch_en_hz: parsed_header.base_pitch_en_hz,
        formants: parsed_header.formants,
        acoustic_tensor_slab,
    })
}

// ============================================================================
// 3. TEXT NORMALIZER, SSML SANITIZER & STREAMING CHUNK SYNTHESIZER
// ============================================================================

/// Strips untrusted HTML/SSML tags (`<...>`) and control characters, normalizes whitespace,
/// and enforces `MAX_INPUT_TEXT_BYTES`.
pub fn normalize_and_sanitize_tts_text(raw_input: &str) -> Result<String, TtsError> {
    if raw_input.len() > MAX_INPUT_TEXT_BYTES {
        return Err(TtsError::TextTooLong {
            actual_bytes: raw_input.len(),
            max_bytes: MAX_INPUT_TEXT_BYTES,
        });
    }

    let mut clean = String::with_capacity(raw_input.len());
    let mut inside_tag = false;

    for ch in raw_input.chars() {
        if ch == '<' {
            inside_tag = true;
            clean.push(' ');
            continue;
        }
        if ch == '>' {
            inside_tag = false;
            clean.push(' ');
            continue;
        }
        if inside_tag {
            continue;
        }
        if ch.is_control() {
            clean.push(' ');
            continue;
        }
        if ch.is_alphanumeric() || " .,;:?!-_'/()áéíóúüñÁÉÍÓÚÜÑ".contains(ch) {
            clean.push(ch);
        } else {
            clean.push(' ');
        }
    }

    let collapsed = clean.split_whitespace().collect::<Vec<&str>>().join(" ");
    let has_speakable = collapsed.chars().any(|c| c.is_alphanumeric());
    if !has_speakable {
        return Err(TtsError::EmptySanitizedText);
    }

    Ok(collapsed)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsAudioChunk {
    pub chunk_index: usize,
    pub sample_rate_hz: u32,
    pub samples_pcm16: Vec<i16>,
    pub duration_ms: f64,
    pub rms_lip_sync: f32,
    pub generation_latency_ms: f64,
    pub is_final_chunk: bool,
}

#[derive(Debug, Clone)]
struct ProsodyFrameSpec {
    f0_hz: f32,
    f1_hz: f32,
    f2_hz: f32,
    f3_hz: f32,
    amplitude: f32,
}

pub struct TtsChunkStream {
    sample_rate_hz: u32,
    samples_per_chunk: usize,
    frames: Vec<ProsodyFrameSpec>,
    current_chunk_idx: usize,
    phase_f0: f32,
    phase_f1: f32,
    phase_f2: f32,
    phase_f3: f32,
    tensor_probe_byte: u8,
}

impl Iterator for TtsChunkStream {
    type Item = TtsAudioChunk;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_chunk_idx >= self.frames.len() {
            return None;
        }

        let t0 = Instant::now();
        let spec = &self.frames[self.current_chunk_idx];
        let n = self.samples_per_chunk;
        let sr = self.sample_rate_hz as f32;
        let mut pcm = Vec::with_capacity(n);
        let mut sum_sq = 0.0f64;

        let probe_bias = (self.tensor_probe_byte as f32) * 1e-6;
        let two_pi = 2.0 * std::f32::consts::PI;

        for i in 0..n {
            let progress = i as f32 / n as f32;
            // Smooth raised-cosine edge window at chunk boundaries to avoid clicks
            let edge_env = if progress < 0.08 {
                progress / 0.08
            } else if progress > 0.92 {
                (1.0 - progress) / 0.08
            } else {
                1.0
            };

            self.phase_f0 = (self.phase_f0 + two_pi * spec.f0_hz / sr) % two_pi;
            self.phase_f1 = (self.phase_f1 + two_pi * spec.f1_hz / sr) % two_pi;
            self.phase_f2 = (self.phase_f2 + two_pi * spec.f2_hz / sr) % two_pi;
            self.phase_f3 = (self.phase_f3 + two_pi * spec.f3_hz / sr) % two_pi;

            let glottal = self.phase_f0.sin() * 0.45 + (2.0 * self.phase_f0).sin() * 0.20;
            let resonance =
                self.phase_f1.sin() * 0.50 + self.phase_f2.sin() * 0.32 + self.phase_f3.sin() * 0.18;

            let raw_val = (glottal * 0.45 + resonance * 0.55 + probe_bias) * spec.amplitude * edge_env;
            let sample_i16 = (raw_val * 18_500.0).clamp(-32_000.0, 32_000.0) as i16;
            let norm = sample_i16 as f64 / 32_768.0;
            sum_sq += norm * norm;
            pcm.push(sample_i16);
        }

        let rms = (sum_sq / (n.max(1) as f64)).sqrt() as f32;
        // Scale RMS into [0.0, 1.0] lip-sync opening factor for the avatar's `mouth_open` spring
        let rms_lip_sync = (rms * 2.8).clamp(0.0, 1.0);
        let gen_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let chunk_index = self.current_chunk_idx;
        self.current_chunk_idx += 1;
        let is_final_chunk = self.current_chunk_idx >= self.frames.len();

        Some(TtsAudioChunk {
            chunk_index,
            sample_rate_hz: self.sample_rate_hz,
            samples_pcm16: pcm,
            duration_ms: CHUNK_DURATION_MS as f64,
            rms_lip_sync,
            generation_latency_ms: gen_ms,
            is_final_chunk,
        })
    }
}

// ============================================================================
// 4. ON-DEMAND TTS SESSION WITH TTL LIFECYCLE
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TtsLifecycleState {
    Unloaded,
    Ready {
        loaded_at_ms: u64,
        last_used_ms: u64,
        allocated_bytes: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingSynthesisSummary {
    pub sanitized_text: String,
    pub language: Language,
    pub sample_rate_hz: u32,
    pub chunk_count: usize,
    pub total_samples: usize,
    pub total_audio_duration_ms: f64,
    pub ttfa_ms: f64,
    pub total_synthesis_time_ms: f64,
    pub rtf: f64,
    pub max_chunk_latency_ms: f64,
    pub underrun_chunks: usize,
    pub mean_lip_sync_rms: f32,
}

pub struct OnDemandTtsSession {
    ttl_ms: u64,
    max_voice_pack_bytes: usize,
    state: TtsLifecycleState,
    voice_bank: Option<VerifiedVoiceBank>,
}

impl OnDemandTtsSession {
    pub fn new(ttl_ms: u64, max_voice_pack_bytes: usize) -> Self {
        Self {
            ttl_ms,
            max_voice_pack_bytes,
            state: TtsLifecycleState::Unloaded,
            voice_bank: None,
        }
    }

    pub fn state(&self) -> &TtsLifecycleState {
        &self.state
    }

    pub fn allocated_voice_bytes(&self) -> usize {
        self.voice_bank
            .as_ref()
            .map(|vb| vb.allocated_bytes())
            .unwrap_or(0)
    }

    pub fn load_verified(
        &mut self,
        raw_voice_bytes: &[u8],
        manifest: &VoicePackManifest,
        now_ms: u64,
    ) -> Result<usize, TtsError> {
        let verified =
            verify_and_parse_voice_pack(raw_voice_bytes, manifest, self.max_voice_pack_bytes)?;
        let bytes = verified.allocated_bytes();
        self.voice_bank = Some(verified);
        self.state = TtsLifecycleState::Ready {
            loaded_at_ms: now_ms,
            last_used_ms: now_ms,
            allocated_bytes: bytes,
        };
        Ok(bytes)
    }

    pub fn start_stream(
        &mut self,
        raw_text: &str,
        language: Language,
        sample_rate_hz: u32,
        now_ms: u64,
    ) -> Result<TtsChunkStream, TtsError> {
        if sample_rate_hz != DEFAULT_SAMPLE_RATE_HZ && sample_rate_hz != FALLBACK_SAMPLE_RATE_HZ {
            return Err(TtsError::UnsupportedSampleRate(sample_rate_hz));
        }
        let bank = self.voice_bank.as_ref().ok_or(TtsError::VoicePackNotLoaded)?;
        let sanitized = normalize_and_sanitize_tts_text(raw_text)?;

        let base_pitch = match language {
            Language::Es => bank.base_pitch_es_hz,
            Language::En => bank.base_pitch_en_hz,
        };

        let samples_per_chunk = ((sample_rate_hz as usize) * (CHUNK_DURATION_MS as usize)) / 1000;
        let tensor_probe_byte = bank.acoustic_tensor_slab.first().copied().unwrap_or(1);

        let mut frames = Vec::new();
        let chars: Vec<char> = sanitized.chars().collect();
        // Group characters into ~2-char syllable steps (each step = one 40 ms chunk)
        let total_steps = (chars.len() / 2).max(4);

        for step in 0..total_steps {
            let ch = chars[(step * 2).min(chars.len() - 1)].to_ascii_lowercase();
            if ch == '.' || ch == ',' || ch == ';' || ch == ':' {
                frames.push(ProsodyFrameSpec {
                    f0_hz: base_pitch,
                    f1_hz: 500.0,
                    f2_hz: 1500.0,
                    f3_hz: 2500.0,
                    amplitude: 0.0, // Prosodic pause chunk
                });
                continue;
            }

            let formant_idx = match ch {
                'a' | 'á' => 0,
                'e' | 'é' => 1,
                'i' | 'í' | 'y' => 2,
                'o' | 'ó' => 3,
                'u' | 'ú' | 'w' => 4,
                _ => (ch as usize) % bank.formants.len().max(1),
            };
            let entry = &bank.formants[formant_idx.min(bank.formants.len() - 1)];
            let intonation = 1.0 - 0.08 * (step as f32 / total_steps as f32);
            let amp = if ch == ' ' { 0.18 } else { 0.85 };

            frames.push(ProsodyFrameSpec {
                f0_hz: base_pitch * intonation,
                f1_hz: entry.f1_hz,
                f2_hz: entry.f2_hz,
                f3_hz: entry.f3_hz,
                amplitude: amp,
            });
        }

        if let TtsLifecycleState::Ready {
            loaded_at_ms,
            allocated_bytes,
            ..
        } = self.state
        {
            self.state = TtsLifecycleState::Ready {
                loaded_at_ms,
                last_used_ms: now_ms,
                allocated_bytes,
            };
        }

        Ok(TtsChunkStream {
            sample_rate_hz,
            samples_per_chunk,
            frames,
            current_chunk_idx: 0,
            phase_f0: 0.0,
            phase_f1: 0.0,
            phase_f2: 0.0,
            phase_f3: 0.0,
            tensor_probe_byte,
        })
    }

    pub fn synthesize_streaming_utterance(
        &mut self,
        raw_text: &str,
        language: Language,
        sample_rate_hz: u32,
        now_ms: u64,
    ) -> Result<(StreamingSynthesisSummary, Vec<TtsAudioChunk>), TtsError> {
        let t_start = Instant::now();
        let sanitized_text = normalize_and_sanitize_tts_text(raw_text)?;
        let mut stream = self.start_stream(raw_text, language, sample_rate_hz, now_ms)?;

        let mut chunks = Vec::new();
        let mut ttfa_ms = 0.0f64;
        let mut max_chunk_latency_ms = 0.0f64;
        let mut underrun_chunks = 0usize;
        let mut sum_lip_sync = 0.0f32;
        let mut total_samples = 0usize;

        while let Some(chunk) = stream.next() {
            if chunks.is_empty() {
                ttfa_ms = t_start.elapsed().as_secs_f64() * 1000.0;
            }
            if chunk.generation_latency_ms > max_chunk_latency_ms {
                max_chunk_latency_ms = chunk.generation_latency_ms;
            }
            if chunk.generation_latency_ms > chunk.duration_ms {
                underrun_chunks += 1;
            }
            sum_lip_sync += chunk.rms_lip_sync;
            total_samples += chunk.samples_pcm16.len();
            chunks.push(chunk);
        }

        let total_synthesis_time_ms = t_start.elapsed().as_secs_f64() * 1000.0;
        let total_audio_duration_ms = chunks.len() as f64 * (CHUNK_DURATION_MS as f64);
        let rtf = if total_audio_duration_ms > 0.0 {
            total_synthesis_time_ms / total_audio_duration_ms
        } else {
            0.0
        };
        let mean_lip_sync_rms = if !chunks.is_empty() {
            sum_lip_sync / (chunks.len() as f32)
        } else {
            0.0
        };

        let summary = StreamingSynthesisSummary {
            sanitized_text,
            language,
            sample_rate_hz,
            chunk_count: chunks.len(),
            total_samples,
            total_audio_duration_ms,
            ttfa_ms,
            total_synthesis_time_ms,
            rtf,
            max_chunk_latency_ms,
            underrun_chunks,
            mean_lip_sync_rms,
        };

        Ok((summary, chunks))
    }

    pub fn poll_ttl(&mut self, now_ms: u64) -> bool {
        if let TtsLifecycleState::Ready { last_used_ms, .. } = self.state {
            if now_ms.saturating_sub(last_used_ms) >= self.ttl_ms {
                self.unload_now();
                return true;
            }
        }
        false
    }

    pub fn unload_now(&mut self) {
        if let Some(mut vb) = self.voice_bank.take() {
            vb.zero_and_release();
        }
        self.state = TtsLifecycleState::Unloaded;
    }
}

// ============================================================================
// 5. PROCESS TREE & PRIVATEPAGECOUNT AUDITOR (WITH CREATIONDATE IDENTITY TUPLE)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRow {
    pub pid: u32,
    pub parent_pid: u32,
    pub private_bytes: u64,
    pub creation_date: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub name_lower: String,
    pub creation_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeAggregation {
    pub root_pid: u32,
    pub total_private_bytes: u64,
    pub process_count: usize,
    pub descendant_count: usize,
    pub identities: Vec<ProcessIdentity>,
}

pub fn parse_powershell_process_tsv(tsv: &str) -> Result<Vec<ProcessRow>, String> {
    let mut rows = Vec::new();
    for (idx, raw_line) in tsv.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 5 {
            return Err(format!(
                "Invalid TSV line {} (expected 5 tab-separated columns pid/ppid/bytes/creation_date/name): '{}'",
                idx + 1,
                line
            ));
        }
        let pid: u32 = parts[0]
            .trim()
            .parse()
            .map_err(|e| format!("Invalid PID on line {}: {}", idx + 1, e))?;
        let parent_pid: u32 = parts[1]
            .trim()
            .parse()
            .map_err(|e| format!("Invalid ParentPID on line {}: {}", idx + 1, e))?;
        let private_bytes: u64 = parts[2]
            .trim()
            .parse()
            .map_err(|e| format!("Invalid PrivatePageCount on line {}: {}", idx + 1, e))?;
        let creation_date = parts[3].trim().to_string();
        let name = parts[4..].join("\t").trim().to_string();
        rows.push(ProcessRow {
            pid,
            parent_pid,
            private_bytes,
            creation_date,
            name,
        });
    }
    Ok(rows)
}

pub fn aggregate_process_tree(root_pid: u32, snapshot: &[ProcessRow]) -> Result<TreeAggregation, String> {
    let by_pid: HashMap<u32, &ProcessRow> = snapshot.iter().map(|r| (r.pid, r)).collect();
    if !by_pid.contains_key(&root_pid) {
        return Err(format!("root_pid {} not found in OS snapshot", root_pid));
    }

    let mut children_map: HashMap<u32, Vec<u32>> = HashMap::new();
    for row in snapshot {
        if row.pid != row.parent_pid {
            children_map.entry(row.parent_pid).or_default().push(row.pid);
        }
    }

    let mut visited = HashSet::new();
    let mut stack = vec![root_pid];
    let mut total_private_bytes = 0u64;
    let mut identities = Vec::new();

    while let Some(pid) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if let Some(row) = by_pid.get(&pid) {
            total_private_bytes = total_private_bytes.saturating_add(row.private_bytes);
            identities.push(ProcessIdentity {
                pid: row.pid,
                name_lower: row.name.to_ascii_lowercase(),
                creation_date: row.creation_date.clone(),
            });
        }
        if let Some(kids) = children_map.get(&pid) {
            for &kid in kids {
                if !visited.contains(&kid) {
                    stack.push(kid);
                }
            }
        }
    }

    let process_count = identities.len();
    let descendant_count = process_count.saturating_sub(1);

    Ok(TreeAggregation {
        root_pid,
        total_private_bytes,
        process_count,
        descendant_count,
        identities,
    })
}

pub fn detect_orphans_by_identity(
    tracked: &[ProcessIdentity],
    post_close_snapshot: &[ProcessRow],
) -> Vec<ProcessIdentity> {
    let live: HashSet<ProcessIdentity> = post_close_snapshot
        .iter()
        .map(|r| ProcessIdentity {
            pid: r.pid,
            name_lower: r.name.to_ascii_lowercase(),
            creation_date: r.creation_date.clone(),
        })
        .collect();

    tracked
        .iter()
        .filter(|id| live.contains(id))
        .cloned()
        .collect()
}

pub fn percentile_f64(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let clamped = pct.clamp(0.0, 100.0) / 100.0;
    let idx = ((sorted.len() - 1) as f64 * clamped).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spike05AuditReport {
    pub step_id: String,
    pub binary_size_mb: f64,
    pub sha256_verified_ok: bool,
    pub sha256_tamper_rejected: bool,
    pub repetitions: usize,
    pub sample_rate_hz: u32,
    pub es_utterances_synthesized: usize,
    pub en_utterances_synthesized: usize,
    pub total_chunks_streamed: usize,
    pub underrun_chunks_total: usize,
    pub ttfa_p50_ms: f64,
    pub ttfa_p95_ms: f64,
    pub rtf_p50: f64,
    pub rtf_p95: f64,
    pub idle_pre_ram_p50_mb: f64,
    pub idle_pre_ram_p95_mb: f64,
    pub active_peak_ram_p50_mb: f64,
    pub active_peak_ram_p95_mb: f64,
    pub post_ttl_ram_p50_mb: f64,
    pub post_ttl_ram_p95_mb: f64,
    pub orphan_count_total: usize,
    pub verifier_ok: bool,
}

pub fn verify_spike_05_report(report: &Spike05AuditReport) -> Result<(), String> {
    if report.repetitions != 20 {
        return Err(format!("Expected 20 repetitions, got {}", report.repetitions));
    }
    if report.binary_size_mb <= 0.0 || report.binary_size_mb >= 15.0 {
        return Err(format!(
            "BUD-09 violation: binary_size_mb {:.2} MB must be > 0 and < 15.0 MB",
            report.binary_size_mb
        ));
    }
    if !report.sha256_verified_ok || !report.sha256_tamper_rejected {
        return Err("SHA-256 verification or tamper rejection failed".to_string());
    }
    if report.es_utterances_synthesized != 40 || report.en_utterances_synthesized != 40 {
        return Err(format!(
            "Expected 40 ES and 40 EN utterances across 20 reps, got ES={} EN={}",
            report.es_utterances_synthesized, report.en_utterances_synthesized
        ));
    }
    if report.underrun_chunks_total != 0 {
        return Err(format!(
            "Detected {} buffer underrun chunks during streaming synthesis",
            report.underrun_chunks_total
        ));
    }
    if report.ttfa_p95_ms > 250.0 {
        return Err(format!(
            "TTFA p95 {:.2} ms exceeds 250.0 ms threshold",
            report.ttfa_p95_ms
        ));
    }
    if report.rtf_p95 >= 0.25 {
        return Err(format!(
            "Real-Time Factor (RTF) p95 {:.4} must be < 0.25",
            report.rtf_p95
        ));
    }
    if report.idle_pre_ram_p95_mb > 30.0 {
        return Err(format!(
            "Idle pre-load RAM p95 {:.2} MB exceeds 30.0 MB threshold",
            report.idle_pre_ram_p95_mb
        ));
    }
    if report.active_peak_ram_p50_mb < 45.0 {
        return Err(format!(
            "Active peak RAM p50 {:.2} MB did not reflect the >45 MB loaded voice bank",
            report.active_peak_ram_p50_mb
        ));
    }
    if report.post_ttl_ram_p95_mb > 30.0 {
        return Err(format!(
            "BUD-01 violation: Post-TTL RAM p95 {:.2} MB exceeds 30.0 MB threshold",
            report.post_ttl_ram_p95_mb
        ));
    }
    if report.orphan_count_total != 0 {
        return Err(format!(
            "Detected {} orphan processes after worker termination",
            report.orphan_count_total
        ));
    }
    Ok(())
}

// ============================================================================
// 6. TDD UNIT & PROPERTY TESTS (LEDGER-0.5-1 .. LEDGER-0.5-4)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tts_voice_pack_sha256_verifier_rejects_corrupted_or_truncated_weights() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let (mut raw, manifest) = build_voice_pack_and_manifest("miku-voice-es-en-v1", 64 * 1024);
        let bank = verify_and_parse_voice_pack(&raw, &manifest, DEFAULT_MAX_VOICE_PACK_BYTES)
            .expect("Valid voice pack must verify");
        assert_eq!(bank.voice_id, "miku-voice-es-en-v1");
        assert!(!bank.formants.is_empty());

        // Flip 1 bit in the acoustic slab
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        let err = verify_and_parse_voice_pack(&raw, &manifest, DEFAULT_MAX_VOICE_PACK_BYTES)
            .err()
            .expect("Tampered voice pack must be rejected before allocating");
        assert!(matches!(err, TtsError::IntegrityMismatch { .. }));

        // Truncate 1 byte
        raw.pop();
        let size_err = verify_and_parse_voice_pack(&raw, &manifest, DEFAULT_MAX_VOICE_PACK_BYTES)
            .err()
            .expect("Truncated voice pack must be rejected");
        assert!(matches!(size_err, TtsError::SizeMismatch { .. }));
    }

    #[test]
    fn test_bilingual_es_en_streaming_tts_first_chunk_latency_and_prosody() {
        let (raw_pack, manifest) = build_voice_pack_and_manifest("miku-voice-es-en-v1", 32 * 1024);
        let mut session = OnDemandTtsSession::new(5_000, DEFAULT_MAX_VOICE_PACK_BYTES);
        session
            .load_verified(&raw_pack, &manifest, 100)
            .expect("Voice pack must load");

        for &sr in &[DEFAULT_SAMPLE_RATE_HZ, FALLBACK_SAMPLE_RATE_HZ] {
            for (phrase, lang) in canonical_bilingual_tts_phrases() {
                let (summary, chunks) = session
                    .synthesize_streaming_utterance(phrase, lang, sr, 200)
                    .expect("Streaming synthesis must succeed");

                assert!(summary.chunk_count >= 4);
                assert!(
                    summary.ttfa_ms <= 250.0,
                    "TTFA {:.2} ms exceeded 250 ms",
                    summary.ttfa_ms
                );
                assert!(summary.rtf < 0.25, "RTF {:.4} exceeded 0.25", summary.rtf);
                assert_eq!(summary.underrun_chunks, 0);

                let expected_samples_per_chunk = (sr as usize * CHUNK_DURATION_MS as usize) / 1000;
                let mut active_lip_sync_chunks = 0usize;
                for c in &chunks {
                    assert_eq!(c.samples_pcm16.len(), expected_samples_per_chunk);
                    assert!(
                        (0.0..=1.0).contains(&c.rms_lip_sync),
                        "rms_lip_sync {} out of [0.0, 1.0]",
                        c.rms_lip_sync
                    );
                    if c.rms_lip_sync > 0.05 {
                        active_lip_sync_chunks += 1;
                    }
                }
                assert!(active_lip_sync_chunks > 0);
                assert!(chunks.last().unwrap().is_final_chunk);
            }
        }
    }

    #[test]
    fn test_tts_text_normalizer_and_ssml_sanitizer_never_panics_on_hostile_input() {
        // 1. Empty / punctuation-only / script tag only inputs return EmptySanitizedText
        for hostile in ["", "   \t\r\n  ", "???...!!!", "<script></script><speak></speak>"] {
            assert_eq!(
                normalize_and_sanitize_tts_text(hostile).unwrap_err(),
                TtsError::EmptySanitizedText
            );
        }

        // 2. Mixed SSML / HTML tags and control chars are stripped while preserving speech text
        let dirty = "<speak><prosody rate=\"fast\">Hola\x00\x07 Miku</prosody><script>alert(1)</script></speak>";
        let clean = normalize_and_sanitize_tts_text(dirty).expect("Must sanitize");
        assert_eq!(clean, "Hola Miku alert(1)");

        // 3. Oversized text > MAX_INPUT_TEXT_BYTES is rejected deterministically
        let huge = "a".repeat(MAX_INPUT_TEXT_BYTES + 100);
        assert!(matches!(
            normalize_and_sanitize_tts_text(&huge).unwrap_err(),
            TtsError::TextTooLong { .. }
        ));
    }

    #[test]
    fn test_tts_on_demand_ttl_lifecycle_unloads_voice_bank_and_zeroes_buffers() {
        let (raw_pack, manifest) =
            build_voice_pack_and_manifest("miku-voice-es-en-v1", 256 * 1024);
        let mut session = OnDemandTtsSession::new(2_000, DEFAULT_MAX_VOICE_PACK_BYTES);

        assert_eq!(*session.state(), TtsLifecycleState::Unloaded);
        assert_eq!(session.allocated_voice_bytes(), 0);

        // Load at t = 500 ms
        let bytes = session
            .load_verified(&raw_pack, &manifest, 500)
            .expect("Must load verified voice pack");
        assert!(bytes >= 256 * 1024);
        assert!(session.allocated_voice_bytes() >= 256 * 1024);

        // Synthesize at t = 1800 ms refreshes last_used_ms to 1800 ms
        let (summary, _) = session
            .synthesize_streaming_utterance(
                "Hola Miku, prueba de ciclo de vida.",
                Language::Es,
                DEFAULT_SAMPLE_RATE_HZ,
                1_800,
            )
            .expect("Must synthesize");
        assert!(summary.chunk_count > 0);

        // At t = 3000 ms (elapsed 1200 ms < 2000 ms TTL), still loaded
        assert!(!session.poll_ttl(3_000));
        assert!(session.allocated_voice_bytes() >= 256 * 1024);

        // At t = 3900 ms (elapsed 2100 ms >= 2000 ms TTL), unloads and zeroes buffers
        assert!(session.poll_ttl(3_900));
        assert_eq!(*session.state(), TtsLifecycleState::Unloaded);
        assert_eq!(session.allocated_voice_bytes(), 0);

        // Attempting to stream while unloaded returns VoicePackNotLoaded
        assert_eq!(
            session
                .start_stream("Hola", Language::Es, DEFAULT_SAMPLE_RATE_HZ, 4_000)
                .err()
                .unwrap(),
            TtsError::VoicePackNotLoaded
        );
    }
}
