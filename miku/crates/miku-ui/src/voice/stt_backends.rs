#![allow(unused_imports)]
use crate::prelude::*;
use std::collections::BTreeMap;
use std::process::Command;

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
    pub expected_exe_sha256: Option<String>,
}

impl WhisperCliLocalBackend {
    pub fn new() -> Self {
        let expected_sha = std::env::var("MIKU_WHISPER_CLI_SHA256")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| WHISPER_CLI_EXE_V192_SHA256.to_string());
        Self {
            executable_override: None,
            expected_exe_sha256: Some(expected_sha),
        }
    }

    pub fn with_executable_path(path: PathBuf) -> Self {
        let mut backend = Self::new();
        backend.executable_override = Some(path);
        backend
    }

    pub fn verify_executable_integrity(&self, exe_path: &Path) -> Result<String, SttModelError> {
        let raw_exe = fs::read(exe_path).map_err(|e| {
            SttModelError::IoFailure(redact_sensitive_voice_diagnostic(&e.to_string()))
        })?;
        let actual_sha = sha256_hex(&raw_exe);
        if let Some(ref expected_sha) = self.expected_exe_sha256 {
            if !actual_sha.eq_ignore_ascii_case(expected_sha.trim()) {
                return Err(SttModelError::Sha256Mismatch {
                    expected_hex: expected_sha.clone(),
                    actual_hex: actual_sha,
                });
            }
        }
        Ok(actual_sha)
    }

    pub fn resolve_executable(&self) -> Option<PathBuf> {
        if let Some(ref custom) = self.executable_override {
            return custom.is_file().then(|| custom.clone());
        }
        if let Ok(env_bin) = std::env::var("MIKU_WHISPER_CLI_PATH") {
            let p = PathBuf::from(env_bin);
            if p.is_file() {
                return Some(p);
            }
        }
        let manifest_fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(|root| root.join("target").join("stt_real_fixtures"));
        if let Some(ref dir) = manifest_fixture_dir {
            for rel in [
                "Release/whisper-cli.exe",
                "whisper-cli.exe",
                "bin/whisper-cli.exe",
                "whisper-cli",
            ] {
                let cand = dir.join(rel);
                if cand.is_file() {
                    return Some(cand);
                }
            }
        }
        if let Ok(local_app) = std::env::var("LOCALAPPDATA") {
            let p = PathBuf::from(local_app)
                .join("Miku")
                .join("bin")
                .join("whisper-cli.exe");
            if p.is_file() {
                return Some(p);
            }
        }
        if let Some(path_os) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_os) {
                for name in ["whisper-cli.exe", "whisper-cli"] {
                    let cand = dir.join(name);
                    if cand.is_file() {
                        return Some(cand);
                    }
                }
            }
        }
        None
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

pub fn decode_riff_wav_pcm16_mono(raw_wav: &[u8]) -> Result<Vec<i16>, SttModelError> {
    if raw_wav.len() < 44 || &raw_wav[0..4] != b"RIFF" || &raw_wav[8..12] != b"WAVE" {
        return Err(SttModelError::IoFailure(
            "invalid RIFF WAVE header".to_string(),
        ));
    }
    let mut pos = 12usize;
    let mut fmt_verified = false;
    while pos + 8 <= raw_wav.len() {
        let chunk_id = &raw_wav[pos..pos + 4];
        let chunk_len = u32::from_le_bytes([
            raw_wav[pos + 4],
            raw_wav[pos + 5],
            raw_wav[pos + 6],
            raw_wav[pos + 7],
        ]) as usize;
        pos += 8;
        if pos + chunk_len > raw_wav.len() {
            break;
        }
        if chunk_id == b"fmt " && chunk_len >= 16 {
            let fmt_tag = u16::from_le_bytes([raw_wav[pos], raw_wav[pos + 1]]);
            let channels = u16::from_le_bytes([raw_wav[pos + 2], raw_wav[pos + 3]]);
            let rate = u32::from_le_bytes([
                raw_wav[pos + 4],
                raw_wav[pos + 5],
                raw_wav[pos + 6],
                raw_wav[pos + 7],
            ]);
            let bits = u16::from_le_bytes([raw_wav[pos + 14], raw_wav[pos + 15]]);
            if fmt_tag != 1 || channels != 1 || rate != 16_000 || bits != 16 {
                return Err(SttModelError::IoFailure(format!(
                    "expected 16kHz mono PCM16 WAV, got tag={fmt_tag} ch={channels} rate={rate} bits={bits}"
                )));
            }
            fmt_verified = true;
        } else if chunk_id == b"data" && fmt_verified {
            let mut pcm = Vec::with_capacity(chunk_len / 2);
            for pair in raw_wav[pos..pos + chunk_len].chunks_exact(2) {
                pcm.push(i16::from_le_bytes([pair[0], pair[1]]));
            }
            return Ok(pcm);
        }
        pos += chunk_len + (chunk_len & 1);
    }
    Err(SttModelError::IoFailure(
        "missing valid fmt/data chunk in WAV".to_string(),
    ))
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

fn normalize_whisper_stdout(raw_stdout: &str) -> String {
    let mut cleaned_lines = Vec::new();
    for line in raw_stdout.lines() {
        let mut s = line.trim();
        if s.starts_with('[') {
            if let Some(end_bracket) = s.find(']') {
                let inside = &s[1..end_bracket];
                if inside.contains("-->") {
                    s = s[end_bracket + 1..].trim();
                }
            }
        }
        if !s.is_empty()
            && !s.eq_ignore_ascii_case("[BLANK_AUDIO]")
            && !s.eq_ignore_ascii_case("(blank audio)")
        {
            cleaned_lines.push(s);
        }
    }
    cleaned_lines.join(" ").trim().to_string()
}

pub fn infer_language_from_text(text: &str) -> SttLanguage {
    let lower = text.to_ascii_lowercase();
    let es = [
        "ejecuta", "estado", "revisa", "abre", "compila", "pruebas", "prueba", "aprueba",
        "cancela", "hola", "permiso", "proyecto", "archivo", "sistema", "rama", "codigo",
        "código",
    ];
    if es.iter().any(|w| lower.contains(w)) {
        SttLanguage::Es
    } else {
        SttLanguage::En
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
        let _verified_exe_sha = self.verify_executable_integrity(&exe)?;
        let wav = encode_riff_wav_pcm16_mono(pcm, sample_rate_hz);
        let wav_path = model_file_path.with_extension("ephemeral_ptt.wav");
        fs::write(&wav_path, &wav).map_err(|e| {
            SttModelError::IoFailure(redact_sensitive_voice_diagnostic(&e.to_string()))
        })?;
        let _guard = EphemeralWavGuard(wav_path.clone(), wav.len());

        let mut cmd = Command::new(&exe);
        if let Some(exe_dir) = exe.parent() {
            cmd.current_dir(exe_dir);
        }
        cmd.args([
            "-m",
            &model_file_path.to_string_lossy(),
            "-f",
            &wav_path.to_string_lossy(),
            "-l",
            "auto",
            "-nt",
            "-np",
        ]);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let out = cmd.output().map_err(|e| {
            SttModelError::DecoderProcessFailed(redact_sensitive_voice_diagnostic(&e.to_string()))
        })?;
        if !out.status.success() {
            return Err(SttModelError::DecoderProcessFailed(format!(
                "status {}",
                out.status
            )));
        }
        let text = normalize_whisper_stdout(&String::from_utf8_lossy(&out.stdout));
        if text.is_empty() {
            return Err(SttModelError::NoSpeechRecognized);
        }
        let language = infer_language_from_text(&text);
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
        Self {
            entries_by_pcm_sha256: BTreeMap::new(),
        }
    }

    pub fn register_utterance_fixture(
        &mut self,
        seed: u32,
        transcript: &str,
        language: SttLanguage,
    ) -> Vec<i16> {
        let mut pcm = Vec::with_capacity(6_400);
        let mut state = seed.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
        for i in 0..6_400usize {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let carrier = (((i as u32).wrapping_mul(19 + (seed & 7))) % 64) as i16 - 32;
            pcm.push(((state >> 16) as i16 % 1400) + carrier * 180);
        }
        self.entries_by_pcm_sha256
            .insert(compute_pcm_sha256_hex(&pcm), (transcript.to_string(), language));
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
        let (text, lang) = self
            .entries_by_pcm_sha256
            .get(&compute_pcm_sha256_hex(pcm))
            .ok_or(SttModelError::NoSpeechRecognized)?;
        Ok(LocalSttTranscript {
            text: text.clone(),
            language: *lang,
            confidence: 0.99,
            sample_count: pcm.len(),
            duration_ms: ((pcm.len() as u64) * 1000) / (sample_rate_hz.max(1) as u64),
        })
    }
}
