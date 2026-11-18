//! Voice interaction domain: Push-to-Talk (`Alt+Space`) and Wake-Word controller, 5-band procedural
//! soundwave indicator (`60 fps` active / `0.00 fps` idle), cooperative cancellation (`Escape`),
//! cryptographic zero-fill PCM ring buffer (`ADR-008` / `F-12` / `F-13`), and Groq Whisper
//! Speech-to-Text (`POST https://api.groq.com/openai/v1/audio/transcriptions`) with Gatekeeper
//! Network authorization, explicit cloud audio consent, and alias-only API key retrieval (`F-16` / Step `4.1`).

pub mod audit;
pub mod capture_devices;
mod controller;
pub mod media_and_ducking;
pub mod ptt_pipeline;
pub mod sapi_engine;
pub mod stt_backends;
pub mod stt_engine;
pub mod types_and_buffer;
pub mod win32_sapi_native;

pub use self::audit::*;
pub use self::capture_devices::*;
pub use self::media_and_ducking::*;
pub use self::ptt_pipeline::*;
pub use self::sapi_engine::*;
pub use self::stt_backends::*;
pub use self::stt_engine::*;
pub use self::types_and_buffer::*;
pub use self::win32_sapi_native::{
    capture_microphone_pcm_bounded, encode_pcm16_wav_in_ram, recognize_pcm_via_windows_speech,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_media_ducking;
#[cfg(test)]
mod tests_ptt_stt;
