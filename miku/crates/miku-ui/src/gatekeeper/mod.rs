//! Gatekeeper UI domain: pre-execution capability interceptor, interactive Y/N confirmation modal
//! with TTL countdown (`15_000 ms`), anti-TOCTOU single-use SHA-256 tokens, and Global Kill Switch
//! (`Ctrl+Alt+Shift+K`, `< 20.0 ms` atomic lockdown & 2-step constant-time re-arm) (`ADR-007` / `F-08` / `F-21`).

pub mod interceptor;
pub mod interceptor_decision_audit;
pub mod kill_switch_ops_audit;
pub mod kill_switch_types;

pub use self::interceptor::*;
pub use self::interceptor_decision_audit::*;
pub use self::kill_switch_ops_audit::*;
pub use self::kill_switch_types::*;

#[cfg(test)]
mod tests_interceptor;
#[cfg(test)]
mod tests_kill_switch;
