pub mod qr_pairing;
pub mod session_revocation;

#[cfg(test)]
mod tests;

pub use qr_pairing::{
    compute_cert_spki_pin_sha256, encode_pairing_qr_matrix, render_pairing_qr_svg,
    LanPairingTicket, PairingUriPayload, DEFAULT_QR_TTL_MS, MAX_FAILED_QR_ATTEMPTS,
};
pub use session_revocation::{
    LanPairingConfig, LanPairingError, LanPairingManager, PairedDeviceSession,
};
