use miku_protocol::sha256_hex;
use serde::{Deserialize, Serialize};

pub const DEFAULT_QR_TTL_MS: u64 = 60_000;
pub const MAX_FAILED_QR_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingUriPayload {
    pub protocol_version: u8,
    pub lan_host: String,
    pub port: u16,
    pub pairing_nonce: String,
    pub cert_pin_sha256: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
}

impl PairingUriPayload {
    pub fn to_pairing_uri(&self) -> String {
        format!(
            "miku-link://pair?v={}&host={}&port={}&nonce={}&pin=sha256:{}&exp={}",
            self.protocol_version,
            self.lan_host,
            self.port,
            self.pairing_nonce,
            self.cert_pin_sha256,
            self.expires_at_ms
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanPairingTicket {
    pub payload: PairingUriPayload,
    pub consumed: bool,
    pub invalidated: bool,
    pub failed_attempts: u8,
}

pub fn compute_cert_spki_pin_sha256(cert_der: &[u8]) -> String {
    sha256_hex(cert_der)
}

use crate::qr_codec::{encode_qr_matrix, QrEccLevel, QrMatrix};

pub fn encode_pairing_qr_matrix(uri: &str) -> Option<QrMatrix> {
    encode_qr_matrix(uri.as_bytes(), QrEccLevel::M)
        .or_else(|_| encode_qr_matrix(uri.as_bytes(), QrEccLevel::L))
        .ok()
}

/// Real ISO/IEC 18004 QR SVG (Byte mode + Reed-Solomon GF(2^8) + BCH format/version info)
/// scannable by any mobile camera to open `miku-link://pair?...`.
pub fn render_pairing_qr_svg(uri: &str) -> String {
    match encode_pairing_qr_matrix(uri) {
        Some(matrix) => matrix.to_svg(6, 4),
        None => {
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 120 120\" width=\"120\" height=\"120\" role=\"img\" aria-label=\"Miku-Link Pairing QR\"><rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/></svg>".to_string()
        }
    }
}
