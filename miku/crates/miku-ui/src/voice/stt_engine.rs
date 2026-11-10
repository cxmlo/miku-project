#![allow(unused_imports)]
use crate::prelude::*;
use std::fmt;
use std::sync::atomic::{compiler_fence, Ordering as AtomicOrdering};

pub const GROQ_STT_DEFAULT_ENDPOINT: &str =
    "https://api.groq.com/openai/v1/audio/transcriptions";
pub const GROQ_API_HOST: &str = "api.groq.com";
pub const GROQ_DEFAULT_KEY_ALIAS: &str = "groq-stt-key";
pub const GROQ_MIN_BILLED_SECONDS_PER_REQUEST: u64 = 10;
pub const DEFAULT_GROQ_STT_TIMEOUT_MS: u64 = 10_000;
pub const MAX_STT_WAV_UPLOAD_BYTES: usize = 25 * 1024 * 1024;
pub const MIN_SPEECH_RMS_AMPLITUDE: f32 = 120.0;

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

    pub fn from_groq_language_field(raw: &str) -> Option<Self> {
        let lower = raw.trim().to_ascii_lowercase();
        if lower == "es" || lower == "spa" || lower.starts_with("spanish") || lower.starts_with("espa") {
            Some(Self::Es)
        } else if lower == "en" || lower == "eng" || lower.starts_with("english") || lower.starts_with("ingl") {
            Some(Self::En)
        } else {
            None
        }
    }
}

/// Official Groq Speech-to-Text Whisper models (`POST /openai/v1/audio/transcriptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroqWhisperModel {
    #[serde(rename = "whisper-large-v3-turbo")]
    WhisperLargeV3Turbo,
    #[serde(rename = "whisper-large-v3")]
    WhisperLargeV3,
}

impl GroqWhisperModel {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::WhisperLargeV3Turbo => "whisper-large-v3-turbo",
            Self::WhisperLargeV3 => "whisper-large-v3",
        }
    }

    pub const fn hourly_rate_usd_micros(&self) -> u64 {
        match self {
            Self::WhisperLargeV3Turbo => 40_000, // $0.04 / hour
            Self::WhisperLargeV3 => 111_000,     // $0.111 / hour
        }
    }

    pub fn parse_id(raw: &str) -> Result<Self, SttEngineError> {
        match raw.trim() {
            "whisper-large-v3-turbo" => Ok(Self::WhisperLargeV3Turbo),
            "whisper-large-v3" => Ok(Self::WhisperLargeV3),
            other => Err(SttEngineError::UnsupportedGroqModel(
                redact_sensitive_voice_diagnostic(other),
            )),
        }
    }
}

/// Cloud billing metadata for a Groq Whisper transcription request.
/// Groq bills by audio duration with a strict 10-second minimum per request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroqBillingEstimate {
    pub model: GroqWhisperModel,
    pub actual_audio_duration_ms: u64,
    pub billed_duration_seconds: u64,
    pub minimum_billing_applied: bool,
    pub estimated_cost_usd_nanos: u64,
}

impl GroqBillingEstimate {
    pub fn compute(model: GroqWhisperModel, actual_audio_duration_ms: u64) -> Self {
        let ceil_seconds = actual_audio_duration_ms.div_ceil(1_000).max(1);
        let billed_duration_seconds = ceil_seconds.max(GROQ_MIN_BILLED_SECONDS_PER_REQUEST);
        let minimum_billing_applied = ceil_seconds < GROQ_MIN_BILLED_SECONDS_PER_REQUEST;
        // hourly_rate_usd_micros * 1_000 = nanos per hour; divide by 3600 seconds/hour
        let estimated_cost_usd_nanos =
            (billed_duration_seconds * model.hourly_rate_usd_micros() * 1_000).div_ceil(3_600);
        Self {
            model,
            actual_audio_duration_ms,
            billed_duration_seconds,
            minimum_billing_applied,
            estimated_cost_usd_nanos,
        }
    }
}

/// Configuration for Groq Speech-to-Text (`F-16` / Step 4.1).
/// Never stores raw API keys: holds only `api_key_alias` resolved via the secure credential store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroqSttConfig {
    pub endpoint_url: String,
    pub model: GroqWhisperModel,
    pub api_key_alias: String,
    pub language_hint: Option<SttLanguage>,
    pub response_format: String,
    pub timeout_ms: u64,
    pub cloud_audio_consent_granted: bool,
}

impl Default for GroqSttConfig {
    fn default() -> Self {
        Self {
            endpoint_url: GROQ_STT_DEFAULT_ENDPOINT.to_string(),
            model: GroqWhisperModel::WhisperLargeV3Turbo,
            api_key_alias: GROQ_DEFAULT_KEY_ALIAS.to_string(),
            language_hint: None,
            response_format: "verbose_json".to_string(),
            timeout_ms: DEFAULT_GROQ_STT_TIMEOUT_MS,
            cloud_audio_consent_granted: false,
        }
    }
}

impl GroqSttConfig {
    pub fn with_consent(mut self, granted: bool) -> Self {
        self.cloud_audio_consent_granted = granted;
        self
    }

    pub fn with_endpoint(mut self, endpoint_url: impl Into<String>) -> Self {
        self.endpoint_url = endpoint_url.into();
        self
    }

    pub fn with_model(mut self, model: GroqWhisperModel) -> Self {
        self.model = model;
        self
    }

    pub fn with_key_alias(mut self, alias: impl Into<String>) -> Self {
        self.api_key_alias = alias.into();
        self
    }

    pub fn with_language_hint(mut self, lang: Option<SttLanguage>) -> Self {
        self.language_hint = lang;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroqSttTranscript {
    pub text: String,
    pub language: SttLanguage,
    pub model: GroqWhisperModel,
    pub sample_count: usize,
    pub duration_ms: u64,
    pub billing: GroqBillingEstimate,
}

pub type LocalSttTranscript = GroqSttTranscript;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttEngineError {
    CloudAudioConsentRequired,
    GatekeeperDenied(String),
    InvalidSecretAlias(String),
    MissingApiKeyAlias(String),
    SecretStoreFailure(String),
    InsecureTransport(String),
    InvalidEndpoint(String),
    UnsupportedGroqModel(String),
    AudioPayloadTooLarge { actual_bytes: usize, max_bytes: usize },
    EmptyAudioSample,
    SilenceOrNoSpeechDetected,
    NoSpeechRecognized,
    NetworkTimeout(String),
    NetworkFailure(String),
    UpstreamHttpError { status_code: u16, safe_body: String },
    MalformedProviderResponse(String),
    IoFailure(String),
    UiFailure(String),
    DiagnosticFailure { context: String, safe_message: String },
}

pub type SttModelError = SttEngineError;

impl SttEngineError {
    pub fn from_untrusted_diagnostic(context: &str, raw_diag: &str, active_secrets: &[&[u8]]) -> Self {
        Self::DiagnosticFailure {
            context: context.to_string(),
            safe_message: redact_sensitive_voice_diagnostic_with_secrets(raw_diag, active_secrets),
        }
    }
}

impl fmt::Display for SttEngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DiagnosticFailure { context, safe_message } => {
                write!(f, "{}: {}", context, safe_message)
            }
            Self::UpstreamHttpError { status_code, safe_body } => {
                write!(f, "Groq STT HTTP {}: {}", status_code, safe_body)
            }
            other => write!(f, "{:?}", other),
        }
    }
}

/// RAII volatile-zeroizing byte buffer for API keys and ephemeral audio/multipart payloads.
pub struct ZeroizingBytes(Vec<u8>);

impl ZeroizingBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn zeroize_now(&mut self) {
        for b in self.0.iter_mut() {
            unsafe {
                std::ptr::write_volatile(b, 0);
            }
        }
        compiler_fence(AtomicOrdering::SeqCst);
        self.0.clear();
        self.0.shrink_to_fit();
    }
}

impl Drop for ZeroizingBytes {
    fn drop(&mut self) {
        self.zeroize_now();
    }
}

impl fmt::Debug for ZeroizingBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[REDACTED_SECRET]")
    }
}

/// Validates that a credential alias conforms to `SecretManager` naming rules:
/// non-empty, ASCII alphanumeric with `-` or `_`, and zero path traversal characters.
pub fn validate_voice_secret_alias(alias: &str) -> Result<(), SttEngineError> {
    let trimmed = alias.trim();
    if trimmed.is_empty() {
        return Err(SttEngineError::InvalidSecretAlias(
            "alias cannot be empty".to_string(),
        ));
    }
    if trimmed.contains("..") || trimmed.contains('/') || trimmed.contains('\\') {
        return Err(SttEngineError::InvalidSecretAlias(
            "alias cannot contain path traversal characters".to_string(),
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(SttEngineError::InvalidSecretAlias(format!(
            "alias '{}' must be alphanumeric, '-' or '_'",
            trimmed
        )));
    }
    Ok(())
}

/// Secure credential resolver interface retrieving API keys strictly by alias.
pub trait VoiceSecretAliasStore: Send {
    fn load_secret_by_alias(&self, alias: &str) -> Result<Option<ZeroizingBytes>, SttEngineError>;
}

/// In-memory encrypted/zeroizing credential vault for isolated unit and integration tests.
#[derive(Default)]
pub struct InMemoryVoiceSecretVault {
    entries: BTreeMap<String, Vec<u8>>,
    read_attempts: AtomicU64,
}

impl InMemoryVoiceSecretVault {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_alias_secret(alias: &str, secret: &[u8]) -> Result<Self, SttEngineError> {
        let mut vault = Self::new();
        vault.set_alias_secret(alias, secret)?;
        Ok(vault)
    }

    pub fn set_alias_secret(&mut self, alias: &str, secret: &[u8]) -> Result<(), SttEngineError> {
        validate_voice_secret_alias(alias)?;
        if secret.is_empty() {
            return Err(SttEngineError::MissingApiKeyAlias(alias.to_string()));
        }
        self.entries.insert(alias.to_string(), secret.to_vec());
        Ok(())
    }

    pub fn remove_alias(&mut self, alias: &str) {
        if let Some(mut old) = self.entries.remove(alias) {
            for b in old.iter_mut() {
                unsafe { std::ptr::write_volatile(b, 0) };
            }
            compiler_fence(AtomicOrdering::SeqCst);
        }
    }

    pub fn read_attempts(&self) -> u64 {
        self.read_attempts.load(Ordering::SeqCst)
    }
}

impl Drop for InMemoryVoiceSecretVault {
    fn drop(&mut self) {
        for val in self.entries.values_mut() {
            for b in val.iter_mut() {
                unsafe { std::ptr::write_volatile(b, 0) };
            }
        }
        compiler_fence(AtomicOrdering::SeqCst);
        self.entries.clear();
    }
}

impl VoiceSecretAliasStore for InMemoryVoiceSecretVault {
    fn load_secret_by_alias(&self, alias: &str) -> Result<Option<ZeroizingBytes>, SttEngineError> {
        self.read_attempts.fetch_add(1, Ordering::SeqCst);
        validate_voice_secret_alias(alias)?;
        Ok(self
            .entries
            .get(alias)
            .map(|v| ZeroizingBytes::new(v.clone())))
    }
}

/// Production Windows Credential Manager (`Advapi32.dll` `CredReadW`) alias store reading
/// `miku/secrets/{alias}` (matching `miku_agent::secrets::WindowsCredentialBackend`).
#[derive(Debug, Default)]
pub struct WindowsCredentialAliasStore;

impl WindowsCredentialAliasStore {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "windows")]
mod win_cred {
    use super::*;
    use std::ffi::c_void;

    #[repr(C)]
    struct CREDENTIALW {
        flags: u32,
        cred_type: u32,
        target_name: *mut u16,
        comment: *mut u16,
        last_written: [u32; 2],
        credential_blob_size: u32,
        credential_blob: *mut u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *mut u16,
        user_name: *mut u16,
    }

    const CRED_TYPE_GENERIC: u32 = 1;
    const ERROR_NOT_FOUND: u32 = 1168;

    #[link(name = "advapi32")]
    extern "system" {
        fn CredReadW(
            target_name: *const u16,
            cred_type: u32,
            flags: u32,
            credential: *mut *mut CREDENTIALW,
        ) -> i32;
        fn CredFree(buffer: *mut c_void);
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }

    pub fn read_win32_credential(alias: &str) -> Result<Option<ZeroizingBytes>, SttEngineError> {
        let target = format!("miku/secrets/{}", alias);
        let wide_target: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
        let mut cred_ptr: *mut CREDENTIALW = std::ptr::null_mut();
        let ok = unsafe {
            CredReadW(
                wide_target.as_ptr(),
                CRED_TYPE_GENERIC,
                0,
                &mut cred_ptr,
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(SttEngineError::SecretStoreFailure(format!(
                "CredReadW failed with code {}",
                err
            )));
        }
        if cred_ptr.is_null() {
            return Ok(None);
        }
        let bytes = unsafe {
            let cred = &*cred_ptr;
            let sz = cred.credential_blob_size as usize;
            let slice = std::slice::from_raw_parts(cred.credential_blob, sz);
            slice.to_vec()
        };
        unsafe {
            CredFree(cred_ptr as *mut c_void);
        }
        if bytes.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ZeroizingBytes::new(bytes)))
        }
    }
}

impl VoiceSecretAliasStore for WindowsCredentialAliasStore {
    fn load_secret_by_alias(&self, alias: &str) -> Result<Option<ZeroizingBytes>, SttEngineError> {
        validate_voice_secret_alias(alias)?;
        #[cfg(target_os = "windows")]
        {
            win_cred::read_win32_credential(alias)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = alias;
            Ok(None)
        }
    }
}

/// Redacts transcripts, PCM arrays, API keys (`gsk_*`, `sk-*`), `Bearer` tokens, and any active
/// secret byte slices so no sensitive data ever leaks into errors, logs, IPC, or diagnostics.
pub fn redact_sensitive_voice_diagnostic_with_secrets(
    raw: &str,
    active_secrets: &[&[u8]],
) -> String {
    let mut sanitized = raw.to_string();
    for sec in active_secrets {
        if !sec.is_empty() {
            if let Ok(s) = std::str::from_utf8(sec) {
                let trimmed = s.trim();
                if !trimmed.is_empty() && sanitized.contains(trimmed) {
                    sanitized = sanitized.replace(trimmed, "[REDACTED]");
                }
            }
        }
    }
    redact_sensitive_voice_diagnostic(&sanitized)
}

pub fn redact_sensitive_voice_diagnostic(raw: &str) -> String {
    let mut result = raw.to_string();
    for marker in ["transcript=\"", "transcript='"] {
        while let Some(start) = result.find(marker) {
            let quote = marker.chars().last().unwrap();
            let content_start = start + marker.len();
            if let Some(end_rel) = result[content_start..].find(quote) {
                result.replace_range(
                    start..content_start + end_rel + 1,
                    "transcript=\"[REDACTED]\"",
                );
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
        let clean = lower.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == ';');
        if clean == "bearer" || clean == "authorization:" {
            tokens.push(tok.to_string());
            skip_next = true;
        } else if clean.starts_with("gsk_")
            || clean.starts_with("sk-")
            || clean.starts_with("token=")
            || clean.starts_with("api_key=")
            || clean.starts_with("apikey=")
            || clean.starts_with("eyj")
        {
            tokens.push("[REDACTED]".to_string());
        } else {
            tokens.push(tok.to_string());
        }
    }
    tokens.join(" ")
}
