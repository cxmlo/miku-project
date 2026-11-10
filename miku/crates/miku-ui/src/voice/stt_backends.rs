#![allow(unused_imports)]
use crate::prelude::*;
use std::io::{Read, Write as IoWrite};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{compiler_fence, Ordering as AtomicOrdering};
use std::time::Duration;

pub const GROQ_MULTIPART_BOUNDARY: &str = "MikuGroqSttBoundaryV1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroqEndpointTarget {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,
}

pub fn is_loopback_host(host: &str) -> bool {
    let clean = host.trim().trim_start_matches('[').trim_end_matches(']');
    if clean.eq_ignore_ascii_case("localhost") || clean.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }
    clean.parse::<std::net::IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// Validates and parses the Groq STT endpoint URL.
/// Remote hosts strictly require `https://`; `http://` is permitted only on local loopback for tests.
pub fn parse_groq_endpoint(endpoint: &str) -> Result<GroqEndpointTarget, SttEngineError> {
    let trimmed = endpoint.trim();
    let (scheme, without_scheme, default_port) = if let Some(rest) = trimmed.strip_prefix("https://") {
        ("https".to_string(), rest, 443u16)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("http".to_string(), rest, 80u16)
    } else {
        return Err(SttEngineError::InsecureTransport(
            "unsupported scheme: only https:// (or loopback http://) is allowed".to_string(),
        ));
    };

    let (host_port, path) = match without_scheme.find('/') {
        Some(idx) => (&without_scheme[..idx], &without_scheme[idx..]),
        None => (without_scheme, "/"),
    };

    let (host, port) = if host_port.starts_with('[') {
        let Some(end_idx) = host_port.find(']') else {
            return Err(SttEngineError::InvalidEndpoint("unclosed IPv6 literal".to_string()));
        };
        let h = &host_port[1..end_idx];
        let after = &host_port[end_idx + 1..];
        let p = if let Some(colon_idx) = after.find(':') {
            after[colon_idx + 1..]
                .parse::<u16>()
                .map_err(|_| SttEngineError::InvalidEndpoint("invalid IPv6 port".to_string()))?
        } else {
            default_port
        };
        (h.to_string(), p)
    } else {
        match host_port.find(':') {
            Some(idx) => {
                let h = &host_port[..idx];
                let p = host_port[idx + 1..]
                    .parse::<u16>()
                    .map_err(|_| SttEngineError::InvalidEndpoint("invalid port".to_string()))?;
                (h.to_string(), p)
            }
            None => (host_port.to_string(), default_port),
        }
    };

    if host.trim().is_empty() || host == "0.0.0.0" || host == "::" {
        return Err(SttEngineError::InvalidEndpoint("empty or unspecified host is forbidden".to_string()));
    }
    if scheme == "http" && !is_loopback_host(&host) {
        return Err(SttEngineError::InsecureTransport(format!(
            "remote plaintext HTTP rejected for host '{host}': HTTPS required"
        )));
    }

    Ok(GroqEndpointTarget {
        scheme,
        host,
        port,
        path: path.to_string(),
    })
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

pub fn decode_riff_wav_pcm16_mono(raw_wav: &[u8]) -> Result<Vec<i16>, SttEngineError> {
    if raw_wav.len() < 44 || &raw_wav[0..4] != b"RIFF" || &raw_wav[8..12] != b"WAVE" {
        return Err(SttEngineError::IoFailure("invalid RIFF WAVE header".to_string()));
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
                return Err(SttEngineError::IoFailure(format!(
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
    Err(SttEngineError::IoFailure("missing valid fmt/data chunk in WAV".to_string()))
}

/// Builds a `multipart/form-data` body for `POST /openai/v1/audio/transcriptions`
/// wrapped in `ZeroizingBytes` so the WAV payload is zero-filled on drop.
pub fn build_groq_multipart_body(
    wav_bytes: &[u8],
    config: &GroqSttConfig,
) -> Result<ZeroizingBytes, SttEngineError> {
    if wav_bytes.len() > MAX_STT_WAV_UPLOAD_BYTES {
        return Err(SttEngineError::AudioPayloadTooLarge {
            actual_bytes: wav_bytes.len(),
            max_bytes: MAX_STT_WAV_UPLOAD_BYTES,
        });
    }
    let mut body = Vec::with_capacity(wav_bytes.len() + 1024);
    let mut append_field = |name: &str, value: &str| {
        body.extend_from_slice(
            format!("--{GROQ_MULTIPART_BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
                .as_bytes(),
        );
    };

    append_field("model", config.model.as_str());
    append_field("response_format", config.response_format.trim());
    append_field("temperature", "0");
    if let Some(lang) = config.language_hint {
        append_field("language", lang.as_code());
    }

    body.extend_from_slice(
        format!(
            "--{GROQ_MULTIPART_BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"utterance.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(wav_bytes);
    body.extend_from_slice(format!("\r\n--{GROQ_MULTIPART_BOUNDARY}--\r\n").as_bytes());

    Ok(ZeroizingBytes::new(body))
}

pub fn infer_language_from_text(text: &str) -> SttLanguage {
    let lower = text.to_ascii_lowercase();
    let es = [
        "ejecuta", "estado", "revisa", "abre", "compila", "pruebas", "prueba", "aprueba",
        "cancela", "hola", "permiso", "proyecto", "archivo", "sistema", "rama", "codigo", "código",
    ];
    if es.iter().any(|w| lower.contains(w)) {
        SttLanguage::Es
    } else {
        SttLanguage::En
    }
}

pub fn parse_groq_transcription_json(
    raw_body: &[u8],
    config: &GroqSttConfig,
    sample_count: usize,
    sample_rate_hz: u32,
    active_secrets: &[&[u8]],
) -> Result<GroqSttTranscript, SttEngineError> {
    let parsed: serde_json::Value = serde_json::from_slice(raw_body).map_err(|e| {
        SttEngineError::MalformedProviderResponse(redact_sensitive_voice_diagnostic_with_secrets(
            &e.to_string(),
            active_secrets,
        ))
    })?;

    let text = parsed
        .get("text")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .ok_or_else(|| {
            SttEngineError::MalformedProviderResponse("missing 'text' field in Groq STT JSON response".to_string())
        })?;

    if text.is_empty() {
        return Err(SttEngineError::NoSpeechRecognized);
    }

    let language = parsed
        .get("language")
        .and_then(|v| v.as_str())
        .and_then(SttLanguage::from_groq_language_field)
        .or(config.language_hint)
        .unwrap_or_else(|| infer_language_from_text(&text));

    let duration_ms = ((sample_count as u64) * 1000) / (sample_rate_hz.max(1) as u64);
    let billing = GroqBillingEstimate::compute(config.model, duration_ms);

    Ok(GroqSttTranscript {
        text,
        language,
        model: config.model,
        sample_count,
        duration_ms,
        billing,
    })
}

pub trait GroqSttTransport: Send {
    fn transcribe_pcm16_mono(
        &mut self,
        config: &GroqSttConfig,
        api_key: &ZeroizingBytes,
        pcm: &[i16],
        sample_rate_hz: u32,
    ) -> Result<GroqSttTranscript, SttEngineError>;
}

#[derive(Default)]
pub struct GroqWhisperHttpClient {
    requests_sent: u64,
    last_billed_seconds: u64,
}

impl GroqWhisperHttpClient {
    pub fn new() -> Self {
        Self::default()
    }
    pub const fn requests_sent(&self) -> u64 {
        self.requests_sent
    }
    pub const fn last_billed_seconds(&self) -> u64 {
        self.last_billed_seconds
    }
}

impl GroqSttTransport for GroqWhisperHttpClient {
    fn transcribe_pcm16_mono(
        &mut self,
        config: &GroqSttConfig,
        api_key: &ZeroizingBytes,
        pcm: &[i16],
        sample_rate_hz: u32,
    ) -> Result<GroqSttTranscript, SttEngineError> {
        if pcm.is_empty() {
            return Err(SttEngineError::EmptyAudioSample);
        }
        let rms = (pcm.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / (pcm.len() as f64)).sqrt() as f32;
        if rms < MIN_SPEECH_RMS_AMPLITUDE {
            return Err(SttEngineError::SilenceOrNoSpeechDetected);
        }

        let target = parse_groq_endpoint(&config.endpoint_url)?;
        let key_str = std::str::from_utf8(api_key.as_slice())
            .map_err(|_| SttEngineError::InvalidSecretAlias("non-UTF8 API key".to_string()))?
            .trim();
        if key_str.is_empty() {
            return Err(SttEngineError::MissingApiKeyAlias(config.api_key_alias.clone()));
        }

        let mut wav_bytes = ZeroizingBytes::new(encode_riff_wav_pcm16_mono(pcm, sample_rate_hz));
        let multipart_body = build_groq_multipart_body(wav_bytes.as_slice(), config)?;
        wav_bytes.zeroize_now();

        self.requests_sent = self.requests_sent.saturating_add(1);
        let active_secrets = [api_key.as_slice()];

        let (status_code, resp_body) = if target.scheme == "https" {
            send_https_multipart_post(&target, key_str, multipart_body.as_slice(), config.timeout_ms, &active_secrets)?
        } else {
            send_loopback_http_multipart_post(&target, key_str, multipart_body.as_slice(), config.timeout_ms, &active_secrets)?
        };

        if !(200..300).contains(&status_code) {
            let raw_err = String::from_utf8_lossy(&resp_body);
            return Err(SttEngineError::UpstreamHttpError {
                status_code,
                safe_body: redact_sensitive_voice_diagnostic_with_secrets(&raw_err, &active_secrets),
            });
        }

        let transcript = parse_groq_transcription_json(&resp_body, config, pcm.len(), sample_rate_hz, &active_secrets)?;
        self.last_billed_seconds = transcript.billing.billed_duration_seconds;
        Ok(transcript)
    }
}

fn send_loopback_http_multipart_post(
    target: &GroqEndpointTarget,
    api_key_str: &str,
    body: &[u8],
    timeout_ms: u64,
    active_secrets: &[&[u8]],
) -> Result<(u16, Vec<u8>), SttEngineError> {
    let sock_addr: SocketAddr = format!("{}:{}", target.host, target.port)
        .to_socket_addrs()
        .map_err(|e| SttEngineError::NetworkFailure(redact_sensitive_voice_diagnostic_with_secrets(&e.to_string(), active_secrets)))?
        .next()
        .ok_or_else(|| SttEngineError::NetworkFailure("unresolved loopback address".to_string()))?;

    let dur = Duration::from_millis(timeout_ms.max(50));
    let mut stream = TcpStream::connect_timeout(&sock_addr, dur).map_err(|e| {
        if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock {
            SttEngineError::NetworkTimeout("connection timed out".to_string())
        } else {
            SttEngineError::NetworkFailure(redact_sensitive_voice_diagnostic_with_secrets(&e.to_string(), active_secrets))
        }
    })?;
    let _ = stream.set_read_timeout(Some(dur));
    let _ = stream.set_write_timeout(Some(dur));

    let mut hdr_bytes = ZeroizingBytes::new(
        format!(
            "POST {} HTTP/1.1\r\nHost: {}:{}\r\nAuthorization: Bearer {}\r\nContent-Type: multipart/form-data; boundary={}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            target.path, target.host, target.port, api_key_str, GROQ_MULTIPART_BOUNDARY, body.len()
        )
        .into_bytes(),
    );

    stream
        .write_all(hdr_bytes.as_slice())
        .map_err(|e| SttEngineError::NetworkFailure(redact_sensitive_voice_diagnostic_with_secrets(&e.to_string(), active_secrets)))?;
    hdr_bytes.zeroize_now();

    stream
        .write_all(body)
        .map_err(|e| SttEngineError::NetworkFailure(redact_sensitive_voice_diagnostic_with_secrets(&e.to_string(), active_secrets)))?;
    let _ = stream.flush();

    let mut raw_resp = Vec::new();
    stream.read_to_end(&mut raw_resp).map_err(|e| {
        if e.kind() == std::io::ErrorKind::TimedOut || e.kind() == std::io::ErrorKind::WouldBlock {
            SttEngineError::NetworkTimeout("response read timed out".to_string())
        } else {
            SttEngineError::NetworkFailure(redact_sensitive_voice_diagnostic_with_secrets(&e.to_string(), active_secrets))
        }
    })?;

    let Some(hdr_end) = raw_resp.windows(4).position(|w| w == b"\r\n\r\n") else {
        return Err(SttEngineError::MalformedProviderResponse("missing HTTP response header delimiter".to_string()));
    };
    let status_code = String::from_utf8_lossy(&raw_resp[..hdr_end])
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| SttEngineError::MalformedProviderResponse("invalid HTTP status code".to_string()))?;

    Ok((status_code, raw_resp[hdr_end + 4..].to_vec()))
}

#[cfg(target_os = "windows")]
fn send_https_multipart_post(
    target: &GroqEndpointTarget,
    api_key_str: &str,
    body: &[u8],
    timeout_ms: u64,
    active_secrets: &[&[u8]],
) -> Result<(u16, Vec<u8>), SttEngineError> {
    use std::ffi::c_void;
    type WinHttpHandle = *mut c_void;
    type WinDword = u32;
    type WinBool = i32;
    type WideCStrPtr = *const u16;
    type VoidPtr = *mut c_void;

    const WINHTTP_ACCESS_TYPE_NO_PROXY: WinDword = 1;
    const WINHTTP_FLAG_SECURE: WinDword = 0x00800000;
    const WINHTTP_QUERY_STATUS_CODE: WinDword = 19;
    const WINHTTP_QUERY_FLAG_NUMBER: WinDword = 0x20000000;
    const ERROR_WINHTTP_TIMEOUT: WinDword = 12002;

    #[link(name = "winhttp")]
    extern "system" {
        fn WinHttpOpen(a: WideCStrPtr, t: WinDword, p: WideCStrPtr, b: WideCStrPtr, f: WinDword) -> WinHttpHandle;
        fn WinHttpSetTimeouts(h: WinHttpHandle, r: i32, c: i32, s: i32, rcv: i32) -> WinBool;
        fn WinHttpConnect(h: WinHttpHandle, s: WideCStrPtr, p: u16, r: WinDword) -> WinHttpHandle;
        fn WinHttpOpenRequest(h: WinHttpHandle, v: WideCStrPtr, o: WideCStrPtr, ver: WideCStrPtr, r: WideCStrPtr, a: *const WideCStrPtr, f: WinDword) -> WinHttpHandle;
        fn WinHttpSendRequest(h: WinHttpHandle, hdr: WideCStrPtr, hlen: WinDword, opt: VoidPtr, olen: WinDword, tlen: WinDword, ctx: usize) -> WinBool;
        fn WinHttpReceiveResponse(h: WinHttpHandle, r: VoidPtr) -> WinBool;
        fn WinHttpQueryHeaders(h: WinHttpHandle, lvl: WinDword, n: WideCStrPtr, buf: VoidPtr, blen: *mut WinDword, idx: *mut WinDword) -> WinBool;
        fn WinHttpQueryDataAvailable(h: WinHttpHandle, avail: *mut WinDword) -> WinBool;
        fn WinHttpReadData(h: WinHttpHandle, buf: VoidPtr, to_read: WinDword, read: *mut WinDword) -> WinBool;
        fn WinHttpCloseHandle(h: WinHttpHandle) -> WinBool;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> WinDword;
    }
    struct AutoHandle(WinHttpHandle);
    impl Drop for AutoHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }
    let to_w = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };

    let agent_w = to_w("Miku-Voice-Groq-STT/1.0");
    let session = AutoHandle(unsafe {
        WinHttpOpen(agent_w.as_ptr(), WINHTTP_ACCESS_TYPE_NO_PROXY, std::ptr::null(), std::ptr::null(), 0)
    });
    if session.0.is_null() {
        return Err(SttEngineError::NetworkFailure("WinHttpOpen failed".to_string()));
    }
    let t_i32 = (timeout_ms as i32).max(500);
    unsafe { WinHttpSetTimeouts(session.0, t_i32, t_i32, t_i32, t_i32) };

    let host_w = to_w(&target.host);
    let conn = AutoHandle(unsafe { WinHttpConnect(session.0, host_w.as_ptr(), target.port, 0) });
    if conn.0.is_null() {
        return Err(SttEngineError::NetworkFailure("WinHttpConnect failed".to_string()));
    }
    let verb_w = to_w("POST");
    let path_w = to_w(&target.path);
    let req = AutoHandle(unsafe {
        WinHttpOpenRequest(
            conn.0,
            verb_w.as_ptr(),
            path_w.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    });
    if req.0.is_null() {
        return Err(SttEngineError::NetworkFailure("WinHttpOpenRequest failed".to_string()));
    }

    let mut hdr_w = to_w(&format!(
        "Authorization: Bearer {}\r\nContent-Type: multipart/form-data; boundary={}\r\n",
        api_key_str, GROQ_MULTIPART_BOUNDARY
    ));
    let sent = unsafe {
        WinHttpSendRequest(
            req.0,
            hdr_w.as_ptr(),
            hdr_w.len().saturating_sub(1) as WinDword,
            body.as_ptr() as VoidPtr,
            body.len() as WinDword,
            body.len() as WinDword,
            0,
        )
    };
    for u in hdr_w.iter_mut() {
        unsafe { std::ptr::write_volatile(u, 0) };
    }
    compiler_fence(AtomicOrdering::SeqCst);

    if sent == 0 {
        let code = unsafe { GetLastError() };
        if code == ERROR_WINHTTP_TIMEOUT {
            return Err(SttEngineError::NetworkTimeout("WinHttpSendRequest timed out".to_string()));
        }
        return Err(SttEngineError::NetworkFailure(
            redact_sensitive_voice_diagnostic_with_secrets(&format!("WinHttpSendRequest code {code}"), active_secrets),
        ));
    }
    if unsafe { WinHttpReceiveResponse(req.0, std::ptr::null_mut()) } == 0 {
        let code = unsafe { GetLastError() };
        if code == ERROR_WINHTTP_TIMEOUT {
            return Err(SttEngineError::NetworkTimeout("WinHttpReceiveResponse timed out".to_string()));
        }
        return Err(SttEngineError::NetworkFailure(format!("WinHttpReceiveResponse code {code}")));
    }
    let mut status_code: WinDword = 0;
    let mut sz: WinDword = std::mem::size_of::<WinDword>() as WinDword;
    unsafe {
        WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status_code as *mut WinDword as VoidPtr,
            &mut sz,
            std::ptr::null_mut(),
        );
    }
    let mut out = Vec::new();
    loop {
        let mut avail: WinDword = 0;
        if unsafe { WinHttpQueryDataAvailable(req.0, &mut avail) } == 0 || avail == 0 {
            break;
        }
        let mut chunk = vec![0u8; avail as usize];
        let mut read: WinDword = 0;
        if unsafe { WinHttpReadData(req.0, chunk.as_mut_ptr() as VoidPtr, avail, &mut read) } == 0 {
            break;
        }
        chunk.truncate(read as usize);
        out.extend_from_slice(&chunk);
    }
    Ok((status_code as u16, out))
}

#[cfg(not(target_os = "windows"))]
fn send_https_multipart_post(
    target: &GroqEndpointTarget,
    _api_key_str: &str,
    _body: &[u8],
    _timeout_ms: u64,
    _active_secrets: &[&[u8]],
) -> Result<(u16, Vec<u8>), SttEngineError> {
    Err(SttEngineError::NetworkFailure(format!(
        "WinHTTP HTTPS transport for {} requires Windows host",
        target.host
    )))
}
