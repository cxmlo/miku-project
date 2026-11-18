use std::collections::HashMap;
use std::fmt;

use miku_protocol::{constant_time_token_eq, sha256_hex};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::lan_pairing::qr_pairing::{
    LanPairingTicket, PairingUriPayload, DEFAULT_QR_TTL_MS, MAX_FAILED_QR_ATTEMPTS,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LanPairingError {
    LanDisabledByDefault,
    NoActiveQrTicket,
    QrExpired,
    QrAlreadyUsed,
    QrInvalidatedByRateLimit,
    InvalidPairingNonce,
    TlsPinMismatch,
    UnknownOrRevokedSessionToken,
    SessionEpochRevoked,
}

impl fmt::Display for LanPairingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LanPairingError::LanDisabledByDefault => {
                write!(f, "Miku-Link LAN pairing is disabled (loopback-only default)")
            }
            LanPairingError::NoActiveQrTicket => write!(f, "no active pairing QR code"),
            LanPairingError::QrExpired => write!(f, "pairing QR code expired (> 60s TTL)"),
            LanPairingError::QrAlreadyUsed => {
                write!(f, "pairing QR code was already redeemed once")
            }
            LanPairingError::QrInvalidatedByRateLimit => {
                write!(f, "pairing QR invalidated after repeated failed attempts")
            }
            LanPairingError::InvalidPairingNonce => write!(f, "invalid pairing nonce"),
            LanPairingError::TlsPinMismatch => {
                write!(f, "pinned TLS certificate SHA-256 fingerprint mismatch")
            }
            LanPairingError::UnknownOrRevokedSessionToken => {
                write!(f, "device token unknown or revoked by Pair Again")
            }
            LanPairingError::SessionEpochRevoked => {
                write!(f, "session epoch invalidated by Pair Again")
            }
        }
    }
}

impl std::error::Error for LanPairingError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanPairingConfig {
    pub enabled_by_user: bool,
    pub lan_bind_host: String,
    pub port: u16,
    pub pinned_cert_sha256: String,
    pub qr_ttl_ms: u64,
}

impl Default for LanPairingConfig {
    fn default() -> Self {
        Self {
            enabled_by_user: false, // Loopback-only by default (0 overhead)
            lan_bind_host: "127.0.0.1".to_string(),
            port: 47400,
            pinned_cert_sha256: String::new(),
            qr_ttl_ms: DEFAULT_QR_TTL_MS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedDeviceSession {
    pub device_id: String,
    pub session_token: String,
    pub epoch: u64,
    pub paired_at_ms: u64,
}

pub struct LanPairingManager {
    config: LanPairingConfig,
    active_ticket: Option<LanPairingTicket>,
    paired_tokens: HashMap<String, PairedDeviceSession>,
    epoch: u64,
    epoch_tx: watch::Sender<u64>,
    nonce_counter: u64,
}

impl LanPairingManager {
    pub fn new(config: LanPairingConfig) -> Self {
        let (epoch_tx, _) = watch::channel(1u64);
        Self {
            config,
            active_ticket: None,
            paired_tokens: HashMap::new(),
            epoch: 1,
            epoch_tx,
            nonce_counter: 0,
        }
    }

    pub fn is_lan_enabled(&self) -> bool {
        self.config.enabled_by_user
    }

    pub fn set_lan_enabled(&mut self, enabled: bool) {
        self.config.enabled_by_user = enabled;
        if !enabled {
            self.active_ticket = None;
            let _ = self.pair_again_revoke_all();
        }
    }

    pub fn set_lan_bind_host(&mut self, host: impl Into<String>) {
        self.config.lan_bind_host = host.into();
    }

    pub fn pinned_cert_sha256(&self) -> &str {
        &self.config.pinned_cert_sha256
    }

    pub fn list_paired_sessions(&self) -> Vec<PairedDeviceSession> {
        let mut out: Vec<PairedDeviceSession> = self.paired_tokens.values().cloned().collect();
        out.sort_by_key(|s| s.paired_at_ms);
        out
    }

    pub fn current_epoch(&self) -> u64 {
        self.epoch
    }

    pub fn active_paired_device_count(&self) -> usize {
        self.paired_tokens.len()
    }

    pub fn active_lan_overhead_listeners(&self) -> usize {
        if self.config.enabled_by_user {
            1
        } else {
            0
        }
    }

    pub fn subscribe_epoch_revocation(&self) -> watch::Receiver<u64> {
        self.epoch_tx.subscribe()
    }

    pub fn generate_single_use_qr(
        &mut self,
        now_ms: u64,
    ) -> Result<PairingUriPayload, LanPairingError> {
        if !self.config.enabled_by_user {
            return Err(LanPairingError::LanDisabledByDefault);
        }
        self.nonce_counter = self.nonce_counter.saturating_add(1);
        let ttl = self.config.qr_ttl_ms.clamp(1_000, DEFAULT_QR_TTL_MS);
        let seed = format!(
            "miku-qr|epoch={}|seq={}|ts={}|host={}",
            self.epoch, self.nonce_counter, now_ms, self.config.lan_bind_host
        );
        let nonce = sha256_hex(seed.as_bytes());
        let payload = PairingUriPayload {
            protocol_version: 1,
            lan_host: self.config.lan_bind_host.clone(),
            port: self.config.port,
            pairing_nonce: nonce,
            cert_pin_sha256: self.config.pinned_cert_sha256.clone(),
            issued_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(ttl),
        };
        self.active_ticket = Some(LanPairingTicket {
            payload: payload.clone(),
            consumed: false,
            invalidated: false,
            failed_attempts: 0,
        });
        Ok(payload)
    }

    pub fn redeem_qr_pairing(
        &mut self,
        device_id: &str,
        presented_nonce: &str,
        presented_cert_pin_sha256: &str,
        now_ms: u64,
    ) -> Result<PairedDeviceSession, LanPairingError> {
        if !self.config.enabled_by_user {
            return Err(LanPairingError::LanDisabledByDefault);
        }
        let Some(ticket) = self.active_ticket.as_mut() else {
            return Err(LanPairingError::NoActiveQrTicket);
        };
        if ticket.invalidated {
            return Err(LanPairingError::QrInvalidatedByRateLimit);
        }
        if ticket.consumed {
            return Err(LanPairingError::QrAlreadyUsed);
        }
        if now_ms > ticket.payload.expires_at_ms {
            ticket.invalidated = true;
            return Err(LanPairingError::QrExpired);
        }

        let nonce_ok = constant_time_token_eq(&ticket.payload.pairing_nonce, presented_nonce);
        let pin_ok =
            constant_time_token_eq(&ticket.payload.cert_pin_sha256, presented_cert_pin_sha256);

        if !nonce_ok {
            ticket.failed_attempts = ticket.failed_attempts.saturating_add(1);
            if ticket.failed_attempts >= MAX_FAILED_QR_ATTEMPTS {
                ticket.invalidated = true;
                return Err(LanPairingError::QrInvalidatedByRateLimit);
            }
            return Err(LanPairingError::InvalidPairingNonce);
        }
        if !pin_ok {
            ticket.failed_attempts = ticket.failed_attempts.saturating_add(1);
            if ticket.failed_attempts >= MAX_FAILED_QR_ATTEMPTS {
                ticket.invalidated = true;
                return Err(LanPairingError::QrInvalidatedByRateLimit);
            }
            return Err(LanPairingError::TlsPinMismatch);
        }

        // Single-use QR consumption
        ticket.consumed = true;
        let token_seed = format!(
            "miku-dev|epoch={}|dev={}|nonce={}|ts={}",
            self.epoch, device_id, presented_nonce, now_ms
        );
        let session_token = sha256_hex(token_seed.as_bytes());
        let session = PairedDeviceSession {
            device_id: device_id.to_string(),
            session_token: session_token.clone(),
            epoch: self.epoch,
            paired_at_ms: now_ms,
        };
        self.paired_tokens.insert(session_token, session.clone());
        Ok(session)
    }

    pub fn authenticate_mobile_session(
        &self,
        presented_token: &str,
        presented_cert_pin_sha256: &str,
    ) -> Result<&PairedDeviceSession, LanPairingError> {
        if !self.config.enabled_by_user {
            return Err(LanPairingError::LanDisabledByDefault);
        }
        if !constant_time_token_eq(&self.config.pinned_cert_sha256, presented_cert_pin_sha256) {
            return Err(LanPairingError::TlsPinMismatch);
        }
        for (stored, sess) in &self.paired_tokens {
            if constant_time_token_eq(stored, presented_token) {
                if sess.epoch != self.epoch {
                    return Err(LanPairingError::SessionEpochRevoked);
                }
                return Ok(sess);
            }
        }
        Err(LanPairingError::UnknownOrRevokedSessionToken)
    }

    /// "Pair again" ("Emparejar de nuevo"): revokes EVERY paired phone immediately and notifies active WS sessions.
    pub fn pair_again_revoke_all(&mut self) -> usize {
        let revoked_count = self.paired_tokens.len();
        self.paired_tokens.clear();
        self.active_ticket = None;
        self.epoch = self.epoch.saturating_add(1);
        let _ = self.epoch_tx.send(self.epoch);
        revoked_count
    }
}
