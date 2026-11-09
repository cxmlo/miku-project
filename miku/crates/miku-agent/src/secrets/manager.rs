use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use miku_gatekeeper::{Capability, GatekeeperEngine, GatekeeperVerdict};
use miku_protocol::constant_time_token_eq;

use crate::secrets::backend::SecretStoreBackend;
use crate::secrets::types::{
    SecretError, SecretMetadata, SecretStatus, SecretSummary, Zeroize, Zeroizing,
};

fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Validates that an alias conforms to secure naming rules:
/// non-empty, alphanumeric with hyphens or underscores, and no path traversal attempts.
pub fn validate_secret_alias(alias: &str) -> Result<(), SecretError> {
    let trimmed = alias.trim();
    if trimmed.is_empty() {
        return Err(SecretError::InvalidAlias(
            "alias cannot be empty or whitespace".to_string(),
        ));
    }
    if alias.contains("..") || alias.contains('/') || alias.contains('\\') {
        return Err(SecretError::InvalidAlias(
            "alias cannot contain path traversal characters ('..', '/', '\\')".to_string(),
        ));
    }
    if !alias
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(SecretError::InvalidAlias(format!(
            "alias '{}' contains invalid characters; must be alphanumeric, '-' or '_'",
            alias
        )));
    }
    Ok(())
}

/// Orchestrates encrypted secret persistence, in-memory secure caching (< 1 ms read budget),
/// Gatekeeper access control, zero-leak IPC listings, and rotation.
pub struct SecretManager {
    backend: Box<dyn SecretStoreBackend>,
    expected_session_token: String,
    gatekeeper: GatekeeperEngine,
    metadata_catalog: BTreeMap<String, SecretMetadata>,
    cache: HashMap<String, Zeroizing<Vec<u8>>>,
}

impl SecretManager {
    pub fn new(
        backend: Box<dyn SecretStoreBackend>,
        expected_session_token: String,
        gatekeeper: GatekeeperEngine,
    ) -> Self {
        Self {
            backend,
            expected_session_token,
            gatekeeper,
            metadata_catalog: BTreeMap::new(),
            cache: HashMap::new(),
        }
    }

    /// Configures or updates a secret key under the specified alias.
    /// Securely persists via the backend and updates the in-memory cache and metadata catalog.
    pub fn set_secret(
        &mut self,
        alias: &str,
        provider: &str,
        model: Option<&str>,
        secret: &[u8],
    ) -> Result<(), SecretError> {
        validate_secret_alias(alias)?;

        let prov_trimmed = provider.trim();
        if prov_trimmed.is_empty() {
            return Err(SecretError::InvalidProvider(
                "provider cannot be empty".to_string(),
            ));
        }

        if secret.is_empty() {
            return Err(SecretError::EmptySecret);
        }

        // Persist to underlying store (e.g. Windows Credential Manager)
        self.backend.save_secret(alias, secret)?;

        // Update in-memory secure cache
        let mut cached_buf = Zeroizing::new(Vec::with_capacity(secret.len()));
        cached_buf.extend_from_slice(secret);
        self.cache.insert(alias.to_string(), cached_buf);

        // Update metadata catalog (NO secret stored here)
        let now = current_timestamp_ms();
        self.metadata_catalog.insert(
            alias.to_string(),
            SecretMetadata {
                alias: alias.to_string(),
                provider: prov_trimmed.to_string(),
                model: model.map(|m| m.trim().to_string()),
                status: SecretStatus::Configured,
                updated_at_ms: now,
            },
        );

        Ok(())
    }

    /// Reads an authorized secret for an alias, verifying the session token and Gatekeeper policy.
    /// Reads from in-memory cache with `< 1 ms` latency.
    pub fn get_secret_authorized(
        &mut self,
        alias: &str,
        session_token: &str,
    ) -> Result<Zeroizing<Vec<u8>>, SecretError> {
        // 1. Session token validation must precede any secret operation
        if !constant_time_token_eq(session_token, &self.expected_session_token) {
            return Err(SecretError::UnauthorizedSessionToken);
        }

        // 2. Validate alias syntax and structure fail-closed
        validate_secret_alias(alias)?;

        // 3. Gatekeeper access authorization BEFORE reading persistent store or cache
        let now = current_timestamp_ms();
        let verdict = self.gatekeeper.evaluate_request(
            "secret-manager",
            Capability::Network,
            alias,
            r#"{"action":"read_credential"}"#,
            now,
        );

        match verdict {
            GatekeeperVerdict::Allow { .. } => {}
            GatekeeperVerdict::Deny { reason, detail, .. } => {
                return Err(SecretError::GatekeeperDenied(format!(
                    "{}: {}",
                    reason.as_code(),
                    detail
                )));
            }
            GatekeeperVerdict::AskPending { ticket, .. } => {
                return Err(SecretError::GatekeeperDenied(format!(
                    "confirmation_pending: {}",
                    ticket.req_hash
                )));
            }
        }

        // 4. ONLY AFTER Gatekeeper authorization: return cached buffer (< 1 ms)
        if let Some(cached) = self.cache.get(alias) {
            let mut clone = Zeroizing::new(Vec::with_capacity(cached.len()));
            clone.extend_from_slice(cached.as_slice());
            return Ok(clone);
        }

        // 5. Fallback to persistent backend store ONLY after authorization
        let loaded = self.backend.load_secret(alias)?;
        if let Some(buf) = loaded {
            let mut clone = Zeroizing::new(Vec::with_capacity(buf.len()));
            clone.extend_from_slice(buf.as_slice());
            self.cache.insert(alias.to_string(), buf);
            self.metadata_catalog.entry(alias.to_string()).or_insert_with(|| {
                SecretMetadata {
                    alias: alias.to_string(),
                    provider: "unknown".to_string(),
                    model: None,
                    status: SecretStatus::Configured,
                    updated_at_ms: current_timestamp_ms(),
                }
            });
            Ok(clone)
        } else {
            Err(SecretError::AliasNotFound(alias.to_string()))
        }
    }

    /// Rotates an existing secret to a new key value, ensuring the old buffer is zeroized.
    pub fn rotate_secret(&mut self, alias: &str, new_secret: &[u8]) -> Result<(), SecretError> {
        validate_secret_alias(alias)?;

        if new_secret.is_empty() {
            return Err(SecretError::EmptySecret);
        }

        let meta = self
            .metadata_catalog
            .get_mut(alias)
            .ok_or_else(|| SecretError::AliasNotFound(alias.to_string()))?;

        // Zeroize previous cached buffer
        if let Some(mut old_buf) = self.cache.remove(alias) {
            old_buf.zeroize();
        }

        // Persist new secret
        self.backend.save_secret(alias, new_secret)?;

        // Populate new cache
        let mut new_buf = Zeroizing::new(Vec::with_capacity(new_secret.len()));
        new_buf.extend_from_slice(new_secret);
        self.cache.insert(alias.to_string(), new_buf);

        // Update metadata
        meta.status = SecretStatus::Configured;
        meta.updated_at_ms = current_timestamp_ms();

        Ok(())
    }

    /// Deletes a secret from the backend and zeroizes the cached memory buffer.
    pub fn delete_secret(&mut self, alias: &str) -> Result<bool, SecretError> {
        validate_secret_alias(alias)?;

        // Remove from persistent store
        let deleted = self.backend.delete_secret(alias)?;

        // Remove and zeroize cached buffer
        if let Some(mut cached) = self.cache.remove(alias) {
            cached.zeroize();
        }

        // Remove from metadata catalog
        self.metadata_catalog.remove(alias);

        Ok(deleted)
    }

    /// Produces a public listing of `{ alias, provider, status }` for IPC / CLI / Island consumers.
    /// Strictly guarantees that no secret value or key fragment is ever exposed.
    pub fn list_summaries(&self) -> Vec<SecretSummary> {
        self.metadata_catalog
            .values()
            .map(|meta| SecretSummary {
                alias: meta.alias.clone(),
                provider: meta.provider.clone(),
                status: meta.status.as_str().to_string(),
            })
            .collect()
    }

    /// Retrieves metadata for a specific alias. Contains zero secrets.
    pub fn get_metadata(&self, alias: &str) -> Option<&SecretMetadata> {
        self.metadata_catalog.get(alias)
    }
}
