#![allow(unused_imports)]
use crate::prelude::*;

pub trait PttAudioCaptureStream {
    fn open_capture_stream(&mut self, sample_rate_hz: u32, channels: u8) -> Result<(), UiError>;
    fn is_stream_open(&self) -> bool;
    fn active_stream_handles_count(&self) -> usize;
    fn read_captured_pcm_chunk(&mut self, max_samples: usize) -> Result<Vec<i16>, UiError>;
    fn stop_and_close_stream(&mut self) -> VoiceBufferZeroReport;
    fn retained_device_buffer_bytes(&self) -> usize;
    fn open_calls(&self) -> u64;
    fn close_calls(&self) -> u64;
}

#[derive(Debug, Clone)]
pub struct DeterministicFixtureCaptureDevice {
    stream_open: bool,
    open_calls: u64,
    close_calls: u64,
    staged_pcm: Vec<i16>,
    read_cursor: usize,
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
            injected_read_error: None,
        }
    }

    pub fn with_staged_pcm(pcm: Vec<i16>) -> Self {
        Self {
            stream_open: false,
            open_calls: 0,
            close_calls: 0,
            staged_pcm: pcm,
            read_cursor: 0,
            injected_read_error: None,
        }
    }

    pub fn restage_pcm(&mut self, pcm: Vec<i16>) {
        self.staged_pcm.fill(0);
        self.staged_pcm = pcm;
        self.read_cursor = 0;
        self.injected_read_error = None;
    }

    pub fn staged_remaining_samples(&self) -> usize {
        self.staged_pcm.len().saturating_sub(self.read_cursor)
    }

    pub fn inject_read_error(&mut self, err_msg: Option<String>) {
        self.injected_read_error = err_msg;
    }
}

impl Default for DeterministicFixtureCaptureDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl PttAudioCaptureStream for DeterministicFixtureCaptureDevice {
    fn open_capture_stream(&mut self, sample_rate_hz: u32, channels: u8) -> Result<(), UiError> {
        if sample_rate_hz != VOICE_SAMPLE_RATE_HZ || channels != VOICE_CHANNELS {
            return Err(UiError::VoiceInteractionError(format!(
                "unsupported audio format: {} Hz / {} ch",
                sample_rate_hz, channels
            )));
        }
        self.stream_open = true;
        self.open_calls = self.open_calls.saturating_add(1);
        Ok(())
    }

    fn is_stream_open(&self) -> bool {
        self.stream_open
    }

    fn active_stream_handles_count(&self) -> usize {
        usize::from(self.stream_open)
    }

    fn read_captured_pcm_chunk(&mut self, max_samples: usize) -> Result<Vec<i16>, UiError> {
        if !self.stream_open {
            return Err(UiError::VoiceInteractionError(
                "cannot read PCM chunk when stream is closed".to_string(),
            ));
        }
        if let Some(msg) = self.injected_read_error.take() {
            return Err(UiError::VoiceInteractionError(msg));
        }
        let rem = self.staged_remaining_samples();
        if rem == 0 || max_samples == 0 {
            return Ok(Vec::new());
        }
        let take = rem.min(max_samples);
        let slice = self.staged_pcm[self.read_cursor..self.read_cursor + take].to_vec();
        self.read_cursor += take;
        Ok(slice)
    }

    fn stop_and_close_stream(&mut self) -> VoiceBufferZeroReport {
        let zeroed_samples = self.staged_pcm.len();
        self.staged_pcm.fill(0);
        let residual_non_zero_samples = self.staged_pcm.iter().filter(|&&s| s != 0).count();
        self.staged_pcm.clear();
        self.staged_pcm.shrink_to_fit();
        self.read_cursor = 0;
        if self.stream_open {
            self.stream_open = false;
            self.close_calls = self.close_calls.saturating_add(1);
        }
        VoiceBufferZeroReport {
            zeroed_samples,
            residual_non_zero_samples,
            retained_capacity_bytes: self.staged_pcm.capacity() * std::mem::size_of::<i16>(),
        }
    }

    fn retained_device_buffer_bytes(&self) -> usize {
        self.staged_pcm.capacity() * std::mem::size_of::<i16>()
    }

    fn open_calls(&self) -> u64 {
        self.open_calls
    }

    fn close_calls(&self) -> u64 {
        self.close_calls
    }
}

#[cfg(target_os = "windows")]
mod winmm_capture {
    use crate::voice::types_and_buffer::VoiceBufferZeroReport;
    use crate::win32::constants_and_errors::UiError;

    pub type Hwavein = isize;
    pub const WAVE_MAPPER: u32 = u32::MAX;
    pub const WAVE_FORMAT_PCM: u16 = 1;
    pub const CALLBACK_NULL: u32 = 0;
    pub const MMSYSERR_NOERROR: u32 = 0;
    pub const WHDR_DONE: u32 = 0x0000_0001;
    pub const WHDR_PREPARED: u32 = 0x0000_0002;
    pub const WINMM_RING_BUFFERS: usize = 4;
    pub const WINMM_SAMPLES_PER_BUFFER: usize = 800; // 50 ms @ 16 kHz mono

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct WaveFormatEx {
        pub w_format_tag: u16,
        pub n_channels: u16,
        pub n_samples_per_sec: u32,
        pub n_avg_bytes_per_sec: u32,
        pub n_block_align: u16,
        pub w_bits_per_sample: u16,
        pub cb_size: u16,
    }

    #[repr(C)]
    pub struct WaveHdr {
        pub lp_data: *mut u8,
        pub dw_buffer_length: u32,
        pub dw_bytes_recorded: u32,
        pub dw_user: usize,
        pub dw_flags: u32,
        pub dw_loops: u32,
        pub lp_next: *mut WaveHdr,
        pub reserved: usize,
    }

    #[link(name = "winmm")]
    unsafe extern "system" {
        pub fn waveInGetNumDevs() -> u32;
        pub fn waveInOpen(
            phwi: *mut Hwavein,
            u_device_id: u32,
            pwfx: *const WaveFormatEx,
            dw_callback: usize,
            dw_instance: usize,
            fdw_open: u32,
        ) -> u32;
        pub fn waveInPrepareHeader(hwi: Hwavein, pwh: *mut WaveHdr, cbwh: u32) -> u32;
        pub fn waveInUnprepareHeader(hwi: Hwavein, pwh: *mut WaveHdr, cbwh: u32) -> u32;
        pub fn waveInAddBuffer(hwi: Hwavein, pwh: *mut WaveHdr, cbwh: u32) -> u32;
        pub fn waveInStart(hwi: Hwavein) -> u32;
        pub fn waveInStop(hwi: Hwavein) -> u32;
        pub fn waveInReset(hwi: Hwavein) -> u32;
        pub fn waveInClose(hwi: Hwavein) -> u32;
    }

    pub struct PreparedWaveBuffer {
        pub pcm_storage: Box<[i16]>,
        pub header: Box<WaveHdr>,
    }

    impl PreparedWaveBuffer {
        pub fn new(sample_len: usize) -> Self {
            let mut pcm_storage = vec![0i16; sample_len].into_boxed_slice();
            let byte_len = (sample_len * std::mem::size_of::<i16>()) as u32;
            let lp_data = pcm_storage.as_mut_ptr() as *mut u8;
            let header = Box::new(WaveHdr {
                lp_data,
                dw_buffer_length: byte_len,
                dw_bytes_recorded: 0,
                dw_user: 0,
                dw_flags: 0,
                dw_loops: 0,
                lp_next: std::ptr::null_mut(),
                reserved: 0,
            });
            Self {
                pcm_storage,
                header,
            }
        }

        pub fn zero_storage(&mut self) -> usize {
            let len = self.pcm_storage.len();
            self.pcm_storage.fill(0);
            self.header.dw_bytes_recorded = 0;
            len
        }
    }

    pub fn open_and_start_wave_in(
        sample_rate_hz: u32,
        channels: u8,
    ) -> Result<(Hwavein, Vec<PreparedWaveBuffer>), UiError> {
        let block_align = (channels as u16) * 2;
        let fmt = WaveFormatEx {
            w_format_tag: WAVE_FORMAT_PCM,
            n_channels: channels as u16,
            n_samples_per_sec: sample_rate_hz,
            n_avg_bytes_per_sec: sample_rate_hz * (block_align as u32),
            n_block_align: block_align,
            w_bits_per_sample: 16,
            cb_size: 0,
        };
        let mut handle: Hwavein = 0;
        let rc = unsafe { waveInOpen(&mut handle, WAVE_MAPPER, &fmt, 0, 0, CALLBACK_NULL) };
        if rc != MMSYSERR_NOERROR || handle == 0 {
            return Err(UiError::VoiceInteractionError(format!(
                "waveInOpen failed with code {}",
                rc
            )));
        }

        let cbwh = std::mem::size_of::<WaveHdr>() as u32;
        let mut buffers = Vec::with_capacity(WINMM_RING_BUFFERS);
        for _ in 0..WINMM_RING_BUFFERS {
            let mut buf = PreparedWaveBuffer::new(WINMM_SAMPLES_PER_BUFFER);
            let prep_rc = unsafe { waveInPrepareHeader(handle, &mut *buf.header, cbwh) };
            if prep_rc != MMSYSERR_NOERROR {
                let _ = teardown_wave_in(handle, &mut buffers);
                return Err(UiError::VoiceInteractionError(format!(
                    "waveInPrepareHeader failed: {}",
                    prep_rc
                )));
            }
            let add_rc = unsafe { waveInAddBuffer(handle, &mut *buf.header, cbwh) };
            if add_rc != MMSYSERR_NOERROR {
                let _ = teardown_wave_in(handle, &mut buffers);
                return Err(UiError::VoiceInteractionError(format!(
                    "waveInAddBuffer failed: {}",
                    add_rc
                )));
            }
            buffers.push(buf);
        }

        let start_rc = unsafe { waveInStart(handle) };
        if start_rc != MMSYSERR_NOERROR {
            let _ = teardown_wave_in(handle, &mut buffers);
            return Err(UiError::VoiceInteractionError(format!(
                "waveInStart failed: {}",
                start_rc
            )));
        }
        Ok((handle, buffers))
    }

    pub fn poll_wave_in_buffers(
        handle: Hwavein,
        buffers: &mut [PreparedWaveBuffer],
        max_samples: usize,
    ) -> Result<Vec<i16>, UiError> {
        let cbwh = std::mem::size_of::<WaveHdr>() as u32;
        let mut collected = Vec::new();
        for buf in buffers.iter_mut() {
            if (buf.header.dw_flags & WHDR_DONE) != 0 {
                let recorded_samples = (buf.header.dw_bytes_recorded as usize) / 2;
                let count = recorded_samples.min(buf.pcm_storage.len());
                if count > 0 {
                    let room = max_samples.saturating_sub(collected.len());
                    let take = count.min(room.max(count));
                    collected.extend_from_slice(&buf.pcm_storage[..take]);
                }
                let _ = buf.zero_storage();
                buf.header.dw_flags &= !WHDR_DONE;
                let rc = unsafe { waveInAddBuffer(handle, &mut *buf.header, cbwh) };
                if rc != MMSYSERR_NOERROR {
                    return Err(UiError::VoiceInteractionError(format!(
                        "waveInAddBuffer requeue failed: {}",
                        rc
                    )));
                }
            }
        }
        Ok(collected)
    }

    pub fn teardown_wave_in(
        handle: Hwavein,
        buffers: &mut Vec<PreparedWaveBuffer>,
    ) -> VoiceBufferZeroReport {
        if handle != 0 {
            let cbwh = std::mem::size_of::<WaveHdr>() as u32;
            unsafe {
                let _ = waveInStop(handle);
                let _ = waveInReset(handle);
            }
            let mut zeroed_samples = 0usize;
            for buf in buffers.iter_mut() {
                if (buf.header.dw_flags & WHDR_PREPARED) != 0 {
                    unsafe {
                        let _ = waveInUnprepareHeader(handle, &mut *buf.header, cbwh);
                    }
                }
                zeroed_samples += buf.zero_storage();
            }
            buffers.clear();
            buffers.shrink_to_fit();
            unsafe {
                let _ = waveInClose(handle);
            }
            return VoiceBufferZeroReport {
                zeroed_samples,
                residual_non_zero_samples: 0,
                retained_capacity_bytes: 0,
            };
        }
        VoiceBufferZeroReport {
            zeroed_samples: 0,
            residual_non_zero_samples: 0,
            retained_capacity_bytes: 0,
        }
    }
}

pub struct WinMmWaveInCaptureProbe {
    #[cfg(target_os = "windows")]
    handle: Option<winmm_capture::Hwavein>,
    #[cfg(target_os = "windows")]
    buffers: Vec<winmm_capture::PreparedWaveBuffer>,
    open_calls: u64,
    close_calls: u64,
}

impl WinMmWaveInCaptureProbe {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "windows")]
            handle: None,
            #[cfg(target_os = "windows")]
            buffers: Vec::new(),
            open_calls: 0,
            close_calls: 0,
        }
    }

    pub fn device_count() -> u32 {
        #[cfg(target_os = "windows")]
        unsafe {
            winmm_capture::waveInGetNumDevs()
        }
        #[cfg(not(target_os = "windows"))]
        {
            0
        }
    }

    pub fn prepared_headers_count(&self) -> usize {
        #[cfg(target_os = "windows")]
        {
            self.buffers.len()
        }
        #[cfg(not(target_os = "windows"))]
        {
            0
        }
    }
}

impl Default for WinMmWaveInCaptureProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WinMmWaveInCaptureProbe {
    fn drop(&mut self) {
        let _ = self.stop_and_close_stream();
    }
}

impl PttAudioCaptureStream for WinMmWaveInCaptureProbe {
    fn open_capture_stream(&mut self, sample_rate_hz: u32, channels: u8) -> Result<(), UiError> {
        if self.is_stream_open() {
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        {
            let (h, bufs) = winmm_capture::open_and_start_wave_in(sample_rate_hz, channels)?;
            self.handle = Some(h);
            self.buffers = bufs;
            self.open_calls = self.open_calls.saturating_add(1);
            Ok(())
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (sample_rate_hz, channels);
            Err(UiError::VoiceInteractionError(
                "WinMM capture requires Windows host".to_string(),
            ))
        }
    }

    fn is_stream_open(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            self.handle.is_some()
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    fn active_stream_handles_count(&self) -> usize {
        usize::from(self.is_stream_open())
    }

    fn read_captured_pcm_chunk(&mut self, max_samples: usize) -> Result<Vec<i16>, UiError> {
        #[cfg(target_os = "windows")]
        {
            let Some(h) = self.handle else {
                return Err(UiError::VoiceInteractionError(
                    "WinMM stream is not open".to_string(),
                ));
            };
            winmm_capture::poll_wave_in_buffers(h, &mut self.buffers, max_samples)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = max_samples;
            Err(UiError::VoiceInteractionError(
                "WinMM stream is not open".to_string(),
            ))
        }
    }

    fn stop_and_close_stream(&mut self) -> VoiceBufferZeroReport {
        #[cfg(target_os = "windows")]
        {
            if let Some(h) = self.handle.take() {
                self.close_calls = self.close_calls.saturating_add(1);
                return winmm_capture::teardown_wave_in(h, &mut self.buffers);
            }
        }
        VoiceBufferZeroReport {
            zeroed_samples: 0,
            residual_non_zero_samples: 0,
            retained_capacity_bytes: 0,
        }
    }

    fn retained_device_buffer_bytes(&self) -> usize {
        #[cfg(target_os = "windows")]
        {
            self.buffers
                .iter()
                .map(|b| b.pcm_storage.len() * std::mem::size_of::<i16>())
                .sum()
        }
        #[cfg(not(target_os = "windows"))]
        {
            0
        }
    }

    fn open_calls(&self) -> u64 {
        self.open_calls
    }

    fn close_calls(&self) -> u64 {
        self.close_calls
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceAgentSessionOrder {
    pub session_id: String,
    pub prompt_text: String,
    pub language: SttLanguage,
    pub ipc_envelope_id: String,
    pub enqueued_at_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct VoiceSessionOrderQueue {
    orders: Vec<VoiceAgentSessionOrder>,
}

impl VoiceSessionOrderQueue {
    pub fn new() -> Self {
        Self { orders: Vec::new() }
    }

    pub fn enqueue(&mut self, order: VoiceAgentSessionOrder) {
        self.orders.push(order);
    }

    pub fn orders_for_session(&self, session_id: &str) -> Vec<&VoiceAgentSessionOrder> {
        self.orders
            .iter()
            .filter(|o| o.session_id == session_id)
            .collect()
    }

    pub fn clear_all(&mut self) -> usize {
        let len = self.orders.len();
        self.orders.clear();
        len
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PttPipelineOutcome {
    TranscribedAndEnqueued {
        transcript: GroqSttTranscript,
        order: VoiceAgentSessionOrder,
        completion_report: VoiceUtteranceCompletionReport,
        device_zero_report: VoiceBufferZeroReport,
    },
    Cancelled {
        reason: VoiceCancellationReason,
    },
}
