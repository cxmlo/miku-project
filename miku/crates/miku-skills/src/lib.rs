#![allow(
    unknown_lints,
    clippy::too_many_arguments,
    clippy::manual_range_contains,
    clippy::needless_range_loop,
    clippy::large_enum_variant,
    clippy::chunks_exact_to_as_chunks,
    clippy::type_complexity,
    clippy::invisible_characters
)]

//! `miku-skills`: Transactional `skill.toml` Skill Engine & Zero-Polling OS Watcher (`F-11`, `ADR-004`, `ADR-008`).
//!
//! Responsibilities:
//! 1. Strict `skill.toml` manifest parsing (`#[serde(deny_unknown_fields)]`), SemVer validation,
//!    and `CanonicalPathJail` confinement without ever executing code during manifest loading.
//! 2. Two-layer capability security:
//!    - Layer 1: Declared capabilities in `skill.toml` act as an upper bound (ceiling); any
//!      undeclared capability (e.g. `network` when only `fs_read` is declared) is rejected immediately.
//!    - Layer 2: Every declared capability is still evaluated against `miku_gatekeeper::GatekeeperEngine`
//!      (`Deny > Ask > Allow`, `deny-by-default`). A declared capability is never an automatic grant.
//! 3. Transactional hot-reload (`Shadow Buffer` pattern): candidate manifests are validated in
//!    isolation; invalid TOML or forbidden capabilities keep the previously active version intact.
//! 4. OS event-driven directory watcher (`0` polling timers in idle) using Win32 kernel change
//!    notification handles + event channel coalescing and deterministic clean shutdown.

use miku_gatekeeper::{Capability, DenyReason};
use serde::{Deserialize, Serialize};

pub mod engine;
pub mod manifest;
pub mod watcher;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod runtime_tests;

pub use engine::*;
pub use manifest::*;
pub use watcher::*;

pub const SKILL_MANIFEST_FILENAME: &str = "skill.toml";
pub const SKILL_SCHEMA_VERSION_V1: u16 = 1;
pub const DEFAULT_SKILL_TIMEOUT_MS: u64 = 5_000;
pub const MAX_SKILL_TIMEOUT_MS: u64 = 60_000;
pub const DEFAULT_SKILL_MAX_OUTPUT_BYTES: usize = 65_536;
pub const MAX_SKILL_OUTPUT_BYTES_CEILING: usize = 1_048_576;
pub const MAX_HOT_RELOAD_LATENCY_MS: f64 = 50.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillError {
    IoError(String),
    ManifestParseError(String),
    UnsupportedSchemaVersion(u16),
    InvalidSkillId(String),
    InvalidField {
        field: String,
        reason: String,
    },
    InvalidSemVer(String),
    UnknownCapability(String),
    EmptyCapabilities,
    PathJailViolation(String),
    MissingEntrypointFile(String),
    InvalidLimits(String),
    KillSwitchActive,
    CapabilityForbiddenByGatekeeper {
        skill_id: String,
        capability: Capability,
        reason: String,
    },
    SkillNotFound(String),
    UndeclaredSkillCapability {
        skill_id: String,
        capability: Capability,
    },
    TargetOutOfSkillScope {
        skill_id: String,
        target: String,
    },
    GatekeeperApprovalRequired {
        skill_id: String,
        capability: Capability,
        ticket_hash: String,
        audit_seq: u64,
    },
    GatekeeperDenied {
        skill_id: String,
        capability: Capability,
        reason: DenyReason,
        detail: String,
        audit_seq: u64,
    },
    ExecutionTimeout {
        skill_id: String,
        timeout_ms: u64,
    },
    OutputLimitExceeded {
        skill_id: String,
        produced_bytes: usize,
        max_output_bytes: usize,
    },
    ExecutionFailed {
        skill_id: String,
        reason: String,
    },
    WatcherError(String),
}

impl std::fmt::Display for SkillError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillError::IoError(msg) => write!(f, "skill I/O error: {}", msg),
            SkillError::ManifestParseError(msg) => {
                write!(f, "skill.toml parse error: {}", msg)
            }
            SkillError::UnsupportedSchemaVersion(v) => {
                write!(f, "unsupported skill.toml schema_version {}", v)
            }
            SkillError::InvalidSkillId(id) => write!(f, "invalid skill id '{}'", id),
            SkillError::InvalidField { field, reason } => {
                write!(f, "invalid skill.toml field '{}': {}", field, reason)
            }
            SkillError::InvalidSemVer(v) => write!(f, "invalid SemVer version '{}'", v),
            SkillError::UnknownCapability(cap) => {
                write!(f, "unknown skill capability '{}'", cap)
            }
            SkillError::EmptyCapabilities => {
                write!(f, "skill.toml must declare at least one capability")
            }
            SkillError::PathJailViolation(msg) => {
                write!(f, "skill path jail violation: {}", msg)
            }
            SkillError::MissingEntrypointFile(path) => {
                write!(f, "skill entrypoint file does not exist: '{}'", path)
            }
            SkillError::InvalidLimits(msg) => write!(f, "invalid skill limits: {}", msg),
            SkillError::KillSwitchActive => {
                write!(f, "global Gatekeeper kill switch is active")
            }
            SkillError::CapabilityForbiddenByGatekeeper {
                skill_id,
                capability,
                reason,
            } => write!(
                f,
                "skill '{}' requests capability '{}' forbidden by Gatekeeper: {}",
                skill_id,
                capability.as_str(),
                reason
            ),
            SkillError::SkillNotFound(id) => write!(f, "skill '{}' is not loaded", id),
            SkillError::UndeclaredSkillCapability {
                skill_id,
                capability,
            } => write!(
                f,
                "skill '{}' attempted undeclared capability '{}'",
                skill_id,
                capability.as_str()
            ),
            SkillError::TargetOutOfSkillScope { skill_id, target } => write!(
                f,
                "target '{}' is outside allowed_targets of skill '{}'",
                target, skill_id
            ),
            SkillError::GatekeeperApprovalRequired {
                skill_id,
                capability,
                ticket_hash,
                ..
            } => write!(
                f,
                "skill '{}' capability '{}' requires interactive Gatekeeper approval (ticket={})",
                skill_id,
                capability.as_str(),
                ticket_hash
            ),
            SkillError::GatekeeperDenied {
                skill_id,
                capability,
                reason,
                detail,
                ..
            } => write!(
                f,
                "Gatekeeper denied skill '{}' capability '{}' ({}): {}",
                skill_id,
                capability.as_str(),
                reason.as_code(),
                detail
            ),
            SkillError::ExecutionTimeout {
                skill_id,
                timeout_ms,
            } => write!(
                f,
                "skill '{}' exceeded execution timeout of {} ms",
                skill_id, timeout_ms
            ),
            SkillError::OutputLimitExceeded {
                skill_id,
                produced_bytes,
                max_output_bytes,
            } => write!(
                f,
                "skill '{}' output ({} bytes) exceeded max_output_bytes ({} bytes)",
                skill_id, produced_bytes, max_output_bytes
            ),
            SkillError::ExecutionFailed { skill_id, reason } => {
                write!(f, "skill '{}' execution failed: {}", skill_id, reason)
            }
            SkillError::WatcherError(msg) => write!(f, "skill watcher error: {}", msg),
        }
    }
}

impl std::error::Error for SkillError {}
