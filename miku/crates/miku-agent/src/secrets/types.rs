use std::ops::{Deref, DerefMut};
use std::sync::atomic::{compiler_fence, Ordering};

use serde::{Deserialize, Serialize};

/// Contract for zeroing memory securely using volatile writes and compiler fences.
pub trait Zeroize {
    fn zeroize(&mut self);
}

impl Zeroize for [u8] {
    fn zeroize(&mut self) {
        for b in self.iter_mut() {
            unsafe {
                std::ptr::write_volatile(b, 0);
            }
        }
        compiler_fence(Ordering::SeqCst);
    }
}

impl Zeroize for Vec<u8> {
    fn zeroize(&mut self) {
        self.as_mut_slice().zeroize();
        self.clear();
    }
}

impl Zeroize for String {
    fn zeroize(&mut self) {
        unsafe {
            self.as_mut_vec().as_mut_slice().zeroize();
        }
        self.clear();
    }
}

impl Zeroize for [u16] {
    fn zeroize(&mut self) {
        for b in self.iter_mut() {
            unsafe {
                std::ptr::write_volatile(b, 0);
            }
        }
        compiler_fence(Ordering::SeqCst);
    }
}

impl Zeroize for Vec<u16> {
    fn zeroize(&mut self) {
        self.as_mut_slice().zeroize();
        self.clear();
    }
}

/// RAII memory guard that guarantees zeroization upon drop or rotation.
pub struct Zeroizing<T: Zeroize>(T);

impl<T: Zeroize> Zeroizing<T> {
    pub fn new(inner: T) -> Self {
        Self(inner)
    }

    pub fn into_inner(mut self) -> T {
        // We move the inner out and don't run drop logic on inner twice
        let inner = std::mem::replace(&mut self.0, unsafe { std::mem::zeroed() });
        std::mem::forget(self);
        inner
    }
}

impl<T: Zeroize> Deref for Zeroizing<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Zeroize> DerefMut for Zeroizing<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: Zeroize> Drop for Zeroizing<T> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<T: Zeroize> std::fmt::Debug for Zeroizing<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[REDACTED_SECRET]")
    }
}

/// Operational lifecycle state of an alias in the secret store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretStatus {
    Configured,
    Empty,
    Revoked,
    Deleted,
}

impl SecretStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SecretStatus::Configured => "configured",
            SecretStatus::Empty => "empty",
            SecretStatus::Revoked => "revoked",
            SecretStatus::Deleted => "deleted",
        }
    }
}

/// Metadata entry associated with an API key alias.
/// Guarantees that the secret value itself is NEVER included in metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretMetadata {
    pub alias: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub status: SecretStatus,
    pub updated_at_ms: u64,
}

/// Public summary serialized for CLI, Island and IPC responses.
/// Exposes strictly `{ alias, provider, status }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretSummary {
    pub alias: String,
    pub provider: String,
    pub status: String,
}

/// Mapping of supported LLM providers and model options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    pub provider: String,
    pub default_model: String,
    pub available_models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    InvalidAlias(String),
    InvalidProvider(String),
    EmptySecret,
    AliasNotFound(String),
    UnauthorizedSessionToken,
    GatekeeperDenied(String),
    BackendError(String),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::InvalidAlias(msg) => write!(f, "invalid alias: {}", msg),
            SecretError::InvalidProvider(msg) => write!(f, "invalid provider: {}", msg),
            SecretError::EmptySecret => write!(f, "secret cannot be empty"),
            SecretError::AliasNotFound(alias) => {
                write!(f, "secret alias '{}' not found", alias)
            }
            SecretError::UnauthorizedSessionToken => {
                write!(f, "unauthorized session token for credential access")
            }
            SecretError::GatekeeperDenied(msg) => {
                write!(f, "credential access denied by Gatekeeper: {}", msg)
            }
            SecretError::BackendError(msg) => write!(f, "secret store backend error: {}", msg),
        }
    }
}

impl std::error::Error for SecretError {}
