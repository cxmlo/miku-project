//! Power & Idle Suspension domain: `IdleSuspensionCoordinator`, `DeepIdleSuspended` state after
//! `3_000 ms` TTL (`0` WebView2, `0.00 fps`, `0` timers), fullscreen gaming deferral (`< 0.5%` CPU),
//! `< 16.0 ms` warm-wake without focus stealing, and Bank 0.5 budget certification matrix (`BUD-01..09`).

pub mod coordinator;
pub mod wake_and_bank05;

pub use self::coordinator::*;
pub use self::wake_and_bank05::*;

#[cfg(test)]
mod tests;
