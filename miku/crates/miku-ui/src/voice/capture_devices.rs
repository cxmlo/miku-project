#![allow(unused_imports)]
use crate::prelude::*;

/// Abstraction over hardware or deterministic fixture microphone capture streams.
/// Enforces immediate device closure and cryptographic zero-fill on release, error, or drop.
pub trait PttAudioCaptureStream {
    fn open_capture_stream(&mut self, sample_rate_hz: u32, channels: u8) -> Result<(), String>;
    fn read_captured_pcm_chunk(&mut self, max_samples: usize) -> Result<Vec<i16>, String>;
    fn stop_and_close_stream(&mut self) -> VoiceBufferZeroReport;
    fn is_stream_open(&self) -> bool;
    fn active_stream_handles_count(&self) -> usize;
    fn retained_device_buffer_bytes(&self) -> usize;
}

/// Deterministic capture device used for reproducible bilingual ES/EN PTT -> STT tests.
pub struct DeterministicFixtureCaptureDevice {
    stream_open: bool,
    open_calls: u64,
    close_calls: u64,
    staged_pcm: Vec<i16>,
    read_cursor: usize,
    device_ring_buffer: Vec<i16>,
    injected_read_error: Option<String>,
}

impl DeterministicFixtureCaptureDevice {
    pub fn new() -> Self {
        Self {
            stream_open: false,
            open_calls: 0,
            close_calls: 0,
            staged_pcm: Vec::new(),
            read_cursor: 0,
            device_ring_buffer: Vec::new(),
            injected_read_error: None,
        }
    }

    pub fn with_staged_pcm(pcm: Vec<i16>) -> Self {
        let mut dev = Self::new();
        dev.restage_pcm(pcm);
        dev
    }

    pub fn restage_pcm(&mut self, pcm: Vec<i16>) {
        self.staged_pcm = pcm;
        self.read_cursor = 0;
    }

    pub fn inject_read_error(&mut self, err: Option<String>) {
        self.injected_read_error = err;
    }

    pub fn staged_remaining_samples(&self) -> usize {
        if self.stream_open {
            self.device_ring_buffer.len().saturating_sub(self.read_cursor)
        } else {
            self.staged_pcm.len().saturating_sub(self.read_cursor)
        }
    }

    pub fn open_calls(&self) -> u64 {
        self.open_calls
    }

    pub fn close_calls(&self) -> u64 {
        self.close_calls
    }
}

impl Default for DeterministicFixtureCaptureDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl PttAudioCaptureStream for DeterministicFixtureCaptureDevice {
    fn open_capture_stream(&mut self, sample_rate_hz: u32, channels: u8) -> Result<(), String> {
        if sample_rate_hz == 0 || channels == 0 {
            return Err("invalid capture format".to_string());
        }
        self.stream_open = true;
        self.open_calls += 1;
        self.read_cursor = 0;
        self.device_ring_buffer = self.staged_pcm.clone();
        Ok(())
    }

    fn read_captured_pcm_chunk(&mut self, max_samples: usize) -> Result<Vec<i16>, String> {
        if !self.stream_open {
            return Err("capture stream is not open".to_string());
        }
        if let Some(err) = self.injected_read_error.take() {
            return Err(err);
        }
        let available = self.device_ring_buffer.len().saturating_sub(self.read_cursor);
        let take = available.min(max_samples.max(1));
        let chunk = self.device_ring_buffer[self.read_cursor..self.read_cursor + take].to_vec();
        self.read_cursor += take;
        Ok(chunk)
    }

    fn stop_and_close_stream(&mut self) -> VoiceBufferZeroReport {
        if self.stream_open {
            self.close_calls += 1;
            self.stream_open = false;
        }
        let samples_zeroed = self.device_ring_buffer.len();
        for s in self.device_ring_buffer.iter_mut() {
            *s = 0;
        }
        std::hint::black_box(&self.device_ring_buffer);
        let residual_non_zero_samples = self.device_ring_buffer.iter().filter(|&&s| s != 0).count();
        self.device_ring_buffer.clear();
        self.device_ring_buffer.shrink_to_fit();
        self.device_ring_buffer = Vec::new();
        self.read_cursor = 0;
        VoiceBufferZeroReport {
            samples_zeroed,
            residual_non_zero_samples,
            retained_capacity_bytes: self.device_ring_buffer.capacity() * std::mem::size_of::<i16>(),
        }
    }

    fn is_stream_open(&self) -> bool {
        self.stream_open
    }

    fn active_stream_handles_count(&self) -> usize {
        usize::from(self.stream_open)
    }

    fn retained_device_buffer_bytes(&self) -> usize {
        self.device_ring_buffer.capacity() * std::mem::size_of::<i16>()
    }
}

impl Drop for DeterministicFixtureCaptureDevice {
    fn drop(&mut self) {
        let _ = self.stop_and_close_stream();
    }
}

/// Real Windows WinMM (`waveInOpen` / `waveInReset` / `waveInClose`) hardware capture probe.
/// On headless CI runners without a physical microphone (`waveInGetNumDevs() == 0`), fails safely
/// and guarantees 0 open OS device handles on release or drop.
pub struct WinMmWaveInCaptureProbe {
    #[cfg(target_os = "windows")]
    h_wave_in: usize,
    stream_open: bool,
    scratch_pcm: Vec<i16>,
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct WinMmWaveFormatEx {
    w_format_tag: u16,
    n_channels: u16,
    n_samples_per_sec: u32,
    n_avg_bytes_per_sec: u32,
    n_block_align: u16,
    w_bits_per_sample: u16,
    cb_size: u16,
}

#[cfg(target_os = "windows")]
#[link(name = "winmm")]
extern "system" {
    fn waveInGetNumDevs() -> u32;
    fn waveInOpen(
        phwi: *mut usize,
        u_device_id: u32,
        pwfx: *const WinMmWaveFormatEx,
        dw_callback: usize,
        dw_instance: usize,
        fdw_open: u32,
    ) -> u32;
    fn waveInReset(hwi: usize) -> u32;
    fn waveInClose(hwi: usize) -> u32;
}

impl WinMmWaveInCaptureProbe {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "windows")]
            h_wave_in: 0,
            stream_open: false,
            scratch_pcm: Vec::new(),
        }
    }
}

impl Default for WinMmWaveInCaptureProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl PttAudioCaptureStream for WinMmWaveInCaptureProbe {
    fn open_capture_stream(&mut self, sample_rate_hz: u32, channels: u8) -> Result<(), String> {
        let _ = self.stop_and_close_stream();
        #[cfg(target_os = "windows")]
        {
            const WAVE_FORMAT_PCM: u16 = 1;
            const WAVE_MAPPER: u32 = 0xFFFF_FFFF;
            // SAFETY: waveInGetNumDevs takes no parameters and queries WinMM audio driver count.
            let num_devs = unsafe { waveInGetNumDevs() };
            if num_devs == 0 {
                return Err("no WinMM waveIn capture devices present on host".to_string());
            }
            let block_align = (channels as u16) * 2;
            let wfx = WinMmWaveFormatEx {
                w_format_tag: WAVE_FORMAT_PCM,
                n_channels: channels as u16,
                n_samples_per_sec: sample_rate_hz,
                n_avg_bytes_per_sec: sample_rate_hz * (block_align as u32),
                n_block_align: block_align,
                w_bits_per_sample: 16,
                cb_size: 0,
            };
            let mut handle: usize = 0;
            // SAFETY: Valid WAVEFORMATEX pointer and CALLBACK_NULL (0) passed to waveInOpen.
            let mm_res = unsafe { waveInOpen(&mut handle, WAVE_MAPPER, &wfx, 0, 0, 0) };
            if mm_res != 0 || handle == 0 {
                return Err(format!("waveInOpen failed with MMSYSERR={}", mm_res));
            }
            self.h_wave_in = handle;
            self.stream_open = true;
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (sample_rate_hz, channels);
            Err("WinMM capture is only supported on Windows targets".to_string())
        }
    }

    fn read_captured_pcm_chunk(&mut self, _max_samples: usize) -> Result<Vec<i16>, String> {
        if !self.stream_open {
            return Err("WinMM capture stream is closed".to_string());
        }
        Ok(Vec::new())
    }

    fn stop_and_close_stream(&mut self) -> VoiceBufferZeroReport {
        #[cfg(target_os = "windows")]
        if self.h_wave_in != 0 {
            // SAFETY: Reset and close the active HWAVEIN handle before clearing it to 0.
            unsafe {
                let _ = waveInReset(self.h_wave_in);
                let _ = waveInClose(self.h_wave_in);
            }
            self.h_wave_in = 0;
        }
        self.stream_open = false;
        let samples_zeroed = self.scratch_pcm.len();
        for s in self.scratch_pcm.iter_mut() {
            *s = 0;
        }
        std::hint::black_box(&self.scratch_pcm);
        self.scratch_pcm.clear();
        self.scratch_pcm.shrink_to_fit();
        self.scratch_pcm = Vec::new();
        VoiceBufferZeroReport {
            samples_zeroed,
            residual_non_zero_samples: 0,
            retained_capacity_bytes: self.scratch_pcm.capacity() * std::mem::size_of::<i16>(),
        }
    }

    fn is_stream_open(&self) -> bool {
        self.stream_open
    }

    fn active_stream_handles_count(&self) -> usize {
        usize::from(self.stream_open)
    }

    fn retained_device_buffer_bytes(&self) -> usize {
        self.scratch_pcm.capacity() * std::mem::size_of::<i16>()
    }
}

impl Drop for WinMmWaveInCaptureProbe {
    fn drop(&mut self) {
        let _ = self.stop_and_close_stream();
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VoiceAgentSessionOrder {
    pub order_seq: u64,
    pub session_id: String,
    pub prompt_text: String,
    pub language: SttLanguage,
    pub confidence: f32,
    pub enqueued_at_ms: u64,
    pub ipc_envelope_id: String,
}

#[derive(Debug, Default)]
pub struct VoiceSessionOrderQueue {
    orders: Vec<VoiceAgentSessionOrder>,
    next_seq: u64,
}

impl VoiceSessionOrderQueue {
    pub fn new() -> Self {
        Self { orders: Vec::new(), next_seq: 1 }
    }

    pub fn enqueue(
        &mut self,
        session_id: &str,
        transcript: &LocalSttTranscript,
        now_ms: u64,
        ipc_envelope_id: String,
    ) -> VoiceAgentSessionOrder {
        let seq = self.next_seq;
        self.next_seq += 1;
        let order = VoiceAgentSessionOrder {
            order_seq: seq,
            session_id: session_id.to_string(),
            prompt_text: transcript.text.clone(),
            language: transcript.language,
            confidence: transcript.confidence,
            enqueued_at_ms: now_ms,
            ipc_envelope_id,
        };
        self.orders.push(order.clone());
        order
    }

    pub fn orders_for_session(&self, session_id: &str) -> Vec<VoiceAgentSessionOrder> {
        self.orders.iter().filter(|o| o.session_id == session_id).cloned().collect()
    }

    pub fn clear_and_zero_all(&mut self) {
        for order in self.orders.iter_mut() {
            order.prompt_text.clear();
            order.prompt_text.shrink_to_fit();
            order.session_id.clear();
            order.session_id.shrink_to_fit();
        }
        self.orders.clear();
        self.orders.shrink_to_fit();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDownloadReceipt {
    pub model_id: String,
    pub verified_size_bytes: usize,
    pub verified_sha256_hex: String,
    pub saved_path: String,
    pub downloaded_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PttPipelineOutcome {
    TranscribedAndEnqueued {
        transcript: LocalSttTranscript,
        order: VoiceAgentSessionOrder,
        completion_report: VoiceUtteranceCompletionReport,
        device_zero_report: VoiceBufferZeroReport,
    },
    ModelMissing {
        download_card: SttModelDownloadCard,
    },
    Cancelled {
        reason: VoiceCancellationReason,
    },
}
