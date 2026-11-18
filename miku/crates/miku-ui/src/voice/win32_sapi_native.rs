//! Native WinMM PCM capture + in-memory Windows Speech Recognition (`System.Speech`) helper.
//! Creates zero temporary files on disk and zeroizes captured PCM samples on drop.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::voice::capture_devices::{PttAudioCaptureStream, WinMmWaveInCaptureProbe};
use crate::win32::{UiError, VOICE_CHANNELS, VOICE_SAMPLE_RATE_HZ};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Encodes mono 16-bit PCM samples into an in-memory RIFF/WAV byte vector.
pub fn encode_pcm16_wav_in_ram(samples: &[i16], sample_rate_hz: u32) -> Vec<u8> {
    let data_bytes = (samples.len() * 2) as u32;
    let riff_size = 36u32.saturating_add(data_bytes);
    let byte_rate = sample_rate_hz.saturating_mul(2);
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&sample_rate_hz.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Captures bounded 16 kHz mono PCM samples via WinMM `waveIn` while `stop_flag` stays false.
pub fn capture_microphone_pcm_bounded(
    stop_flag: &AtomicBool,
    max_duration_ms: u64,
) -> Result<Vec<i16>, UiError> {
    let mut probe = WinMmWaveInCaptureProbe::new();
    probe.open_capture_stream(VOICE_SAMPLE_RATE_HZ, VOICE_CHANNELS)?;
    let started = Instant::now();
    let max_dur = Duration::from_millis(max_duration_ms.clamp(250, 12_000));
    let mut all_samples: Vec<i16> = Vec::new();
    while !stop_flag.load(Ordering::SeqCst) && started.elapsed() < max_dur {
        match probe.read_captured_pcm_chunk(1600) {
            Ok(mut chunk) => {
                if !chunk.is_empty() {
                    all_samples.append(&mut chunk);
                }
            }
            Err(_) => break,
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = probe.stop_and_close_stream();
    Ok(all_samples)
}

/// Invokes offline Windows `System.Speech.Recognition.SpeechRecognitionEngine` using an in-memory
/// WAV MemoryStream piped via stdin (zero disk files created).
pub fn recognize_pcm_via_windows_speech(
    samples: &[i16],
    sample_rate_hz: u32,
    language_tag: &str,
) -> Result<String, UiError> {
    if samples.is_empty() {
        return Ok(String::new());
    }
    #[cfg(not(windows))]
    {
        let _ = (sample_rate_hz, language_tag);
        Err(UiError::VoiceInteractionError(
            "native SAPI recognition requires Windows".into(),
        ))
    }
    #[cfg(windows)]
    {
        let mut wav = encode_pcm16_wav_in_ram(samples, sample_rate_hz);
        let b64 = miku_protocol::base64_encode(&wav);
        wav.fill(0);
        drop(wav);

        let lang_clean: String = language_tag
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let script = format!(
            "$ErrorActionPreference='SilentlyContinue'; Add-Type -AssemblyName System.Speech; \
             $b=[Convert]::FromBase64String([Console]::In.ReadToEnd().Trim()); \
             $ms=New-Object System.IO.MemoryStream(,$b); \
             $rec=$null; try{{ $rec=New-Object System.Speech.Recognition.SpeechRecognitionEngine([System.Globalization.CultureInfo]::new('{}')) }}catch{{ $rec=New-Object System.Speech.Recognition.SpeechRecognitionEngine }}; \
             $rec.LoadGrammar((New-Object System.Speech.Recognition.DictationGrammar)); \
             $fmt=New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo({},[System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,[System.Speech.AudioFormat.AudioChannel]::Mono); \
             $rec.SetInputToAudioStream($ms,$fmt); \
             $res=$rec.Recognize(); if($res){{ [Console]::Out.Write($res.Text) }}; \
             $rec.Dispose(); $ms.Dispose()",
            if lang_clean.is_empty() { "es-ES" } else { &lang_clean },
            sample_rate_hz
        );
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| UiError::VoiceInteractionError(format!("SAPI helper spawn failed: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b64.as_bytes());
        }
        let out = child
            .wait_with_output()
            .map_err(|e| UiError::VoiceInteractionError(format!("SAPI helper wait failed: {}", e)))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}
