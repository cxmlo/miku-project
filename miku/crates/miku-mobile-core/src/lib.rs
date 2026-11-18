//! Shared mobile core for Miku-Link (Android Kotlin/Compose + UniFFI/C-ABI).
//!
//! Enforces strict client-side + daemon-side parity invariants:
//! - Mobile `Allow` requires cryptographic `req_hash` verification against the
//!   exact parameters displayed on screen plus an explicit `BiometricProof`
//!   bound to that `req_hash`.
//! - High-risk `pc_only: true` capabilities cannot be approved from mobile.
//! - One-tap `Deny` executes immediately without requiring biometrics.
//! - Mobile configuration and persistent cache MUST NEVER store provider API keys.

pub mod mobile_client;
pub mod uniffi_ffi;

#[cfg(test)]
mod tests;

pub use mobile_client::{
    build_island_mobile_parity_matrix, compute_req_hash_v1, validate_mobile_config_no_provider_keys,
    BiometricProof, DisplayedPermissionCard, MobileAgentFeedEntry, MobileChatQueueItem,
    MobileCoreClient, MobileCoreError, MobileDecisionEnvelopeResult, MobileDecisionInput,
    ParityMatrixArea,
};
pub use uniffi_ffi::{
    miku_mobile_compute_req_hash, miku_mobile_decide_permission_json,
    miku_mobile_parity_matrix_json, miku_mobile_string_free, miku_mobile_validate_config_json,
    UNIFFI_UDL_SPEC,
};
