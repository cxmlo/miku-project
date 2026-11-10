#![allow(unused_imports)]
use crate::prelude::*;
use std::collections::BTreeMap;
use std::process::Command;

pub const GGML_FILE_MAGIC_LE: u32 = 0x67676d6c;
pub const GGML_WHISPER_HEADER_INTS: usize = 12;
pub const GGML_WHISPER_MIN_HEADER_BYTES: usize = GGML_WHISPER_HEADER_INTS * 4;
pub const DEFAULT_STT_MODEL_TTL_MS: u64 = 15_000;
pub const DEFAULT_MAX_STT_MODEL_BYTES: usize = 256 * 1024 * 1024;
pub const MIN_SPEECH_RMS_AMPLITUDE: f32 = 120.0;

pub const WHISPER_GGML_TINY_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin";
pub const WHISPER_GGML_TINY_SHA256: &str =
    "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21";
pub const WHISPER_GGML_TINY_SIZE_BYTES: usize = 77_691_713;

pub const WHISPER_GGML_BASE_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin";
pub const WHISPER_GGML_BASE_SHA256: &str =
    "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe";
pub const WHISPER_GGML_BASE_SIZE_BYTES: usize = 147_951_465;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SttLanguage {
    Es,
    En,
}

impl SttLanguage {
    pub const fn as_code(&self) -> &'static str {
        match self {
            Self::Es => "es",
            Self::En => "en",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SttModelManifest {
    pub model_id: String,
    pub version: String,
    pub license: String,
    pub download_url: String,
    pub expected_sha256_hex: String,
    pub expected_size_bytes: usize,
    pub languages: Vec<SttLanguage>,
}

impl SttModelManifest {
    pub fn default_whisper_tiny() -> Self {
        Self {
            model_id: "ggml-tiny.bin".to_string(),
            version: "1.0.0".to_string(),
            license: "MIT".to_string(),
            download_url: WHISPER_GGML_TINY_URL.to_string(),
            expected_sha256_hex: WHISPER_GGML_TINY_SHA256.to_string(),
            expected_size_bytes: WHISPER_GGML_TINY_SIZE_BYTES,
            languages: vec![SttLanguage::Es, SttLanguage::En],
        }
    }

    pub fn default_whisper_base() -> Self {
        Self {
            model_id: "ggml-base.bin".to_string(),
            version: "1.0.0".to_string(),
            license: "MIT".to_string(),
            download_url: WHISPER_GGML_BASE_URL.to_string(),
            expected_sha256_hex: WHISPER_GGML_BASE_SHA256.to_string(),
            expected_size_bytes: WHISPER_GGML_BASE_SIZE_BYTES,
            languages: vec![SttLanguage::Es, SttLanguage::En],
        }
    }

    pub fn to_download_card(&self) -> SttModelDownloadCard {
        SttModelDownloadCard {
            model_id: self.model_id.clone(),
            version: self.version.clone(),
            license: self.license.clone(),
            expected_size_bytes: self.expected_size_bytes,
            expected_sha256_hex: self.expected_sha256_hex.clone(),
            languages: self.languages.clone(),
            download_url: self.download_url.clone(),
            requires_explicit_user_action: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SttModelDownloadCard {
    pub model_id: String,
    pub version: String,
    pub license: String,
    pub expected_size_bytes: usize,
    pub expected_sha256_hex: String,
    pub languages: Vec<SttLanguage>,
    pub download_url: String,
    pub requires_explicit_user_action: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttModelAvailability {
    VerifiedOnDisk { path: PathBuf, size_bytes: usize },
    Missing { download_card: SttModelDownloadCard },
    CorruptedOnDisk { download_card: SttModelDownloadCard, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttModelError {
    ModelMissingOnDisk(SttModelDownloadCard),
    UserConfirmationRequired,
    GatekeeperDenied(String),
    PathJailViolation(String),
    SizeExceedsMaxLimit { actual_bytes: usize, max_bytes: usize },
    IncompleteDownload { expected_bytes: usize, received_bytes: usize },
    Sha256Mismatch { expected_hex: String, actual_hex: String },
    InvalidModelMagic,
    UnsupportedGgmlHyperparameters(String),
    IoFailure(String),
    UiFailure(String),
    RuntimeNotLoaded,
    EmptyAudioSample,
    SilenceOrNoSpeechDetected,
    NoSpeechRecognized,
    LocalRuntimeBinaryUnavailable(String),
    DecoderProcessFailed(String),
    DiagnosticFailure { context: String, safe_message: String },
}

impl SttModelError {
    pub fn from_untrusted_diagnostic(context: &str, raw_diag: &str) -> Self {
        Self::DiagnosticFailure {
            context: context.to_string(),
            safe_message: redact_sensitive_voice_diagnostic(raw_diag),
        }
    }
}

impl fmt::Display for SttModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DiagnosticFailure { context, safe_message } => write!(f, "{}: {}", context, safe_message),
            other => write!(f, "{:?}", other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GgmlWhisperHparams {
    pub n_vocab: i32,
    pub n_audio_ctx: i32,
    pub n_audio_state: i32,
    pub n_audio_head: i32,
    pub n_audio_layer: i32,
    pub n_text_ctx: i32,
    pub n_text_state: i32,
    pub n_text_head: i32,
    pub n_text_layer: i32,
    pub n_mels: i32,
    pub ftype: i32,
}

impl GgmlWhisperHparams {
    pub fn whisper_tiny_defaults() -> Self {
        Self {
            n_vocab: 51_865,
            n_audio_ctx: 1500,
            n_audio_state: 384,
            n_audio_head: 6,
            n_audio_layer: 4,
            n_text_ctx: 448,
            n_text_state: 384,
            n_text_head: 6,
            n_text_layer: 4,
            n_mels: 80,
            ftype: 1,
        }
    }

    pub fn validate(&self) -> Result<(), SttModelError> {
        let valid_dims = [
            self.n_audio_ctx, self.n_audio_state, self.n_audio_head, self.n_audio_layer,
            self.n_text_ctx, self.n_text_state, self.n_text_head, self.n_text_layer,
        ]
        .iter()
        .all(|&v| v > 0);
        if self.n_vocab < 50_000 || !valid_dims || (self.n_mels != 80 && self.n_mels != 128) {
            return Err(SttModelError::UnsupportedGgmlHyperparameters(format!(
                "invalid whisper hparams: vocab={} mels={}",
                self.n_vocab, self.n_mels
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSttModelWeights {
    pub model_id: String,
    pub version: String,
    pub hparams: GgmlWhisperHparams,
    pub sha256_hex: String,
    pub weight_slab: Vec<u8>,
}

impl VerifiedSttModelWeights {
    pub fn zero_and_release(&mut self) -> usize {
        let released = self.weight_slab.len();
        self.weight_slab.fill(0);
        self.weight_slab.clear();
        self.weight_slab.shrink_to_fit();
        released
    }
}

pub fn compute_pcm_sha256_hex(pcm: &[i16]) -> String {
    let mut bytes = Vec::with_capacity(pcm.len() * 2);
    for s in pcm {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    sha256_hex(&bytes)
}

pub fn build_valid_ggml_whisper_fixture_and_manifest(
    model_id: &str,
    tensor_slab_bytes: usize,
) -> (Vec<u8>, SttModelManifest) {
    let hp = GgmlWhisperHparams::whisper_tiny_defaults();
    let header = [
        GGML_FILE_MAGIC_LE as i32, hp.n_vocab, hp.n_audio_ctx, hp.n_audio_state,
        hp.n_audio_head, hp.n_audio_layer, hp.n_text_ctx, hp.n_text_state,
        hp.n_text_head, hp.n_text_layer, hp.n_mels, hp.ftype,
    ];
    let slab_len = tensor_slab_bytes.max(64);
    let mut payload = Vec::with_capacity(GGML_WHISPER_MIN_HEADER_BYTES + slab_len);
    for val in header {
        payload.extend_from_slice(&val.to_le_bytes());
    }
    for idx in 0..slab_len {
        payload.push(((idx * 31 + 17) & 0xFF) as u8);
    }
    let sha = sha256_hex(&payload);
    let manifest = SttModelManifest {
        model_id: model_id.to_string(),
        version: "1.0.0".to_string(),
        license: "MIT".to_string(),
        download_url: WHISPER_GGML_TINY_URL.to_string(),
        expected_sha256_hex: sha,
        expected_size_bytes: payload.len(),
        languages: vec![SttLanguage::Es, SttLanguage::En],
    };
    (payload, manifest)
}

pub fn verify_and_parse_stt_model_bytes(
    raw: &[u8],
    manifest: &SttModelManifest,
    max_allowed_bytes: usize,
) -> Result<VerifiedSttModelWeights, SttModelError> {
    if raw.len() > max_allowed_bytes {
        return Err(SttModelError::SizeExceedsMaxLimit { actual_bytes: raw.len(), max_bytes: max_allowed_bytes });
    }
    if raw.len() != manifest.expected_size_bytes {
        return Err(SttModelError::IncompleteDownload { expected_bytes: manifest.expected_size_bytes, received_bytes: raw.len() });
    }
    let actual_sha = sha256_hex(raw);
    if !actual_sha.eq_ignore_ascii_case(&manifest.expected_sha256_hex) {
        return Err(SttModelError::Sha256Mismatch { expected_hex: manifest.expected_sha256_hex.clone(), actual_hex: actual_sha });
    }
    if raw.len() < GGML_WHISPER_MIN_HEADER_BYTES {
        return Err(SttModelError::InvalidModelMagic);
    }
    let r32 = |i: usize| i32::from_le_bytes([raw[i * 4], raw[i * 4 + 1], raw[i * 4 + 2], raw[i * 4 + 3]]);
    if (r32(0) as u32) != GGML_FILE_MAGIC_LE {
        return Err(SttModelError::InvalidModelMagic);
    }
    let hparams = GgmlWhisperHparams {
        n_vocab: r32(1), n_audio_ctx: r32(2), n_audio_state: r32(3), n_audio_head: r32(4),
        n_audio_layer: r32(5), n_text_ctx: r32(6), n_text_state: r32(7), n_text_head: r32(8),
        n_text_layer: r32(9), n_mels: r32(10), ftype: r32(11),
    };
    hparams.validate()?;
    Ok(VerifiedSttModelWeights {
        model_id: manifest.model_id.clone(),
        version: manifest.version.clone(),
        hparams,
        sha256_hex: actual_sha,
        weight_slab: raw.to_vec(),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct LocalSttTranscript {
    pub text: String,
    pub language: SttLanguage,
    pub confidence: f32,
    pub sample_count: usize,
    pub duration_ms: u64,
}

pub trait LocalSttDecoderBackend: Send {
    fn decode_pcm16_mono(
        &mut self,
        weights: &VerifiedSttModelWeights,
        model_file_path: &Path,
        pcm: &[i16],
        sample_rate_hz: u32,
    ) -> Result<LocalSttTranscript, SttModelError>;
}

#[derive(Default)]
pub struct WhisperCliLocalBackend {
    pub executable_override: Option<PathBuf>,
}

impl WhisperCliLocalBackend {
    pub fn new() -> Self {
        Self { executable_override: None }
    }

    pub fn with_executable_path(path: PathBuf) -> Self {
        Self { executable_override: Some(path) }
    }

    fn resolve_executable(&self) -> Option<PathBuf> {
        if let Some(ref custom) = self.executable_override {
            return custom.is_file().then(|| custom.clone());
        }
        std::env::var("MIKU_WHISPER_CLI_PATH")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.is_file())
    }
}

pub fn encode_riff_wav_pcm16_mono(pcm: &[i16], sample_rate_hz: u32) -> Vec<u8> {
    let data_bytes = (pcm.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + pcm.len() * 2);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&36u32.saturating_add(data_bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&sample_rate_hz.to_le_bytes());
    wav.extend_from_slice(&sample_rate_hz.saturating_mul(2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_bytes.to_le_bytes());
    for s in pcm {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

struct EphemeralWavGuard(PathBuf, usize);
impl Drop for EphemeralWavGuard {
    fn drop(&mut self) {
        if self.0.is_file() {
            let _ = fs::write(&self.0, vec![0u8; self.1.min(65_536)]);
            let _ = fs::remove_file(&self.0);
        }
    }
}

impl LocalSttDecoderBackend for WhisperCliLocalBackend {
    fn decode_pcm16_mono(
        &mut self,
        _weights: &VerifiedSttModelWeights,
        model_file_path: &Path,
        pcm: &[i16],
        sample_rate_hz: u32,
    ) -> Result<LocalSttTranscript, SttModelError> {
        let exe = self.resolve_executable().ok_or_else(|| {
            SttModelError::LocalRuntimeBinaryUnavailable("whisper-cli missing on host".to_string())
        })?;
        let wav = encode_riff_wav_pcm16_mono(pcm, sample_rate_hz);
        let wav_path = model_file_path.with_extension("ephemeral_ptt.wav");
        fs::write(&wav_path, &wav).map_err(|e| SttModelError::IoFailure(redact_sensitive_voice_diagnostic(&e.to_string())))?;
        let _guard = EphemeralWavGuard(wav_path.clone(), wav.len());

        let mut cmd = Command::new(&exe);
        cmd.args(["-m", &model_file_path.to_string_lossy(), "-f", &wav_path.to_string_lossy(), "-l", "auto", "-nt", "-np"]);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let out = cmd.output().map_err(|e| SttModelError::DecoderProcessFailed(redact_sensitive_voice_diagnostic(&e.to_string())))?;
        if !out.status.success() {
            return Err(SttModelError::DecoderProcessFailed(format!("status {}", out.status)));
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if text.is_empty() {
            return Err(SttModelError::NoSpeechRecognized);
        }
        let lower = text.to_ascii_lowercase();
        let es = ["ejecuta", "estado", "revisa", "abre", "compila", "pruebas", "aprueba", "cancela"];
        let language = if es.iter().any(|w| lower.contains(w)) { SttLanguage::Es } else { SttLanguage::En };
        Ok(LocalSttTranscript {
            text,
            language,
            confidence: 0.92,
            sample_count: pcm.len(),
            duration_ms: ((pcm.len() as u64) * 1000) / (sample_rate_hz.max(1) as u64),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct DeterministicCorpusSttBackend {
    entries_by_pcm_sha256: BTreeMap<String, (String, SttLanguage)>,
}

impl DeterministicCorpusSttBackend {
    pub fn new() -> Self {
        Self { entries_by_pcm_sha256: BTreeMap::new() }
    }

    pub fn register_utterance_fixture(&mut self, seed: u32, transcript: &str, language: SttLanguage) -> Vec<i16> {
        let mut pcm = Vec::with_capacity(6_400);
        let mut state = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
        for i in 0..6_400usize {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let carrier = (((i as u32).wrapping_mul(19 + (seed & 7))) % 64) as i16 - 32;
            pcm.push(((state >> 16) as i16 % 1400) + carrier * 180);
        }
        self.entries_by_pcm_sha256.insert(compute_pcm_sha256_hex(&pcm), (transcript.to_string(), language));
        pcm
    }
}

impl LocalSttDecoderBackend for DeterministicCorpusSttBackend {
    fn decode_pcm16_mono(
        &mut self,
        _weights: &VerifiedSttModelWeights,
        _model_file_path: &Path,
        pcm: &[i16],
        sample_rate_hz: u32,
    ) -> Result<LocalSttTranscript, SttModelError> {
        let (text, lang) = self.entries_by_pcm_sha256.get(&compute_pcm_sha256_hex(pcm)).ok_or(SttModelError::NoSpeechRecognized)?;
        Ok(LocalSttTranscript {
            text: text.clone(),
            language: *lang,
            confidence: 0.99,
            sample_count: pcm.len(),
            duration_ms: ((pcm.len() as u64) * 1000) / (sample_rate_hz.max(1) as u64),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttModelLifecycleState {
    Unloaded,
    Loaded { loaded_at_ms: u64, last_used_ms: u64, byte_len: usize },
}

pub struct OnDemandLocalSttRuntime {
    state: SttModelLifecycleState,
    ttl_ms: u64,
    max_model_bytes: usize,
    loaded_weights: Option<VerifiedSttModelWeights>,
    backend: Box<dyn LocalSttDecoderBackend>,
    network_calls_made: u64,
    provider_tokens_consumed: u64,
}

impl OnDemandLocalSttRuntime {
    pub fn new(ttl_ms: u64, max_model_bytes: usize) -> Self {
        Self::with_backend(ttl_ms, max_model_bytes, Box::new(WhisperCliLocalBackend::new()))
    }

    pub fn with_backend(ttl_ms: u64, max_model_bytes: usize, backend: Box<dyn LocalSttDecoderBackend>) -> Self {
        Self {
            state: SttModelLifecycleState::Unloaded,
            ttl_ms: ttl_ms.max(250),
            max_model_bytes,
            loaded_weights: None,
            backend,
            network_calls_made: 0,
            provider_tokens_consumed: 0,
        }
    }

    pub fn state(&self) -> &SttModelLifecycleState { &self.state }
    pub fn allocated_weight_bytes(&self) -> usize { self.loaded_weights.as_ref().map(|w| w.weight_slab.len()).unwrap_or(0) }
    pub const fn network_calls_made(&self) -> u64 { self.network_calls_made }
    pub const fn provider_tokens_consumed(&self) -> u64 { self.provider_tokens_consumed }

    pub fn load_verified_bytes(&mut self, raw: &[u8], manifest: &SttModelManifest, now_ms: u64) -> Result<usize, SttModelError> {
        let verified = verify_and_parse_stt_model_bytes(raw, manifest, self.max_model_bytes)?;
        let byte_len = verified.weight_slab.len();
        self.loaded_weights = Some(verified);
        self.state = SttModelLifecycleState::Loaded { loaded_at_ms: now_ms, last_used_ms: now_ms, byte_len };
        Ok(byte_len)
    }

    pub fn unload_and_zero(&mut self) -> usize {
        let released = self.loaded_weights.take().map(|mut w| w.zero_and_release()).unwrap_or(0);
        self.state = SttModelLifecycleState::Unloaded;
        released
    }

    pub fn poll_ttl(&mut self, now_ms: u64) -> bool {
        if let SttModelLifecycleState::Loaded { last_used_ms, .. } = self.state {
            if now_ms.saturating_sub(last_used_ms) >= self.ttl_ms {
                self.unload_and_zero();
                return true;
            }
        }
        false
    }

    pub fn transcribe_pcm16_mono(
        &mut self,
        model_file_path: &Path,
        pcm: &[i16],
        sample_rate_hz: u32,
        now_ms: u64,
    ) -> Result<LocalSttTranscript, SttModelError> {
        if pcm.is_empty() { return Err(SttModelError::EmptyAudioSample); }
        let Some(ref weights) = self.loaded_weights else { return Err(SttModelError::RuntimeNotLoaded); };
        if let SttModelLifecycleState::Loaded { loaded_at_ms, byte_len, .. } = self.state {
            self.state = SttModelLifecycleState::Loaded { loaded_at_ms, last_used_ms: now_ms, byte_len };
        }
        let rms = (pcm.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / (pcm.len() as f64)).sqrt() as f32;
        if rms < MIN_SPEECH_RMS_AMPLITUDE { return Err(SttModelError::SilenceOrNoSpeechDetected); }
        self.backend.decode_pcm16_mono(weights, model_file_path, pcm, sample_rate_hz)
    }
}

pub fn redact_sensitive_voice_diagnostic(raw: &str) -> String {
    let mut result = raw.to_string();
    for marker in ["transcript=\"", "transcript='"] {
        while let Some(start) = result.find(marker) {
            let quote = marker.chars().last().unwrap();
            let content_start = start + marker.len();
            if let Some(end_rel) = result[content_start..].find(quote) {
                result.replace_range(start..content_start + end_rel + 1, "transcript=\"[REDACTED]\"");
            } else {
                result.replace_range(start.., "transcript=\"[REDACTED]\"");
                break;
            }
        }
    }
    while let Some(start) = result.find("pcm=[") {
        if let Some(end_rel) = result[start..].find(']') {
            result.replace_range(start..start + end_rel + 1, "pcm=[REDACTED]");
        } else {
            result.replace_range(start.., "pcm=[REDACTED]");
            break;
        }
    }
    let mut tokens = Vec::new();
    let mut skip_next = false;
    for tok in result.split_whitespace() {
        if skip_next {
            tokens.push("[REDACTED]".to_string());
            skip_next = false;
            continue;
        }
        let lower = tok.to_ascii_lowercase();
        if lower == "bearer" {
            tokens.push("Bearer".to_string());
            skip_next = true;
        } else if lower.starts_with("sk-") || lower.starts_with("token=") || lower.starts_with("api_key=") || lower.starts_with("eyj") {
            tokens.push("[REDACTED]".to_string());
        } else {
            tokens.push(tok.to_string());
        }
    }
    tokens.join(" ")
}
