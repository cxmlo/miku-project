//! Voice interaction domain: Push-to-Talk (`Alt+Space`) and Wake-Word controller, 5-band procedural
//! soundwave indicator (`60 fps` active / `0.00 fps` idle), cooperative cancellation (`Escape`),
//! and cryptographic zero-fill PCM ring buffer (`ADR-008` / `F-12` / `F-13`).

pub mod audit;
mod controller;
pub mod types_and_buffer;

pub use self::audit::*;
pub use self::types_and_buffer::*;

#[cfg(test)]
mod tests;
