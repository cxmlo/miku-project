//! Voice interaction domain: Push-to-Talk (`Alt+Space`) and Wake-Word controller, 5-band procedural
//! soundwave indicator (`60 fps` active / `0.00 fps` idle), cooperative cancellation (`Escape`),
//! cryptographic zero-fill PCM ring buffer (`ADR-008` / `F-12` / `F-13`), and on-demand local STT
//! pipeline with SHA-256 model verification and TTL eviction (`F-16` / Step `4.1`).

pub mod audit;
pub mod capture_devices;
mod controller;
pub mod ptt_pipeline;
pub mod stt_engine;
pub mod types_and_buffer;

pub use self::audit::*;
pub use self::capture_devices::*;
pub use self::ptt_pipeline::*;
pub use self::stt_engine::*;
pub use self::types_and_buffer::*;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_ptt_stt;
